//! A playground for the granular engine and the ring modulator.
//!
//! ```text
//! cargo run -p shard-play                      # built-in test tone
//! cargo run -p shard-play -- my-sample.wav     # your own material
//! cargo run -p shard-play -- my-sample.wav --still   # no auto-motion
//! ```
//!
//! The engine runs on the audio thread and reads parameters from an atomic
//! bank. A slow drift on the control thread writes to that bank, so you hear
//! the cloud move rather than a static texture. This is exactly the boundary
//! the UI will use later; the drift is just standing in for hands.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use shard_dsp::rt::{self, BlockTimer};
use shard_dsp::{Engine, ParamBank};

/// Debug builds count allocator calls inside the audio callback and report
/// them once a second. `just play` builds release, where this is absent; run
/// `cargo run -p shard-play` to have it. See `shard_dsp::rt`.
#[cfg(debug_assertions)]
#[global_allocator]
static GUARD: rt::GuardedAlloc = rt::GuardedAlloc;

/// Mono, at the engine's rate. Multi-channel files are summed.
fn load_wav(path: &str, target_rate: f32) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("{path}: {e}"))?;
    let spec = reader.spec();

    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };

    let channels = spec.channels.max(1) as usize;
    let mono: Vec<f32> = if channels == 1 {
        raw
    } else {
        raw.chunks(channels)
            .map(|f| f.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    // Naive resampling. Fine for granular material, which is already being
    // read at arbitrary rates by every grain. Not fine for anything that has
    // to stay in tune, which is why this lives in the playground and not in
    // the DSP crate.
    let ratio = spec.sample_rate as f32 / target_rate;
    let resampled = if (ratio - 1.0).abs() < 1e-6 {
        mono
    } else {
        let out_len = (mono.len() as f32 / ratio) as usize;
        (0..out_len)
            .map(|i| {
                let src = i as f32 * ratio;
                let j = src as usize;
                if j + 1 < mono.len() {
                    let f = src - j as f32;
                    mono[j] + (mono[j + 1] - mono[j]) * f
                } else {
                    *mono.last().unwrap_or(&0.0)
                }
            })
            .collect()
    };

    if resampled.len() < 2 {
        return Err(format!("{path}: too short to granulate"));
    }
    Ok(resampled)
}

/// Two seconds of a detuned drone. Enough structure that granulating it
/// clearly sounds like something, so the playground works with no files.
fn test_tone(sample_rate: f32) -> Vec<f32> {
    let n = (sample_rate * 2.0) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate;
            let body = (core::f32::consts::TAU * 110.0 * t).sin() * 0.5
                + (core::f32::consts::TAU * 164.8 * t).sin() * 0.3
                + (core::f32::consts::TAU * 220.7 * t).sin() * 0.2;
            // A slow amplitude swell gives the grains somewhere to find
            // contrast, so position sweeps are audible.
            body * (0.4 + 0.6 * (core::f32::consts::TAU * 0.25 * t).sin().abs())
        })
        .collect()
}

