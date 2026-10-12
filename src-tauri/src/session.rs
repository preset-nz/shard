//! The open session: one rhizome document, and the engine kept in step with it
//! (epic 32, story 3).
//!
//! **The tree is the source of truth.** Every write is a rhizome edit, and
//! every edit, undo, redo and open leaves through `sync`, which hands the
//! engine only the parts of the compiled `Plan` that changed. A value drag
//! rewrites the bank and nothing else; a `ModSet` is rebuilt only when the
//! modulation changed, and a source is swapped only when a generator reads a
//! different file.
//!
//! What is played rather than edited never enters the tree: brake and
//! reverse write the arrangement's bank directly.
//!
//! **Several tracks, one voice each** (epic 28). Track *i* of the
//! arrangement's order plays on the engine's voice *i*, and every voice plays
//! its own track's patch: its values, modulation and materials, handed to its
//! own feed (`VoiceFeed`). Two tracks on one patch both follow an edit to it.
//! Removing a track moves the later ones down a voice each; the voices' fades
//! and smoothers carry the change.
//!
//! **The current patch is what is edited, never what plays** (epic 10, epic
//! 28). Every patch-scoped command (a value, the chain, presets, links,
//! materials' wiring, the controller map) acts on it, and the patch view
//! shows it. It lives in `sent`, as `Plan::current`, and `sync` selects it
//! onto every new compile. Selecting is not an edit. With a track selected,
//! the current patch is the one that track plays, and follows it through
//! *Plays* picks, undo and redo. When the current patch goes (undoing the New
//! Patch that made it), `sync` falls back to the first track's.
//!
//! **Audition.** A selected patch no track plays loops on its own over the
//! mix, as sound scaping did, on the last voice (`AUDITION`). There is no
//! audition while all `VOICES` voices play tracks.
//!
//! **Lock order:** `doc`, then `gesture`, then `sent`, then `decoded`, then `feeding`, then the
//! engine's hand-off slots. Nothing here takes a controller lock; MIDI calls
//! in with its own locks already released. Decoding a WAV never happens under
//! `doc`: it happens before the edit that needs it.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use rhizome_core::{Edit, GestureId, NodeId, On, Ref, Value};
use rhizome_pom::{Document, FileStore, PresetRef};
use serde::Serialize;
use shard_dsp::arrangement::{self, TRACK_GAIN_DEFAULT};
use shard_dsp::fx::{self, Kind as Fx};
use shard_dsp::params::PARAMS;
use shard_dsp::steps::StepBank;
use shard_dsp::{Generator, ModSet, ParamBank, ReadingBank, StepParams, VOICES};

use crate::mapping::MapRef;
use crate::materials::{self, Pool, Wires, DRONE_NAME};
use crate::modulation::{self, Modulation, Refused};
use crate::object_model::{
    self as om, arrangement_node, instance_of, locate, patch_node, to_value, Plan, Shard, ShardEdit,
};
use crate::source::{self, Loaded};
use crate::tracker::{self, Track, Tracker};

/// One source buffer per generator, by `Generator::index`: a new material
/// waiting for the audio thread, or an old one it handed back.
pub type SourceSlots = [Option<Vec<f32>>; 2];

/// A material's audio by where it comes from: its file, or `None` for the
/// built-in drone. Kept when a material is removed, so undo is instant.
type Decoded = HashMap<Option<String>, Arc<Loaded>>;

/// The voice a selected patch no track plays loops on: the last one, free
/// while fewer than `VOICES` tracks play.
pub const AUDITION: usize = VOICES - 1;

/// `Voices::shown` when no voice plays the current patch.
pub const NO_VOICE: usize = usize::MAX;

/// One voice's inputs. The audio thread reads the atomics once a block, and
/// only ever tries the locks.
pub struct VoiceFeed {
    /// Its patch's values, indexed as `PARAMS`.
    pub bank: ParamBank,
    /// Its track's steps.
    pub steps: StepBank,
    /// How its generators read their materials.
    pub readings: ReadingBank,
    /// Its track's fader, as `f32` bits.
    pub gain: AtomicU32,
    /// Whether it plays. Stored after everything above, so a voice wakes on
    /// its own patch.
    pub active: AtomicBool,
    /// A modulation set waiting for the audio thread, and one it gave back.
    pub mod_swap: Mutex<Option<ModSet>>,
    pub mod_retired: Mutex<Option<ModSet>>,
    /// New materials waiting, by `Generator::index`, and old ones given back.
    pub swap: Mutex<SourceSlots>,
    pub retired: Mutex<SourceSlots>,
}

impl VoiceFeed {
    fn new() -> Self {
        Self {
            bank: ParamBank::new(),
            steps: StepBank::new(StepParams::default()),
            readings: ReadingBank::new(),
            gain: AtomicU32::new(TRACK_GAIN_DEFAULT.to_bits()),
            active: AtomicBool::new(false),
            mod_swap: Mutex::new(None),
            mod_retired: Mutex::new(None),
            swap: Mutex::new([None, None]),
            retired: Mutex::new([None, None]),
        }
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
}

/// Every voice's feed, the transport, and which voice the UI follows.
pub struct Voices {
    pub voice: [VoiceFeed; VOICES],
    /// The transport: tempo, swing and the kit's grid. Always running.
    pub transport: StepBank,
    /// The voice playing the current patch, whose heard values, playhead and
    /// grains the patch view shows, or `NO_VOICE`.
    pub shown: AtomicUsize,
}

impl Voices {
    pub fn new() -> Self {
        Self {
            voice: std::array::from_fn(|_| VoiceFeed::new()),
            transport: StepBank::new(Tracker::default().transport()),
            shown: AtomicUsize::new(NO_VOICE),
        }
    }

    /// Locks every hand-off slot once. On macOS a mutex's first lock
    /// allocates, so this runs before the stream starts and the audio thread
    /// only ever meets primed locks.
    pub fn prime(&self) {
        for v in &self.voice {
            drop(lock(&v.mod_swap));
            drop(lock(&v.mod_retired));
            drop(lock(&v.swap));
            drop(lock(&v.retired));
        }
    }

    /// The voice the patch view follows, if one plays the current patch.
    pub fn shown(&self) -> Option<usize> {
        Some(self.shown.load(Ordering::Acquire)).filter(|i| *i < VOICES)
    }
}

impl Default for Voices {
    fn default() -> Self {
        Self::new()
    }
}

/// Where the session hands things to the engine and the UI.
pub struct Feeds {
    /// The current patch's values, for the patch view, the controllers and
    /// the preview. The audio thread never reads it: each voice has its own.
    pub bank: Arc<ParamBank>,
    pub arrangement: Arc<ParamBank>,
    pub voices: Arc<Voices>,
}

/// What a voice was last handed.
#[derive(Clone, Debug, Default, PartialEq)]
struct Voiced {
    /// The patch it plays, or none while it is switched out.
    patch: Option<NodeId>,
    values: Vec<f32>,
    modulation: Modulation,
    wires: Wires,
    steps: Option<StepParams>,
    gain: f32,
}

/// The plan the engine was handed, what each voice was, and what the scene
/// view has selected.
struct Sent {
    plan: Plan,
    voices: [Voiced; VOICES],
    /// The track selected in the scene view, or none with a patch selected.
    selected_track: Option<usize>,
}

/// What a load could not take as written. Surfaced rather than logged, because
/// a half-applied document and a clean one must not look the same.
#[derive(Serialize, Default, Debug, Clone)]
pub struct LoadReport {
    pub applied: usize,
    /// What the file held that this build could not take.
    pub unknown: Vec<String>,
    pub sample_path: Option<String>,
    /// Set when a material a generator reads could not be found.
    pub sample_missing: bool,
    /// LFOs and links the engine could not use. They stay in the document.
    pub refused: Vec<Refused>,
    /// The name of a controller map the patch wanted and this Mac lacks.
    pub map_missing: Option<String>,
}

/// A patch as the scene view lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PatchInfo {
    /// Its runtime id, for `select_patch`.
    pub id: u64,
    pub name: String,
    /// The patch the engine plays and every patch command acts on.
    pub current: bool,
    /// Whether any track plays it.
    pub played: bool,
    /// Looping on its own over the mix: selected, and played by no track.
    pub auditioned: bool,
    /// Whether Delete may take it: not the last patch, and played by no
    /// track.
    pub deletable: bool,
}

