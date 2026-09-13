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

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use shard_dsp::params::{index_of, Taper, Unit, PARAMS};
use shard_dsp::rt::{self, BlockTimer};
use shard_dsp::{Engine, GrainLog, GrainSpawn, ParamBank};

mod drift;
mod patch;
mod source;

/// Debug builds count every allocator call made inside the audio callback,
/// and `meters` reports the running total. Release builds keep the plain
/// system allocator and always report zero. See `shard_dsp::rt`.
#[cfg(debug_assertions)]
#[global_allocator]
static GUARD: rt::GuardedAlloc = rt::GuardedAlloc;

/// Shared between the UI thread, the drift thread and the audio thread.
pub struct Audio {
    bank: Arc<ParamBank>,
    /// Peak since the UI last asked, as f32 bits. Written by the audio thread.
    peak: Arc<AtomicU32>,
    /// Active grain count, for the UI. Written by the audio thread.
    grains: Arc<AtomicU32>,
    drift: Arc<drift::DriftState>,
    /// Transport, mirrored out of the audio thread for the UI.
    playing: Arc<AtomicBool>,
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
    /// The loaded sample, kept so the UI can draw a waveform and so a reload
    /// can replace it. Swapping goes through the queue, never a lock on audio.
    source: Mutex<source::Loaded>,
    swap: Arc<Mutex<Option<Vec<f32>>>>,
    /// A source buffer the audio thread swapped out and handed back, waiting
    /// for `meters` to free it here rather than on the audio thread.
    retired: Arc<Mutex<Option<Vec<f32>>>>,
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
    /// Whether this parameter can be handed to drift at all. Stepped ones
    /// cannot: walking through window shapes at random is a different feature.
    pub can_drift: bool,
    pub unit: &'static str,
    pub smooth_ms: f32,
}

#[derive(Serialize)]
pub struct Meters {
    pub peak: f32,
    pub grains: u32,
    /// Master drift switch.
    pub drift: bool,
    /// Per-parameter drift flags, indexed as `PARAMS`.
    pub drifting: Vec<bool>,
    pub playing: bool,
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
            can_drift: !matches!(p.taper, Taper::Stepped(_)),
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
        grains: state.grains.load(Ordering::Relaxed),
        drift: state.drift.master(),
        drifting: state.drift.snapshot(),
        playing: state.playing.load(Ordering::Relaxed),
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
        amount: state.bank.get_by_id("env.amount").unwrap_or(1.0)
            * state.bank.get_by_id("env.on").unwrap_or(1.0),
        attack_ms: state.bank.get_by_id("env.attack").unwrap_or(0.0),
        decay_ms: state.bank.get_by_id("env.decay").unwrap_or(0.0),
        sustain: state.bank.get_by_id("env.sustain").unwrap_or(1.0),
        release_ms: state.bank.get_by_id("env.release").unwrap_or(0.0),
    };
    env.curve(256, (hi - lo) as f32, state.sample_rate)
}

/// Write the current sound to a `.shard` file. A few kilobytes of readable
/// JSON: parameter values by id, the drift flags, and where the sample was.
#[tauri::command]
fn save_patch(state: tauri::State<'_, Audio>, path: String) -> Result<(), String> {
    let sample = {
        let s = state.source.lock().expect("source poisoned");
        s.path.clone()
    };
    let p = patch::Patch::capture(&state.bank, &state.drift.snapshot(), sample);
    std::fs::write(&path, p.to_json()?).map_err(|e| format!("{path}: {e}"))
}

