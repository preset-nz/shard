//! The shell. Owns the audio device, the engine, and the commands the UI calls.
//!
//! **Nothing audio crosses the Tauri boundary.** The stream runs here, in this
//! process, reading an atomic parameter bank. The webview writes to that bank
//! through `set_param` and polls `meters` at about 30 Hz. If an IPC call takes
//! a millisecond or ten, the audio thread does not stall — it reads a slightly
//! older value and carries on.
//!
//! The one thing this design genuinely cannot do is sample-accurate triggering
//! from the UI, because an event needs a sample offset and the webview has no
//! idea where the block boundary is. That is fine: triggers are meant to come
//! from the Dark Time over MIDI, which arrives in this process.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::Manager;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use shard_dsp::arrangement;
use shard_dsp::params::{ParamDef, Taper, Unit, PARAMS};
use shard_dsp::rt::{self, BlockTimer};
use shard_dsp::{
    Alone, Engine, Generator, Globals, GrainLog, GrainSpawn, Mix, ModSet, ParamBank, KITS, VOICES,
};

mod controllers;
mod document;
#[cfg(test)]
mod drums_tests;
#[cfg(test)]
mod duck_tests;
#[cfg(test)]
mod e2e;
mod extract;
#[cfg(test)]
mod globals_tests;
mod mapping;
mod materials;
mod menu;
mod midi;
#[cfg(test)]
mod mix_tests;
mod modulation;
pub mod object_model;
#[cfg(test)]
mod patch_tests;
mod profile;
mod recorder;
pub mod session;
#[cfg(test)]
mod session_tests;
// Writes a `.shard` from a flat sketch, for `just shard-write`.
#[cfg(test)]
mod play_mode_tests;
#[cfg(test)]
mod regions_tests;
#[cfg(test)]
mod section_tests;
#[cfg(test)]
mod sketch;
mod source;
mod tracker;
#[cfg(test)]
mod tracks_tests;

/// Debug builds count every allocator call made inside the audio callback,
/// and `meters` reports the running total. Release builds keep the plain
/// system allocator and always report zero. See `shard_dsp::rt`.
#[cfg(debug_assertions)]
#[global_allocator]
static GUARD: rt::GuardedAlloc = rt::GuardedAlloc;

use session::{SourceSlots, Voices};

/// Shared between the UI thread and the audio thread.
/// The live engine's grain pool. The preview renders with the same, or the
/// cloud would steal voices differently from what is heard.
const LIVE_MAX_GRAINS: usize = 256;

pub struct Audio {
    /// The open session: the document, and the engine kept in step with it.
    /// Every edit goes through here (`session.rs`).
    session: Arc<session::Session>,
    /// The current patch's values: what the patch view, the controllers and
    /// the preview read. The audio thread reads each voice's own bank.
    bank: Arc<ParamBank>,
    /// The arrangement's values: its chain over the tracks and the limiter.
    /// Read by the audio thread once a block.
    arrangement: Arc<ParamBank>,
    /// Every voice's feed, from the session to the audio thread.
    voices: Arc<Voices>,
    /// Every parameter as the voice playing the current patch last heard it,
    /// LFOs included. Written by the audio thread once a block; the bank
    /// above stays the hand's.
    heard: Arc<ParamBank>,
    /// Peak since the UI last asked, as f32 bits. Written by the audio thread.
    peak: Arc<AtomicU32>,
    /// The lowest limiter gain since the UI last asked, as f32 bits.
    reduction: Arc<AtomicU32>,
    /// Active grain count, for the UI. Written by the audio thread.
    grains: Arc<AtomicU32>,
    /// Transport, mirrored out of the audio thread for the UI.
    playing: Arc<AtomicBool>,
    /// Each voice's step, plus one; zero while it is off.
    steps: Arc<[AtomicU32; VOICES]>,
    /// Each kit's step, the same way. Each loop has its own length.
    kit_steps: Arc<[AtomicU32; KITS]>,
    /// Every grain each voice's cloud has spawned, by voice, drained by the
    /// UI at poll rate. The patch view reads the shown voice's.
    grain_logs: Vec<Arc<GrainLog>>,
    /// One grain waiting to be auditioned, with the voice whose log it came
    /// from. Handed across the same way a sample swap is: the audio thread
    /// takes it at a block boundary, never blocking.
    audition: Arc<Mutex<Option<(usize, GrainSpawn)>>>,
    /// True while a single grain is being played on its own.
    auditioning: Arc<AtomicBool>,
    /// Whether the reel is actually running backwards, which outlives the
    /// button press by the flick time. The UI lights the button from this.
    reversing: Arc<AtomicBool>,
    playhead: Arc<AtomicU32>,
    /// Set by the UI, read by the audio thread at the top of each block.
    play_request: Arc<AtomicBool>,
    /// The slowest block since `meters` last asked.
    timer: Arc<BlockTimer>,
    /// The audio-thread allocation count `meters` last logged, so a new one is
    /// printed once rather than thirty times a second.
    reported_allocs: AtomicU64,
    sample_rate: f32,
    /// The take being recorded, if any (epic 44). The callback feeds it; the
    /// commands start and stop it.
    recorder: Arc<recorder::Recorder>,
}

/// What the UI needs to build its controls. Generated from the Rust table, so
/// there is one source of truth and adding a parameter is adding a row.
#[derive(Serialize)]
pub struct ParamInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// One sentence for the row's tooltip.
    pub description: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// "linear" | "exponential" | "bipolar" | "stepped"
    pub taper: &'static str,
    /// Present only for stepped parameters.
    pub steps: Option<u32>,
    /// Names for the steps, one each, when they are choices rather than
    /// numbers; empty otherwise.
    pub step_names: &'static [&'static str],
    pub unit: &'static str,
    pub smooth_ms: f32,
}

#[derive(Serialize)]
pub struct Meters {
    pub peak: f32,
    /// The limiter's lowest gain since the last poll, one meaning it did
    /// nothing. Under one, it is holding peaks down.
    pub reduction: f32,
    pub grains: u32,
    pub playing: bool,
    /// What Play plays: the song, or the current patch on its own.
    pub play_mode: session::PlayMode,
    /// Each track's current step, from zero, or -1 while it is off, in the
    /// tracks' order: a melody track's voice's, a drums track's kit's.
    pub steps: Vec<i32>,
    /// Where the current patch's plain playback has reached, 0 to 1.
    pub playhead: f32,
    /// True while one grain is being auditioned on its own.
    pub auditioning: bool,
    /// Whether the tape is running backwards. Not the same as the reverse
    /// gate: a tap keeps this true for the flick time after you let go.
    pub reversing: bool,
    /// The slowest audio block since the last poll, in microseconds.
    pub block_us: u32,
    /// What the device allows for one block, in microseconds. A block that
    /// takes longer is a dropout.
    pub block_budget_us: u32,
    /// Allocator calls inside the audio callback since launch. Debug builds
    /// only; a release build has no guard and reports zero.
    pub audio_allocs: u64,
    /// Each track's mute and solo, as pressed and as heard, in the tracks'
    /// order. They differ while a press waits for its bar.
    pub mix: Vec<session::MixState>,
}

/// One logged grain, on its way to the UI. Mirrors `shard_dsp::GrainSpawn`,
/// which cannot derive `Serialize` because the DSP crate has no dependencies.
#[derive(Serialize, Clone, Copy)]
pub struct GrainInfo {
    /// 0 to 1 across the whole source, so it can be drawn straight onto the
    /// waveform without knowing anything about the trim.
    pub position: f32,
    /// Samples per output sample. Negative means the grain ran backwards.
    pub rate: f32,
    /// Length in samples.
    pub len: f32,
    /// 0 hard left, 1 hard right.
    pub pan: f32,
    pub window: u32,
    /// Spawn number. Strictly increasing, and the UI's key.
    pub seq: u64,
}

#[derive(Serialize)]
pub struct SourceInfo {
    pub name: String,
    pub seconds: f32,
    /// The device rate the sample was resampled to. The UI needs it to turn a
    /// grain's length in samples into a width on the drawn waveform.
    pub sample_rate: f32,
    /// Downsampled absolute peaks for drawing. Precomputed here so raw samples
    /// never cross the boundary.
    pub peaks: Vec<f32>,
}

/// The open patch's LFOs and links as the UI draws them, the global LFOs
/// beside them, and what in them the engine could not use. Every modulation
/// command answers with one, so the UI never draws a document older than its
/// own last edit.
#[derive(Serialize)]
pub struct ModulationView {
    pub lfos: Vec<modulation::LfoRecord>,
    pub envelopes: Vec<modulation::EnvelopeRecord>,
    /// The global LFOs, which any patch's link may follow. Their ids share
    /// the patch's modulators' space, so a link's `source` is one or the other.
    pub globals: Vec<modulation::LfoRecord>,
    pub links: modulation::Links,
    pub refused: Vec<modulation::Refused>,
}

impl ModulationView {
    fn of(
        doc: &modulation::Modulation,
        globals: Vec<modulation::LfoRecord>,
        refused: Vec<modulation::Refused>,
    ) -> Self {
        Self {
            lfos: doc.lfos.clone(),
            envelopes: doc.envelopes.clone(),
            globals,
            links: doc.links.clone(),
            refused,
        }
    }
}

#[tauri::command]
fn param_defs() -> Vec<ParamInfo> {
    infos(&PARAMS[..])
}

/// The arrangement's table, as `param_defs` gives the patch's.
#[tauri::command]
fn arrangement_defs() -> Vec<ParamInfo> {
    infos(arrangement::params())
}

#[tauri::command]
fn get_arrangement(state: tauri::State<'_, Audio>) -> Vec<f32> {
    let bank = &state.arrangement;
    (0..bank.defs().len()).map(|i| bank.get(i)).collect()
}

#[tauri::command]
fn set_arrangement_param(
    state: tauri::State<'_, Audio>,
    id: String,
    value: f32,
) -> Result<(), String> {
    state.session.set_param(&id, value)
}

/// Puts the first free copy of an effect at the end of a level's chain,
/// switched on (`design/effect-palette.md`). Answers with its instance number.
#[tauri::command]
fn fx_add(state: tauri::State<'_, Audio>, layer: String, kind: String) -> Result<usize, String> {
    state.session.fx_add(&layer, &kind)
}

/// Takes an effect out of a level's chain. Undo brings it back with its
/// settings.
#[tauri::command]
fn fx_remove(state: tauri::State<'_, Audio>, layer: String, n: usize) -> Result<(), String> {
    state.session.fx_remove(&layer, n)
}

/// Moves an effect one place earlier (`by` below zero) or later. Answers the
/// instance it plays afterwards, or None at an end.
#[tauri::command]
fn fx_move(
    state: tauri::State<'_, Audio>,
    layer: String,
    n: usize,
    by: i32,
) -> Result<Option<usize>, String> {
    state.session.fx_move(&layer, n, by)
}

