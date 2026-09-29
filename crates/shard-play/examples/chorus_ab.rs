//! Renders the chorus's voicings side by side so they can be listened to:
//! `cargo run --release -p shard-play --example chorus_ab -- <out dir> [voice.wav]`.
//!
//! Given a WAV as well, it also renders that, mixed to mono, through the
//! voicings meant for voices, at the file's own sample rate.
//!
//! The source is a mono "ah": a sawtooth chord (A2, E3, A3, C#4, E4, B4) held
//! through three formant resonators, so there is plenty of harmonic content
//! for the copies to beat against. Each file is 48 kHz stereo, 8 s.

use shard_dsp::chorus::{Chorus, ChorusParams, ChorusType};
use std::f32::consts::TAU;

const SR: f32 = 48_000.0;
const SECONDS: usize = 8;

/// A two-pole resonator, for one formant.
struct Formant {
    a1: f32,
    a2: f32,
    gain: f32,
    y1: f32,
    y2: f32,
}

impl Formant {
    fn new(hz: f32, bandwidth: f32, gain: f32) -> Self {
        let r = (-core::f32::consts::PI * bandwidth / SR).exp();
        Self {
            a1: 2.0 * r * (TAU * hz / SR).cos(),
            a2: -r * r,
            gain,
            y1: 0.0,
            y2: 0.0,
        }
    }
    fn process(&mut self, x: f32) -> f32 {
        let y = x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y * self.gain
    }
}

fn source() -> Vec<f32> {
    let notes = [110.0f32, 164.81, 220.0, 277.18, 329.63, 493.88];
    let mut formants = [
        Formant::new(700.0, 110.0, 0.030),
        Formant::new(1100.0, 120.0, 0.020),
        Formant::new(2500.0, 200.0, 0.012),
    ];
    let mut phases = [0.0f32; 6];
    let mut out: Vec<f32> = (0..SECONDS * SR as usize)
        .map(|i| {
            let t = i as f32 / SR;
            let mut saw = 0.0;
            for (ph, f) in phases.iter_mut().zip(notes) {
                // A touch of vibrato, so the dry is alive and not a test tone.
                let hz = f * (1.0 + 0.003 * (TAU * 5.2 * t).sin());
                *ph = (*ph + hz / SR).fract();
                saw += 2.0 * *ph - 1.0;
            }
            let env = (t / 0.6).min(1.0) * ((SECONDS as f32 - t) / 0.8).min(1.0);
            let x: f32 = formants.iter_mut().map(|f| f.process(saw)).sum();
            x * env
        })
        .collect();
    // Peak at half scale, so the wet has headroom to be judged by ear.
    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    out.iter_mut().for_each(|x| *x *= 0.5 / peak);
    out
}

/// A WAV as mono at half scale, and its sample rate.
fn load(path: &str) -> (Vec<f32>, f32) {
    let mut r = hound::WavReader::open(path).expect("a readable WAV");
    let spec = r.spec();
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(Result::unwrap).collect(),
        hound::SampleFormat::Int => {
            let full = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.unwrap() as f32 / full)
                .collect()
        }
    };
    let ch = spec.channels as usize;
    let mut mono: Vec<f32> = raw
        .chunks(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect();
    let peak = mono.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    mono.iter_mut().for_each(|x| *x *= 0.5 / peak.max(1e-9));
    (mono, spec.sample_rate as f32)
}

fn render(dir: &str, name: &str, src: &[f32], p: Option<ChorusParams>) {
    render_at(dir, name, src, SR, p);
}