/// What applying a preset did.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ApplyReport {
    pub applied: usize,
    /// Values in the preset this node no longer has.
    pub unknown: Vec<String>,
    /// This node's links the engine could not use.
    pub refused: Vec<Refused>,
}

/// A gesture rhizome has open, and the envelope it edits.
struct Open {
    id: GestureId,
    envelope: u64,
}

pub struct Session {
    doc: Mutex<Document<Shard>>,
    /// The plan as last handed over, selected onto the current patch, and
    /// what each voice was handed.
    sent: Mutex<Sent>,
    decoded: Mutex<Decoded>,
    /// What each voice's generators were last sent, by `Generator::index`: a
    /// source key, or `None` for silence.
    feeding: Mutex<[[Option<Option<String>>; 2]; VOICES]>,
    feeds: Feeds,
    sample_rate: f32,
    /// The envelope gesture in progress, if any. Its id cannot cross IPC, so
    /// it sits here. **Lock order:** `doc`, then `gesture`, then the rest. It
    /// is taken only with `doc` held, and only for the length of a read or a
    /// take, never across `sync`.
    gesture: Mutex<Option<Open>>,
    /// Which `Untitled-N` this is, while it has no file.
    untitled: AtomicU32,
    /// Told after every edit, undo, redo, open, new and save, with every lock
    /// released: the menu's Undo title and the window's unsaved mark follow.
    changed: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// A parameter's name, for an undo label: "Set Cutoff".
fn name_of(id: &str) -> &'static str {
    PARAMS
        .iter()
        .chain(arrangement::params())
        .find(|p| p.id == id)
        .map_or("value", |p| p.name)
}

impl Session {
    /// A new session at its defaults, handed to the engine whole.
    pub fn new(feeds: Feeds, drone: Arc<Loaded>, sample_rate: f32) -> Result<Self, String> {
        let doc = Document::<Shard>::new(FileStore).map_err(err)?;
        let session = Self {
            doc: Mutex::new(doc),
            sent: Mutex::new(Sent {
                plan: Plan::default(),
                voices: std::array::from_fn(|_| Voiced::default()),
                selected_track: None,
            }),
            decoded: Mutex::new([(None, drone)].into()),
            feeding: Mutex::new(std::array::from_fn(|_| [None, None])),
            feeds,
            sample_rate,
            gesture: Mutex::new(None),
            untitled: AtomicU32::new(1),
            changed: OnceLock::new(),
        };
        session.sync(&lock(&session.doc), true);
        Ok(session)
    }

