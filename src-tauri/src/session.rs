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
//! reverse write the bank directly, and the mode (tracker or sound scaping)
//! is the session's, not the document's (Georg, 2026-10-06).
//!
//! **Lock order:** `doc`, then `sent`, then `decoded`, then `feeding`, then the
//! engine's hand-off slots. Nothing here takes a controller lock; MIDI calls
//! in with its own locks already released. Decoding a WAV never happens under
//! `doc`: it happens before the edit that needs it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rhizome_core::{Edit, NodeId, On, Ref, Value};
use rhizome_pom::{Document, FileStore, PresetRef};
use serde::Serialize;
use shard_dsp::arrangement;
use shard_dsp::fx::{self, Kind as Fx};
use shard_dsp::params::PARAMS;
use shard_dsp::steps::StepBank;
use shard_dsp::{Generator, ModSet, ParamBank, ReadingBank};

use crate::mapping::MapRef;
use crate::materials::{self, Pool, DRONE_NAME};
use crate::modulation::{self, Modulation, Refused, MAX_STAGE_MS};
use crate::object_model::{
    self as om, arrangement_node, instance_of, locate, patch_node, to_value, Plan, Shard, ShardEdit,
};
use crate::source::{self, Loaded};
use crate::tracker::{self, Tracker};

/// One source buffer per generator, by `Generator::index`: a new material
/// waiting for the audio thread, or an old one it handed back.
pub type SourceSlots = [Option<Vec<f32>>; 2];

/// A material's audio by where it comes from: its file, or `None` for the
/// built-in drone. Kept when a material is removed, so undo is instant.
type Decoded = HashMap<Option<String>, Arc<Loaded>>;

/// Where the session hands things to the engine. The audio thread reads each
/// of these once a block.
pub struct Feeds {
    pub bank: Arc<ParamBank>,
    pub arrangement: Arc<ParamBank>,
    pub steps: Arc<StepBank>,
    pub mod_swap: Arc<Mutex<Option<ModSet>>>,
    pub swap: Arc<Mutex<SourceSlots>>,
    pub readings: Arc<ReadingBank>,
}