/// The generators the current patch holds, by table prefix, in table order.
/// A generator is in the patch while its node is; its switch only mutes it.
#[tauri::command]
fn generators(state: tauri::State<'_, Audio>) -> Vec<&'static str> {
    state.session.generators()
}

/// Adds a generator to the current patch, switched on. One undo step.
#[tauri::command]
fn add_generator(state: tauri::State<'_, Audio>, kind: String) -> Result<bool, String> {
    state.session.add_generator(&kind)
}

/// Takes a generator out of the current patch. Undo brings it back with its
/// settings.
#[tauri::command]
fn remove_generator(state: tauri::State<'_, Audio>, kind: String) -> Result<bool, String> {
    state.session.remove_generator(&kind)
}

fn infos(defs: &'static [ParamDef]) -> Vec<ParamInfo> {
    defs.iter()
        .map(|p| ParamInfo {
            id: p.id,
            name: p.name,
            description: p.description,
            min: p.min,
            max: p.max,
            default: p.default,
            taper: match p.taper {
                Taper::Linear => "linear",
                Taper::Exponential => "exponential",
                Taper::Bipolar => "bipolar",
                Taper::Stepped(_) => "stepped",
            },
            steps: match p.taper {
                Taper::Stepped(n) => Some(n),
                _ => None,
            },
            step_names: p.steps,
            unit: match p.unit {
                Unit::None => "",
                Unit::Ms => "ms",
                Unit::Seconds => "s",
                Unit::Hz => "Hz",
                Unit::Semitones => "st",
                Unit::Percent => "%",
                Unit::Octaves => "oct",
                Unit::Db => "dB",
            },
            smooth_ms: p.smooth_ms,
        })
        .collect()
}

#[tauri::command]
fn get_params(state: tauri::State<'_, Audio>) -> Vec<f32> {
    (0..PARAMS.len()).map(|i| state.bank.get(i)).collect()
}

/// Every parameter as the engine last heard it on the current patch,
/// indexed as `get_params`. Differs from it only where a parameter follows an
/// LFO; the same as it while no voice plays the current patch.
#[tauri::command]
fn get_heard(state: tauri::State<'_, Audio>) -> Vec<f32> {
    let from = if state.voices.shown().is_some() {
        &state.heard
    } else {
        &state.bank
    };
    (0..PARAMS.len()).map(|i| from.get(i)).collect()
}

#[tauri::command]
fn set_param(state: tauri::State<'_, Audio>, id: String, value: f32) -> Result<(), String> {
    state.session.set_param(&id, value)
}

#[tauri::command]
fn meters(state: tauri::State<'_, Audio>) -> Meters {
    // Free whatever source buffers and modulation sets the audio thread
    // retired since the last poll. Blocking here is fine; the audio thread
    // only ever tries the locks.
    state.voices.collect_retired();

    let audio_allocs = rt::violations();
    let logged = state.reported_allocs.swap(audio_allocs, Ordering::Relaxed);
    if audio_allocs > logged {
        eprintln!(
            "shard: the audio thread touched the allocator {} more time(s), {audio_allocs} since launch",
            audio_allocs - logged
        );
    }

    Meters {
        block_us: state.timer.take_worst_us(),
        block_budget_us: state.timer.budget_us(),
        audio_allocs,
        peak: f32::from_bits(state.peak.swap(0, Ordering::Relaxed)),
        reduction: f32::from_bits(state.reduction.swap(1.0f32.to_bits(), Ordering::Relaxed)),
        grains: state.grains.load(Ordering::Relaxed),
        playing: state.playing.load(Ordering::Relaxed),
        play_mode: state.session.play_mode(),
        steps: state
            .session
            .slots()
            .into_iter()
            .map(|slot| {
                let step = match slot {
                    tracker::Slot::Melody(k) => state.steps.get(k),
                    tracker::Slot::Drums(k) => state.kit_steps.get(k),
                };
                step.map_or(-1, |s| s.load(Ordering::Relaxed) as i32 - 1)
            })
            .collect(),
        playhead: f32::from_bits(state.playhead.load(Ordering::Relaxed)),
        auditioning: state.auditioning.load(Ordering::Relaxed),
        reversing: state.reversing.load(Ordering::Relaxed),
        mix: state.session.mix(),
    }
}

/// Everything the cloud has spawned since the last call.
///
/// Drained rather than snapshotted, because a grain is an event and there is
/// no meaningful "current set" to snapshot: at the default settings about a
/// dozen exist at any instant and stopping destroys them. The UI keeps the
/// scrollback and decides what to show.
#[tauri::command]
fn grain_log(state: tauri::State<'_, Audio>) -> Vec<GrainInfo> {
    // Every log is drained each poll, so showing another patch starts from
    // now rather than dumping that voice's backlog.
    let shown = state.voices.shown();
    let mut spawns = Vec::new();
    let mut discard = Vec::new();
    for (i, log) in state.grain_logs.iter().enumerate() {
        log.drain(if Some(i) == shown {
            &mut spawns
        } else {
            &mut discard
        });
        discard.clear();
    }
    spawns
        .into_iter()
        .map(|g| GrainInfo {
            position: g.position,
            rate: g.rate,
            len: g.len,
            pan: g.pan,
            window: g.window,
            seq: g.seq,
        })
        .collect()
}

/// Play one logged grain on its own.
///
/// Refused while the transport runs, and that is deliberate rather than a
/// limitation to route around: a single grain dropped into a live cloud cannot
/// be heard, so accepting it would be a lie.
#[tauri::command]
fn audition_grain(
    state: tauri::State<'_, Audio>,
    position: f32,
    rate: f32,
    len: f32,
    pan: f32,
    window: u32,
) -> Result<(), String> {
    if state.playing.load(Ordering::Relaxed) {
        return Err("stop playback first — one grain cannot be heard inside the cloud".into());
    }
    let voice = state.voices.shown().unwrap_or(0);
    *state.audition.lock().expect("audition poisoned") = Some((
        voice,
        GrainSpawn {
            position,
            rate,
            len,
            pan,
            window,
            seq: 0,
        },
    ));
    Ok(())
}

/// The envelope as currently set, sampled across the trimmed window, for
/// drawing. Asked for when the envelope controls change rather than polled,
/// since it only moves when something is dragged.
#[tauri::command]
fn envelope_curve(state: tauri::State<'_, Audio>) -> Vec<f32> {
    let (lo, hi) = state.trim_indices();
    let env = shard_dsp::EnvParams {
        // Switched off, the envelope draws flat, which is what it sounds like.
        amount: state.bank.get_by_id("env.mix").unwrap_or(1.0)
            * state.bank.get_by_id("env.on").unwrap_or(1.0),
        attack_ms: state.bank.get_by_id("env.attack").unwrap_or(0.0),
        decay_ms: state.bank.get_by_id("env.decay").unwrap_or(0.0),
        sustain: state.bank.get_by_id("env.sustain").unwrap_or(1.0),
        release_ms: state.bank.get_by_id("env.release").unwrap_or(0.0),
    };
    env.curve(256, (hi - lo) as f32, state.sample_rate)
}

/// How many columns the patch preview's overview and zoom are drawn from.
const PREVIEW_COLUMNS: usize = 900;
const PREVIEW_ZOOM_COLUMNS: usize = 320;
/// The zoom: a window this long, this far into the pass, where the timbre is
/// rather than the attack.
const PREVIEW_ZOOM_MS: f32 = 20.0;
const PREVIEW_ZOOM_AT: f32 = 0.25;
/// The longest pass the preview draws.
const PREVIEW_MAX_S: f32 = 8.0;

/// One pass of the patch as sound scaping plays it, folded for drawing.
#[derive(Serialize)]
pub struct PatchPreview {
    /// Per column, the lowest and highest sample across both channels, so
    /// grains panned apart do not cancel.
    pub min: Vec<f32>,
    pub max: Vec<f32>,
    /// The zoom window, folded the same way. Peaks, not samples: the webview
    /// never sees a sample.
    pub zoom_min: Vec<f32>,
    pub zoom_max: Vec<f32>,
    /// Where the zoom starts, 0 to 1 of the pass.
    pub zoom_at: f32,
    pub zoom_ms: f32,
    /// How long the drawn pass is, after any cut.
    pub seconds: f32,
    /// The pass ran past the longest the preview draws.
    pub capped: bool,
}

/// Render one pass of the patch offline and fold it for drawing. The same
/// values, materials, readings and modulation as the live engine, heard
/// alone as sound scaping hears it. Async and off the main thread, because a
/// dense cloud in a debug build takes a while.
#[tauri::command]
async fn patch_preview(state: tauri::State<'_, Audio>) -> Result<PatchPreview, String> {
    let sr = state.sample_rate;
    // Snapshots, so no lock is held while rendering.
    let bank = ParamBank::new();
    for i in 0..bank.defs().len() {
        bank.set(i, state.bank.get(i));
    }
    let arr = ParamBank::for_table(arrangement::params());
    for i in 0..arr.defs().len() {
        arr.set(i, state.arrangement.get(i));
    }
    let pool = state.session.pool();
    let materials =
        Generator::ALL.map(|g| pool.wires.get(g).and_then(|id| state.session.loaded(id)));
    let readings = Generator::ALL.map(|g| Audio::reading_of(&state.session, &pool, g));
    let plan = state.session.plan();
    let mods = plan.mod_set().0;
    let globals = plan.global_set().0;

    tauri::async_runtime::spawn_blocking(move || {
        let empty: &[f32] = &[];
        let samples = |i: usize| materials[i].as_ref().map_or(empty, |l| &l.samples[..]);
        let preview = shard_dsp::preview::render_pass(shard_dsp::preview::PreviewInput {
            sample_rate: sr,
            max_grains: LIVE_MAX_GRAINS,
            bank: &bank,
            arrangement: &arr,
            player: samples(Generator::Player.index()),
            player_reading: readings[Generator::Player.index()],
            grain: samples(Generator::Grain.index()),
            grain_reading: readings[Generator::Grain.index()],
            mods,
            globals,
            max_seconds: PREVIEW_MAX_S,
        });
        fold_preview(&preview.samples, sr, preview.capped)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Min and max per column across both channels of interleaved stereo.
/// Silence, not garbage, when there is nothing to fold.
fn fold_columns(samples: &[f32], columns: usize) -> (Vec<f32>, Vec<f32>) {
    let frames = samples.len() / 2;
    let mut min = vec![0.0f32; columns];
    let mut max = vec![0.0f32; columns];
    if frames == 0 {
        return (min, max);
    }
    for c in 0..columns {
        let a = (c * frames / columns).min(frames - 1);
        let b = ((c + 1) * frames / columns).clamp(a + 1, frames);
        let (lo, hi) = samples[a * 2..b * 2]
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), s| (lo.min(*s), hi.max(*s)));
        min[c] = lo;
        max[c] = hi;
    }
    (min, max)
}

/// The overview, and the zoom window a quarter of the way in.
fn fold_preview(samples: &[f32], sr: f32, capped: bool) -> PatchPreview {
    let frames = samples.len() / 2;
    let (min, max) = fold_columns(samples, PREVIEW_COLUMNS);
    let zoom_len = ((PREVIEW_ZOOM_MS * 0.001 * sr) as usize).min(frames);
    let start = ((frames as f32 * PREVIEW_ZOOM_AT) as usize).min(frames - zoom_len);
    let (zoom_min, zoom_max) = fold_columns(
        &samples[start * 2..(start + zoom_len) * 2],
        PREVIEW_ZOOM_COLUMNS,
    );
    PatchPreview {
        min,
        max,
        zoom_min,
        zoom_max,
        zoom_at: if frames > 0 {
            start as f32 / frames as f32
        } else {
            0.0
        },
        zoom_ms: zoom_len as f32 / sr * 1000.0,
        seconds: frames as f32 / sr,
        capped,
    }
}

/// Makes the map the patch names the active one, so opening a patch or
/// undoing a map choice brings the same knobs back. Answers the name of a map
/// this Mac does not have, which leaves the active map alone. Called with the
/// session's lock released.
fn follow_map(state: &Audio, ctl: &midi::Controllers) -> Option<String> {
    let want = state.session.controller_map()?;
    let mut registry = ctl.registry.lock().expect("registry poisoned");
    if registry.active_map().is_some_and(|m| m.id == want.id) {
        return None;
    }
    if !registry.set_active(want.id) {
        return Some(want.name);
    }
    ctl.pickup.lock().expect("pickup poisoned").rearm_all();
    if let Err(e) = ctl.save(&registry) {
        eprintln!("shard: could not save controllers: {e}");
    }
    None
}

/// The preset names saved for one node, such as `grain`, in the open patch.
#[tauri::command]
fn preset_names(state: tauri::State<'_, Audio>, node: String) -> Vec<String> {
    state.session.preset_names(&node)
}

/// Store a node's current values and links as a new preset. Refuses a name in
/// use, so a preset is never overwritten by accident; that is what
/// `update_preset` is for.
#[tauri::command]
fn save_preset(state: tauri::State<'_, Audio>, node: String, name: String) -> Result<(), String> {
    state.session.save_preset(&node, &name)
}

/// Overwrite an existing preset with the node's current values and links.
#[tauri::command]
fn update_preset(state: tauri::State<'_, Audio>, node: String, name: String) -> Result<(), String> {
    state.session.update_preset(&node, &name)
}

/// Apply a preset: the node's values and links. Smoothed parameters glide to
/// their new values; the section's switch is left as it is.
#[tauri::command]
fn apply_preset(
    state: tauri::State<'_, Audio>,
    node: String,
    name: String,
) -> Result<session::ApplyReport, String> {
    state.session.apply_preset(&node, &name)
}

/// What an LFO can be set to, so the UI offers exactly what this build reads.
#[derive(Serialize)]
pub struct LfoLimits {
    /// Every shape name, in the engine's order. A wire format.
    pub shapes: Vec<&'static str>,
    pub min_rate: f32,
    pub max_rate: f32,
}

#[tauri::command]
fn lfo_limits() -> LfoLimits {
    LfoLimits {
        shapes: shard_dsp::Shape::NAMES.to_vec(),
        min_rate: shard_dsp::modulation::MIN_RATE_HZ,
        max_rate: shard_dsp::modulation::MAX_RATE_HZ,
    }
}

/// The open patch's LFOs and links, and what in them the engine refuses.
#[tauri::command]
fn modulation(state: tauri::State<'_, Audio>) -> ModulationView {
    modulation_view(&state)
}

fn modulation_view(state: &Audio) -> ModulationView {
    let (doc, refused) = state.session.modulation();
    ModulationView::of(&doc, state.session.globals(), refused)
}

/// Add an LFO. It is last in the answer's `lfos`.
#[tauri::command]
fn add_lfo(state: tauri::State<'_, Audio>) -> Result<ModulationView, String> {
    state.session.add_lfo()?;
    Ok(modulation_view(&state))
}

/// Add a global LFO, beside the patches. It is last in the answer's
/// `globals`.
#[tauri::command]
fn add_global_lfo(state: tauri::State<'_, Audio>) -> Result<ModulationView, String> {
    state.session.add_global_lfo()?;
    Ok(modulation_view(&state))
}

/// Remove an LFO, a patch's or a global one, and the links that followed it.
#[tauri::command]
fn remove_lfo(state: tauri::State<'_, Audio>, id: u64) -> Result<ModulationView, String> {
    state.session.remove_lfo(id)?;
    Ok(modulation_view(&state))
}

/// Change an LFO's name, rate, shape and phase. Running LFOs keep their place.
#[tauri::command]
fn set_lfo(
    state: tauri::State<'_, Audio>,
    lfo: modulation::LfoRecord,
) -> Result<ModulationView, String> {
    state.session.set_lfo(lfo)?;
    Ok(modulation_view(&state))
}

/// Add a modulation envelope. It is last in the answer's `envelopes`.
#[tauri::command]
fn add_envelope(state: tauri::State<'_, Audio>) -> Result<ModulationView, String> {
    state.session.add_envelope()?;
    Ok(modulation_view(&state))
}

/// Remove a modulation envelope, and the links that followed it.
#[tauri::command]
fn remove_envelope(state: tauri::State<'_, Audio>, id: u64) -> Result<ModulationView, String> {
    state.session.remove_envelope(id)?;
    Ok(modulation_view(&state))
}

/// Change a modulation envelope's name, mode and curve. `coalesce`
/// is for input with no begin and end (a nudge), which shares one undo step;
/// a drag uses the gesture commands below.
#[tauri::command]
fn set_envelope(
    state: tauri::State<'_, Audio>,
    envelope: modulation::EnvelopeRecord,
    coalesce: bool,
) -> Result<ModulationView, String> {
    state.session.set_envelope(envelope, coalesce)?;
    Ok(modulation_view(&state))
}

/// Start dragging an envelope: until the end, its changes are one undo step.
#[tauri::command]
fn envelope_gesture_begin(state: tauri::State<'_, Audio>, id: u64) -> Result<(), String> {
    state.session.envelope_gesture_begin(id)
}

/// A step of the drag: the envelope as it now stands.
#[tauri::command]
fn envelope_gesture_apply(
    state: tauri::State<'_, Audio>,
    envelope: modulation::EnvelopeRecord,
) -> Result<ModulationView, String> {
    state.session.envelope_gesture_apply(envelope)?;
    Ok(modulation_view(&state))
}

/// Let go: the drag becomes one undo step.
#[tauri::command]
fn envelope_gesture_end(state: tauri::State<'_, Audio>) -> Result<(), String> {
    state.session.envelope_gesture_end()
}

/// Escape: the envelope goes back to where the drag began, with no undo step.
#[tauri::command]
fn envelope_gesture_cancel(state: tauri::State<'_, Audio>) -> Result<ModulationView, String> {
    state.session.envelope_gesture_cancel()?;
    Ok(modulation_view(&state))
}

/// Make a parameter follow an LFO or envelope, or change the range it
/// follows over.
#[tauri::command]
fn link_param(
    state: tauri::State<'_, Audio>,
    id: String,
    source: u64,
    lo: f32,
    hi: f32,
) -> Result<ModulationView, String> {
    state.session.link_param(&id, source, lo, hi)?;
    Ok(modulation_view(&state))
}

/// Make a parameter follow a new LFO or envelope (`kind` "lfo" or
/// "envelope"), added to the current patch for it, or a new global LFO
/// ("global-lfo"), in one undo step.
#[tauri::command]
fn link_new(
    state: tauri::State<'_, Audio>,
    id: String,
    kind: String,
    lo: f32,
    hi: f32,
) -> Result<ModulationView, String> {
    state.session.link_new(&id, &kind, lo, hi)?;
    Ok(modulation_view(&state))
}

/// Stop a parameter following anything.
#[tauri::command]
fn unlink_param(state: tauri::State<'_, Audio>, id: String) -> Result<ModulationView, String> {
    state.session.unlink_param(&id)?;
    Ok(modulation_view(&state))
}

/// Put every patch parameter back to its default. One step of Undo.
#[tauri::command]
fn reset_sound(state: tauri::State<'_, Audio>) -> Result<bool, String> {
    state.session.reset_sound()
}

/// The session's patches in order, which is current, which a track plays
/// and which is auditioned (epics 10 and 28). For the scene view.
#[tauri::command]
fn patches(state: tauri::State<'_, Audio>) -> Vec<session::PatchInfo> {
    state.session.patches()
}

/// New Patch: a patch at its defaults, made current. One step of Undo. The
/// webview reads everything again along `document-opened`, and the controller
/// map follows the patch.
#[tauri::command]
fn new_patch(app: tauri::AppHandle, state: tauri::State<'_, Audio>) -> Result<u64, String> {
    let id = state.session.new_patch()?;
    state.replaced(&app, session::LoadReport::default());
    Ok(id)
}

/// Selects a patch: the one every patch command acts on, auditioned while
/// no track plays it. Not an edit, and what the tracks play never changes.
/// Read again as after New Patch when the current patch changed.
#[tauri::command]
fn select_patch(
    app: tauri::AppHandle,
    state: tauri::State<'_, Audio>,
    id: u64,
) -> Result<(), String> {
    let moved = state.session.current_patch() != Some(id);
    state.session.select_patch(id)?;
    if moved {
        state.replaced(&app, session::LoadReport::default());
    }
    Ok(())
}

/// Selects a track: its patch becomes current and nothing is auditioned.
/// Not an edit. Read again when the current patch changed.
#[tauri::command]
fn select_track(
    app: tauri::AppHandle,
    state: tauri::State<'_, Audio>,
    track: usize,
) -> Result<(), String> {
    if state.session.select_track(track)? {
        state.replaced(&app, session::LoadReport::default());
    }
    Ok(())
}

/// Add Track: a melody track at the end, playing the current patch, one step
/// of Undo, "Add Track"; with `drums`, a drums track, "Add Drums". Answers
/// its place.
#[tauri::command]
fn add_track(state: tauri::State<'_, Audio>, drums: bool) -> Result<usize, String> {
    state.session.add_track(drums)
}

/// Mutes or unmutes a track, on the next bar while playing. Not an edit.
#[tauri::command]
fn toggle_track_mute(
    state: tauri::State<'_, Audio>,
    track: usize,
) -> Result<Vec<session::MixState>, String> {
    state.session.toggle_mute(track)
}

/// Starts a take from the master or from a track's place (epic 44). Not an
/// edit: the source is session state, like mute. `armed` waits for the first
/// sound over the threshold and opens the take there.
#[tauri::command]
fn record_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, Audio>,
    source: recorder::Source,
    armed: Option<bool>,
) -> Result<recorder::RecordingState, String> {
    let mut patch = None;
    let (tap, track) = match source {
        recorder::Source::Master(_) => {
            // The master plays no one patch; the one on screen is the nearest.
            patch = state.session.current_patch();
            (shard_dsp::Tap::Master, None)
        }
        recorder::Source::Track(i) => {
            let slot = *state
                .session
                .slots()
                .get(i)
                .ok_or_else(|| format!("there is no track {}", i + 1))?;
            let tap = match slot {
                tracker::Slot::Melody(k) => {
                    patch = state.session.tracker().tracks.get(i).and_then(|t| t.patch);
                    shard_dsp::Tap::Voice(k)
                }
                // A kit has no patch.
                tracker::Slot::Drums(k) => shard_dsp::Tap::Kit(k),
            };
            (tap, Some(session::track_name(slot)))
        }
    };
    let patch = patch.and_then(|id| {
        state
            .session
            .patches()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.name)
    });
    let meta = recorder::TakeMeta {
        product: app.package_info().name.clone(),
        document: state.session.path().map(|p| p.canonicalize().unwrap_or(p)),
        patch,
        track: track.clone(),
        source: if track.is_some() { "track" } else { "master" },
    };
    let begin = if armed.unwrap_or(false) {
        recorder::Recorder::arm
    } else {
        recorder::Recorder::start
    };
    begin(
        &state.recorder,
        &recorder::materials_dir()?,
        tap,
        source,
        track.as_deref(),
        meta,
        std::time::SystemTime::now(),
    )?;
    Ok(state.recorder.state())
}

