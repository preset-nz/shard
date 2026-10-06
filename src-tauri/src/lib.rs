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
use shard_dsp::steps::StepBank;
use shard_dsp::{Engine, Generator, GrainLog, GrainSpawn, ModSet, ParamBank, ReadingBank};

mod controllers;
mod document;
#[cfg(test)]
mod e2e;
mod mapping;
mod materials;
mod menu;
mod midi;
mod modulation;
pub mod object_model;
mod profile;
pub mod session;
#[cfg(test)]
mod session_tests;
// Writes a `.shard` from a flat sketch, for `just shard-write`.
#[cfg(test)]
mod sketch;
mod source;
mod tracker;

/// Debug builds count every allocator call made inside the audio callback,
/// and `meters` reports the running total. Release builds keep the plain
/// system allocator and always report zero. See `shard_dsp::rt`.
#[cfg(debug_assertions)]
#[global_allocator]
static GUARD: rt::GuardedAlloc = rt::GuardedAlloc;

use session::SourceSlots;

/// Shared between the UI thread and the audio thread.
/// The live engine's grain pool. The preview renders with the same, or the
/// cloud would steal voices differently from what is heard.
const LIVE_MAX_GRAINS: usize = 256;

pub struct Audio {
    /// The open session: the document, and the engine kept in step with it.
    /// Every edit goes through here (`session.rs`).
    session: Arc<session::Session>,
    bank: Arc<ParamBank>,
    /// The arrangement's values: its chain over the patch, the patch's
    /// fader and the limiter. Read by the audio thread once a block.
    arrangement: Arc<ParamBank>,
    /// Every parameter as the engine last heard it, LFOs included. Written by
    /// the audio thread once a block; the bank above stays the hand's.
    heard: Arc<ParamBank>,
    /// Peak since the UI last asked, as f32 bits. Written by the audio thread.
    peak: Arc<AtomicU32>,
    /// The lowest limiter gain since the UI last asked, as f32 bits.
    reduction: Arc<AtomicU32>,
    /// Active grain count, for the UI. Written by the audio thread.
    grains: Arc<AtomicU32>,
    /// Transport, mirrored out of the audio thread for the UI.
    playing: Arc<AtomicBool>,
    /// The sequencer's step, plus one; zero while it is off.
    step: Arc<AtomicU32>,
    /// The drum kit's step, the same way. Its loop has its own length.
    kit_step: Arc<AtomicU32>,
    /// Every grain the cloud has spawned, drained by the UI at poll rate.
    grain_log: Arc<GrainLog>,
    /// One grain waiting to be auditioned. Handed across the same way a sample
    /// swap is: the audio thread takes it at a block boundary, never blocking.
    audition: Arc<Mutex<Option<GrainSpawn>>>,
    /// True while a single grain is being played on its own.
    auditioning: Arc<AtomicBool>,
    /// Whether the reel is actually running backwards, which outlives the
    /// button press by the flick time. The UI lights the button from this.
    reversing: Arc<AtomicBool>,
    playhead: Arc<AtomicU32>,
    /// Set by the UI, read by the audio thread at the top of each block.
    play_request: Arc<AtomicBool>,
    /// Source buffers the audio thread swapped out and handed back, waiting
    /// for `meters` to free them here rather than on the audio thread.
    retired: Arc<Mutex<SourceSlots>>,
    /// A modulation set the audio thread replaced, handed back the same way.
    mod_retired: Arc<Mutex<Option<ModSet>>>,
    /// The slowest block since `meters` last asked.
    timer: Arc<BlockTimer>,
    /// The audio-thread allocation count `meters` last logged, so a new one is
    /// printed once rather than thirty times a second.
    reported_allocs: AtomicU64,
    sample_rate: f32,
}

/// What the UI needs to build its controls. Generated from the Rust table, so
/// there is one source of truth and adding a parameter is adding a row.
#[derive(Serialize)]
pub struct ParamInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// "linear" | "exponential" | "bipolar" | "stepped"
    pub taper: &'static str,
    /// Present only for stepped parameters.
    pub steps: Option<u32>,
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
    /// The sequencer's current step, from zero, or -1 while it is off.
    pub step: i32,
    /// The drum kit's current step, the same way.
    pub kit_step: i32,
    /// Where plain playback has reached, 0 to 1.
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

