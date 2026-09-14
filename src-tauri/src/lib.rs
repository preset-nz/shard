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
use shard_dsp::params::{Taper, Unit, PARAMS};
use shard_dsp::rt::{self, BlockTimer};
use shard_dsp::steps::StepBank;
use shard_dsp::{Engine, GrainLog, GrainSpawn, ModSet, ParamBank};

mod controllers;
mod mapping;
mod midi;
mod modulation;
mod patch;
mod presets;
mod source;
mod tracker;

/// Debug builds count every allocator call made inside the audio callback,
/// and `meters` reports the running total. Release builds keep the plain
/// system allocator and always report zero. See `shard_dsp::rt`.
#[cfg(debug_assertions)]
#[global_allocator]
static GUARD: rt::GuardedAlloc = rt::GuardedAlloc;

/// Shared between the UI thread and the audio thread.
pub struct Audio {
    bank: Arc<ParamBank>,
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
    /// The tracker as the engine plays it, read once a block. Written only by
    /// `apply_tracker`, together with the document below.
    steps: Arc<StepBank>,
    /// The level above the patch: tempo, swing and tracks. Saved in the same
    /// `.shard` file, above the patch.
    tracker: Mutex<tracker::Tracker>,
    /// The loaded sample, kept so the UI can draw a waveform and so a reload
    /// can replace it. Swapping goes through the queue, never a lock on audio.
    source: Mutex<source::Loaded>,
    /// Node presets for the open patch. Saved with it and replaced when
    /// another patch loads; never touched by the audio thread.
    presets: Mutex<presets::Presets>,
    /// The open patch's LFOs and links. Every edit rebuilds a `ModSet` from
    /// this and hands it across through `mod_swap`.
    modulation: Mutex<modulation::Modulation>,
    swap: Arc<Mutex<Option<Vec<f32>>>>,
    /// A source buffer the audio thread swapped out and handed back, waiting
    /// for `meters` to free it here rather than on the audio thread.
    retired: Arc<Mutex<Option<Vec<f32>>>>,
    /// A modulation set waiting for the audio thread, and the one it replaced,
    /// handed back the same way as a source buffer.
    mod_swap: Arc<Mutex<Option<ModSet>>>,
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
    pub links: modulation::Links,
    pub refused: Vec<modulation::Refused>,
}

impl ModulationView {
    fn of(doc: &modulation::Modulation, refused: Vec<modulation::Refused>) -> Self {
        Self {
            lfos: doc.lfos.clone(),
            links: doc.links.clone(),
            refused,
        }
    }
}

#[tauri::command]
fn param_defs() -> Vec<ParamInfo> {
    PARAMS
        .iter()
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
                Unit::Hz => "Hz",
                Unit::Semitones => "st",
                Unit::Percent => "%",
                Unit::Octaves => "oct",
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
    if state.bank.set_by_id(&id, value) {
        Ok(())
    } else {
        Err(format!("unknown parameter: {id}"))
    }
}

