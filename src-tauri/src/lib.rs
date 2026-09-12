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

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use shard_dsp::params::{index_of, Taper, Unit, PARAMS};
use shard_dsp::{Engine, ParamBank};

mod drift;
mod source;

/// Shared between the UI thread, the drift thread and the audio thread.
pub struct Audio {
    bank: Arc<ParamBank>,
    /// Peak since the UI last asked, as f32 bits. Written by the audio thread.
    peak: Arc<AtomicU32>,
    /// Active grain count, for the UI. Written by the audio thread.
    grains: Arc<AtomicU32>,
    drift: Arc<drift::DriftState>,
    /// The loaded sample, kept so the UI can draw a waveform and so a reload
    /// can replace it. Swapping goes through the queue, never a lock on audio.
    source: Mutex<source::Loaded>,
    swap: Arc<Mutex<Option<Vec<f32>>>>,
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
}

#[derive(Serialize)]
pub struct SourceInfo {
    pub name: String,
    pub seconds: f32,
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
    Meters {
        peak: f32::from_bits(state.peak.swap(0, Ordering::Relaxed)),
        grains: state.grains.load(Ordering::Relaxed),
        drift: state.drift.master(),
        drifting: state.drift.snapshot(),
    }
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
fn build_audio() -> Result<Audio, String> {
    let bank = Arc::new(ParamBank::new());
    let peak = Arc::new(AtomicU32::new(0));
    let grains = Arc::new(AtomicU32::new(0));
    let drift = Arc::new(drift::DriftState::new());
    let swap: Arc<Mutex<Option<Vec<f32>>>> = Arc::new(Mutex::new(None));

    let audio_bank = Arc::clone(&bank);
    let audio_peak = Arc::clone(&peak);
    let audio_grains = Arc::clone(&grains);
    let audio_swap = Arc::clone(&swap);

    let (tx, rx) = std::sync::mpsc::channel::<Result<f32, String>>();

    std::thread::Builder::new()
        .name("shard-audio".into())
        .spawn(move || {
            let built = (|| -> Result<(cpal::Stream, f32), String> {
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
                let mut scratch = vec![0.0f32; 8192];

                let stream = device
                    .build_output_stream(
                        &config.into(),
                        move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            // The only lock on this thread, and it never
                            // blocks. If the UI holds it this block, the swap
                            // happens on the next one.
                            if let Ok(mut pending) = audio_swap.try_lock() {
                                if let Some(buf) = pending.take() {
                                    engine.set_source(buf);
                                }
                            }

                            let frames = out.len() / channels;
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

                            // Hold the running maximum until the UI reads it,
                            // so a peak between polls is never missed.
                            let block_peak = engine.take_peak();
                            let prev = f32::from_bits(audio_peak.load(Ordering::Relaxed));
                            audio_peak.store(block_peak.max(prev).to_bits(), Ordering::Relaxed);
                            audio_grains.store(engine.active_grains() as u32, Ordering::Relaxed);
                        },
                        |err| eprintln!("audio stream error: {err}"),
                        None,
                    )
                    .map_err(|e| format!("could not open the output stream: {e}"))?;

                stream
                    .play()
                    .map_err(|e| format!("could not start the stream: {e}"))?;
                Ok((stream, sample_rate))
            })();

            match built {
                Ok((stream, sample_rate)) => {
                    let _ = tx.send(Ok(sample_rate));
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

    let sample_rate = rx
        .recv()
        .map_err(|_| "the audio thread died during startup".to_string())??;

    Ok(Audio {
        bank,
        peak,
        grains,
        drift,
        source: Mutex::new(source::startup_drone(sample_rate)),
        swap,
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
            source_info,
            load_sample,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