/// Ends the take, finishes the WAV and adds it to the materials as one undo
/// step, "Add Take". Stopping an armed take no sound ever started writes
/// nothing and answers `None`.
#[tauri::command]
fn record_stop(state: tauri::State<'_, Audio>) -> Result<Option<recorder::TakeInfo>, String> {
    let Some(take) = state.recorder.stop_or_disarm()? else {
        return Ok(None);
    };
    state.session.add_take(&take.path)?;
    Ok(Some(take))
}

/// Whether a take is being recorded, from what, for how long, and how many
/// frames the disk made us drop.
#[tauri::command]
fn recording(state: tauri::State<'_, Audio>) -> recorder::RecordingState {
    state.recorder.state()
}

/// Solos or unsolos a track, on the next bar while playing. Not an edit.
#[tauri::command]
fn toggle_track_solo(
    state: tauri::State<'_, Audio>,
    track: usize,
) -> Result<Vec<session::MixState>, String> {
    state.session.toggle_solo(track)
}

/// Remove Track: one step of Undo, "Remove Track 2" or "Remove Drums 1".
/// Refused for the last melody track.
#[tauri::command]
fn remove_track(state: tauri::State<'_, Audio>, track: usize) -> Result<(), String> {
    state.session.remove_track(track)
}

/// Renames a patch. One step of Undo, "Rename Patch 2"; an empty or
/// unchanged name makes none. Answers whether it changed.
#[tauri::command]
fn rename_patch(state: tauri::State<'_, Audio>, id: u64, name: String) -> Result<bool, String> {
    state.session.rename_patch(id, &name)
}