#[tauri::command]
fn meters(state: tauri::State<'_, Audio>) -> Meters {
    // Free whatever source buffer the audio thread retired since the last
    // poll. Blocking here is fine; the audio thread only ever tries the lock.
    if let Ok(mut slot) = state.retired.lock() {
        drop(slot.take());
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

/// Write the document to a `.shard` file. A few kilobytes of readable JSON:
/// the tracker, and under it the patch, with parameter values by id, LFOs and
/// links, presets, and where the sample was.
#[tauri::command]
fn save_patch(
    state: tauri::State<'_, Audio>,
    ctl: tauri::State<'_, Arc<midi::Controllers>>,
    path: String,
) -> Result<(), String> {
    let sample = {
        let s = state.source.lock().expect("source poisoned");
        s.path.clone()
    };
    let mut p = patch::Patch::capture(&state.bank, sample);
    p.presets = state.presets.lock().expect("presets poisoned").clone();
    p.modulation = state
        .modulation
        .lock()
        .expect("modulation poisoned")
        .clone();
    // The patch records the map it was played with, so loading it brings
    // the same knobs back.
    p.controller_map = ctl
        .registry
        .lock()
        .expect("registry poisoned")
        .active_map()
        .map(|m| mapping::MapRef {
            id: m.id,
            name: m.name.clone(),
        });
    let tracker = state.tracker.lock().expect("tracker poisoned").clone();
    let doc = patch::Document::new(tracker, p);
    std::fs::write(&path, doc.to_json()?).map_err(|e| format!("{path}: {e}"))
}

/// Read a `.shard` file back. Reloads the sample it names when that file is
/// still there, and says so plainly when it is not rather than loading half
/// the patch and looking fine.
#[tauri::command]
fn load_patch(
    state: tauri::State<'_, Audio>,
    ctl: tauri::State<'_, Arc<midi::Controllers>>,
    path: String,
) -> Result<patch::LoadReport, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let doc = patch::Document::from_json(&text)?;
    // The tracker comes with the document, above the patch.
    apply_tracker(&state, doc.tracker.clone());
    let p = &doc.patch;
    let mut report = p.apply(&state.bank);
    // A patch names its map. One this Mac does not have leaves the active
    // map alone and is reported, like an unknown id.
    if let Some(want) = &p.controller_map {
        let mut registry = ctl.registry.lock().expect("registry poisoned");
        if registry.set_active(want.id) {
            ctl.pickup.lock().expect("pickup poisoned").rearm_all();
            if let Err(e) = ctl.save(&registry) {
                eprintln!("shard: could not save controllers: {e}");
            }
        } else {
            report.map_missing = Some(want.name.clone());
        }
    }
    // Presets belong to the patch, so the loaded patch's set replaces the old.
    *state.presets.lock().expect("presets poisoned") = p.presets.clone();
    // So do LFOs and links. Anything the engine cannot use stays in the
    // document and is reported.
    report.refused = state.send_modulation(&p.modulation);
    *state.modulation.lock().expect("modulation poisoned") = p.modulation.clone();

    if let Some(sample) = &p.sample_path {
        match source::load(sample, state.sample_rate) {
            Ok(loaded) => {
                *state.swap.lock().expect("swap poisoned") = Some(loaded.samples.clone());
                *state.source.lock().expect("source poisoned") = loaded;
            }
            // Not an error: the patch is still worth having with a different
            // sample under it. The UI says which file is missing.
            Err(_) => report.sample_missing = true,
        }
    }

    Ok(report)
}

/// The preset names saved for one node, such as `grain`, in the open patch.
#[tauri::command]
fn preset_names(state: tauri::State<'_, Audio>, node: String) -> Vec<String> {
    state.presets.lock().expect("presets poisoned").names(&node)
}

// The preset commands take the presets lock, then the modulation lock, always
// in that order.

/// Store a node's current values and links as a new preset. Refuses a name in
/// use, so a preset is never overwritten by accident; that is what
/// `update_preset` is for.
#[tauri::command]
fn save_preset(state: tauri::State<'_, Audio>, node: String, name: String) -> Result<(), String> {
    let mut presets = state.presets.lock().expect("presets poisoned");
    let doc = state.modulation.lock().expect("modulation poisoned");
    presets.save(&state.bank, &doc.links, &node, &name)
}

/// Overwrite an existing preset with the node's current values and links.
#[tauri::command]
fn update_preset(state: tauri::State<'_, Audio>, node: String, name: String) -> Result<(), String> {
    let mut presets = state.presets.lock().expect("presets poisoned");
    let doc = state.modulation.lock().expect("modulation poisoned");
    presets.update(&state.bank, &doc.links, &node, &name)
}

/// Write a preset's values into the bank and its links into the patch.
/// Smoothed parameters glide to their new values; the section's switch is
/// left as it is.
#[tauri::command]
fn apply_preset(
    state: tauri::State<'_, Audio>,
    node: String,
    name: String,
) -> Result<presets::ApplyReport, String> {
    let presets = state.presets.lock().expect("presets poisoned");
    let mut doc = state.modulation.lock().expect("modulation poisoned");
    let mut report = presets.apply(&state.bank, &mut doc.links, &node, &name)?;
    // Only this node's refusals: anything else refused was already reported
    // when it arrived.
    report.refused = state
        .send_modulation(&doc)
        .into_iter()
        .filter(|r| presets::belongs(&node, &r.id))
        .collect();
    Ok(report)
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

// The modulation commands take only the modulation lock.

/// The open patch's LFOs and links. Builds the set to find what is refused,
/// but sends nothing: the audio thread already has it.
#[tauri::command]
fn modulation(state: tauri::State<'_, Audio>) -> ModulationView {
    let doc = state.modulation.lock().expect("modulation poisoned");
    ModulationView::of(&doc, doc.build().1)
}

/// Add an LFO. It is last in the answer's `lfos`.
#[tauri::command]
fn add_lfo(state: tauri::State<'_, Audio>) -> Result<ModulationView, String> {
    state.edit_modulation(|doc| {
        doc.add_lfo();
        Ok(())
    })
}

/// Remove an LFO. Parameters that followed it stay linked, and are reported.
#[tauri::command]
fn remove_lfo(state: tauri::State<'_, Audio>, id: u64) -> Result<ModulationView, String> {
    state.edit_modulation(|doc| doc.remove_lfo(id))
}

/// Change an LFO's name, rate, shape and phase. Running LFOs keep their place.
#[tauri::command]
fn set_lfo(
    state: tauri::State<'_, Audio>,
    lfo: modulation::LfoRecord,
) -> Result<ModulationView, String> {
    state.edit_modulation(|doc| doc.set_lfo(lfo))
}

/// Make a parameter follow an LFO, or change the depth it follows at.
#[tauri::command]
fn link_param(
    state: tauri::State<'_, Audio>,
    id: String,
    lfo: u64,
    lo: f32,
    hi: f32,
) -> Result<ModulationView, String> {
    state.edit_modulation(|doc| doc.link(&id, lfo, lo, hi))
}

/// Stop a parameter following anything.
#[tauri::command]
fn unlink_param(state: tauri::State<'_, Audio>, id: String) -> Result<ModulationView, String> {
    state.edit_modulation(|doc| {
        doc.unlink(&id);
        Ok(())
    })
}

/// Start or stop playback. Stopping clears the grain pool, so stop is stop.
#[tauri::command]
fn set_playing(state: tauri::State<'_, Audio>, playing: bool) {
    state.play_request.store(playing, Ordering::Relaxed);
}

/// The tracker, as saved and as the engine plays it.
#[tauri::command]
fn tracker(state: tauri::State<'_, Audio>) -> tracker::Tracker {
    state.tracker.lock().expect("tracker poisoned").clone()
}

/// Replace the tracker whole. Answers with what the engine now plays, which
/// differs from what was sent only where a value was out of range.
#[tauri::command]
fn set_tracker(state: tauri::State<'_, Audio>, tracker: tracker::Tracker) -> tracker::Tracker {
    apply_tracker(&state, tracker)
}

/// The one way the tracker changes: brought into range, handed to the audio
/// thread, and kept as the document.
fn apply_tracker(state: &Audio, next: tracker::Tracker) -> tracker::Tracker {
    let next = next.sanitised();
    state.steps.store(&next.params());
    *state.tracker.lock().expect("tracker poisoned") = next.clone();
    next
}

#[tauri::command]
fn source_info(state: tauri::State<'_, Audio>) -> SourceInfo {
    let s = state.source.lock().expect("source poisoned");
    SourceInfo {
        name: s.name.clone(),
        seconds: s.samples.len() as f32 / state.sample_rate,
        sample_rate: state.sample_rate,
        peaks: source::peaks(&s.samples, 900),
    }
}

/// Decode on this thread, then hand the buffer to the audio thread through a
/// queue. The audio thread takes it at a block boundary; it never decodes and
/// never allocates.
#[tauri::command]
fn load_sample(state: tauri::State<'_, Audio>, path: String) -> Result<SourceInfo, String> {
    let loaded = source::load(&path, state.sample_rate)?;
    let info = SourceInfo {
        name: loaded.name.clone(),
        seconds: loaded.samples.len() as f32 / state.sample_rate,
        sample_rate: state.sample_rate,
        peaks: source::peaks(&loaded.samples, 900),
    };
    *state.swap.lock().expect("swap poisoned") = Some(loaded.samples.clone());
    *state.source.lock().expect("source poisoned") = loaded;
    Ok(info)
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
    pub name: String,
    /// Moved within the last quarter second.
    pub active: bool,
    /// The last value it sent, 0 to 127, once it has sent one.
    pub last: Option<u8>,
}

#[derive(Serialize)]
pub struct ControllersView {
    pub devices: Vec<DeviceView>,
    pub controls: Vec<ControlView>,
    pub roles: &'static [&'static str],
}

impl ControllersView {
    fn of(c: &midi::Controllers) -> Self {
        let registry = c.registry.lock().expect("registry poisoned");
        let activity = c.activity.lock().expect("activity poisoned");
        let connected = c.connected.lock().expect("connected poisoned");
        let now = Instant::now();
        ControllersView {
            devices: registry
                .devices
                .iter()
                .map(|d| DeviceView {
                    port: d.port.clone(),
                    name: d.name.clone(),
                    connected: connected.contains(&d.port),
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
                        name: k.name.clone(),
                        active,
                        last,
                    }
                })
                .collect(),
            roles: &controllers::Role::NAMES,
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
                let Some(index) = shard_dsp::params::index_of(id) else {
                    continue;
                };
                let name = registry
                    .control(*control)
                    .map(|k| k.name.clone())
                    .unwrap_or_default();
                let (armed, knob) = pickup.state(*control, &PARAMS[index], c.bank.get(index));
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
    if shard_dsp::params::index_of(&id).is_none() {
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
    if shard_dsp::params::index_of(&id).is_none() {
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

#[tauri::command]
fn set_active_map(
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
    Ok(MappingsView::of(&state))
}

#[tauri::command]
fn add_map(
    state: tauri::State<'_, Arc<midi::Controllers>>,
    name: String,
) -> Result<MappingsView, String> {
    edit_controllers(&state, |r| {
        let id = r.add_map(&name)?;
        r.set_active(id);
        Ok(())
    })?;
    state.pickup.lock().expect("pickup poisoned").rearm_all();
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
    /// Build the engine's modulation set from `doc` here, on the command
    /// thread, and leave it for the audio thread to take at a block boundary.
    /// A set still waiting from an earlier edit is replaced and freed here.
    fn send_modulation(&self, doc: &modulation::Modulation) -> Vec<modulation::Refused> {
        let (set, refused) = doc.build();
        *self.mod_swap.lock().expect("modulation swap poisoned") = Some(set);
        refused
    }

    /// Apply one edit to the open patch's LFOs and links, hand the rebuilt set
    /// to the audio thread, and answer with what the UI should now draw. A
    /// refused edit has changed nothing, so nothing is sent.
    fn edit_modulation(
        &self,
        edit: impl FnOnce(&mut modulation::Modulation) -> Result<(), String>,
    ) -> Result<ModulationView, String> {
        let mut doc = self.modulation.lock().expect("modulation poisoned");
        edit(&mut doc)?;
        let refused = self.send_modulation(&doc);
        Ok(ModulationView::of(&doc, refused))
    }

    /// The trimmed window in sample indices, mirroring the engine's own
    /// clamping so a drawn curve matches the one being heard.
    fn trim_indices(&self) -> (usize, usize) {
        let n = self.source.lock().expect("source poisoned").samples.len();
        if n < 2 {
            return (0, n);
        }
        let a = self
            .bank
            .get_by_id("trim.start")
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let b = self
            .bank
            .get_by_id("trim.end")
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let lo = ((a * n as f32) as usize).min(n - 2);
        let hi = ((b * n as f32) as usize).clamp(lo + 2, n);
        (lo, hi)
    }
}

fn build_audio() -> Result<Audio, String> {
    let bank = Arc::new(ParamBank::new());
    let heard = Arc::new(ParamBank::new());
    let peak = Arc::new(AtomicU32::new(0));
    let reduction = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    let grains = Arc::new(AtomicU32::new(0));
    let playing = Arc::new(AtomicBool::new(false));
    let step = Arc::new(AtomicU32::new(0));
    let steps = Arc::new(StepBank::new(tracker::Tracker::default().params()));
    let playhead = Arc::new(AtomicU32::new(0));
    let play_request = Arc::new(AtomicBool::new(false));
    let swap: Arc<Mutex<Option<Vec<f32>>>> = Arc::new(Mutex::new(None));
    let audition: Arc<Mutex<Option<GrainSpawn>>> = Arc::new(Mutex::new(None));
    let auditioning = Arc::new(AtomicBool::new(false));
    let reversing = Arc::new(AtomicBool::new(false));
    let retired: Arc<Mutex<Option<Vec<f32>>>> = Arc::new(Mutex::new(None));
    let mod_swap: Arc<Mutex<Option<ModSet>>> = Arc::new(Mutex::new(None));
    let mod_retired: Arc<Mutex<Option<ModSet>>> = Arc::new(Mutex::new(None));
    let timer = Arc::new(BlockTimer::new());

    // Lock every slot the callback hands off through once, here. On macOS a
    // mutex allocates on its first lock, and without this that first lock
    // would land on the audio thread. `tests/audio_thread.rs` in shard-dsp
    // holds the pattern. The tracker's `steps` is not here on purpose: it is
    // atomics, like the bank, and has no lock to prime.
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
    let audio_steps = Arc::clone(&steps);
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

                let mut engine = Engine::new(sample_rate, 256);
                engine.set_source(source::startup_drone(sample_rate).samples);
                let log = engine.grain_log();
                let mut scratch = vec![0.0f32; 8192];
                // A swapped-out source, held until the UI side can take it.
                let mut retiring: Option<Vec<f32>> = None;
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
                                if let Some(old) = retiring.take() {
                                    match audio_retired.try_lock() {
                                        Ok(mut slot) if slot.is_none() => *slot = Some(old),
                                        _ => retiring = Some(old),
                                    }
                                }

                                // Locks on this thread are only ever tried,
                                // never waited on. If the UI holds one this
                                // block, the hand-off happens on the next.
                                if retiring.is_none() {
                                    if let Ok(mut pending) = audio_swap.try_lock() {
                                        if let Some(buf) = pending.take() {
                                            retiring = Some(engine.set_source(buf));
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
                                engine.set_steps(audio_steps.load());
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

    Ok(Audio {
        bank,
        heard,
        peak,
        reduction,
        grains,
        playing,
        step,
        grain_log,
        audition,
        auditioning,
        reversing,
        playhead,
        play_request,
        steps,
        tracker: Mutex::new(tracker::Tracker::default()),
        source: Mutex::new(source::startup_drone(sample_rate)),
        presets: Mutex::new(presets::Presets::default()),
        modulation: Mutex::new(modulation::Modulation::default()),
        swap,
        retired,
        mod_swap,
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(audio)
        .setup(|app| {
            // Controllers belong to this machine, so they live in the app's
            // config directory rather than in any patch.
            let path = app
                .path()
                .app_config_dir()
                .map_err(|e| e.to_string())?
                .join("controllers.json");
            let bank = Arc::clone(&app.state::<Audio>().bank);
            let shared = Arc::new(midi::Controllers::load(path, bank));
            midi::start(Arc::clone(&shared));
            app.manage(shared);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            param_defs,
            get_params,
            set_param,
            meters,
            set_playing,
            tracker,
            set_tracker,
            envelope_curve,
            save_patch,
            load_patch,
            source_info,
            load_sample,
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
            link_param,
            unlink_param,
            get_heard,
            lfo_limits,
            controllers,
            rename_control,
            set_control_role,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
        for key in ["tempo", "swing", "tracks"] {
            assert!(tracker.get(key).is_some(), "Tracker lost `{key}`");
        }
        for key in ["on", "length", "pattern", "pitches"] {
            assert!(
                tracker["tracks"][0].get(key).is_some(),
                "Track lost `{key}`"
            );
        }

        let report = serde_json::to_value(presets::ApplyReport::default())
            .expect("ApplyReport is serialisable");
        for key in ["applied", "unknown", "refused"] {
            assert!(report.get(key).is_some(), "ApplyReport lost `{key}`");
        }

        let load =
            serde_json::to_value(patch::LoadReport::default()).expect("LoadReport is serialisable");
        for key in [
            "applied",
            "unknown",
            "missing",
            "sample_path",
            "sample_missing",
            "refused",
            "map_missing",
        ] {
            assert!(load.get(key).is_some(), "LoadReport lost `{key}`");
        }

        let refused = serde_json::to_value(modulation::Refused {
            id: "grain.size".into(),
            reason: "x".into(),
        })
        .expect("Refused is serialisable");
        for key in ["id", "reason"] {
            assert!(refused.get(key).is_some(), "Refused lost `{key}`");
        }

        let mut doc = modulation::Modulation::default();
        let id = doc.add_lfo().id;
        doc.link("grain.size", id, 0.25, 0.75).unwrap();
        let view = serde_json::to_value(ModulationView::of(&doc, Vec::new()))
            .expect("ModulationView is serialisable");
        for key in ["lfos", "links", "refused"] {
            assert!(view.get(key).is_some(), "ModulationView lost `{key}`");
        }

        let cv = serde_json::to_value(ControllersView {
            devices: vec![DeviceView {
                port: "LPD8".into(),
                name: "LPD8".into(),
                connected: true,
            }],
            controls: vec![ControlView {
                id: 1,
                device: "LPD8".into(),
                channel: 0,
                kind: "cc",
                number: 1,
                role: "knob",
                name: "K1".into(),
                active: false,
                last: None,
            }],
            roles: &controllers::Role::NAMES,
        })
        .expect("ControllersView is serialisable");
        for key in ["devices", "controls", "roles"] {
            assert!(cv.get(key).is_some(), "ControllersView lost `{key}`");
        }
        for key in ["port", "name", "connected"] {
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
        for key in ["id", "name", "device"] {
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
        for key in ["lfo", "lo", "hi"] {
            assert!(
                view["links"]["grain.size"].get(key).is_some(),
                "LinkRecord lost `{key}`"
            );
        }
    }
}
