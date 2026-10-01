//! Renders the palette's time-based effects so they can be listened to:
//! `cargo run --release -p shard-play --example palette_ab -- <out dir> [voice.wav]`.
//!
//! The source is a plucked phrase (four notes, a decaying tone with a few
//! harmonics, so repeats and tails have something to repeat). Given a WAV it
//! also renders that, mixed to mono, through the effects that suit a voice.
//! Each file is stereo, 10 s, with the settings in its name. The last pair
//! puts the same three effects in two orders, to hear what the order does.

use shard_dsp::fx::{self, Kind, Order};
use shard_dsp::{Engine, ParamBank};
use std::f32::consts::TAU;

const SR: f32 = 48_000.0;
const SECONDS: usize = 10;

/// Four plucked notes a phrase, repeating every three seconds.
fn pluck_phrase() -> Vec<f32> {
    let notes = [220.0f32, 277.18, 329.63, 246.94];
    let len = SECONDS * SR as usize;
    let mut out = vec![0.0f32; len];
    for (k, hz) in notes.iter().enumerate() {
        let start = ((k as f32 * 0.5) * SR) as usize;
        for i in 0..(1.2 * SR) as usize {
            if start + i >= len {
                break;
            }
            let t = i as f32 / SR;
            let env = (-t * 4.0).exp() * (t / 0.004).min(1.0);
            let tone: f32 = (1..=4)
                .map(|h| (TAU * hz * h as f32 * t).sin() / (h * h) as f32)
                .sum();
            out[start + i] += tone * env;
        }
    }
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

/// The instance of the first copy of `kind`, and a row's id on it.
fn row(kind: Kind, param: &str) -> String {
    let n = fx::POOL
        .iter()
        .position(|&(k, c)| k == kind && c == 0)
        .unwrap();
    format!("fx.{n}.{}.{param}", kind.name())
}

/// One effect in the chain, with the rows to set (`mix`, `time`, ...).
type Stage<'a> = (Kind, &'a [(&'a str, f32)]);

fn render(dir: &str, name: &str, source: &[f32], sr: f32, chain: &[Stage]) {
    let mut engine = Engine::new(sr, 256);
    engine.set_source(source.to_vec());
    engine.set_playing(true);
    let bank = ParamBank::new();
    let mut order = Order::EMPTY;
    for (kind, rows) in chain {
        order.add(*kind).expect("a free copy");
        for (param, value) in rows.iter() {
            assert!(
                bank.set_by_id(&row(*kind, param), *value),
                "no row {}",
                row(*kind, param)
            );
        }
        bank.set_by_id(&row(*kind, "on"), 1.0);
    }
    fx::set_order(&bank, &order);

    let path = format!("{dir}/{name}.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: sr as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("a writable file");
    let mut buf = vec![0.0f32; 512];
    let blocks = (SECONDS as f32 * sr) as usize / 256;
    let mut peak = 0.0f32;
    for _ in 0..blocks {
        engine.process_block(&mut buf, &bank);
        for s in &buf {
            peak = peak.max(s.abs());
            writer
                .write_sample((s.clamp(-1.0, 1.0) * 32_767.0) as i16)
                .unwrap();
        }
    }
    writer.finalize().unwrap();
    println!("{name}: peak {peak:.2}");
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: palette_ab <out dir> [voice.wav]");
    std::fs::create_dir_all(&dir).unwrap();
    let pluck = pluck_phrase();

    use Kind::*;
    let cases: Vec<(&str, Vec<Stage>)> = vec![
        ("01-dry", vec![]),
        (
            "02-delay-350ms-fb40-mix50",
            vec![(Delay, &[("mix", 0.5), ("time", 350.0), ("feedback", 0.4)])],
        ),
        (
            "03-delay-pingpong-300ms-fb55",
            vec![(
                Delay,
                &[
                    ("mix", 0.5),
                    ("time", 300.0),
                    ("feedback", 0.55),
                    ("pingpong", 1.0),
                ],
            )],
        ),
        (
            "04-echo-300ms-fb50-tone3500-wobble30-grit30",
            vec![(Echo, &[("mix", 0.5), ("feedback", 0.5)])],
        ),
        (
            "05-echo-worn-fb65-tone1800-wobble80-grit70",
            vec![(
                Echo,
                &[
                    ("mix", 0.55),
                    ("feedback", 0.65),
                    ("tone", 1800.0),
                    ("wobble", 0.8),
                    ("grit", 0.7),
                ],
            )],
        ),
        (
            "06-flanger-rate0.25-depth70-fb50",
            vec![(Flanger, &[("mix", 0.6)])],
        ),
        (
            "07-flanger-jet-fb-negative",
            vec![(
                Flanger,
                &[("mix", 0.7), ("feedback", -0.8), ("manual", 0.8)],
            )],
        ),
        (
            "08-reverb-room-decay1.2",
            vec![(Reverb, &[("mix", 0.4), ("type", 0.0), ("decay", 1.2)])],
        ),
        (
            "09-reverb-hall-decay3.5",
            vec![(Reverb, &[("mix", 0.4), ("type", 1.0), ("decay", 3.5)])],
        ),
        (
            "10-reverb-plate-decay2.5",
            vec![(Reverb, &[("mix", 0.4), ("type", 2.0), ("decay", 2.5)])],
        ),
        (
            "11-wear-wow40-flutter30-dropouts20-dull40",
            vec![(Wear, &[("mix", 0.9)])],
        ),
        (
            "12-wear-unstable80",
            vec![(Wear, &[("mix", 0.9), ("unstable", 0.8), ("wow", 0.6)])],
        ),
        ("13-rise-plus7-fb60", vec![(Rise, &[("mix", 0.5)])]),
        (
            "14-rise-plus12-fb70",
            vec![(Rise, &[("mix", 0.5), ("shift", 12.0), ("feedback", 0.7)])],
        ),
        (
            "15-overtone-sub50-octave30",
            vec![(Overtone, &[("mix", 0.8)])],
        ),
        (
            "16-overtone-all-three",
            vec![(
                Overtone,
                &[("mix", 0.8), ("sub", 0.6), ("octave", 0.5), ("fifth", 0.5)],
            )],
        ),
        (
            "17-order-wear-echo-reverb",
            vec![
                (Wear, &[("mix", 0.8)]),
                (Echo, &[("mix", 0.5)]),
                (Reverb, &[("mix", 0.4), ("decay", 2.5)]),
            ],
        ),
        (
            "18-order-reverb-echo-wear",
            vec![
                (Reverb, &[("mix", 0.4), ("decay", 2.5)]),
                (Echo, &[("mix", 0.5)]),
                (Wear, &[("mix", 0.8)]),
            ],
        ),
    ];
    for (name, chain) in &cases {
        render(&dir, name, &pluck, SR, chain);
    }

    if let Some(path) = std::env::args().nth(2) {
        let (voice, sr) = load(&path);
        let voice_cases: Vec<(&str, Vec<Stage>)> = vec![
            ("voice-1-dry", vec![]),
            (
                "voice-2-reverb-hall-decay3.5",
                vec![(Reverb, &[("mix", 0.35), ("type", 1.0), ("decay", 3.5)])],
            ),
            (
                "voice-3-echo-300ms",
                vec![(Echo, &[("mix", 0.4), ("feedback", 0.45)])],
            ),
            ("voice-4-rise-plus7", vec![(Rise, &[("mix", 0.4)])]),
            (
                "voice-5-overtone-sub-octave",
                vec![(Overtone, &[("mix", 0.7), ("sub", 0.5), ("octave", 0.3)])],
            ),
            (
                "voice-6-wear-chain-wear-echo-reverb",
                vec![
                    (Wear, &[("mix", 0.7)]),
                    (Echo, &[("mix", 0.4)]),
                    (Reverb, &[("mix", 0.35), ("decay", 3.0)]),
                ],
            ),
        ];
        for (name, chain) in &voice_cases {
            render(&dir, name, &voice, sr, chain);
        }
    }
}