/// Deletes a patch, refused for the last one and one a track plays.
/// One step of Undo, "Delete Patch 2". When it was current the track's patch
/// takes over, and the webview reads everything again along
/// `document-opened`.
#[tauri::command]
fn delete_patch(
    app: tauri::AppHandle,
    state: tauri::State<'_, Audio>,
    id: u64,
) -> Result<(), String> {
    if state.session.delete_patch(id)? {
        state.replaced(&app, session::LoadReport::default());
    }
    Ok(())
}

/// Duplicates a patch, straight after it, and makes the copy current. One
/// step of Undo, "Duplicate Patch 2". Read again as after New Patch.
#[tauri::command]
fn duplicate_patch(
    app: tauri::AppHandle,
    state: tauri::State<'_, Audio>,
    id: u64,
) -> Result<u64, String> {
    let copy = state.session.duplicate_patch(id)?;
    state.replaced(&app, session::LoadReport::default());
    Ok(copy)
}

/// Sets the patch track `track` plays (its *Plays* picker): one step of
/// Undo, "Set Plays". With that track selected the current patch follows,
/// and the webview reads everything again as after selecting a patch.
#[tauri::command]
fn set_track_patch(
    app: tauri::AppHandle,
    state: tauri::State<'_, Audio>,
    track: usize,
    id: u64,
) -> Result<(), String> {
    if state.session.set_track_patch(track, id)? {
        state.replaced(&app, session::LoadReport::default());
    }
    Ok(())
}

/// Sets which track ducks track `track`, by its place, or none: one step of
/// Undo, "Set Ducked by" (roadmap row 41). A track can't be ducked by
/// itself.
#[tauri::command]
fn set_track_duck(
    state: tauri::State<'_, Audio>,
    track: usize,
    key: Option<usize>,
) -> Result<tracker::Tracker, String> {
    state.session.set_track_duck(track, key)?;
    Ok(state.session.tracker())
}

/// Start or stop playback. Stopping clears the grain pool, so stop is stop.
#[tauri::command]
fn set_playing(state: tauri::State<'_, Audio>, playing: bool) {
    state.play_request.store(playing, Ordering::Relaxed);
}

/// Chooses what Play plays: `song`, `loop` (the current patch alone, held
/// for ever) or `once` (the current patch alone, one pass, then the
/// transport stops by itself). Not an edit: no undo step, no unsaved mark,
/// not saved in the document. Answers with the mode now set.
#[tauri::command]
fn set_play_mode(state: tauri::State<'_, Audio>, mode: session::PlayMode) -> session::PlayMode {
    state.session.set_play_mode(mode);
    state.session.play_mode()
}

/// The tracker, as saved and as the engine plays it, each track with the
/// patch it plays.
#[tauri::command]
fn tracker(state: tauri::State<'_, Audio>) -> tracker::Tracker {
    state.session.tracker()
}

/// Takes the tempo, swing and each track's steps or grid, length, fader and
/// kit from `tracker`, by place; never adds, removes or re-patches a track. Answers
/// with what the engine now plays, which differs from what was sent only
/// where a value was out of range.
#[tauri::command]
fn set_tracker(
    state: tauri::State<'_, Audio>,
    tracker: tracker::Tracker,
) -> Result<tracker::Tracker, String> {
    state.session.set_tracker(tracker)
}

/// A whole-pattern edit on track `track`: clear, paste or repeat a bar,
/// rotate, transpose (`tracker::Edit`). One step of Undo, and none when it
/// changes nothing.
#[tauri::command]
fn edit_tracker(
    state: tauri::State<'_, Audio>,
    track: usize,
    edit: tracker::Edit,
) -> Result<tracker::Tracker, String> {
    state.session.edit_tracker(track, &edit)
}

/// A material's waveform for drawing. Refused for a material whose file could
/// not be read.
#[tauri::command]
fn material_wave(state: tauri::State<'_, Audio>, id: u64) -> Result<SourceInfo, String> {
    let loaded = state
        .session
        .loaded(id)
        .ok_or_else(|| format!("material {id} could not be read"))?;
    Ok(SourceInfo {
        name: loaded.name.clone(),
        seconds: loaded.samples.len() as f32 / state.sample_rate,
        sample_rate: state.sample_rate,
        peaks: loaded.peaks.clone(),
    })
}

/// A material's peaks over part of its length, for a zoomed waveform: `buckets`
/// absolute peaks across the fractions `start` to `end` of the whole. Peaks
/// only, so the webview still never sees a sample.
#[tauri::command]
fn material_peaks(
    state: tauri::State<'_, Audio>,
    id: u64,
    start: f32,
    end: f32,
    buckets: usize,
) -> Result<Vec<f32>, String> {
    let loaded = state
        .session
        .loaded(id)
        .ok_or_else(|| format!("material {id} could not be read"))?;
    Ok(source::peaks_range(&loaded.samples, start, end, buckets))
}

/// A material as the tree and the inspector show it.
#[derive(Serialize)]
pub struct MaterialView {
    pub id: u64,
    pub name: String,
    /// None for the built-in drone.
    pub path: Option<String>,
    /// The file is not where the document said it was.
    pub missing: bool,
    pub octave: f32,
    pub trim_start: f32,
    pub trim_end: f32,
    /// The note it sounds at, as a MIDI number, or none.
    pub root: Option<u8>,
    /// Marked regions, as fractions of the file.
    pub regions: Vec<materials::Region>,
}

/// The pool in the tree's order, and what each generator reads.
#[derive(Serialize)]
pub struct MaterialsView {
    pub materials: Vec<MaterialView>,
    pub wires: materials::Wires,
}

impl MaterialsView {
    fn of(pool: &materials::Pool) -> Self {
        Self {
            materials: pool
                .materials
                .iter()
                .map(|m| MaterialView {
                    id: m.id,
                    name: m.name.clone(),
                    path: m.path.clone(),
                    missing: m
                        .path
                        .as_deref()
                        .is_some_and(|p| !std::path::Path::new(p).exists()),
                    octave: m.octave,
                    trim_start: m.trim_start,
                    trim_end: m.trim_end,
                    root: m.root,
                    regions: m.regions.clone(),
                })
                .collect(),
            wires: pool.wires,
        }
    }
}

// The material commands that can decode are async: they run off the main
// thread, so a long decode never freezes the window. Each decodes before it
// changes anything, so a file that will not load leaves the session as it was.

#[tauri::command]
fn materials(state: tauri::State<'_, Audio>) -> MaterialsView {
    MaterialsView::of(&state.session.pool())
}

/// Add a WAV to the pool, whole and at its own pitch. A generator with no
/// material yet reads it, so the first file added is heard at once.
#[tauri::command(async)]
fn add_material(state: tauri::State<'_, Audio>, path: String) -> Result<MaterialsView, String> {
    state.session.add_material(&path)?;
    Ok(MaterialsView::of(&state.session.pool()))
}

/// Add the built-in drone back to the pool, after it was removed. Like a WAV,
/// a generator with no material yet reads it.
#[tauri::command]
fn add_drone(state: tauri::State<'_, Audio>) -> Result<MaterialsView, String> {
    state.session.add_drone()?;
    Ok(MaterialsView::of(&state.session.pool()))
}

/// Remove a material. A generator that read it goes silent.
#[tauri::command]
fn remove_material(state: tauri::State<'_, Audio>, id: u64) -> Result<MaterialsView, String> {
    state.session.remove_material(id)?;
    Ok(MaterialsView::of(&state.session.pool()))
}

/// Wire a material into a generator, `material` (Sample) or `grain`
/// (Granular), or unwire it with none, which is silent. A material whose file
/// cannot be read is refused.
#[tauri::command(async)]
fn wire_material(
    state: tauri::State<'_, Audio>,
    node: String,
    material: Option<u64>,
) -> Result<MaterialsView, String> {
    state.session.wire_material(&node, material)?;
    Ok(MaterialsView::of(&state.session.pool()))
}

/// Set a material's octave, trim and root note. Every generator reading it
/// follows at the next block.
#[tauri::command]
fn set_material(
    state: tauri::State<'_, Audio>,
    id: u64,
    octave: f32,
    trim_start: f32,
    trim_end: f32,
    root: Option<u8>,
) -> Result<MaterialsView, String> {
    state
        .session
        .set_material(id, octave, trim_start, trim_end, root)?;
    Ok(MaterialsView::of(&state.session.pool()))
}

/// A region just added: the pool, and the index it took.
#[derive(Serialize)]
pub struct RegionAdded {
    pub materials: MaterialsView,
    pub index: usize,
}

/// Mark a region of a material by hand (roadmap row 49), in fractions of the
/// file. One undo step.
#[tauri::command]
fn add_region(
    state: tauri::State<'_, Audio>,
    id: u64,
    start: f32,
    end: f32,
) -> Result<RegionAdded, String> {
    let index = state.session.add_region(id, start, end)?;
    Ok(RegionAdded {
        materials: MaterialsView::of(&state.session.pool()),
        index,
    })
}

#[tauri::command]
fn remove_region(
    state: tauri::State<'_, Audio>,
    id: u64,
    index: usize,
) -> Result<MaterialsView, String> {
    state.session.remove_region(id, index)?;
    Ok(MaterialsView::of(&state.session.pool()))
}

/// Move or resize a region. A drag is one undo step.
#[tauri::command]
fn set_region(
    state: tauri::State<'_, Audio>,
    id: u64,
    index: usize,
    start: f32,
    end: f32,
) -> Result<MaterialsView, String> {
    state.session.set_region(id, index, start, end)?;
    Ok(MaterialsView::of(&state.session.pool()))
}

/// Replace a material's regions with the ones its transients make. Runs off
/// the main thread: it walks the whole file.
#[tauri::command(async)]
fn detect_regions(
    state: tauri::State<'_, Audio>,
    id: u64,
    sensitivity: f32,
) -> Result<MaterialsView, String> {
    state.session.detect_regions(id, sensitivity)?;
    Ok(MaterialsView::of(&state.session.pool()))
}

/// A region written out as a new material.
#[derive(Serialize)]
pub struct Extracted {
    pub materials: MaterialsView,
    pub name: String,
}

/// Write region `index` of a material to a new WAV in Materials and add it to
/// the pool, wired to nothing. One undo step, "Extract".
#[tauri::command(async)]
fn extract_region(
    app: tauri::AppHandle,
    state: tauri::State<'_, Audio>,
    id: u64,
    index: usize,
) -> Result<Extracted, String> {
    let path = state.session.extract_region_to(
        &recorder::materials_dir()?,
        id,
        index,
        &app.package_info().name,
        &recorder::date_stamp(std::time::SystemTime::now()),
    )?;
    let name = std::path::Path::new(&path)
        .file_name()
        .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
    Ok(Extracted {
        materials: MaterialsView::of(&state.session.pool()),
        name,
    })
}

/// Open the device and start the stream.
///
/// `cpal::Stream` is not `Send` on macOS, so it cannot live in Tauri's managed
/// state. Instead a dedicated thread builds it, plays it, and then parks
/// forever holding it — dropping the stream would stop the device. Only the
/// atomics and queues cross back, and those are all `Send`.
/// A device as Settings → Controllers lists it.
#[derive(Serialize)]
pub struct DeviceView {
    pub port: String,
    pub name: String,
    pub connected: bool,
    /// The profile's name when shard drives this device, else `None`.
    pub surface: Option<&'static str>,
}

