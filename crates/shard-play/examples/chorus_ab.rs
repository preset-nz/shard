//! Renders the chorus's voicings side by side so they can be listened to:
//! `cargo run --release -p shard-play --example chorus_ab -- <out dir>`.
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

fn render(dir: &str, name: &str, src: &[f32], p: Option<ChorusParams>) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(format!("{dir}/{name}.wav"), spec).unwrap();
    let mut c = Chorus::new(SR);
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
    println!("{name}: peak {peak:.2}, {took:?} for {SECONDS} s");
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
}