/// What a load could not take as written. Surfaced rather than logged, because
/// a half-applied document and a clean one must not look the same.
#[derive(Serialize, Default, Debug)]
pub struct LoadReport {
    pub applied: usize,
    /// What the file held that this build could not take.
    pub unknown: Vec<String>,
    pub missing: Vec<String>,
    pub sample_path: Option<String>,
    /// Set when a material a generator reads could not be found.
    pub sample_missing: bool,
    /// LFOs and links the engine could not use. They stay in the document.
    pub refused: Vec<Refused>,
    /// The name of a controller map the patch wanted and this Mac lacks.
    pub map_missing: Option<String>,
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

pub struct Session {
    doc: Mutex<Document<Shard>>,
    /// The plan the engine has, as last handed over.
    sent: Mutex<Plan>,
    /// Tracker mode: the first track plays. Not the document's.
    tracking: AtomicBool,
    decoded: Mutex<Decoded>,
    /// What each generator was last sent, by `Generator::index`: a source key,
    /// or `None` for silence.
    feeding: Mutex<[Option<Option<String>>; 2]>,
    feeds: Feeds,
    sample_rate: f32,
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
            sent: Mutex::new(Plan::default()),
            tracking: AtomicBool::new(false),
            decoded: Mutex::new([(None, drone)].into()),
            feeding: Mutex::new([None, None]),
            feeds,
            sample_rate,
        };
        session.sync(&lock(&session.doc), true);
        Ok(session)
    }

    /// Hands the engine what changed since the last hand-over, or everything.
    fn sync(&self, doc: &Document<Shard>, all: bool) {
        let plan = doc.projection();
        let mut sent = lock(&self.sent);
        if all || plan.patch != sent.patch {
            plan.write_patch(&self.feeds.bank);
        }
        if all || plan.arrangement != sent.arrangement {
            plan.write_arrangement(&self.feeds.arrangement);
        }
        if all || plan.tracker != sent.tracker {
            self.store_steps(plan);
        }
        if all || plan.modulation != sent.modulation {
            *lock(&self.feeds.mod_swap) = Some(plan.mod_set().0);
        }
        if all || plan.pool != sent.pool {
            self.send_pool(&plan.pool);
        }
        *sent = plan.clone();
    }

    fn store_steps(&self, plan: &Plan) {
        let mut p = plan.steps();
        p.on = self.tracking.load(Ordering::Relaxed);
        self.feeds.steps.store(&p);
    }

    /// Every generator's reading, and a new source only for a generator whose
    /// file changed. Never decodes: a material not yet decoded is silent.
    fn send_pool(&self, pool: &Pool) {
        let decoded = lock(&self.decoded);
        let mut feeding = lock(&self.feeding);
        let mut readings = [shard_dsp::Reading::default(); 2];
        for g in Generator::ALL {
            let m = pool.wires.get(g).and_then(|id| pool.get(id));
            let key = m
                .map(|m| m.path.clone())
                .filter(|k| decoded.contains_key(k));
            if let (Some(m), Some(_)) = (m, &key) {
                readings[g.index()] = m.reading();
            }
            if feeding[g.index()] != key {
                // Silence is an empty buffer, which allocates nothing.
                let samples = key
                    .as_ref()
                    .and_then(|k| decoded.get(k))
                    .map(|l| l.samples.clone())
                    .unwrap_or_default();
                lock(&self.feeds.swap)[g.index()] = Some(samples);
                feeding[g.index()] = key;
            }
        }
        self.feeds.readings.store(readings[0], readings[1]);
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
        Ok((value, commit.is_some()))
    }

    /// Reads the document.
    pub fn read<T>(&self, f: impl FnOnce(&Document<Shard>) -> T) -> T {
        f(&lock(&self.doc))
    }

    pub fn plan(&self) -> Plan {
        self.read(|d| d.projection().clone())
    }

    // Values.

    /// Sets a patch or arrangement parameter by id. A drag is one undo step,
    /// "Set Cutoff". Brake and reverse are played: they reach the bank and
    /// never the document.
    pub fn set_param(&self, id: &str, value: f32) -> Result<(), String> {
        if om::PLAYED.contains(&id) {
            self.feeds.bank.set_by_id(id, value);
            return Ok(());
        }
        let (node, key, want) = self
            .read(|d| {
                let (node, key) = locate(d.tree(), id)?;
                let spec = d.tree().get(node)?.node_type()?.spec(&key)?.clone();
                Some((node, key, to_value(&spec, value)?))
            })
            .ok_or_else(|| format!("unknown parameter: {id}"))?;
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
            "patch" => patch_node(d.tree()).map(|n| n.id()),
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

    /// Moves an effect one place earlier (`by` below zero) or later. False at
    /// an end.
    pub fn fx_move(&self, layer: &str, n: usize, by: i32) -> Result<bool, String> {
        let owner = self.owner(layer)?;
        let node = self.effect_at(layer, n)?;
        let mut order: Vec<NodeId> = self.read(|d| {
            d.tree()
                .get(owner)
                .map(|o| o.order(om::CHAIN).iter().map(|e| e.id()).collect())
                .unwrap_or_default()
        });
        let Some(at) = order.iter().position(|e| *e == node) else {
            return Ok(false);
        };
        let to = at as i64 + i64::from(by.signum());
        if to < 0 || to as usize >= order.len() || by == 0 {
            return Ok(false);
        }
        order.swap(at, to as usize);
        let label = fx::POOL.get(n).map_or("effect", |(k, _)| k.label());
        let (_, changed) = self.edit(&format!("Move {label}"), None, |tx| {
            tx.set_order(owner, om::CHAIN, order)
        })?;
        Ok(changed)
    }

    // History.

    pub fn undo(&self) -> Result<Option<String>, String> {
        let mut doc = lock(&self.doc);
        let commit = doc.undo().map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        Ok(commit.map(|c| bare(&c.label, "Undo ")))
    }

    pub fn redo(&self) -> Result<Option<String>, String> {
        let mut doc = lock(&self.doc);
        let commit = doc.redo().map_err(err)?;
        if commit.is_some() {
            self.sync(&doc, false);
        }
        Ok(commit.map(|c| bare(&c.label, "Redo ")))
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

    /// The tracker, its first track on in tracker mode.
    pub fn tracker(&self) -> Tracker {
        let mut t = lock(&self.sent).tracker.clone();
        if let Some(first) = t.tracks.first_mut() {
            first.on = self.tracking.load(Ordering::Relaxed);
        }
        t
    }

    /// Tracker mode or sound scaping. Not an edit.
    pub fn set_mode(&self, tracking: bool) {
        if self.tracking.swap(tracking, Ordering::Relaxed) != tracking {
            self.store_steps(&lock(&self.sent));
        }
    }

    /// Replaces the tracker. The first track's switch sets the mode and is not
    /// an edit; anything else that changed is one step, "Edit the steps".
    pub fn set_tracker(&self, next: Tracker) -> Result<Tracker, String> {
        if let Some(first) = next.tracks.first() {
            self.set_mode(first.on);
        }
        self.write_tracker("Edit the steps", Some("tracker"), next)
    }

    /// A whole-pattern edit, one step; none when it changes nothing.
    pub fn edit_tracker(&self, op: &tracker::Edit) -> Result<Tracker, String> {
        let next = self.tracker().apply(op);
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

    // Modulation.

    /// The patch's LFOs, envelopes and links, and what the engine refuses.
    pub fn modulation(&self) -> (Modulation, Vec<Refused>) {
        let plan = lock(&self.sent);
        (plan.modulation.clone(), plan.mod_set().1)
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
        let key = format!("lfo:{}", lfo.id);
        self.edit("Change LFO", Some(&key), |tx| {
            tx.put(node, "lfo.label", Value::Text(lfo.name.clone()))?;
            tx.put(node, "lfo.rate", Value::Float(rate.into()))?;
            tx.put(node, "lfo.shape", Value::Choice(lfo.shape.clone()))?;
            tx.put(
                node,
                "lfo.phase",
                Value::Float(lfo.phase.clamp(0.0, 1.0).into()),
            )
        })?;
        Ok(())
    }

    pub fn set_envelope(&self, env: modulation::EnvelopeRecord) -> Result<(), String> {
        let node = self.modulator(env.id, om::MOD_ENV)?;
        let stage = |ms: f32| Value::Float(ms.clamp(0.0, MAX_STAGE_MS).into());
        let key = format!("envelope:{}", env.id);
        self.edit("Change envelope", Some(&key), |tx| {
            tx.put(node, "mod-env.label", Value::Text(env.name.clone()))?;
            tx.put(node, "mod-env.attack", stage(env.attack))?;
            tx.put(node, "mod-env.decay", stage(env.decay))?;
            tx.put(
                node,
                "mod-env.sustain",
                Value::Float(env.sustain.clamp(0.0, 1.0).into()),
            )?;
            tx.put(node, "mod-env.release", stage(env.release))
        })?;
        Ok(())
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
            .read(|d| locate(d.tree(), id))
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
        let Some((target, key)) = self.read(|d| locate(d.tree(), id)) else {
            return Ok(());
        };
        self.edit("Unlink a parameter", None, |tx| unlink(tx, target, &key))?;
        Ok(())
    }

    // Presets.

    /// The node a preset names, such as `grain` or `fx.3.chorus`: patch only.
    fn preset_node(&self, node: &str) -> Result<NodeId, String> {
        self.read(|d| locate(d.tree(), &format!("{node}.on")).map(|(n, _)| n))
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
        let mut doc = lock(&self.doc);
        doc.save_preset(n, name).map_err(err)?;
        Ok(())
    }

    pub fn update_preset(&self, node: &str, name: &str) -> Result<(), String> {
        let n = self.preset_node(node)?;
        let mut doc = lock(&self.doc);
        doc.update_preset(n, name).map_err(err)?;
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
        let refused = doc
            .projection()
            .mod_set()
            .1
            .into_iter()
            .filter(|r| r.id.starts_with(&prefix))
            .collect();
        Ok(ApplyReport {
            applied: report.applied,
            unknown: report.skipped,
            refused,
        })
    }

    // Materials.

    pub fn pool(&self) -> Pool {
        lock(&self.sent).pool.clone()
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
                Value::Float(trim_start.clamp(0.0, 1.0).into()),
            )?;
            tx.put(
                node,
                "sample.end",
                Value::Float(trim_end.clamp(0.0, 1.0).into()),
            )?;
            tx.put(node, "sample.root", Value::Shaped(root))
        })?;
        Ok(())
    }

    // The controller map a patch plays with.

    pub fn controller_map(&self) -> Option<MapRef> {
        self.read(|d| match patch_node(d.tree())?.value("patch.map")? {
            Value::Shaped(j) => serde_json::from_value(j).ok(),
            _ => None,
        })
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

    // Files.

    /// Writes the session to `path` in rhizome's format.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        lock(&self.doc).save_as(path).map_err(err)
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
            let decoded = lock(&self.decoded);
            let missing = Generator::ALL
                .iter()
                .filter_map(|g| pool.wires.get(*g).and_then(|id| pool.get(id)))
                .find(|m| !decoded.contains_key(&m.path));
            if let Some(m) = missing {
                report.sample_path = m.path.clone();
                report.sample_missing = true;
            }
        }
        let mut current = lock(&self.doc);
        *current = doc;
        self.sync(&current, true);
        Ok(report)
    }
}

/// An undo or redo commit's label without the word rhizome puts in front:
/// the webview says "Undid “Set Size”".
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