/// A control as Settings → Controllers lists it, with its activity.
#[derive(Serialize)]
pub struct ControlView {
    pub id: u64,
    pub device: String,
    pub channel: u8,
    /// "cc" | "note"
    pub kind: &'static str,
    pub number: u8,
    /// "knob" | "pad"
    pub role: &'static str,
    /// "absolute" for a pot, or the encoding for an endless encoder.
    pub mode: &'static str,
    /// What this channel is called: "Ch 5", or the name you gave it.
    pub bank: String,
    pub name: String,
    /// Moved within the last quarter second.
    pub active: bool,
    /// The last value it sent, 0 to 127, once it has sent one.
    pub last: Option<u8>,
    /// For an endless encoder, what that last value decoded to. This is the
    /// turn-left test: one click anticlockwise reads −1 under the right
    /// encoding and something absurd under the wrong one.
    pub delta: Option<i8>,
}

#[derive(Serialize)]
pub struct CalibrateReport {
    pub seq: u64,
    pub text: String,
}

#[derive(Serialize)]
pub struct ControllersView {
    pub devices: Vec<DeviceView>,
    pub controls: Vec<ControlView>,
    pub roles: &'static [&'static str],
    pub modes: &'static [&'static str],
    /// The control being calibrated, and what to tell the person to do.
    pub calibrating: Option<u64>,
    pub instruction: Option<&'static str>,
    /// What calibration last worked out, in plain words.
    pub found: Option<CalibrateReport>,
    /// What went wrong with `controllers.json` at startup, if anything. The
    /// panel says it, because starting with no controllers otherwise looks
    /// like the app forgot them on purpose.
    pub trouble: Option<String>,
}

impl ControllersView {
    fn of(c: &midi::Controllers) -> Self {
        let registry = c.registry.lock().expect("registry poisoned");
        let activity = c.activity.lock().expect("activity poisoned");
        let connected = c.connected.lock().expect("connected poisoned");
        let cal = c.calibrate.lock().expect("calibrate poisoned");
        let now = Instant::now();
        ControllersView {
            devices: registry
                .devices
                .iter()
                .map(|d| DeviceView {
                    port: d.port.clone(),
                    name: d.name.clone(),
                    connected: connected.contains(&d.port),
                    // A device shard drives rather than learns. Its controls
                    // are not listed because they are not yours to map.
                    surface: profile::for_input(&d.port).map(|p| p.name),
                })
                .collect(),
            controls: registry
                .controls
                .iter()
                .map(|k| {
                    let (active, last) = activity.of(k.id, now);
                    ControlView {
                        id: k.id,
                        device: k.address.device.clone(),
                        channel: k.address.channel,
                        kind: match k.address.kind {
                            controllers::Kind::Cc => "cc",
                            controllers::Kind::Note => "note",
                        },
                        number: k.address.number,
                        role: match k.role {
                            controllers::Role::Knob => "knob",
                            controllers::Role::Pad => "pad",
                        },
                        mode: k.mode.name(),
                        bank: registry.bank_name(&k.address.device, k.address.channel),
                        name: k.name.clone(),
                        active,
                        last,
                        delta: last.and_then(|v| k.mode.delta(v)),
                    }
                })
                .collect(),
            roles: &controllers::Role::NAMES,
            modes: &controllers::Mode::NAMES,
            calibrating: cal.active.as_ref().map(|c| c.control),
            instruction: cal.active.as_ref().map(|c| c.instruction()),
            found: cal.report.as_ref().map(|(seq, text)| CalibrateReport {
                seq: *seq,
                text: text.clone(),
            }),
            trouble: c.trouble.lock().expect("trouble poisoned").clone(),
        }
    }
}

/// Every device and control learned so far, with what moved just now. Polled
/// while Settings is open.
#[tauri::command]
fn controllers(state: tauri::State<'_, Arc<midi::Controllers>>) -> ControllersView {
    ControllersView::of(&state)
}

/// Apply one edit to the registry, save it, and answer with the new view.
fn edit_controllers(
    c: &midi::Controllers,
    edit: impl FnOnce(&mut controllers::Registry) -> Result<(), String>,
) -> Result<(), String> {
    let mut registry = c.registry.lock().expect("registry poisoned");
    edit(&mut registry)?;
    c.save(&registry)
}

#[tauri::command]
fn rename_control(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    id: u64,
    name: String,
) -> Result<ControllersView, String> {
    edit_controllers(&state, |r| r.rename_control(id, &name))?;
    Ok(ControllersView::of(&state))
}

#[tauri::command]
fn set_control_role(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    id: u64,
    role: String,
) -> Result<ControllersView, String> {
    let role = controllers::Role::parse(&role).ok_or_else(|| format!("unknown role: {role}"))?;
    edit_controllers(&state, |r| r.set_role(id, role))?;
    Ok(ControllersView::of(&state))
}

/// Say whether a knob is a pot or an endless encoder, and in which encoding.
/// Never inferred: see `controllers::Mode`.
#[tauri::command]
fn set_control_mode(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    id: u64,
    mode: String,
) -> Result<ControllersView, String> {
    let mode = controllers::Mode::parse(&mode).ok_or_else(|| format!("unknown mode: {mode}"))?;
    edit_controllers(&state, |r| r.set_mode(id, mode))?;
    // An encoder that was a pot may have left a slot armed behind it.
    state.pickup.lock().expect("pickup poisoned").forget(id);
    Ok(ControllersView::of(&state))
}

/// Work out whether a knob is a pot or an endless encoder by turning it.
/// The alternative is asking the person which of three 7-bit encodings their
/// hardware speaks, which is not a reasonable question.
#[tauri::command]
fn calibrate_control(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    id: u64,
) -> Result<ControllersView, String> {
    {
        let registry = state.registry.lock().expect("registry poisoned");
        let c = registry
            .control(id)
            .ok_or_else(|| format!("no control {id}"))?;
        if c.role != controllers::Role::Knob {
            return Err(format!("{} is a pad, so there is nothing to turn", c.name));
        }
    }
    let mut cal = state.calibrate.lock().expect("calibrate poisoned");
    cal.active = Some(controllers::Calibration::new(id));
    // The last answer was about a different knob. Do not leave it standing
    // next to a question about this one.
    cal.report = None;
    drop(cal);
    Ok(ControllersView::of(&state))
}

#[tauri::command]
fn cancel_calibrate(state: tauri::State<'_, Arc<midi::Controllers>>) -> ControllersView {
    state.calibrate.lock().expect("calibrate poisoned").active = None;
    ControllersView::of(&state)
}

/// Name a device's channel, so the control list reads as sections. Blank
/// hands it back its default, "Ch N".
#[tauri::command]
fn rename_bank(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    port: String,
    channel: u8,
    name: String,
) -> Result<ControllersView, String> {
    edit_controllers(&state, |r| r.rename_bank(&port, channel, &name))?;
    Ok(ControllersView::of(&state))
}

#[tauri::command]
fn forget_control(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    id: u64,
) -> Result<ControllersView, String> {
    edit_controllers(&state, |r| r.forget_control(id))?;
    state.activity.lock().expect("activity poisoned").forget(id);
    Ok(ControllersView::of(&state))
}

#[tauri::command]
fn rename_device(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    port: String,
    name: String,
) -> Result<ControllersView, String> {
    edit_controllers(&state, |r| r.rename_device(&port, &name))?;
    Ok(ControllersView::of(&state))
}

#[tauri::command]
fn forget_device(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    port: String,
) -> Result<ControllersView, String> {
    edit_controllers(&state, |r| r.forget_device(&port))?;
    Ok(ControllersView::of(&state))
}

/// One mapped parameter as its row draws it.
#[derive(Serialize)]
pub struct MappingView {
    pub control: u64,
    pub control_name: String,
    /// Waiting for the knob to pass through the value.
    pub armed: bool,
    /// Where the knob physically is, 0 to 1, once it has said.
    pub knob: Option<f32>,
}

#[derive(Serialize)]
pub struct MapInfo {
    pub id: u64,
    pub name: String,
}

/// A control a row can pick from its menu.
#[derive(Serialize)]
pub struct KnobInfo {
    pub id: u64,
    pub name: String,
    pub device: String,
    /// The channel's name. Device plus bank is the namespace the menu groups
    /// by, so a list of forty controls reads as a few short sections.
    pub bank: String,
    /// True for an endless encoder, so a row can say so rather than draw a
    /// ghost mark it will never move.
    pub relative: bool,
}

#[derive(Serialize)]
pub struct MappingsView {
    pub active: Option<MapInfo>,
    pub maps: Vec<MapInfo>,
    /// Every knob known, so a row can choose one rather than learn it.
    pub knobs: Vec<KnobInfo>,
    /// By parameter id, in the active map.
    pub mappings: std::collections::BTreeMap<String, MappingView>,
    /// The parameter waiting for a control to move.
    pub learning: Option<String>,
    pub report: Option<LearnReport>,
}

#[derive(Serialize)]
pub struct LearnReport {
    pub seq: u64,
    pub ok: bool,
    pub text: String,
}

impl MappingsView {
    fn of(c: &midi::Controllers) -> Self {
        let registry = c.registry.lock().expect("registry poisoned");
        let pickup = c.pickup.lock().expect("pickup poisoned");
        let learn = c.learn.lock().expect("learn poisoned");
        let active = registry.active_map();
        let mut mappings = std::collections::BTreeMap::new();
        if let Some(map) = active {
            for (control, target) in &map.mappings {
                let mapping::Target::Param(id) = target;
                let Some((bank, index)) = c.locate(id) else {
                    continue;
                };
                let name = registry
                    .control(*control)
                    .map(|k| k.name.clone())
                    .unwrap_or_default();
                let relative = registry
                    .control(*control)
                    .is_some_and(|k| k.mode.is_relative());
                let (armed, knob) =
                    pickup.state(*control, relative, &bank.defs()[index], bank.get(index));
                mappings.insert(
                    id.clone(),
                    MappingView {
                        control: *control,
                        control_name: name,
                        armed,
                        knob,
                    },
                );
            }
        }
        MappingsView {
            active: active.map(|m| MapInfo {
                id: m.id,
                name: m.name.clone(),
            }),
            maps: registry
                .maps
                .iter()
                .map(|m| MapInfo {
                    id: m.id,
                    name: m.name.clone(),
                })
                .collect(),
            knobs: registry
                .controls
                .iter()
                .filter(|k| k.role == controllers::Role::Knob)
                .map(|k| KnobInfo {
                    id: k.id,
                    name: k.name.clone(),
                    device: registry
                        .devices
                        .iter()
                        .find(|d| d.port == k.address.device)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| k.address.device.clone()),
                    bank: registry.bank_name(&k.address.device, k.address.channel),
                    relative: k.mode.is_relative(),
                })
                .collect(),
            mappings,
            learning: learn.waiting.as_ref().map(|t| t.describe()),
            report: learn.report.as_ref().map(|(seq, r)| LearnReport {
                seq: *seq,
                ok: r.is_ok(),
                text: match r {
                    Ok(t) | Err(t) => t.clone(),
                },
            }),
        }
    }
}

/// The active map as the rows draw it. Polled with the parameters.
#[tauri::command]
fn mappings(state: tauri::State<'_, Arc<midi::Controllers>>) -> MappingsView {
    MappingsView::of(&state)
}