fn render_at(dir: &str, name: &str, src: &[f32], sr: f32, p: Option<ChorusParams>) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: sr as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(format!("{dir}/{name}.wav"), spec).unwrap();
    let mut c = Chorus::new(sr);
    let mut peak = 0.0f32;
    let started = std::time::Instant::now();
    let out: Vec<(f32, f32)> = src
        .iter()
        .map(|&x| match &p {
            Some(p) => c.process(x, x, p),
            None => (x, x),
        })
        .collect();
    let took = started.elapsed();
    for (l, r) in out {
        peak = peak.max(l.abs()).max(r.abs());
        for v in [l, r] {
            w.write_sample((v.clamp(-1.0, 1.0) * 32_767.0) as i16)
                .unwrap();
        }
    }
    w.finalize().unwrap();
    let seconds = src.len() as f32 / sr;
    println!("{name}: peak {peak:.2}, {took:?} for {seconds:.1} s");
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: chorus_ab <out dir>");
    std::fs::create_dir_all(&dir).unwrap();
    let src = source();
    let at = |kind, mix, rate, depth, eq_db| ChorusParams {
        kind,
        mix,
        rate,
        depth,
        voices: 4.0,
        eq_db,
    };
    render(&dir, "1-dry", &src, None);
    render(
        &dir,
        "2-chorus-defaults-mix50-rate0.6-depth50-eq0",
        &src,
        Some(at(ChorusType::Chorus, 0.5, 0.6, 0.5, 0.0)),
    );
    render(
        &dir,
        "3-ensemble-mix60-rate0.6-depth80-eq+3",
        &src,
        Some(at(ChorusType::Ensemble, 0.6, 0.6, 0.8, 3.0)),
    );
    render(
        &dir,
        "4-ensemble-lush-mix75-rate0.4-depth100-eq+4",
        &src,
        Some(at(ChorusType::Ensemble, 0.75, 0.4, 1.0, 4.0)),
    );
    render(
        &dir,
        "5-ensemble-wet-only-mix100-rate0.6-depth80-eq+3",
        &src,
        Some(at(ChorusType::Ensemble, 1.0, 0.6, 0.8, 3.0)),
    );
    // The voice count's two ends, against file 2 (four voices).
    render(
        &dir,
        "6-chorus-1voice-mix50-rate0.6-depth50-eq0",
        &src,
        Some(ChorusParams {
            voices: 1.0,
            ..at(ChorusType::Chorus, 0.5, 0.6, 0.5, 0.0)
        }),
    );
    render(
        &dir,
        "7-chorus-8voices-mix50-rate0.6-depth50-eq0",
        &src,
        Some(ChorusParams {
            voices: 8.0,
            ..at(ChorusType::Chorus, 0.5, 0.6, 0.5, 0.0)
        }),
    );
    // File 4 with every voice: the widest, smoothest wall it makes.
    render(
        &dir,
        "8-ensemble-lush-8voices-mix75-rate0.4-depth100-eq+4",
        &src,
        Some(ChorusParams {
            voices: 8.0,
            ..at(ChorusType::Ensemble, 0.75, 0.4, 1.0, 4.0)
        }),
    );
    let choir = |mix, rate, depth, voices| ChorusParams {
        voices,
        ..at(ChorusType::Choir, mix, rate, depth, 0.0)
    };
    render(
        &dir,
        "9-choir-mix50-rate0.6-depth70-voices6",
        &src,
        Some(choir(0.5, 0.6, 0.7, 6.0)),
    );

    let Some(path) = std::env::args().nth(2) else {
        return;
    };
    let (voice, sr) = load(&path);
    render_at(&dir, "voice-1-dry", &voice, sr, None);
    render_at(
        &dir,
        "voice-2-chorus-mix50-rate0.6-depth50-voices4",
        &voice,
        sr,
        Some(at(ChorusType::Chorus, 0.5, 0.6, 0.5, 0.0)),
    );
    render_at(
        &dir,
        "voice-3-choir-mix50-rate0.6-depth70-voices6",
        &voice,
        sr,
        Some(choir(0.5, 0.6, 0.7, 6.0)),
    );
    render_at(
        &dir,
        "voice-4-choir-big-mix60-rate0.8-depth100-voices8",
        &voice,
        sr,
        Some(choir(0.6, 0.8, 1.0, 8.0)),
    );
}