    /// Sets what is told after every change. Once; a second call is ignored.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        let _ = self.changed.set(Box::new(f));
    }

    /// Tells whoever listens. Never call it holding a lock of this session's.
    fn changed(&self) {
        if let Some(f) = self.changed.get() {
            f();
        }
    }

    /// Hands the engine what changed since the last hand-over, or everything.
    /// The current patch stays current while it exists, and follows the
    /// selected track; `all` (a new or opened document) starts on the patch
    /// the first track plays.
    fn sync(&self, doc: &Document<Shard>, all: bool) {
        let mut sent = lock(&self.sent);
        let mut plan = doc.projection().clone();
        if let Some(current) = sent.plan.current.filter(|_| !all) {
            // A patch that has gone leaves the plan on the first track's.
            plan.select(current);
        }
        // A selected track stays selected by its place, as the webview keeps
        // it, brought within the tracks there are.
        let tracks = plan.tracker.tracks.len();
        sent.selected_track = sent.selected_track.map(|t| t.min(tracks.saturating_sub(1)));
        if let Some(Some(p)) = sent.selected_track.and_then(|t| plan.track_patches.get(t)) {
            plan.select(*p);
        }
        if all || plan.patch != sent.plan.patch {
            plan.write_patch(&self.feeds.bank);
        }
        if all || plan.arrangement != sent.plan.arrangement {
            plan.write_arrangement(&self.feeds.arrangement);
        }
        let voices = &self.feeds.voices;
        if all || plan.tracker != sent.plan.tracker {
            voices.transport.store(&plan.tracker.transport());
        }
        let materials = all || plan.pool.materials != sent.plan.pool.materials;
        let next = voicing(&plan, sent.selected_track);
        for (i, want) in next.into_iter().enumerate() {
            self.hand(i, &want, &sent.voices[i], all, materials, &plan.pool);
            sent.voices[i] = want;
        }
        let shown = sent
            .voices
            .iter()
            .position(|v| v.patch.is_some() && v.patch == plan.current)
            .unwrap_or(NO_VOICE);
        voices.shown.store(shown, Ordering::Release);
        sent.plan = plan;
    }

    /// Hands voice `i` what differs from what it was last handed (`was`), or
    /// everything. Switched in last, so it wakes on its own patch.
    fn hand(&self, i: usize, want: &Voiced, was: &Voiced, all: bool, materials: bool, pool: &Pool) {
        let feed = &self.feeds.voices.voice[i];
        if want.patch.is_none() {
            if all || was.patch.is_some() {
                feed.active.store(false, Ordering::Release);
            }
            return;
        }
        if all || want.values != was.values {
            om::write_values(&want.values, &feed.bank);
        }
        if all || want.modulation != was.modulation || was.patch.is_none() {
            *lock(&feed.mod_swap) = Some(want.modulation.build().0);
        }
        if all || materials || want.wires != was.wires || was.patch.is_none() {
            self.send_pool(i, pool, want.wires);
        }
        if all || want.steps != was.steps {
            if let Some(steps) = &want.steps {
                feed.steps.store(steps);
            }
        }
        if all || want.gain != was.gain {
            feed.gain.store(want.gain.to_bits(), Ordering::Relaxed);
        }
        if all || was.patch.is_none() {
            feed.active.store(true, Ordering::Release);
        }
    }

    /// The patch every patch-scoped command acts on. Takes `sent`, so call it
    /// holding `doc` or nothing, never `sent`.
    fn current(&self) -> Option<NodeId> {
        lock(&self.sent).plan.current
    }

    /// Voice `i`'s readings, and a new source only for a generator whose file
    /// changed. Never decodes: a material not yet decoded is silent.
    fn send_pool(&self, i: usize, pool: &Pool, wires: Wires) {
        let decoded = lock(&self.decoded);
        let mut feeding = lock(&self.feeding);
        let feed = &self.feeds.voices.voice[i];
        let mut readings = [shard_dsp::Reading::default(); 2];
        for g in Generator::ALL {
            let m = wires.get(g).and_then(|id| pool.get(id));
            let key = m
                .map(|m| m.path.clone())
                .filter(|k| decoded.contains_key(k));
            if let (Some(m), Some(_)) = (m, &key) {
                readings[g.index()] = m.reading();
            }
            if feeding[i][g.index()] != key {
                // Silence is an empty buffer, which allocates nothing.
                let samples = key
                    .as_ref()
                    .and_then(|k| decoded.get(k))
                    .map(|l| l.samples.clone())
                    .unwrap_or_default();
                lock(&feed.swap)[g.index()] = Some(samples);
                feeding[i][g.index()] = key;
            }
        }
        feed.readings.store(readings[0], readings[1]);
    }

    /// Runs `f` as one edit, coalesced with the last one under the same key,
    /// and syncs. Answers its value and whether anything changed.
    pub fn edit<T>(
        &self,
        label: &str,
        coalesce: Option<&str>,
        f: impl FnOnce(&mut Edit<'_>) -> rhizome_core::Result<T>,
    ) -> Result<(T, bool), String> {
        let mut doc = lock(&self.doc);
        let result = match coalesce {
            Some(key) => doc.edit_coalesced(label, key, f),
            None => doc.edit(label, f),
        };
        let (value, commit) = result.map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        drop(doc);
        if commit.is_some() {
            self.changed();
        }
        Ok((value, commit.is_some()))
    }

    /// Reads the document.
    pub fn read<T>(&self, f: impl FnOnce(&Document<Shard>) -> T) -> T {
        f(&lock(&self.doc))
    }

    /// What the engine was last handed, selected onto the current patch.
    pub fn plan(&self) -> Plan {
        lock(&self.sent).plan.clone()
    }

    // Values.

    /// Sets a patch or arrangement parameter by id. A drag is one undo step,
    /// "Set Cutoff". Brake and reverse are played: they reach the
    /// arrangement's bank and never the document.
    pub fn set_param(&self, id: &str, value: f32) -> Result<(), String> {
        if om::PLAYED.contains(&id) {
            self.feeds.arrangement.set_by_id(id, value);
            return Ok(());
        }
        let found = self.read(|d| {
            let (node, key) = locate(d.tree(), self.current(), id)?;
            let spec = d.tree().get(node)?.node_type()?.spec(&key)?.clone();
            Some((node, key, to_value(&spec, value)?))
        });
        let Some((node, key, want)) = found else {
            // An effect no node plays: a knob still mapped to it moves
            // nothing, as it did when only the bank heard it.
            if fx::parse_row(id).is_some() {
                return Ok(());
            }
            return Err(format!("unknown parameter: {id}"));
        };
        let label = format!("Set {}", name_of(id));
        self.edit(&label, Some(&format!("param:{id}")), |tx| {
            tx.put(node, &key, want)
        })?;
        Ok(())
    }

    /// Every patch value back to its default and the chain emptied, as one
    /// step. The sound only: materials and their wiring, the tracker, the
    /// arrangement, the LFOs and the controller map are left alone. False when
    /// there was nothing to reset.
    pub fn reset_sound(&self) -> Result<bool, String> {
        let patch = self.owner("patch")?;
        let (_, changed) = self.edit("Reset the sound", None, |tx| {
            let mut writes = Vec::new();
            let mut effects = Vec::new();
            if let Some(p) = tx.at(patch) {
                for n in std::iter::once(p).chain(p.children()) {
                    if om::is_effect_node(&n) {
                        effects.push(n.id());
                        continue;
                    }
                    for (key, _) in n.stored_values() {
                        if PARAMS.iter().any(|d| d.id == key) {
                            writes.push((n.id(), key.to_string()));
                        }
                    }
                }
            }
            for (node, key) in writes {
                tx.reset(node, key.as_str())?;
            }
            for e in effects {
                tx.remove(e)?;
            }
            Ok(())
        })?;
        Ok(changed)
    }

    // The chain.

    fn owner(&self, layer: &str) -> Result<NodeId, String> {
        self.read(|d| match layer {
            "patch" => patch_node(d.tree(), self.current()).map(|n| n.id()),
            "arrangement" => arrangement_node(d.tree()).map(|n| n.id()),
            _ => None,
        })
        .ok_or_else(|| format!("there is no level called {layer}"))
    }

    fn effect_at(&self, layer: &str, n: usize) -> Result<NodeId, String> {
        let owner = self.owner(layer)?;
        self.read(|d| {
            let chain = d.tree().get(owner)?.order(om::CHAIN);
            chain
                .into_iter()
                .find(|e| instance_of(d.tree(), e.id()) == Some(n))
                .map(|e| e.id())
        })
        .ok_or_else(|| "that effect is not in the chain".to_string())
    }

    /// An effect at the end of a level's chain, switched on. Answers the pool
    /// instance it plays.
    pub fn fx_add(&self, layer: &str, kind: &str) -> Result<usize, String> {
        let kind =
            Fx::from_name(kind).ok_or_else(|| format!("there is no effect called {kind}"))?;
        let owner = self.owner(layer)?;
        let (node, _) = self.edit(&format!("Add {}", kind.label()), None, |tx| {
            tx.add_effect(owner, kind)
        })?;
        self.read(|d| instance_of(d.tree(), node))
            .ok_or_else(|| "the effect did not land in the chain".to_string())
    }

    /// Takes an effect out of a level's chain. Undo brings it back with its
    /// settings.
    pub fn fx_remove(&self, layer: &str, n: usize) -> Result<(), String> {
        let node = self.effect_at(layer, n)?;
        let label = fx::POOL.get(n).map_or("effect", |(k, _)| k.label());
        self.edit(&format!("Remove {label}"), None, |tx| tx.remove(node))?;
        Ok(())
    }

    /// Moves an effect one place earlier (`by` below zero) or later. Answers
    /// the instance it plays afterwards, which changes when it passes another
    /// of its kind (the k-th of a kind plays copy k), or None at an end.
    pub fn fx_move(&self, layer: &str, n: usize, by: i32) -> Result<Option<usize>, String> {
        let owner = self.owner(layer)?;
        let node = self.effect_at(layer, n)?;
        let mut order: Vec<NodeId> = self.read(|d| {
            d.tree()
                .get(owner)
                .map(|o| o.order(om::CHAIN).iter().map(|e| e.id()).collect())
                .unwrap_or_default()
        });
        let Some(at) = order.iter().position(|e| *e == node) else {
            return Ok(None);
        };
        let to = at as i64 + i64::from(by.signum());
        if to < 0 || to as usize >= order.len() || by == 0 {
            return Ok(None);
        }
        order.swap(at, to as usize);
        let label = fx::POOL.get(n).map_or("effect", |(k, _)| k.label());
        let (_, changed) = self.edit(&format!("Move {label}"), None, |tx| {
            tx.set_order(owner, om::CHAIN, order)
        })?;
        if !changed {
            return Ok(None);
        }
        Ok(self.read(|d| instance_of(d.tree(), node)))
    }

    // History.

    pub fn undo(&self) -> Result<Option<String>, String> {
        let mut doc = lock(&self.doc);
        let commit = doc.undo().map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        drop(doc);
        if commit.is_some() {
            self.changed();
        }
        Ok(commit.map(|c| bare(&c.label, "Undo ")))
    }

    pub fn redo(&self) -> Result<Option<String>, String> {
        let mut doc = lock(&self.doc);
        let commit = doc.redo().map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        drop(doc);
        if commit.is_some() {
            self.changed();
        }
        Ok(commit.map(|c| bare(&c.label, "Redo ")))
    }

    /// Every step Undo can take, oldest first, and every step Redo can,
    /// next first. For the Edit menu and a history list.
    pub fn history_labels(&self) -> (Vec<String>, Vec<String>) {
        self.read(|d| {
            let tree = d.tree();
            (
                tree.undo_labels().map(str::to_string).collect(),
                tree.redo_labels().map(str::to_string).collect(),
            )
        })
    }

    /// What Undo and Redo would do, by name.
    pub fn history(&self) -> (Option<String>, Option<String>) {
        self.read(|d| {
            (
                d.tree().undo_label().map(str::to_string),
                d.tree().redo_label().map(str::to_string),
            )
        })
    }

    // The tracker.

    /// The tempo, swing, kit grid and every track, with the patch each plays.
    pub fn tracker(&self) -> Tracker {
        lock(&self.sent).plan.tracker.clone()
    }

    /// How many tracks there are, and so how many voices play them.
    pub fn track_count(&self) -> usize {
        lock(&self.sent).plan.tracker.tracks.len()
    }

    /// Takes the tempo, swing, kit grid and each track's steps, length and
    /// fader from `next`, matching tracks by place, as one step. Never adds
    /// or removes a track, and never changes what one plays. A drag on one
    /// track's fader is "Set Gain"; anything else "Edit the steps", coalesced
    /// per track, so edits to two tracks are two steps.
    pub fn set_tracker(&self, next: Tracker) -> Result<Tracker, String> {
        let next = next.sanitised();
        let now = self.tracker();
        let differs =
            |a: &Track, b: &Track| (a.length, &a.steps, a.gain) != (b.length, &b.steps, b.gain);
        let changed: Vec<usize> = now
            .tracks
            .iter()
            .zip(&next.tracks)
            .enumerate()
            .filter(|(_, (a, b))| differs(a, b))
            .map(|(i, _)| i)
            .collect();
        let shared = (now.tempo, now.swing, &now.kit) != (next.tempo, next.swing, &next.kit);
        let (label, key) = match changed.as_slice() {
            [i] if !shared => {
                let (a, b) = (&now.tracks[*i], &next.tracks[*i]);
                if (a.length, &a.steps) == (b.length, &b.steps) {
                    ("Set Gain", format!("gain:{i}"))
                } else {
                    ("Edit the steps", format!("tracker:{i}"))
                }
            }
            _ => ("Edit the steps", "tracker".to_string()),
        };
        self.write_tracker(label, Some(&key), next)
    }

    /// A whole-pattern edit on track `track`, one step; none when it changes
    /// nothing.
    pub fn edit_tracker(&self, track: usize, op: &tracker::Edit) -> Result<Tracker, String> {
        let next = self.tracker().apply(track, op);
        self.write_tracker(Tracker::label_of(op), None, next)
    }

    fn write_tracker(
        &self,
        label: &str,
        coalesce: Option<&str>,
        next: Tracker,
    ) -> Result<Tracker, String> {
        let next = next.sanitised();
        let arr = self.owner("arrangement")?;
        self.edit(label, coalesce, |tx| tx.write_tracker(arr, &next))?;
        Ok(self.tracker())
    }

    /// Add Track: a melody track at the end, sixteen empty steps at unity,
    /// playing the current patch. One undo step, "Add Track". Refused once
    /// every voice has a track. Answers its place.
    pub fn add_track(&self) -> Result<usize, String> {
        let tracks = self.track_count();
        if tracks >= VOICES {
            return Err(format!("a session plays at most {VOICES} tracks"));
        }
        let arr = self.owner("arrangement")?;
        let patch = self.read(|d| patch_node(d.tree(), self.current()).map(|p| p.id()));
        self.edit("Add Track", None, |tx| {
            let track = tx.add_track(arr, &Track::blank())?;
            match patch {
                Some(p) => tx.set_ref(track, om::TRACK_PATCH, Ref::here(p)),
                None => Ok(()),
            }
        })?;
        Ok(tracks)
    }

    /// Remove Track: track `track` with its steps, one undo step named for
    /// it, "Remove Track 2". Undo brings it back in its place. The tracks
    /// after it move down a place, and so a voice. Refused for the last one.
    pub fn remove_track(&self, track: usize) -> Result<(), String> {
        let node = self
            .read(|d| om::tracks(d.tree()).get(track).map(|t| t.id()))
            .ok_or_else(|| format!("there is no track {}", track + 1))?;
        if self.track_count() <= 1 {
            return Err("a session keeps at least one track".into());
        }
        {
            let mut sent = lock(&self.sent);
            // The selection stays on the track it was on.
            sent.selected_track = sent
                .selected_track
                .map(|s| if s > track { s - 1 } else { s });
        }
        self.edit(&format!("Remove Track {}", track + 1), None, |tx| {
            tx.remove(node)
        })?;
        Ok(())
    }

    /// Selects track `track` in the scene view: the current patch becomes the
    /// one it plays, and no patch is auditioned. Not an edit. Answers whether
    /// the current patch changed, which the webview reads everything again
    /// for.
    pub fn select_track(&self, track: usize) -> Result<bool, String> {
        let mut doc = lock(&self.doc);
        let tracks = om::tracks(doc.tree());
        let played = tracks
            .get(track)
            .and_then(|t| om::played_by(doc.tree(), *t))
            .map(|p| p.id())
            .ok_or_else(|| format!("there is no track {}", track + 1))?;
        let moved = self.current() != Some(played);
        // A gesture on the patch being left ends with it.
        let ended = moved && self.end_gesture(&mut doc);
        {
            let mut sent = lock(&self.sent);
            sent.selected_track = Some(track);
            sent.plan.current = Some(played);
        }
        self.sync(&doc, false);
        drop(doc);
        if ended {
            self.changed();
        }
        Ok(moved)
    }

    // Modulation.

    /// The patch's LFOs, envelopes and links, and what the engine refuses.
    pub fn modulation(&self) -> (Modulation, Vec<Refused>) {
        let sent = lock(&self.sent);
        (sent.plan.modulation.clone(), sent.plan.mod_set().1)
    }

    fn modulator(&self, id: u64, kind: &str) -> Result<NodeId, String> {
        self.read(|d| {
            let node = d.projection().node_of(id)?;
            (d.tree().get(node)?.type_name() == kind).then_some(node)
        })
        .ok_or_else(|| {
            let what = if kind == om::LFO { "LFO" } else { "envelope" };
            format!("there is no {what} {id}")
        })
    }

    pub fn add_lfo(&self) -> Result<(), String> {
        let patch = self.owner("patch")?;
        self.edit("Add LFO", None, |tx| {
            let lfo = tx.add_lfo(patch)?;
            let n = count(tx, patch, om::LFO);
            tx.set_value(lfo, "lfo.label", Value::Text(format!("LFO {n}")))
        })?;
        Ok(())
    }

    pub fn add_envelope(&self) -> Result<(), String> {
        let patch = self.owner("patch")?;
        self.edit("Add envelope", None, |tx| {
            let env = tx.add_mod_env(patch)?;
            let n = count(tx, patch, om::MOD_ENV);
            tx.set_value(
                env,
                "mod-env.label",
                Value::Text(format!("Mod envelope {n}")),
            )
        })?;
        Ok(())
    }

    /// Removes an LFO, and the links that followed it.
    pub fn remove_lfo(&self, id: u64) -> Result<(), String> {
        let node = self.modulator(id, om::LFO)?;
        self.edit("Remove LFO", None, |tx| tx.remove(node))?;
        Ok(())
    }

    pub fn remove_envelope(&self, id: u64) -> Result<(), String> {
        let node = self.modulator(id, om::MOD_ENV)?;
        self.edit("Remove envelope", None, |tx| tx.remove(node))?;
        Ok(())
    }

    pub fn set_lfo(&self, lfo: modulation::LfoRecord) -> Result<(), String> {
        let node = self.modulator(lfo.id, om::LFO)?;
        if shard_dsp::Shape::from_name(&lfo.shape).is_none() {
            return Err(format!("there is no LFO shape called “{}”", lfo.shape));
        }
        let rate = lfo.rate.clamp(
            shard_dsp::modulation::MIN_RATE_HZ,
            shard_dsp::modulation::MAX_RATE_HZ,
        );
        let name = modulation::valid_name(&lfo.name, "an LFO")?;
        let key = format!("lfo:{}", lfo.id);
        self.edit("Change LFO", Some(&key), |tx| {
            tx.put(node, "lfo.label", Value::Text(name))?;
            tx.put(node, "lfo.rate", Value::Float(om::float(rate)))?;
            tx.put(node, "lfo.shape", Value::Choice(lfo.shape.clone()))?;
            tx.put(
                node,
                "lfo.phase",
                Value::Float(om::float(lfo.phase.clamp(0.0, 1.0))),
            )
        })?;
        Ok(())
    }

    /// Writes an envelope's name, mode and curve as one edit,
    /// checking the record first so a bad one is a reason and not a refusal
    /// from deep in the tree. Coalesced under `envelope:<id>` for input with no
    /// begin and end (a slider, an arrow-key nudge); a plain edit otherwise.
    pub fn set_envelope(
        &self,
        env: modulation::EnvelopeRecord,
        coalesce: bool,
    ) -> Result<(), String> {
        let node = self.modulator(env.id, om::MOD_ENV)?;
        let writes = envelope_writes(&env)?;
        let key = format!("envelope:{}", env.id);
        self.edit("Change envelope", coalesce.then_some(key.as_str()), |tx| {
            write_envelope(tx, node, &writes)
        })?;
        Ok(())
    }

    /// Opens a gesture on an envelope: a drag of a curve point, a handle or a
    /// slider. Its applies are visible at once and cut one undo step at
    /// `envelope_gesture_end`.
    ///
    /// A gesture already open (a drag whose end never came) is ended first, so
    /// what it did is kept as its own step, rather than cancelled: the user
    /// saw those changes land.
    pub fn envelope_gesture_begin(&self, id: u64) -> Result<(), String> {
        self.modulator(id, om::MOD_ENV)?;
        let mut doc = lock(&self.doc);
        let mut slot = lock(&self.gesture);
        if let Some(old) = slot.take() {
            let _ = doc.end(old.id);
        }
        let g = doc.begin("Change envelope").map_err(err)?;
        *slot = Some(Open {
            id: g,
            envelope: id,
        });
        Ok(())
    }

    /// Writes the envelope inside the open gesture, and hands the engine what
    /// changed. Nothing is cut into history until the gesture ends.
    pub fn envelope_gesture_apply(&self, env: modulation::EnvelopeRecord) -> Result<(), String> {
        let writes = envelope_writes(&env)?;
        let mut doc = lock(&self.doc);
        let g = self.open_gesture(env.id)?;
        let node = doc
            .projection()
            .node_of(env.id)
            .ok_or_else(|| format!("there is no envelope {}", env.id))?;
        let (_, commit) = doc
            .within(g, |tx| write_envelope(tx, node, &writes))
            .map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        drop(doc);
        if commit.is_some() {
            self.changed();
        }
        Ok(())
    }

    /// Closes the gesture and cuts its undo step, if it changed anything.
    /// Ending with none open is not an error: a release that follows a cancel.
    pub fn envelope_gesture_end(&self) -> Result<(), String> {
        let mut doc = lock(&self.doc);
        let Some(open) = lock(&self.gesture).take() else {
            return Ok(());
        };
        doc.end(open.id).map_err(err)?;
        drop(doc);
        // The step now exists, so the Undo title has changed.
        self.changed();
        Ok(())
    }

    /// Closes the gesture and puts the envelope back as it was when the
    /// gesture began, leaving no undo step. Edits made elsewhere while it was
    /// open (a knob) are put back with it, because rhizome restores the whole
    /// tree to the gesture's start.
    pub fn envelope_gesture_cancel(&self) -> Result<(), String> {
        let mut doc = lock(&self.doc);
        let Some(open) = lock(&self.gesture).take() else {
            return Ok(());
        };
        let commit = doc.cancel(open.id).map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        drop(doc);
        self.changed();
        Ok(())
    }

    /// The open gesture's id, if it is on envelope `envelope`.
    fn open_gesture(&self, envelope: u64) -> Result<GestureId, String> {
        match &*lock(&self.gesture) {
            Some(open) if open.envelope == envelope => Ok(open.id),
            Some(_) => Err("a gesture is open on another envelope".into()),
            None => Err("no envelope gesture is open".into()),
        }
    }

    /// The patch parameter `id` follows modulator `source`, from `lo` to `hi`
    /// of its range, replacing whatever it followed.
    pub fn link_param(&self, id: &str, source: u64, lo: f32, hi: f32) -> Result<(), String> {
        if id.starts_with(arrangement::PREFIX) {
            return Err(format!(
                "{id} is the arrangement's and can't follow a modulator"
            ));
        }
        let (target, key) = self
            .read(|d| locate(d.tree(), self.current(), id))
            .ok_or_else(|| format!("there is no parameter called {id}"))?;
        let source = self
            .read(|d| d.projection().node_of(source))
            .ok_or_else(|| format!("there is no LFO or envelope {source} to link to"))?;
        self.edit("Link a parameter", Some(&format!("link:{id}")), |tx| {
            unlink(tx, target, &key)?;
            tx.link(target, &key, source, lo.clamp(0.0, 1.0), hi.clamp(0.0, 1.0))
        })?;
        Ok(())
    }

    pub fn unlink_param(&self, id: &str) -> Result<(), String> {
        let Some((target, key)) = self.read(|d| locate(d.tree(), self.current(), id)) else {
            return Ok(());
        };
        self.edit("Unlink a parameter", None, |tx| unlink(tx, target, &key))?;
        Ok(())
    }

    // Presets.

    /// The node a preset names, such as `grain`, `amp`, `patch` or
    /// `fx.3.chorus`: patch only.
    fn preset_node(&self, node: &str) -> Result<NodeId, String> {
        self.read(|d| {
            let current = self.current();
            let patch = patch_node(d.tree(), current)?;
            if node == om::PATCH {
                return Some(patch.id());
            }
            if node.starts_with("fx.") {
                return locate(d.tree(), current, &format!("{node}.on")).map(|(n, _)| n);
            }
            patch.child(node).map(|n| n.id())
        })
        .ok_or_else(|| format!("there is no {node} to keep presets for"))
    }

    pub fn preset_names(&self, node: &str) -> Vec<String> {
        let Ok(n) = self.preset_node(node) else {
            return Vec::new();
        };
        self.read(|d| d.preset_names(n).unwrap_or_default())
            .into_iter()
            .filter_map(|p| match p {
                PresetRef::User(label) => Some(label),
                PresetRef::Catalogue(_) => None,
            })
            .collect()
    }

    pub fn save_preset(&self, node: &str, name: &str) -> Result<(), String> {
        let n = self.preset_node(node)?;
        lock(&self.doc).save_preset(n, name).map_err(err)?;
        self.changed();
        Ok(())
    }

    pub fn update_preset(&self, node: &str, name: &str) -> Result<(), String> {
        let n = self.preset_node(node)?;
        lock(&self.doc).update_preset(n, name).map_err(err)?;
        self.changed();
        Ok(())
    }

    /// Applies a preset: the node's values and links, never its switch.
    pub fn apply_preset(&self, node: &str, name: &str) -> Result<ApplyReport, String> {
        let n = self.preset_node(node)?;
        let mut doc = lock(&self.doc);
        let (report, commit) = doc
            .apply_preset(n, &PresetRef::User(name.to_string()))
            .map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        let prefix = format!("{node}.");
        let refused = lock(&self.sent)
            .plan
            .mod_set()
            .1
            .into_iter()
            .filter(|r| r.id.starts_with(&prefix))
            .collect();
        drop(doc);
        if commit.is_some() {
            self.changed();
        }
        Ok(ApplyReport {
            applied: report.applied,
            unknown: report.skipped,
            refused,
        })
    }

    // Materials.

    pub fn pool(&self) -> Pool {
        lock(&self.sent).plan.pool.clone()
    }

    /// Whether a material's audio is in hand: decoded, or the drone.
    pub fn loaded(&self, id: u64) -> Option<Arc<Loaded>> {
        let pool = self.pool();
        let m = pool.get(id)?;
        lock(&self.decoded).get(&m.path).cloned()
    }

    /// Decodes a file into the cache, outside any document lock.
    fn decode(&self, path: &Option<String>) -> Result<(), String> {
        if lock(&self.decoded).contains_key(path) {
            return Ok(());
        }
        let Some(p) = path else { return Ok(()) };
        let loaded = Arc::new(source::load(p, self.sample_rate)?);
        lock(&self.decoded).insert(path.clone(), loaded);
        Ok(())
    }

    /// Adds a WAV, whole and at its own pitch. A generator with no material
    /// yet reads it, so the first file added is heard at once.
    pub fn add_material(&self, path: &str) -> Result<(), String> {
        let key = Some(path.to_string());
        self.decode(&key)?;
        let name = lock(&self.decoded)
            .get(&key)
            .map_or_else(|| path.to_string(), |l| l.name.clone());
        self.add_sample(&name, path)
    }

    /// Adds the built-in drone back, after it was removed.
    pub fn add_drone(&self) -> Result<(), String> {
        self.add_sample(DRONE_NAME, "")
    }

    fn add_sample(&self, name: &str, path: &str) -> Result<(), String> {
        let label = if path.is_empty() {
            "Add the drone"
        } else {
            "Add material"
        };
        let patch = self.owner("patch")?;
        self.edit(label, None, |tx| {
            let mut pool = Pool::default();
            pool.materials.push(materials::MaterialRecord {
                id: 0,
                name: name.to_string(),
                path: (!path.is_empty()).then(|| path.to_string()),
                octave: 0.0,
                trim_start: 0.0,
                trim_end: 1.0,
                root: None,
            });
            let added = tx.add_pool(&pool)?;
            let Some(&(_, sample)) = added.first() else {
                return Ok(());
            };
            for g in om::READS_MATERIAL {
                let key = format!("{g}.source");
                let generator = tx
                    .at(patch)
                    .and_then(|p| p.child(g))
                    .filter(|n| n.resolve(&key).is_none())
                    .map(|n| n.id());
                if let Some(generator) = generator {
                    tx.set_ref(generator, key.as_str(), Ref::here(sample))?;
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    fn sample(&self, id: u64) -> Result<NodeId, String> {
        self.read(|d| {
            let node = d.projection().node_of(id)?;
            (d.tree().get(node)?.type_name() == om::SAMPLE).then_some(node)
        })
        .ok_or_else(|| format!("there is no material {id}"))
    }

    /// Removes a material. A generator that read it goes silent.
    pub fn remove_material(&self, id: u64) -> Result<(), String> {
        let node = self.sample(id)?;
        self.edit("Remove material", None, |tx| tx.remove(node))?;
        Ok(())
    }

    /// Wires a material into `material` (Sample) or `grain` (Granular), or
    /// unwires it with none. A material whose file can't be read is refused.
    pub fn wire_material(&self, generator: &str, material: Option<u64>) -> Result<(), String> {
        if !om::READS_MATERIAL.contains(&generator) {
            return Err(format!("{generator} does not read a material"));
        }
        let sample = match material {
            Some(id) => {
                let node = self.sample(id)?;
                let path = self.pool().get(id).and_then(|m| m.path.clone());
                self.decode(&path)?;
                Some(node)
            }
            None => None,
        };
        let patch = self.owner("patch")?;
        let key = format!("{generator}.source");
        self.edit("Wire material", None, |tx| {
            let Some(g) = tx
                .at(patch)
                .and_then(|p| p.child(generator))
                .map(|n| n.id())
            else {
                return Ok(());
            };
            match sample {
                Some(s) => tx.set_ref(g, key.as_str(), Ref::here(s)),
                None => tx.clear_ref(g, key.as_str()),
            }
        })?;
        Ok(())
    }

    /// Sets a material's octave, trim and root note.
    pub fn set_material(
        &self,
        id: u64,
        octave: f32,
        trim_start: f32,
        trim_end: f32,
        root: Option<u8>,
    ) -> Result<(), String> {
        let node = self.sample(id)?;
        let range = f64::from(shard_dsp::sources::OCTAVE_RANGE);
        let octave = f64::from(octave).round().clamp(-range, range) as i64;
        let root = match root {
            Some(r) => serde_json::json!(r.min(127)),
            None => serde_json::Value::Null,
        };
        self.edit("Change material", Some(&format!("material:{id}")), |tx| {
            tx.put(node, "sample.octave", Value::Int(octave))?;
            tx.put(
                node,
                "sample.start",
                Value::Float(om::float(trim_start.clamp(0.0, 1.0))),
            )?;
            tx.put(
                node,
                "sample.end",
                Value::Float(om::float(trim_end.clamp(0.0, 1.0))),
            )?;
            tx.put(node, "sample.root", Value::Shaped(root))
        })?;
        Ok(())
    }

    // The controller map a patch plays with.

    pub fn controller_map(&self) -> Option<MapRef> {
        self.read(
            |d| match patch_node(d.tree(), self.current())?.value("patch.map")? {
                Value::Shaped(j) => serde_json::from_value(j).ok(),
                _ => None,
            },
        )
    }

    /// Records the map the patch plays with. One step, as choosing a map is
    /// part of how the patch is played.
    pub fn set_controller_map(&self, map: Option<&MapRef>) -> Result<(), String> {
        let patch = self.owner("patch")?;
        let value = map.map_or(
            serde_json::Value::Null,
            |m| serde_json::json!({ "id": m.id, "name": m.name }),
        );
        self.edit("Choose controller map", None, |tx| {
            tx.put(patch, "patch.map", Value::Shaped(value))
        })?;
        Ok(())
    }

    // Patches.

    /// Every patch, in order, for the scene view.
    pub fn patches(&self) -> Vec<PatchInfo> {
        self.read(|d| {
            let tree = d.tree();
            let current = patch_node(tree, self.current()).map(|n| n.id());
            let played = played_patches(tree);
            let auditioned = lock(&self.sent).voices[AUDITION].patch;
            let all = om::patches(tree);
            let several = all.len() > 1;
            all.iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    let is_played = played.contains(&p.id());
                    Some(PatchInfo {
                        id: d.projection().id_of(p.id())?,
                        name: om::patch_name(p, i),
                        current: Some(p.id()) == current,
                        played: is_played,
                        auditioned: !is_played && auditioned == Some(p.id()),
                        deletable: several && !is_played,
                    })
                })
                .collect()
        })
    }

    /// The current patch's runtime id, as `patches` lists it.
    pub fn current_patch(&self) -> Option<u64> {
        let sent = lock(&self.sent);
        sent.plan.current.and_then(|n| sent.plan.id_of(n))
    }

    /// New Patch: a patch at its defaults, "Patch N", the drone in both
    /// generators that read a material and an empty chain, at the end of the
    /// order. One undo step, and it becomes the current patch, selected, so
    /// it is auditioned.
    pub fn new_patch(&self) -> Result<u64, String> {
        let mut doc = lock(&self.doc);
        // A gesture on the patch being left ends first, as its own step.
        self.end_gesture(&mut doc);
        let (patch, commit) = doc.edit("New Patch", |tx| tx.new_patch()).map_err(err)?;
        self.make_current(patch);
        if commit.is_some() {
            self.sync(&doc, false);
        }
        let id = doc.projection().id_of(patch);
        drop(doc);
        self.changed();
        id.ok_or_else(|| "the new patch did not compile".to_string())
    }

    /// Renames a patch: its `patch.label`, trimmed. One undo step named for
    /// the name it had, "Rename Patch 2". An empty name, or the name it
    /// already shows, changes nothing and makes no step. Answers whether it
    /// changed.
    pub fn rename_patch(&self, id: u64, name: &str) -> Result<bool, String> {
        let (node, old) = self.read(|d| {
            let node = patch_of(d, id)?;
            Ok::<_, String>((node, display_name(d.tree(), node)))
        })?;
        if name.trim().is_empty() || name.trim() == old {
            return Ok(false);
        }
        let name = modulation::valid_name(name, "a patch")?;
        let (_, changed) = self.edit(&format!("Rename {old}"), None, |tx| {
            tx.set_value(node, om::PATCH_LABEL, Value::Text(name))
        })?;
        Ok(changed)
    }

    /// Deletes a patch and everything in it: its chain, modulators and links.
    /// One undo step, "Delete Patch 2", and undo brings it back as it was, in
    /// its place. Refused for the last patch and for one a track plays.
    /// When it was current, the first track's patch becomes current. Answers
    /// whether the current patch changed.
    pub fn delete_patch(&self, id: u64) -> Result<bool, String> {
        let mut doc = lock(&self.doc);
        let node = patch_of(&doc, id)?;
        let name = display_name(doc.tree(), node);
        if om::patches(doc.tree()).len() <= 1 {
            return Err(format!("{name} is the only patch, so it can't be deleted"));
        }
        if played_patches(doc.tree()).contains(&node) {
            return Err(format!(
                "a track plays {name}; pick another patch for it to play first"
            ));
        }
        let was_current = patch_node(doc.tree(), self.current()).map(|n| n.id()) == Some(node);
        // A gesture on the patch going ends first, as its own step.
        let ended = was_current && self.end_gesture(&mut doc);
        let (_, commit) = doc
            .edit(&format!("Delete {name}"), |tx| tx.remove(node))
            .map_err(err)?;
        if commit.is_some() {
            // The current patch has gone, so `sync` falls back to the first
            // track's.
            self.sync(&doc, false);
        }
        drop(doc);
        if commit.is_some() || ended {
            self.changed();
        }
        Ok(was_current && commit.is_some())
    }

    /// Duplicates a patch: a deep copy straight after it in the order, named
    /// "Patch 2 copy", with its chain, modulators, links (onto the copy's own
    /// modulators), materials' wiring and controller map. One undo step,
    /// "Duplicate Patch 2", and the copy becomes current and selected. Presets are kept
    /// per kind, not per patch, so the copy offers the same ones.
    pub fn duplicate_patch(&self, id: u64) -> Result<u64, String> {
        let mut doc = lock(&self.doc);
        let node = patch_of(&doc, id)?;
        let name = display_name(doc.tree(), node);
        let copy_name = copy_name(doc.tree(), &name);
        // A gesture on the patch being left ends first, as its own step.
        self.end_gesture(&mut doc);
        let (copy, commit) = doc
            .edit(&format!("Duplicate {name}"), |tx| {
                let category = format!("/{}", om::PATCHES);
                let mut order: Vec<NodeId> = tx
                    .at(category.as_str())
                    .map(|c| om::patches_in(c).iter().map(|p| p.id()).collect())
                    .unwrap_or_default();
                // rhizome's copy points the copy's links at its own
                // modulators and its chain and modulators orders at its own
                // nodes; a material stays the same material. A generator whose
                // material was removed reads the same nothing.
                let copy = tx.copy(node, category.as_str())?;
                tx.set_value(copy, om::PATCH_LABEL, Value::Text(copy_name))?;
                // The whole order, so the copy sits straight after the
                // original even when the original was missing from it.
                let at = order
                    .iter()
                    .position(|p| *p == node)
                    .map_or(order.len(), |i| i + 1);
                order.insert(at, copy);
                tx.set_order(category.as_str(), om::PATCH_ORDER, order)?;
                Ok(copy)
            })
            .map_err(err)?;
        self.make_current(copy);
        if commit.is_some() {
            self.sync(&doc, false);
        }
        let id = doc.projection().id_of(copy);
        drop(doc);
        self.changed();
        id.ok_or_else(|| "the copy did not compile".to_string())
    }

    /// Selects a patch in the scene view: it becomes current, and loops on
    /// its own while no track plays it. Not an edit: no undo step, no unsaved
    /// mark. What the tracks play never changes.
    pub fn select_patch(&self, id: u64) -> Result<(), String> {
        let mut doc = lock(&self.doc);
        let node = patch_of(&doc, id)?;
        let (same, selected) = {
            let sent = lock(&self.sent);
            (
                sent.plan.current == Some(node),
                sent.selected_track.is_none(),
            )
        };
        if same && selected {
            return Ok(());
        }
        // A gesture on an envelope of the patch being left ends with it.
        let ended = !same && self.end_gesture(&mut doc);
        self.make_current(node);
        self.sync(&doc, false);
        drop(doc);
        if ended {
            // Its step now exists, so the Undo title has changed.
            self.changed();
        }
        Ok(())
    }

    /// `patch` current and selected, with no track selected. Hold `doc`, and
    /// `sync` after.
    fn make_current(&self, patch: NodeId) {
        let mut sent = lock(&self.sent);
        sent.plan.current = Some(patch);
        sent.selected_track = None;
    }

    /// Sets what track `track` plays (its *Plays* picker, epic 10 story c) to
    /// the patch `id` names. One undo step, "Set Plays", and none when it
    /// already plays it. With that track selected the current patch follows,
    /// and the answer is true: the webview reads everything again.
    pub fn set_track_patch(&self, track: usize, id: u64) -> Result<bool, String> {
        let mut doc = lock(&self.doc);
        let node = patch_of(&doc, id)?;
        let tracks = om::tracks(doc.tree());
        let t = tracks
            .get(track)
            .copied()
            .ok_or_else(|| format!("there is no track {}", track + 1))?;
        if om::played_by(doc.tree(), t).map(|p| p.id()) == Some(node) {
            return Ok(false);
        }
        let t = t.id();
        let moved = {
            let sent = lock(&self.sent);
            sent.selected_track == Some(track) && sent.plan.current != Some(node)
        };
        // A gesture on the patch being left ends first, as its own step.
        let ended = moved && self.end_gesture(&mut doc);
        let (_, commit) = doc
            .edit("Set Plays", |tx| {
                tx.set_ref(t, om::TRACK_PATCH, Ref::here(node))
            })
            .map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        drop(doc);
        if commit.is_some() || ended {
            self.changed();
        }
        Ok(moved)
    }

    /// Ends an open envelope gesture, keeping what it did as its own step.
    /// True when one was open.
    fn end_gesture(&self, doc: &mut Document<Shard>) -> bool {
        let Some(old) = lock(&self.gesture).take() else {
            return false;
        };
        let _ = doc.end(old.id);
        true
    }

    // Files.

    /// Writes the session to `path` in rhizome's format, and remembers it.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let saved = lock(&self.doc).save_as(path).map_err(err);
        self.changed();
        saved
    }

    /// The file the session was opened from or last saved to.
    pub fn path(&self) -> Option<PathBuf> {
        self.read(|d| d.path().map(Path::to_path_buf))
    }

    pub fn is_unsaved(&self) -> bool {
        self.read(|d| d.is_unsaved())
    }

    /// Which `Untitled-N` the session is while it has no file.
    pub fn untitled_number(&self) -> u32 {
        self.untitled.load(Ordering::Relaxed)
    }

    /// Replaces the session with a new one at its defaults, `Untitled-{n}`,
    /// with an empty history. Decoded materials stay cached.
    pub fn new_document(&self, untitled: u32) -> Result<(), String> {
        let fresh = Document::<Shard>::new(FileStore).map_err(err)?;
        {
            let mut doc = lock(&self.doc);
            *doc = fresh;
            // A gesture belonged to the document it was opened on.
            lock(&self.gesture).take();
            self.untitled.store(untitled, Ordering::Relaxed);
            self.sync(&doc, true);
        }
        self.changed();
        Ok(())
    }

    /// Replaces the session with the file at `path`. Its materials are decoded
    /// before anything changes, so a slow decode never shows a half-open
    /// document. A file in the old JSON format is refused with rhizome's own
    /// error.
    pub fn open(&self, path: &Path) -> Result<LoadReport, String> {
        let (doc, load) = Document::<Shard>::open(FileStore, path).map_err(err)?;
        let pool = doc.projection().pool.clone();
        for m in &pool.materials {
            // A file that has gone is simply absent from the cache.
            let _ = self.decode(&m.path);
        }
        let mut report = LoadReport {
            applied: doc.tree().len(),
            unknown: load.issues.iter().map(ToString::to_string).collect(),
            refused: doc.projection().mod_set().1,
            ..LoadReport::default()
        };
        {
            // Every track's patch sounds, so a material missing from any of
            // them is a silent track.
            let decoded = lock(&self.decoded);
            let plan = doc.projection();
            let missing = plan
                .track_patches
                .iter()
                .flatten()
                .filter_map(|p| plan.patch_plan(*p))
                .flat_map(|p| Generator::ALL.map(|g| p.wires.get(g)))
                .chain(Generator::ALL.map(|g| pool.wires.get(g)))
                .filter_map(|id| id.and_then(|id| pool.get(id)))
                .find(|m| !decoded.contains_key(&m.path));
            if let Some(m) = missing {
                report.sample_path = m.path.clone();
                report.sample_missing = true;
            }
        }
        {
            let mut current = lock(&self.doc);
            *current = doc;
            lock(&self.gesture).take();
            self.sync(&current, true);
        }
        self.changed();
        Ok(report)
    }
}

/// What every voice should play: track *i* on voice *i*, its patch, steps
/// and fader; and a selected patch no track plays on `AUDITION`, looping
/// free, while that voice is spare and no track is selected.
fn voicing(plan: &Plan, selected_track: Option<usize>) -> [Voiced; VOICES] {
    let mut out: [Voiced; VOICES] = std::array::from_fn(|_| Voiced::default());
    let of = |patch: NodeId, steps: StepParams, gain: f32| {
        plan.patch_plan(patch).map(|p| Voiced {
            patch: Some(patch),
            values: p.values.clone(),
            modulation: p.modulation.clone(),
            wires: p.wires,
            steps: Some(steps),
            gain,
        })
    };
    let tracks = plan.tracker.tracks.len().min(VOICES);
    for (i, track) in plan.tracker.tracks.iter().take(tracks).enumerate() {
        if let Some(Some(patch)) = plan.track_patches.get(i) {
            if let Some(v) = of(*patch, plan.tracker.params(i), track.gain) {
                out[i] = v;
            }
        }
    }
    let audition = plan
        .current
        .filter(|c| selected_track.is_none() && !plan.track_patches.contains(&Some(*c)));
    if let Some(patch) = audition.filter(|_| tracks <= AUDITION) {
        let free = StepParams {
            on: false,
            ..plan.tracker.transport()
        };
        if let Some(v) = of(patch, free, TRACK_GAIN_DEFAULT) {
            out[AUDITION] = v;
        }
    }
    out
}

/// Every patch some track plays.
fn played_patches(tree: &rhizome_core::Tree) -> Vec<NodeId> {
    om::tracks(tree)
        .into_iter()
        .filter_map(|t| om::played_by(tree, t).map(|p| p.id()))
        .collect()
}

/// The patch a runtime id names.
fn patch_of(doc: &Document<Shard>, id: u64) -> Result<NodeId, String> {
    doc.projection()
        .node_of(id)
        .filter(|n| {
            doc.tree()
                .get(*n)
                .is_some_and(|p| p.type_name() == om::PATCH)
        })
        .ok_or_else(|| format!("there is no patch {id}"))
}

/// A patch's name as the scene view shows it.
fn display_name(tree: &rhizome_core::Tree, patch: NodeId) -> String {
    om::patches(tree)
        .iter()
        .enumerate()
        .find(|(_, p)| p.id() == patch)
        .map_or_else(|| "Patch".to_string(), |(i, p)| om::patch_name(p, i))
}

/// "Patch 2 copy", or "Patch 2 copy 2" when that is taken, as the Finder
/// names a duplicate.
fn copy_name(tree: &rhizome_core::Tree, name: &str) -> String {
    let taken: Vec<String> = om::patches(tree)
        .iter()
        .enumerate()
        .map(|(i, p)| om::patch_name(p, i))
        .collect();
    let first = format!("{name} copy");
    if !taken.contains(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{name} copy {n}"))
        .find(|c| !taken.contains(c))
        .unwrap_or(first)
}

/// An undo or redo commit's label without the word rhizome puts in front:
/// the webview says "Undid “Set Size”".
/// An envelope record, checked and ready to write as one edit.
struct EnvelopeWrites {
    name: String,
    mode: String,
    curve: serde_json::Value,
}

fn envelope_writes(env: &modulation::EnvelopeRecord) -> Result<EnvelopeWrites, String> {
    if !modulation::MODES.contains(&env.mode.as_str()) {
        return Err(format!("there is no envelope mode called “{}”", env.mode));
    }
    Ok(EnvelopeWrites {
        name: modulation::valid_name(&env.name, "an envelope")?,
        mode: env.mode.clone(),
        curve: om::curve_json(&env.curve),
    })
}

/// Label, mode and curve in one go, so the node's check sees the mode and the
/// curve together: an ADSR mode and a free curve never meet half-written.
fn write_envelope(tx: &mut Edit<'_>, node: NodeId, w: &EnvelopeWrites) -> rhizome_core::Result<()> {
    tx.put(node, "mod-env.label", Value::Text(w.name.clone()))?;
    tx.put(node, "mod-env.mode", Value::Choice(w.mode.clone()))?;
    tx.put(node, "mod-env.curve", Value::Shaped(w.curve.clone()))
}

fn bare(label: &str, word: &str) -> String {
    label.strip_prefix(word).unwrap_or(label).to_string()
}

/// How many modulators of `kind` a patch has.
fn count(tx: &Edit<'_>, patch: NodeId, kind: &str) -> usize {
    tx.at(patch).map_or(0, |p| {
        p.children().filter(|c| c.type_name() == kind).count()
    })
}

/// Takes every link off `key` on `target`.
fn unlink(tx: &mut Edit<'_>, target: NodeId, key: &str) -> rhizome_core::Result<()> {
    let sources: Vec<NodeId> = tx
        .at(target)
        .map(|n| {
            n.bindings()
                .iter()
                .filter(|b| matches!(b.on, On::Value(k) if k.as_str() == key))
                .map(|b| b.source.id())
                .collect()
        })
        .unwrap_or_default();
    for s in sources {
        tx.unbind(target, On::Value(key.to_string()), s)?;
    }
    Ok(())
}