/// Wait for the next control to move and bind it to `id`.
#[tauri::command]
fn learn_midi(state: tauri::State<'_, Arc<midi::Controllers>>, id: String) -> Result<(), String> {
    if state.locate(&id).is_none() {
        return Err(format!("unknown parameter: {id}"));
    }
    state.learn.lock().expect("learn poisoned").waiting = Some(mapping::Target::Param(id));
    Ok(())
}

/// Point a control you already know at `id`, no touching needed.
#[tauri::command]
fn map_midi(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    id: String,
    control: u64,
) -> Result<(), String> {
    if state.locate(&id).is_none() {
        return Err(format!("unknown parameter: {id}"));
    }
    let said = {
        let mut registry = state.registry.lock().expect("registry poisoned");
        let said = registry.learn(control, mapping::Target::Param(id))?;
        state.save(&registry)?;
        said
    };
    state
        .pickup
        .lock()
        .expect("pickup poisoned")
        .forget(control);
    state.learn.lock().expect("learn poisoned").say(Ok(said));
    Ok(())
}

#[tauri::command]
fn cancel_learn(state: tauri::State<'_, Arc<midi::Controllers>>) {
    state.learn.lock().expect("learn poisoned").waiting = None;
}

/// Take `id` off its control in the active map.
#[tauri::command]
fn forget_midi(state: tauri::State<'_, Arc<midi::Controllers>>, id: String) -> Result<(), String> {
    edit_controllers(&state, |r| r.unlearn(&mapping::Target::Param(id)))
}

/// Records the active map in the patch, so opening it brings the same knobs
/// back. Called with every controller lock released.
fn record_active_map(audio: &Audio, ctl: &midi::Controllers) {
    let active = ctl
        .registry
        .lock()
        .expect("registry poisoned")
        .active_map()
        .map(|m| mapping::MapRef {
            id: m.id,
            name: m.name.clone(),
        });
    if let Err(e) = audio.session.set_controller_map(active.as_ref()) {
        eprintln!("shard: could not record the controller map: {e}");
    }
}

#[tauri::command]
fn set_active_map(
    audio: tauri::State<'_, Audio>,
    state: tauri::State<'_, Arc<midi::Controllers>>,
    id: u64,
) -> Result<MappingsView, String> {
    edit_controllers(&state, |r| {
        if r.set_active(id) {
            Ok(())
        } else {
            Err(format!("no map {id}"))
        }
    })?;
    state.pickup.lock().expect("pickup poisoned").rearm_all();
    record_active_map(&audio, &state);
    Ok(MappingsView::of(&state))
}

#[tauri::command]
fn add_map(
    audio: tauri::State<'_, Audio>,
    state: tauri::State<'_, Arc<midi::Controllers>>,
    name: String,
) -> Result<MappingsView, String> {
    edit_controllers(&state, |r| {
        let id = r.add_map(&name)?;
        r.set_active(id);
        Ok(())
    })?;
    state.pickup.lock().expect("pickup poisoned").rearm_all();
    record_active_map(&audio, &state);
    Ok(MappingsView::of(&state))
}

#[tauri::command]
fn rename_map(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    id: u64,
    name: String,
) -> Result<MappingsView, String> {
    edit_controllers(&state, |r| r.rename_map(id, &name))?;
    Ok(MappingsView::of(&state))
}

impl Audio {
    /// How a generator reads: through its material's octave and trim, or at
    /// the defaults when it has no readable material and so reads nothing.
    fn reading_of(
        session: &session::Session,
        pool: &materials::Pool,
        g: Generator,
    ) -> shard_dsp::Reading {
        pool.wires
            .get(g)
            .filter(|id| session.loaded(*id).is_some())
            .and_then(|id| pool.get(id))
            .map(|m| m.reading())
            .unwrap_or_default()
    }

    /// Plain playback's trimmed window in sample indices, worked out as the
    /// engine does, so a drawn envelope matches the one heard.
    fn trim_indices(&self) -> (usize, usize) {
        let pool = self.session.pool();
        let len = pool
            .wires
            .get(Generator::Player)
            .and_then(|id| self.session.loaded(id))
            .map_or(0, |l| l.samples.len());
        Self::reading_of(&self.session, &pool, Generator::Player).window(len)
    }
}

/// What the audio callback holds between blocks: materials and modulation
/// sets the engine gave back, waiting for their voice's retired slot to come
/// free. Sized here, so a block never allocates.
pub(crate) struct Held {
    retiring: [SourceSlots; VOICES],
    mods: [Option<ModSet>; VOICES],
    /// The global LFOs the engine gave back.
    globals: Option<Globals>,
    /// Each voice's `VoiceFeed::placed` as last seen, and each kit's.
    placed: [u32; VOICES],
    kit_placed: [u32; KITS],
}

impl Held {
    pub(crate) fn new() -> Self {
        Self {
            retiring: std::array::from_fn(|_| [None, None]),
            mods: std::array::from_fn(|_| None),
            globals: None,
            placed: [0; VOICES],
            kit_placed: [0; KITS],
        }
    }
}

/// One block's hand-offs from the session's feeds into the engine, voice by
/// voice: buffers and sets given back, new materials (once the voice is
/// silent) and modulation taken, then the steps, readings, fader and mix,
/// and last whether it plays; then the transport and the arrangement. Locks are only ever tried: a slot the
/// command thread holds this block is taken on the next.
pub(crate) fn hand_over(
    engine: &mut Engine,
    voices: &Voices,
    arrangement: &ParamBank,
    held: &mut Held,
) {
    // The global LFOs first, so a voice's new link to a new global finds it
    // in the same block. One set for the engine, handed across as a voice's
    // modulation is.
    if let Some(old) = held.globals.take() {
        match voices.globals_retired.try_lock() {
            Ok(mut slot) if slot.is_none() => *slot = Some(old),
            _ => held.globals = Some(old),
        }
    }
    if held.globals.is_none() {
        if let Ok(mut pending) = voices.globals_swap.try_lock() {
            if let Some(set) = pending.take() {
                held.globals = Some(engine.set_globals(set));
            }
        }
    }
    for (i, feed) in voices.voice.iter().enumerate() {
        // A retired source waits here until `meters` collects it, and no new
        // swap is taken while one is waiting, so a buffer is never freed on
        // this thread. Each generator's source swaps on its own.
        let retiring = &mut held.retiring[i];
        if retiring.iter().any(Option::is_some) {
            if let Ok(mut slot) = feed.retired.try_lock() {
                if slot.iter().all(Option::is_none) {
                    *slot = std::mem::take(retiring);
                }
            }
        }
        // A new material for a voice that is sounding waits for it to fade
        // out: swapping under it would cut its pass mid-sample (a track
        // moving down a voice, an audition or a pick changing material). It
        // stays in its slot, the voice is switched out, and once idle the
        // swap lands and the voice wakes clean on it.
        let mut parked = false;
        if retiring.iter().all(Option::is_none) {
            if let Ok(mut pending) = feed.swap.try_lock() {
                let waiting = pending.iter().any(Option::is_some);
                if waiting && engine.playing() && !engine.voice_idle(i) {
                    parked = true;
                } else {
                    for g in Generator::ALL {
                        if let Some(buf) = pending[g.index()].take() {
                            retiring[g.index()] = Some(engine.swap_voice_source(i, g, buf));
                        }
                    }
                }
            }
        }
        // LFOs and links arrive the same way, with their own pair of slots.
        // The engine carries running LFOs across by id.
        if let Some(old) = held.mods[i].take() {
            match feed.mod_retired.try_lock() {
                Ok(mut slot) if slot.is_none() => *slot = Some(old),
                _ => held.mods[i] = Some(old),
            }
        }
        if held.mods[i].is_none() {
            if let Ok(mut pending) = feed.mod_swap.try_lock() {
                if let Some(set) = pending.take() {
                    held.mods[i] = Some(engine.set_voice_modulation(i, set));
                }
            }
        }
        engine.set_voice_steps(i, feed.steps.load());
        let (player, grain) = feed.readings.load();
        engine.set_voice_readings(i, player, grain);
        engine.set_voice_gain(i, feed.gain());
        engine.set_voice_duck(i, feed.duck.load());
        // Mute and solo: a press lands on the next bar; a track arriving on
        // the voice brings its switches at once. Before the switch, so a
        // voice wakes with its gate where its track has it.
        let placed = feed.placed.load(Ordering::Acquire);
        let mix = Mix::from_bits(feed.mix.load(Ordering::Acquire));
        if placed != held.placed[i] {
            held.placed[i] = placed;
            engine.place_voice_mix(i, mix);
        } else {
            engine.set_voice_mix(i, mix);
        }
        let active = feed.active.load(Ordering::Acquire) && !parked;
        if active != engine.voice_active(i) {
            engine.set_voice_active(i, active);
        }
    }
    // The drums tracks' kits: grid and sound, fader, mix, then the switch.
    for (k, feed) in voices.kits.iter().enumerate() {
        let (pattern, params) = feed.bank.load();
        engine.set_kit(k, pattern, params);
        engine.set_kit_gain(k, feed.gain());
        engine.set_kit_duck(k, feed.duck.load());
        let placed = feed.placed.load(Ordering::Acquire);
        let mix = Mix::from_bits(feed.mix.load(Ordering::Acquire));
        if placed != held.kit_placed[k] {
            held.kit_placed[k] = placed;
            engine.place_kit_mix(k, mix);
        } else {
            engine.set_kit_mix(k, mix);
        }
        let active = feed.active.load(Ordering::Acquire);
        if active != engine.kit_active(k) {
            engine.set_kit_active(k, active);
        }
    }
    // The transport always runs: every track plays while it does, and an
    // auditioned patch loops free over them because its own steps are off.
    engine.set_alone(Alone::from_bits(voices.alone.load(Ordering::Acquire)));
    engine.set_transport(&voices.transport.load());
    engine.set_arrangement(arrangement, false);
}

/// One block of every voice into `out`, what the voice playing the current
/// patch heard into `heard`, and every voice's applied mix.
pub(crate) fn render(engine: &mut Engine, voices: &Voices, heard: &ParamBank, out: &mut [f32]) {
    let banks: [&ParamBank; VOICES] = std::array::from_fn(|i| &voices.voice[i].bank);
    engine.process_voices(out, &banks);
    for (i, feed) in voices.voice.iter().enumerate() {
        feed.heard_mix
            .store(engine.voice_mix(i).bits(), Ordering::Release);
    }
    for (k, feed) in voices.kits.iter().enumerate() {
        feed.heard_mix
            .store(engine.kit_mix(k).bits(), Ordering::Release);
    }
    if let Some(i) = voices.shown() {
        engine.publish_voice_heard(i, &voices.voice[i].bank, heard);
    }
}