/// Render the same engine to a file instead of a device. Useful when the
/// speakers are the thing that is broken, and it makes the signal path
/// checkable in CI where there is no audio device at all.
fn render(source: Vec<f32>, sample_rate: f32, seconds: f32, out: &str) -> Result<(), String> {
    let mut engine = Engine::new(sample_rate, 256);
    engine.set_source(source);
    let bank = ParamBank::new();

    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: sample_rate as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(out, spec).map_err(|e| e.to_string())?;

    let block = 512;
    let blocks = ((sample_rate * seconds) / (block / 2) as f32) as usize;
    let mut buf = vec![0.0f32; block];
    let mut peak = 0.0f32;

    for b in 0..blocks {
        let t = (b * block / 2) as f32 / sample_rate;
        let osc =
            |hz: f32, phase: f32| 0.5 + 0.5 * (core::f32::consts::TAU * (hz * t + phase)).sin();
        bank.set_normalised("grain.position", osc(0.13, 0.0));
        bank.set_normalised("grain.size", osc(0.19, 0.3));
        bank.set_normalised("grain.density", 0.50 + 0.4 * osc(0.11, 0.6));
        bank.set_by_id("ring.mix", 0.5 * osc(0.23, 0.8));
        bank.set_normalised("ring.freq", osc(0.17, 0.5));

        engine.process_block(&mut buf, &bank);
        for s in &buf {
            peak = peak.max(s.abs());
            writer.write_sample(*s).map_err(|e| e.to_string())?;
        }
    }
    writer.finalize().map_err(|e| e.to_string())?;
    println!("wrote {out}: {seconds:.1}s, peak {peak:.3}");
    if peak < 0.001 {
        return Err("rendered silence — the signal path is broken".into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let still = args.iter().any(|a| a == "--still");
    let render_to = args
        .iter()
        .position(|a| a == "--render")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let path = args
        .iter()
        .find(|a| !a.starts_with("--") && Some(*a) != render_to.as_ref());

    if let Some(out) = render_to {
        // 48 kHz regardless of any device, so the render is reproducible.
        let sr = 48_000.0;
        let source = match path {
            Some(p) => load_wav(p, sr)?,
            None => test_tone(sr),
        };
        render(source, sr, 8.0, &out)?;
        return Ok(());
    }

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default output device")?;
    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate().0 as f32;
    let channels = config.channels() as usize;

    let source = match path {
        Some(p) => {
            let s = load_wav(p, sample_rate)?;
            println!("loaded {p}: {:.2}s", s.len() as f32 / sample_rate);
            s
        }
        None => {
            println!("no file given, using the built-in drone");
            test_tone(sample_rate)
        }
    };

    let mut engine = Engine::new(sample_rate, 256);
    engine.set_source(source);

    let bank = Arc::new(ParamBank::new());
    let audio_bank = Arc::clone(&bank);

    // Interleaved stereo scratch, sized once. The device may hand us a
    // different channel count, so the engine renders stereo here and the
    // callback spreads it.
    let mut scratch = vec![0.0f32; 4096];

    let timer = Arc::new(BlockTimer::new());
    let audio_timer = Arc::clone(&timer);

    let stream = device.build_output_stream(
        &config.clone().into(),
        move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let started = Instant::now();
            let frames = out.len() / channels;
            // Inside the guard, set here because the callback runs on the
            // device's own thread.
            rt::no_alloc(|| {
                let needed = frames * 2;
                if scratch.len() < needed {
                    // Cannot grow here without allocating on the audio thread,
                    // so render what fits and leave the rest silent. With a
                    // 4096-slot scratch this never happens at any sane buffer
                    // size.
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
            });
            audio_timer.record(
                started.elapsed(),
                Duration::from_secs_f64(frames as f64 / sample_rate as f64),
            );
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )?;

    stream.play()?;

    println!(
        "\n  {} Hz, {} ch, {} grains max",
        sample_rate as u32, channels, 256
    );
    if still {
        println!("  --still: parameters held at their defaults");
    } else {
        println!("  drifting position, size, density and ring frequency");
        println!("  ring modulation fades in after about 8 seconds");
    }
    println!("\n  ctrl-c to stop\n");

    let running = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&running);
    ctrl_c_hook(move || flag.store(false, Ordering::Relaxed));

    let start = Instant::now();
    let mut last_report = Instant::now();
    while running.load(Ordering::Relaxed) {
        if !still {
            let t = start.elapsed().as_secs_f32();
            // Slow, incommensurate rates so the texture never quite repeats.
            let osc =
                |hz: f32, phase: f32| 0.5 + 0.5 * (core::f32::consts::TAU * (hz * t + phase)).sin();
            bank.set_normalised("grain.position", osc(0.013, 0.0));
            bank.set_normalised("grain.size", osc(0.019, 0.3));
            bank.set_normalised("grain.density", 0.50 + 0.4 * osc(0.011, 0.6));
            bank.set_normalised("grain.jitter", 0.1 + 0.3 * osc(0.007, 0.2));
            bank.set_normalised("ring.freq", osc(0.017, 0.5));
            // Hold off the ring modulator so the granular layer is audible
            // on its own first, then bring it in and let it breathe.
            let ring = ((t - 8.0) / 12.0).clamp(0.0, 1.0);
            bank.set_by_id("ring.mix", ring * (0.25 + 0.45 * osc(0.023, 0.8)));
        }
        // Once a second, the slowest block since the last report. The average
        // would flatter; the worst case is the one that drops out.
        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            println!(
                "  worst block {:.2} ms of {:.1} ms · {} audio-thread allocations",
                timer.take_worst_us() as f32 / 1000.0,
                timer.budget_us() as f32 / 1000.0,
                rt::violations(),
            );
        }
        std::thread::sleep(Duration::from_millis(16));
    }

    println!("stopped");
    Ok(())
}

/// A ctrl-c handler without pulling in a crate for it. The default handler
/// would kill the process before the stream is dropped, which on some hosts
/// leaves the device in a bad state.
fn ctrl_c_hook<F: Fn() + Send + 'static>(f: F) {
    use std::sync::Mutex;
    static HOOK: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

    extern "C" fn handler(_: i32) {
        if let Ok(guard) = HOOK.lock() {
            if let Some(f) = guard.as_ref() {
                f();
            }
        }
    }

    if let Ok(mut guard) = HOOK.lock() {
        *guard = Some(Box::new(f));
    }
    // SAFETY: installing a signal handler that only sets an atomic through a
    // mutex we own. No allocation happens in the handler path.
    unsafe {
        libc_signal(2, handler as *const () as usize);
    }
}

#[cfg(unix)]
unsafe fn libc_signal(sig: i32, handler: usize) {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    signal(sig, handler);
}

#[cfg(not(unix))]
unsafe fn libc_signal(_sig: i32, _handler: usize) {}