/// The open patch's LFOs and links as the UI draws them, and what in them the
/// engine could not use. Every modulation command answers with one, so the UI
/// never draws a document older than its own last edit.
#[derive(Serialize)]
pub struct ModulationView {
    pub lfos: Vec<modulation::LfoRecord>,
    pub envelopes: Vec<modulation::EnvelopeRecord>,
    pub links: modulation::Links,
    pub refused: Vec<modulation::Refused>,
}

impl ModulationView {
    fn of(doc: &modulation::Modulation, refused: Vec<modulation::Refused>) -> Self {
        Self {
            lfos: doc.lfos.clone(),
            envelopes: doc.envelopes.clone(),
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

/// Moves an effect one place earlier (`by` below zero) or later.
#[tauri::command]
fn fx_move(
    state: tauri::State<'_, Audio>,
    layer: String,
    n: usize,
    by: i32,
) -> Result<bool, String> {
    state.session.fx_move(&layer, n, by)
}

fn infos(defs: &'static [ParamDef]) -> Vec<ParamInfo> {
    defs.iter()
        .map(|p| ParamInfo {
            id: p.id,
            name: p.name,
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

/// Every parameter as the engine last heard it, indexed as `get_params`. Differs
/// from it only where a parameter follows an LFO.
#[tauri::command]
fn get_heard(state: tauri::State<'_, Audio>) -> Vec<f32> {
    (0..PARAMS.len()).map(|i| state.heard.get(i)).collect()
}

#[tauri::command]
fn set_param(state: tauri::State<'_, Audio>, id: String, value: f32) -> Result<(), String> {
    state.session.set_param(&id, value)
}

#[tauri::command]
fn meters(state: tauri::State<'_, Audio>) -> Meters {
    // Free whatever source buffer the audio thread retired since the last
    // poll. Blocking here is fine; the audio thread only ever tries the lock.
    if let Ok(mut slot) = state.retired.lock() {
        drop(std::mem::take(&mut *slot));
    }
    if let Ok(mut slot) = state.mod_retired.lock() {
        drop(slot.take());
    }

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
        step: state.step.load(Ordering::Relaxed) as i32 - 1,
        kit_step: state.kit_step.load(Ordering::Relaxed) as i32 - 1,
        playhead: f32::from_bits(state.playhead.load(Ordering::Relaxed)),
        auditioning: state.auditioning.load(Ordering::Relaxed),
        reversing: state.reversing.load(Ordering::Relaxed),
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
    let mut spawns = Vec::new();
    state.grain_log.drain(&mut spawns);
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
    *state.audition.lock().expect("audition poisoned") = Some(GrainSpawn {
        position,
        rate,
        len,
        pan,
        window,
        seq: 0,
    });
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
    let mods = state.session.plan().mod_set().0;

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
    /// The longest envelope stage, in ms.
    pub max_stage_ms: f32,
}

#[tauri::command]
fn lfo_limits() -> LfoLimits {
    LfoLimits {
        shapes: shard_dsp::Shape::NAMES.to_vec(),
        min_rate: shard_dsp::modulation::MIN_RATE_HZ,
        max_rate: shard_dsp::modulation::MAX_RATE_HZ,
        max_stage_ms: modulation::MAX_STAGE_MS,
    }
}

/// The open patch's LFOs and links, and what in them the engine refuses.
#[tauri::command]
fn modulation(state: tauri::State<'_, Audio>) -> ModulationView {
    modulation_view(&state)
}

fn modulation_view(state: &Audio) -> ModulationView {
    let (doc, refused) = state.session.modulation();
    ModulationView::of(&doc, refused)
}

/// Add an LFO. It is last in the answer's `lfos`.
#[tauri::command]
fn add_lfo(state: tauri::State<'_, Audio>) -> Result<ModulationView, String> {
    state.session.add_lfo()?;
    Ok(modulation_view(&state))
}

/// Remove an LFO, and the links that followed it.
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

/// Change a modulation envelope's name and stages.
#[tauri::command]
fn set_envelope(
    state: tauri::State<'_, Audio>,
    envelope: modulation::EnvelopeRecord,
) -> Result<ModulationView, String> {
    state.session.set_envelope(envelope)?;
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

/// Start or stop playback. Stopping clears the grain pool, so stop is stop.
#[tauri::command]
fn set_playing(state: tauri::State<'_, Audio>, playing: bool) {
    state.play_request.store(playing, Ordering::Relaxed);
}

/// The tracker, as saved and as the engine plays it. The first track is on
/// in tracker mode.
#[tauri::command]
fn tracker(state: tauri::State<'_, Audio>) -> tracker::Tracker {
    state.session.tracker()
}

/// Replace the tracker whole. The first track's switch is the mode and not an
/// edit. Answers with what the engine now plays, which differs from what was
/// sent only where a value was out of range.
#[tauri::command]
fn set_tracker(
    state: tauri::State<'_, Audio>,
    tracker: tracker::Tracker,
) -> Result<tracker::Tracker, String> {
    state.session.set_tracker(tracker)
}

/// A whole-pattern edit: clear, paste or repeat a bar, rotate, transpose
/// (`tracker::Edit`). One step of Undo, and none when it changes nothing.
#[tauri::command]
fn edit_tracker(
    state: tauri::State<'_, Audio>,
    edit: tracker::Edit,
) -> Result<tracker::Tracker, String> {
    state.session.edit_tracker(&edit)
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

fn build_audio() -> Result<Audio, String> {
    let bank = Arc::new(ParamBank::new());
    let arrangement = Arc::new(ParamBank::for_table(arrangement::params()));
    let heard = Arc::new(ParamBank::new());
    let peak = Arc::new(AtomicU32::new(0));
    let reduction = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    let grains = Arc::new(AtomicU32::new(0));
    let playing = Arc::new(AtomicBool::new(false));
    let step = Arc::new(AtomicU32::new(0));
    let kit_step = Arc::new(AtomicU32::new(0));
    let steps = Arc::new(StepBank::new(tracker::Tracker::default().params()));
    let playhead = Arc::new(AtomicU32::new(0));
    let play_request = Arc::new(AtomicBool::new(false));
    let swap: Arc<Mutex<SourceSlots>> = Arc::new(Mutex::new([None, None]));
    let audition: Arc<Mutex<Option<GrainSpawn>>> = Arc::new(Mutex::new(None));
    let auditioning = Arc::new(AtomicBool::new(false));
    let reversing = Arc::new(AtomicBool::new(false));
    let retired: Arc<Mutex<SourceSlots>> = Arc::new(Mutex::new([None, None]));
    let readings = Arc::new(ReadingBank::new());
    let mod_swap: Arc<Mutex<Option<ModSet>>> = Arc::new(Mutex::new(None));
    let mod_retired: Arc<Mutex<Option<ModSet>>> = Arc::new(Mutex::new(None));
    let timer = Arc::new(BlockTimer::new());

    // Lock every slot the callback hands off through once, here. On macOS a
    // mutex allocates on its first lock, and without this that first lock
    // would land on the audio thread. `tests/audio_thread.rs` in shard-dsp
    // holds the pattern. The tracker's `steps` and the materials' `readings`
    // are not here on purpose: they are atomics, like the bank, and have no
    // lock to prime.
    drop(swap.lock());
    drop(audition.lock());
    drop(retired.lock());
    drop(mod_swap.lock());
    drop(mod_retired.lock());

    let audio_bank = Arc::clone(&bank);
    let audio_heard = Arc::clone(&heard);
    let audio_peak = Arc::clone(&peak);
    let audio_reduction = Arc::clone(&reduction);
    let audio_grains = Arc::clone(&grains);
    let audio_swap = Arc::clone(&swap);
    let audio_playing = Arc::clone(&playing);
    let audio_playhead = Arc::clone(&playhead);
    let audio_step = Arc::clone(&step);
    let audio_kit_step = Arc::clone(&kit_step);
    let audio_steps = Arc::clone(&steps);
    let audio_arrangement = Arc::clone(&arrangement);
    let audio_readings = Arc::clone(&readings);
    let audio_request = Arc::clone(&play_request);
    let audio_audition = Arc::clone(&audition);
    let audio_auditioning = Arc::clone(&auditioning);
    let audio_reversing = Arc::clone(&reversing);
    let audio_retired = Arc::clone(&retired);
    let audio_mod_swap = Arc::clone(&mod_swap);
    let audio_mod_retired = Arc::clone(&mod_retired);
    let audio_timer = Arc::clone(&timer);

    // The log is created by the engine, which only exists inside the audio
    // thread, so its handle comes back out alongside the sample rate.
    let (tx, rx) = std::sync::mpsc::channel::<Result<(f32, Arc<GrainLog>), String>>();

    std::thread::Builder::new()
        .name("shard-audio".into())
        .spawn(move || {
            let built = (|| -> Result<(cpal::Stream, f32, Arc<GrainLog>), String> {
                let host = cpal::default_host();
                let device = host
                    .default_output_device()
                    .ok_or("no default output device")?;
                let config = device
                    .default_output_config()
                    .map_err(|e| format!("no output config: {e}"))?;
                let sample_rate = config.sample_rate().0 as f32;
                let channels = config.channels().max(1) as usize;

                let mut engine = Engine::new(sample_rate, LIVE_MAX_GRAINS);
                engine.set_source(source::startup_drone(sample_rate).samples);
                let log = engine.grain_log();
                let mut scratch = vec![0.0f32; 8192];
                // Swapped-out sources, held until the UI side can take them.
                let mut retiring: SourceSlots = [None, None];
                // A swapped-out modulation set, held the same way.
                let mut mods_retiring: Option<ModSet> = None;

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
                                // A retired source waits here until `meters`
                                // collects it, and no new swap is taken while
                                // one is waiting, so a buffer is never freed
                                // on this thread.
                                if retiring.iter().any(Option::is_some) {
                                    if let Ok(mut slot) = audio_retired.try_lock() {
                                        if slot.iter().all(Option::is_none) {
                                            *slot = std::mem::take(&mut retiring);
                                        }
                                    }
                                }

                                // Locks on this thread are only ever tried,
                                // never waited on. If the UI holds one this
                                // block, the hand-off happens on the next.
                                // Each generator's source swaps on its own.
                                if retiring.iter().all(Option::is_none) {
                                    if let Ok(mut pending) = audio_swap.try_lock() {
                                        for g in Generator::ALL {
                                            if let Some(buf) = pending[g.index()].take() {
                                                retiring[g.index()] =
                                                    Some(engine.swap_source(g, buf));
                                            }
                                        }
                                    }
                                }

                                // LFOs and links arrive the same way as a
                                // sample, with their own pair of slots. The
                                // engine carries running LFOs across by id.
                                if let Some(old) = mods_retiring.take() {
                                    match audio_mod_retired.try_lock() {
                                        Ok(mut slot) if slot.is_none() => *slot = Some(old),
                                        _ => mods_retiring = Some(old),
                                    }
                                }
                                if mods_retiring.is_none() {
                                    if let Ok(mut pending) = audio_mod_swap.try_lock() {
                                        if let Some(set) = pending.take() {
                                            mods_retiring = Some(engine.set_modulation(set));
                                        }
                                    }
                                }

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
                                    if let Some(g) = pending.take() {
                                        engine.audition(&g);
                                    }
                                }

                                let needed = frames * 2;
                                if needed > scratch.len() {
                                    out.fill(0.0);
                                    return;
                                }
                                // The tracker, once a block, as the bank is.
                                let steps = audio_steps.load();
                                engine.set_steps(steps);
                                // The arrangement, the same way. Sound scaping
                                // hears the patch alone, and the mode is what
                                // switches the steps, so the steps being off is
                                // the patch alone. One line of coupling, here in
                                // the host rather than in the engine.
                                engine.set_arrangement(&audio_arrangement, !steps.on);
                                // How each generator reads its material, the same way.
                                let (player, grain) = audio_readings.load();
                                engine.set_readings(player, grain);
                                engine.process_block(&mut scratch[..needed], &audio_bank);
                                engine.publish_heard(&audio_bank, &audio_heard);

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
                                audio_grains
                                    .store(engine.active_grains() as u32, Ordering::Relaxed);
                                audio_playhead
                                    .store(engine.play_position().to_bits(), Ordering::Relaxed);
                                audio_step.store(
                                    engine.current_step().map_or(0, |s| s + 1),
                                    Ordering::Relaxed,
                                );
                                audio_kit_step.store(
                                    engine.current_kit_step().map_or(0, |s| s + 1),
                                    Ordering::Relaxed,
                                );
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
                Ok((stream, sample_rate, log))
            })();

            match built {
                Ok((stream, sample_rate, log)) => {
                    let _ = tx.send(Ok((sample_rate, log)));
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

    let (sample_rate, grain_log) = rx
        .recv()
        .map_err(|_| "the audio thread died during startup".to_string())??;

    // A new session plays the built-in drone through both generators, and
    // lists it, so nothing sounds that the pool does not show (Georg,
    // 2026-09-15). The engine above starts on the same drone.
    let drone = Arc::new(source::startup_drone(sample_rate));
    let session = session::Session::new(
        session::Feeds {
            bank: Arc::clone(&bank),
            arrangement: Arc::clone(&arrangement),
            steps,
            mod_swap,
            swap,
            readings,
        },
        drone,
        sample_rate,
    )?;

    Ok(Audio {
        session: Arc::new(session),
        bank,
        arrangement,
        heard,
        peak,
        reduction,
        grains,
        playing,
        step,
        kit_step,
        grain_log,
        audition,
        auditioning,
        reversing,
        playhead,
        play_request,
        retired,
        mod_retired,
        timer,
        reported_allocs: AtomicU64::new(0),
        sample_rate,
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
                // webview, a knob, undo itself.
                Ok(()) => {
                    let handle = app.handle().clone();
                    audio
                        .session
                        .on_change(move || preset_app_kit::refresh_history(&handle));
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
            reset_sound,
            meters,
            set_playing,
            tracker,
            set_tracker,
            edit_tracker,
            envelope_curve,
            patch_preview,
            material_wave,
            materials,
            add_material,
            remove_material,
            add_drone,
            wire_material,
            set_material,
            grain_log,
            audition_grain,
            preset_names,
            save_preset,
            update_preset,
            apply_preset,
            modulation,
            add_lfo,
            remove_lfo,
            set_lfo,
            add_envelope,
            remove_envelope,
            set_envelope,
            link_param,
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
            step: -1,
            kit_step: -1,
            playhead: 0.0,
            auditioning: false,
            reversing: false,
            block_us: 0,
            block_budget_us: 0,
            audio_allocs: 0,
        })
        .expect("Meters is serialisable");
        for key in [
            "peak",
            "reduction",
            "grains",
            "step",
            "kit_step",
            "playing",
            "playhead",
            "auditioning",
            "reversing",
            "block_us",
            "block_budget_us",
            "audio_allocs",
        ] {
            assert!(meters.get(key).is_some(), "Meters lost `{key}`");
        }

        let tracker =
            serde_json::to_value(tracker::Tracker::default()).expect("Tracker is serialisable");
        for key in ["tempo", "swing", "tracks", "kit"] {
            assert!(tracker.get(key).is_some(), "Tracker lost `{key}`");
        }
        for key in ["on", "length", "steps"] {
            assert!(
                tracker["tracks"][0].get(key).is_some(),
                "Track lost `{key}`"
            );
        }

        let report = serde_json::to_value(session::ApplyReport::default())
            .expect("ApplyReport is serialisable");
        for key in ["applied", "unknown", "refused"] {
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
        let view = serde_json::to_value(ModulationView::of(&doc, Vec::new()))
            .expect("ModulationView is serialisable");
        for key in ["lfos", "envelopes", "links", "refused"] {
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
        for key in ["shapes", "min_rate", "max_rate", "max_stage_ms"] {
            assert!(limits.get(key).is_some(), "LfoLimits lost `{key}`");
        }
        for key in ["id", "name", "rate", "shape", "phase"] {
            assert!(view["lfos"][0].get(key).is_some(), "LfoRecord lost `{key}`");
        }
        for key in ["id", "name", "attack", "decay", "sustain", "release"] {
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