/// Read a `.shard` file back. Reloads the sample it names when that file is
/// still there, and says so plainly when it is not rather than loading half
/// the patch and looking fine.
#[tauri::command]
fn load_patch(state: tauri::State<'_, Audio>, path: String) -> Result<patch::LoadReport, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let p = patch::Patch::from_json(&text)?;
    let mut report = p.apply(&state.bank);

    for (i, def) in PARAMS.iter().enumerate() {
        state.drift.set(i, p.drifting.iter().any(|id| id == def.id));
    }

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

/// Start or stop playback. Stopping clears the grain pool, so stop is stop.
#[tauri::command]
fn set_playing(state: tauri::State<'_, Audio>, playing: bool) {
    state.play_request.store(playing, Ordering::Relaxed);
}

/// The master switch. Off means nothing drifts; flipping it back restores the
/// per-parameter flags rather than clearing them.
#[tauri::command]
fn set_drift(state: tauri::State<'_, Audio>, on: bool) {
    state.drift.set_master(on);
}

/// Hand one parameter to the oscillator, or take it back.
#[tauri::command]
fn set_param_drift(state: tauri::State<'_, Audio>, id: String, on: bool) -> Result<(), String> {
    let i = index_of(&id).ok_or_else(|| format!("unknown parameter: {id}"))?;
    if !drift::DriftState::can_drift(i) {
        return Err(format!("{id} cannot drift"));
    }
    state.drift.set(i, on);
    Ok(())
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
impl Audio {
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
    let peak = Arc::new(AtomicU32::new(0));
    let grains = Arc::new(AtomicU32::new(0));
    let drift = Arc::new(drift::DriftState::new());
    let playing = Arc::new(AtomicBool::new(false));
    let playhead = Arc::new(AtomicU32::new(0));
    let play_request = Arc::new(AtomicBool::new(false));
    let swap: Arc<Mutex<Option<Vec<f32>>>> = Arc::new(Mutex::new(None));
    let audition: Arc<Mutex<Option<GrainSpawn>>> = Arc::new(Mutex::new(None));
    let auditioning = Arc::new(AtomicBool::new(false));
    let reversing = Arc::new(AtomicBool::new(false));
    let retired: Arc<Mutex<Option<Vec<f32>>>> = Arc::new(Mutex::new(None));
    let timer = Arc::new(BlockTimer::new());

    // Lock every slot the callback hands off through once, here. On macOS a
    // mutex allocates on its first lock, and without this that first lock
    // would land on the audio thread. `tests/audio_thread.rs` in shard-dsp
    // holds the pattern.
    drop(swap.lock());
    drop(audition.lock());
    drop(retired.lock());

    let audio_bank = Arc::clone(&bank);
    let audio_peak = Arc::clone(&peak);
    let audio_grains = Arc::clone(&grains);
    let audio_swap = Arc::clone(&swap);
    let audio_playing = Arc::clone(&playing);
    let audio_playhead = Arc::clone(&playhead);
    let audio_request = Arc::clone(&play_request);
    let audio_audition = Arc::clone(&audition);
    let audio_auditioning = Arc::clone(&auditioning);
    let audio_reversing = Arc::clone(&reversing);
    let audio_retired = Arc::clone(&retired);
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
                                engine.process_block(&mut scratch[..needed], &audio_bank);

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
                                audio_grains
                                    .store(engine.active_grains() as u32, Ordering::Relaxed);
                                audio_playhead
                                    .store(engine.play_position().to_bits(), Ordering::Relaxed);
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
        peak,
        grains,
        drift,
        playing,
        grain_log,
        audition,
        auditioning,
        reversing,
        playhead,
        play_request,
        source: Mutex::new(source::startup_drone(sample_rate)),
        swap,
        retired,
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

    drift::spawn(Arc::clone(&audio.bank), Arc::clone(&audio.drift));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(audio)
        .invoke_handler(tauri::generate_handler![
            param_defs,
            get_params,
            set_param,
            meters,
            set_drift,
            set_param_drift,
            set_playing,
            envelope_curve,
            save_patch,
            load_patch,
            source_info,
            load_sample,
            grain_log,
            audition_grain,
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
            grains: 0,
            drift: true,
            drifting: vec![false],
            playing: false,
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
            "grains",
            "drift",
            "drifting",
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
    }
}