fn build_audio() -> Result<Audio, String> {
    let bank = Arc::new(ParamBank::new());
    let arrangement = Arc::new(ParamBank::for_table(arrangement::params()));
    let heard = Arc::new(ParamBank::new());
    let voices = Arc::new(Voices::new());
    let peak = Arc::new(AtomicU32::new(0));
    let reduction = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    let grains = Arc::new(AtomicU32::new(0));
    let playing = Arc::new(AtomicBool::new(false));
    let steps: Arc<[AtomicU32; VOICES]> = Arc::new(std::array::from_fn(|_| AtomicU32::new(0)));
    let kit_steps: Arc<[AtomicU32; KITS]> = Arc::new(std::array::from_fn(|_| AtomicU32::new(0)));
    let playhead = Arc::new(AtomicU32::new(0));
    let play_request = Arc::new(AtomicBool::new(false));
    let audition: Arc<Mutex<Option<(usize, GrainSpawn)>>> = Arc::new(Mutex::new(None));
    let auditioning = Arc::new(AtomicBool::new(false));
    let reversing = Arc::new(AtomicBool::new(false));
    let timer = Arc::new(BlockTimer::new());

    // Lock every slot the callback hands off through once, here. On macOS a
    // mutex allocates on its first lock, and without this that first lock
    // would land on the audio thread. `tests/audio_thread.rs` in shard-dsp
    // holds the pattern. The voices' banks, steps, readings and switches are
    // atomics and have no lock to prime.
    voices.prime();
    drop(audition.lock());

    let audio_heard = Arc::clone(&heard);
    let audio_voices = Arc::clone(&voices);
    let audio_peak = Arc::clone(&peak);
    let audio_reduction = Arc::clone(&reduction);
    let audio_grains = Arc::clone(&grains);
    let audio_playing = Arc::clone(&playing);
    let audio_playhead = Arc::clone(&playhead);
    let audio_steps = Arc::clone(&steps);
    let audio_kit_steps = Arc::clone(&kit_steps);
    let audio_arrangement = Arc::clone(&arrangement);
    let audio_request = Arc::clone(&play_request);
    let audio_audition = Arc::clone(&audition);
    let audio_auditioning = Arc::clone(&auditioning);
    let audio_reversing = Arc::clone(&reversing);
    let audio_timer = Arc::clone(&timer);

    // The log is created by the engine, which only exists inside the audio
    // thread, so its handle comes back out alongside the sample rate.
    // What the audio thread hands back once its stream runs: the device's
    // rate, each voice's grain log and the take recorder.
    type Started = (f32, Vec<Arc<GrainLog>>, Arc<recorder::Recorder>);
    let (tx, rx) = std::sync::mpsc::channel::<Result<Started, String>>();

    std::thread::Builder::new()
        .name("shard-audio".into())
        .spawn(move || {
            let built = (|| -> Result<(cpal::Stream, Started), String> {
                let host = cpal::default_host();
                let device = host
                    .default_output_device()
                    .ok_or("no default output device")?;
                let config = device
                    .default_output_config()
                    .map_err(|e| format!("no output config: {e}"))?;
                let sample_rate = config.sample_rate().0 as f32;
                let channels = config.channels().max(1) as usize;

                // Every voice starts idle; the session switches in one per
                // track, with its materials, before anything plays.
                let mut engine = Engine::new(sample_rate, LIVE_MAX_GRAINS);
                engine.set_voice_active(0, false);
                engine.set_kit_active(0, false);
                let log = engine.grain_logs();
                let recorder = Arc::new(recorder::Recorder::new(sample_rate));
                let audio_recorder = Arc::clone(&recorder);
                let mut tap_applied = 0u32;
                let mut arm = shard_dsp::arm::Arm::new(sample_rate);
                let mut scratch = vec![0.0f32; 8192];
                let mut held = Held::new();

                let stream = device
                    .build_output_stream(
                        &config.into(),
                        move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            let started = Instant::now();
                            let frames = out.len() / channels;

                            // The whole block runs inside the guard. The flag
                            // is set here, in the callback, because CoreAudio
                            // calls this on its own thread rather than the one
                            // that built the stream.
                            rt::no_alloc(|| {
                                hand_over(
                                    &mut engine,
                                    &audio_voices,
                                    &audio_arrangement,
                                    &mut held,
                                );

                                // The transport is a request the audio thread
                                // honours at a block boundary, so a stop never
                                // lands mid-grain.
                                let want = audio_request.load(Ordering::Relaxed);
                                if want != engine.playing() {
                                    engine.set_playing(want);
                                    audio_playing.store(want, Ordering::Relaxed);
                                }

                                // Same non-blocking hand-off as the sample swap.
                                // A missed block just means the grain sounds one
                                // buffer later, which nobody can perceive.
                                if let Ok(mut pending) = audio_audition.try_lock() {
                                    if let Some((voice, g)) = pending.take() {
                                        engine.audition_on(voice, &g);
                                    }
                                }

                                let needed = frames * 2;
                                if needed > scratch.len() {
                                    out.fill(0.0);
                                    return;
                                }
                                // The tap is set at the block boundary, and
                                // what it caught is pushed to the ring: both
                                // atomics, neither waits.
                                audio_recorder.before_render(&mut engine, &mut tap_applied);
                                render(
                                    &mut engine,
                                    &audio_voices,
                                    &audio_heard,
                                    &mut scratch[..needed],
                                );
                                audio_recorder.after_render(&engine, tap_applied, &mut arm);

                                // Once, the engine stops the transport itself
                                // when the pass has played out. Withdraw the
                                // request, unless a press has already changed
                                // it, and mirror the stop for the UI.
                                if audio_playing.load(Ordering::Relaxed) && !engine.playing() {
                                    let _ = audio_request.compare_exchange(
                                        true,
                                        false,
                                        Ordering::Relaxed,
                                        Ordering::Relaxed,
                                    );
                                    audio_playing.store(false, Ordering::Relaxed);
                                }

                                for (i, frame) in out.chunks_mut(channels).enumerate() {
                                    let l = scratch[i * 2];
                                    let r = scratch[i * 2 + 1];
                                    match frame.len() {
                                        0 => {}
                                        1 => frame[0] = (l + r) * 0.5,
                                        _ => {
                                            frame[0] = l;
                                            frame[1] = r;
                                            for extra in &mut frame[2..] {
                                                *extra = 0.0;
                                            }
                                        }
                                    }
                                }

                                // Hold the running maximum until the UI reads
                                // it, so a peak between polls is never missed.
                                let block_peak = engine.take_peak();
                                let prev = f32::from_bits(audio_peak.load(Ordering::Relaxed));
                                audio_peak.store(block_peak.max(prev).to_bits(), Ordering::Relaxed);
                                let block_red = engine.take_reduction();
                                let prev = f32::from_bits(audio_reduction.load(Ordering::Relaxed));
                                audio_reduction
                                    .store(block_red.min(prev).to_bits(), Ordering::Relaxed);
                                // The patch view follows the voice playing the
                                // current patch.
                                let shown = audio_voices.shown();
                                audio_grains.store(
                                    shown.map_or(0, |i| engine.voice_active_grains(i)) as u32,
                                    Ordering::Relaxed,
                                );
                                audio_playhead.store(
                                    shown
                                        .map_or(0.0, |i| engine.voice_play_position(i))
                                        .to_bits(),
                                    Ordering::Relaxed,
                                );
                                for (i, step) in audio_steps.iter().enumerate() {
                                    step.store(
                                        engine.voice_step(i).map_or(0, |s| s + 1),
                                        Ordering::Relaxed,
                                    );
                                }
                                for (k, step) in audio_kit_steps.iter().enumerate() {
                                    step.store(
                                        engine.kit_step(k).map_or(0, |s| s + 1),
                                        Ordering::Relaxed,
                                    );
                                }
                                audio_auditioning.store(engine.auditioning(), Ordering::Relaxed);
                                audio_reversing.store(engine.reversing(), Ordering::Relaxed);
                            });

                            // Measured around everything, including the
                            // hand-offs, because a dropout does not care which
                            // part was slow. `Instant::now` is a clock read,
                            // not a syscall, so it stays on in release too.
                            audio_timer.record(
                                started.elapsed(),
                                Duration::from_secs_f64(frames as f64 / sample_rate as f64),
                            );
                        },
                        |err| eprintln!("audio stream error: {err}"),
                        None,
                    )
                    .map_err(|e| format!("could not open the output stream: {e}"))?;

                stream
                    .play()
                    .map_err(|e| format!("could not start the stream: {e}"))?;
                Ok((stream, (sample_rate, log, recorder)))
            })();

            match built {
                Ok((stream, started)) => {
                    let _ = tx.send(Ok(started));
                    // Park forever. The stream must outlive this scope or the
                    // device stops, and this thread exists only to hold it.
                    let _held = stream;
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        })
        .map_err(|e| format!("could not start the audio thread: {e}"))?;

    let (sample_rate, grain_logs, recorder) = rx
        .recv()
        .map_err(|_| "the audio thread died during startup".to_string())??;

    // A new session plays the built-in drone through its material, and
    // lists it, so nothing sounds that the pool does not show (Georg,
    // 2026-09-15).
    let drone = Arc::new(source::startup_drone(sample_rate));
    let session = session::Session::new(
        session::Feeds {
            bank: Arc::clone(&bank),
            arrangement: Arc::clone(&arrangement),
            voices: Arc::clone(&voices),
        },
        drone,
        sample_rate,
    )?;

    Ok(Audio {
        session: Arc::new(session),
        bank,
        arrangement,
        voices,
        heard,
        peak,
        reduction,
        grains,
        playing,
        steps,
        kit_steps,
        grain_logs,
        audition,
        auditioning,
        reversing,
        playhead,
        play_request,
        timer,
        reported_allocs: AtomicU64::new(0),
        sample_rate,
        recorder,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let audio = match build_audio() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("shard: audio failed to start: {e}");
            std::process::exit(1);
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        // The window comes back where it was, at the size it was, and app-kit
        // reopens the last document (`native-apps.md` rule 4).
        .plugin(preset_app_kit::window_state())
        .manage(audio)
        // The close guard: an unsaved session asks before the window goes.
        .on_window_event(preset_app_kit::on_window_event)
        .setup(|app| {
            // Controllers belong to this machine, so they live in the app's
            // config directory rather than in any patch.
            let path = app
                .path()
                .app_config_dir()
                .map_err(|e| e.to_string())?
                .join("controllers.json");
            let audio = app.state::<Audio>();
            let bank = Arc::clone(&audio.bank);
            let arrangement = Arc::clone(&audio.arrangement);
            let play_request = Arc::clone(&audio.play_request);
            let playing = Arc::clone(&audio.playing);
            let shared = Arc::new(midi::Controllers::load(
                path,
                bank,
                arrangement,
                play_request,
                playing,
            ));
            // A caught knob writes into the session, as the UI does.
            let session = Arc::clone(&audio.session);
            let _ = shared.write.set(Box::new(move |id, v| {
                if let Err(e) = session.set_param(id, v) {
                    eprintln!("shard: a controller could not set {id}: {e}");
                }
            }));
            midi::start(Arc::clone(&shared));
            app.manage(shared);
            // The menu bar, from app-kit and Shard's own commands. If it
            // cannot be built the default one stays and the webview's key
            // handlers carry the shortcuts.
            let installed = preset_app_kit::AppKit::<tauri::Wry>::new(menu::APP_NAME)
                .menu_config(menu::MENU_CONFIG)
                .file_type("Shard session", "shard")
                .commands(menu::commands())
                .install::<Audio>(app.handle());
            match installed {
                // Undo's title follows every change, whoever made it: the
                // webview, a knob, undo itself. Posted to the main thread,
                // because a menu setter called from any other blocks until the
                // main thread runs it, and a knob turned during a drag would
                // deadlock on app-kit's menu lock. A burst of drag ticks
                // becomes one refresh.
                Ok(()) => {
                    let handle = app.handle().clone();
                    let pending = Arc::new(AtomicBool::new(false));
                    audio.session.on_change(move || {
                        if pending.swap(true, Ordering::SeqCst) {
                            return;
                        }
                        let (h, p) = (handle.clone(), Arc::clone(&pending));
                        let _ = handle.run_on_main_thread(move || {
                            p.store(false, Ordering::SeqCst);
                            preset_app_kit::refresh_history(&h);
                        });
                    });
                }
                Err(e) => eprintln!("shard: could not build the menu bar: {e}"),
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            preset_app_kit::app_kit_commands,
            preset_app_kit::app_kit_menu_state,
            preset_app_kit::app_kit_history,
            preset_app_kit::app_kit_undo,
            preset_app_kit::app_kit_redo,
            preset_app_kit::app_kit_text_menu,
            preset_app_kit::app_kit_document,
            preset_app_kit::app_kit_document_note,
            param_defs,
            get_params,
            set_param,
            arrangement_defs,
            get_arrangement,
            set_arrangement_param,
            fx_add,
            fx_remove,
            fx_move,
            generators,
            add_generator,
            remove_generator,
            reset_sound,
            patches,
            new_patch,
            select_patch,
            select_track,
            add_track,
            remove_track,
            toggle_track_mute,
            toggle_track_solo,
            record_start,
            record_stop,
            menu::set_record_menu,
            recording,
            set_track_patch,
            set_track_duck,
            rename_patch,
            delete_patch,
            duplicate_patch,
            meters,
            set_playing,
            set_play_mode,
            tracker,
            set_tracker,
            edit_tracker,
            envelope_curve,
            patch_preview,
            material_wave,
            material_peaks,
            materials,
            add_material,
            remove_material,
            add_drone,
            wire_material,
            set_material,
            add_region,
            remove_region,
            set_region,
            detect_regions,
            extract_region,
            grain_log,
            audition_grain,
            preset_names,
            save_preset,
            update_preset,
            apply_preset,
            modulation,
            add_lfo,
            add_global_lfo,
            remove_lfo,
            set_lfo,
            add_envelope,
            remove_envelope,
            set_envelope,
            envelope_gesture_begin,
            envelope_gesture_apply,
            envelope_gesture_end,
            envelope_gesture_cancel,
            link_param,
            link_new,
            unlink_param,
            get_heard,
            lfo_limits,
            controllers,
            rename_control,
            set_control_role,
            set_control_mode,
            calibrate_control,
            cancel_calibrate,
            rename_bank,
            forget_control,
            rename_device,
            forget_device,
            mappings,
            learn_midi,
            map_midi,
            cancel_learn,
            forget_midi,
            set_active_map,
            add_map,
            rename_map,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // The last document at launch, a document from Finder or `open`,
            // and the close guard on quit: app-kit's.
            preset_app_kit::on_run_event(app, &event);
            // Hand a profiled surface back before quitting, so the device
            // returns to standalone rather than sitting in a DAW mode that
            // nothing is driving. A crash cannot do this; a clean exit can.
            // On `Exit`, not `ExitRequested`: the close guard may cancel that.
            if matches!(event, tauri::RunEvent::Exit) {
                midi::release_surfaces();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire names the UI reads. Nothing here renames on the way out, so a
    /// field added in Rust arrives in TypeScript spelled exactly as it is
    /// written above — and a mismatch is invisible: `undefined` flows into the
    /// drawing maths as `NaN`, `fillRect` silently draws nothing, and the
    /// inspector looks empty with no error anywhere. Hence a test.
    #[test]
    fn the_ui_facing_structs_keep_their_field_names() {
        let source = serde_json::to_value(SourceInfo {
            name: "x".into(),
            seconds: 1.0,
            sample_rate: 48_000.0,
            peaks: vec![0.0],
        })
        .expect("SourceInfo is serialisable");
        for key in ["name", "seconds", "sample_rate", "peaks"] {
            assert!(source.get(key).is_some(), "SourceInfo lost `{key}`");
        }

        let grain = serde_json::to_value(GrainInfo {
            position: 0.5,
            rate: 1.0,
            len: 4_800.0,
            pan: 0.5,
            window: 0,
            seq: 1,
        })
        .expect("GrainInfo is serialisable");
        for key in ["position", "rate", "len", "pan", "window", "seq"] {
            assert!(grain.get(key).is_some(), "GrainInfo lost `{key}`");
        }

        let meters = serde_json::to_value(Meters {
            peak: 0.0,
            reduction: 1.0,
            grains: 0,
            playing: false,
            play_mode: session::PlayMode::Song,
            steps: vec![-1],
            playhead: 0.0,
            auditioning: false,
            reversing: false,
            block_us: 0,
            block_budget_us: 0,
            audio_allocs: 0,
            mix: vec![session::MixState::default()],
        })
        .expect("Meters is serialisable");
        for key in [
            "peak",
            "reduction",
            "grains",
            "steps",
            "playing",
            "play_mode",
            "playhead",
            "auditioning",
            "reversing",
            "block_us",
            "block_budget_us",
            "audio_allocs",
            "mix",
        ] {
            assert!(meters.get(key).is_some(), "Meters lost `{key}`");
        }
        let mix = &meters["mix"][0];
        for key in ["mute", "solo", "heard_mute", "heard_solo"] {
            assert!(mix.get(key).is_some(), "MixState lost `{key}`");
        }

        let tracker =
            serde_json::to_value(tracker::Tracker::default()).expect("Tracker is serialisable");
        for key in ["tempo", "swing", "tracks"] {
            assert!(tracker.get(key).is_some(), "Tracker lost `{key}`");
        }
        for key in ["length", "gain", "patch", "steps", "drums"] {
            assert!(
                tracker["tracks"][0].get(key).is_some(),
                "Track lost `{key}`"
            );
        }
        for key in ["on", "tune", "decay", "kick", "snare", "hat"] {
            assert!(
                tracker["tracks"][1]["drums"].get(key).is_some(),
                "Kit lost `{key}`"
            );
        }

        let report = serde_json::to_value(session::ApplyReport::default())
            .expect("ApplyReport is serialisable");
        for key in ["applied", "unknown", "other_patch", "refused"] {
            assert!(report.get(key).is_some(), "ApplyReport lost `{key}`");
        }

        let load = serde_json::to_value(session::LoadReport::default())
            .expect("LoadReport is serialisable");
        for key in [
            "applied",
            "unknown",
            "sample_path",
            "sample_missing",
            "refused",
            "map_missing",
        ] {
            assert!(load.get(key).is_some(), "LoadReport lost `{key}`");
        }

        let pool = materials::Pool::with_drone();
        let view = serde_json::to_value(MaterialsView::of(&pool)).expect("MaterialsView");
        for key in ["materials", "wires"] {
            assert!(view.get(key).is_some(), "MaterialsView lost `{key}`");
        }
        for key in ["material", "grain"] {
            assert!(view["wires"].get(key).is_some(), "Wires lost `{key}`");
        }
        for key in [
            "id",
            "name",
            "path",
            "missing",
            "octave",
            "trim_start",
            "trim_end",
            "regions",
        ] {
            assert!(
                view["materials"][0].get(key).is_some(),
                "MaterialView lost `{key}`"
            );
        }

        let refused = serde_json::to_value(modulation::Refused {
            id: "grain.size".into(),
            reason: "x".into(),
        })
        .expect("Refused is serialisable");
        for key in ["id", "reason"] {
            assert!(refused.get(key).is_some(), "Refused lost `{key}`");
        }

        let doc = modulation::Modulation {
            lfos: vec![modulation::LfoRecord {
                id: 1,
                name: "LFO 1".into(),
                rate: 1.0,
                shape: "sine".into(),
                phase: 0.0,
            }],
            envelopes: vec![modulation::new_envelope(2)],
            links: [(
                "grain.size".to_string(),
                modulation::LinkRecord {
                    source: 1,
                    lo: 0.25,
                    hi: 0.75,
                },
            )]
            .into(),
        };
        let view = serde_json::to_value(ModulationView::of(&doc, Vec::new(), Vec::new()))
            .expect("ModulationView is serialisable");
        for key in ["lfos", "envelopes", "globals", "links", "refused"] {
            assert!(view.get(key).is_some(), "ModulationView lost `{key}`");
        }

        let cv = serde_json::to_value(ControllersView {
            devices: vec![DeviceView {
                port: "LPD8".into(),
                name: "LPD8".into(),
                connected: true,
                surface: None,
            }],
            controls: vec![ControlView {
                id: 1,
                device: "LPD8".into(),
                channel: 0,
                kind: "cc",
                number: 1,
                role: "knob",
                mode: "absolute",
                bank: "Ch 1".into(),
                name: "K1".into(),
                active: false,
                last: None,
                delta: None,
            }],
            roles: &controllers::Role::NAMES,
            modes: &controllers::Mode::NAMES,
            calibrating: None,
            instruction: None,
            found: None,
            trouble: None,
        })
        .expect("ControllersView is serialisable");
        for key in [
            "devices",
            "controls",
            "roles",
            "modes",
            "calibrating",
            "found",
            "trouble",
        ] {
            assert!(cv.get(key).is_some(), "ControllersView lost `{key}`");
        }
        for key in ["port", "name", "connected", "surface"] {
            assert!(
                cv["devices"][0].get(key).is_some(),
                "DeviceView lost `{key}`"
            );
        }
        for key in [
            "id", "device", "channel", "kind", "number", "role", "name", "active", "last",
        ] {
            assert!(
                cv["controls"][0].get(key).is_some(),
                "ControlView lost `{key}`"
            );
        }

        let mv = serde_json::to_value(MappingsView {
            active: Some(MapInfo {
                id: 1,
                name: "live".into(),
            }),
            maps: Vec::new(),
            knobs: vec![KnobInfo {
                id: 1,
                name: "K1".into(),
                device: "LPD8".into(),
                bank: "Ch 1".into(),
                relative: false,
            }],
            mappings: [(
                "grain.size".to_string(),
                MappingView {
                    control: 1,
                    control_name: "K1".into(),
                    armed: true,
                    knob: None,
                },
            )]
            .into(),
            learning: None,
            report: Some(LearnReport {
                seq: 1,
                ok: true,
                text: "x".into(),
            }),
        })
        .expect("MappingsView is serialisable");
        for key in ["active", "maps", "knobs", "mappings", "learning", "report"] {
            assert!(mv.get(key).is_some(), "MappingsView lost `{key}`");
        }
        for key in ["id", "name", "device", "bank", "relative"] {
            assert!(mv["knobs"][0].get(key).is_some(), "KnobInfo lost `{key}`");
        }
        for key in ["control", "control_name", "armed", "knob"] {
            assert!(
                mv["mappings"]["grain.size"].get(key).is_some(),
                "MappingView lost `{key}`"
            );
        }
        for key in ["seq", "ok", "text"] {
            assert!(mv["report"].get(key).is_some(), "LearnReport lost `{key}`");
        }

        let limits = serde_json::to_value(lfo_limits()).expect("LfoLimits is serialisable");
        for key in ["shapes", "min_rate", "max_rate"] {
            assert!(limits.get(key).is_some(), "LfoLimits lost `{key}`");
        }
        for key in ["id", "name", "rate", "shape", "phase"] {
            assert!(view["lfos"][0].get(key).is_some(), "LfoRecord lost `{key}`");
        }
        for key in ["id", "name", "mode", "curve"] {
            assert!(
                view["envelopes"][0].get(key).is_some(),
                "EnvelopeRecord lost `{key}`"
            );
        }
        for key in ["source", "lo", "hi"] {
            assert!(
                view["links"]["grain.size"].get(key).is_some(),
                "LinkRecord lost `{key}`"
            );
        }
    }

    #[test]
    fn the_preview_folds_both_channels_without_cancelling() {
        // Hard left and hard right at once: an average would draw silence.
        let stereo: Vec<f32> = (0..1_000).flat_map(|_| [0.8, -0.6]).collect();
        let (min, max) = fold_columns(&stereo, 10);
        assert!(min.iter().all(|v| *v == -0.6) && max.iter().all(|v| *v == 0.8));
        // Nothing to fold is silence, and more columns than frames is fine.
        assert_eq!(fold_columns(&[], 4), (vec![0.0; 4], vec![0.0; 4]));
        let (lo, _) = fold_columns(&[0.5, 0.5], 4);
        assert_eq!(lo, vec![0.5; 4]);
        let p = fold_preview(&stereo, 48_000.0, false);
        assert_eq!(p.zoom_min.len(), PREVIEW_ZOOM_COLUMNS);
        assert!(p.seconds > 0.0 && !p.capped);
    }
}
