//! End to end: a `.shard` document in, audio out, through the paths the app
//! takes on open and in tracker mode, with no window and no sound card.
//!
//! The document is `tests/fixtures/trip-hop-acid.shard`, in rhizome's format
//! since epic 32 (Georg, 2026-10-04:
//! *"a trip hop drum loop with an acidic bass"*): pattern 6b of
//! `design/drum-programming.md` on the drum kit at 82 bpm and 56 % swing,
//! under a two-bar FM line in F through drive and a resonant low-pass that a
//! mod envelope plucks on every note. It opens in the app as it is.
//!
//! The assertions are contracts, not a golden buffer: it loads clean, it
//! stays in range, the drums land where the grid says, and the filter opens
//! and closes on each note. `just e2e` also writes the render to
//! `~/rhizomatic-preset/renders/` to listen to.

use std::path::Path;

use shard_dsp::{Engine, StepParams};

use crate::session_tests::{rig, Rig};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/trip-hop-acid.shard"
);

/// The document, opened the way `load_patch` opens it, and the steps tracker
/// mode plays.
struct Loaded {
    rig: Rig,
    steps: StepParams,
}

fn open(path: &str) -> (Loaded, crate::session::LoadReport) {
    let rig = rig();
    let report = rig.session.open(Path::new(path)).expect("the file opens");
    // Tracker mode is what switches the steps on.
    let mut steps = rig.session.plan().steps();
    steps.on = true;
    (Loaded { rig, steps }, report)
}

fn load() -> Loaded {
    let (l, report) = open(FIXTURE);
    assert!(report.unknown.is_empty(), "unknown: {:?}", report.unknown);
    assert!(report.refused.is_empty(), "refused: {:?}", report.refused);
    assert!(!report.sample_missing, "a material is missing");
    l
}

/// Interleaved stereo, `bars` bars, as the audio callback renders it.
fn render(l: &Loaded, steps: StepParams, bars: usize) -> Vec<f32> {
    let (set, refused) = l.rig.session.plan().mod_set();
    assert!(refused.is_empty(), "modulation refused: {refused:?}");
    let mut e = Engine::new(SR, 256);
    e.set_modulation(set);
    e.set_playing(true);
    let frames = (bars as f32 * step_len(&steps) * 16.0) as usize;
    let mut out = vec![0.0; BLOCK * 2];
    let mut all = Vec::with_capacity(frames * 2 + BLOCK * 2);
    while all.len() < frames * 2 {
        e.set_steps(steps);
        e.set_arrangement(&l.rig.arrangement, false);
        e.process_block(&mut out, &l.rig.bank);
        all.extend_from_slice(&out);
    }
    all
}

fn step_len(p: &StepParams) -> f32 {
    SR * 60.0 / p.tempo_bpm / 4.0
}

/// Where step `k` of a loop starts, in frames, with swing.
fn onset(p: &StepParams, k: usize) -> usize {
    let late = if k % 2 == 1 { p.swing } else { 0.0 };
    ((k as f32 + late) * step_len(p)) as usize
}

/// Mean square of the left channel over `frames`.
fn energy(x: &[f32], frames: std::ops::Range<usize>) -> f32 {
    let n = frames.len() as f32;
    frames.map(|i| x[i * 2] * x[i * 2]).sum::<f32>() / n
}

/// Mean square of the left channel's first difference: how much it jumps.
fn edges(x: &[f32], frames: std::ops::Range<usize>) -> f32 {
    let n = frames.len() as f32;
    frames
        .map(|i| (x[i * 2] - x[i * 2 - 2]).powi(2))
        .sum::<f32>()
        / n
}

/// How bright the left channel is over `frames`: the energy of its first
/// difference against its own, so a note getting quieter is not mistaken for
/// one getting darker.
fn brightness(x: &[f32], frames: std::ops::Range<usize>) -> f32 {
    let edges: f32 = frames
        .clone()
        .map(|i| (x[i * 2] - x[i * 2 - 2]).powi(2))
        .sum();
    let level: f32 = frames.map(|i| x[i * 2] * x[i * 2]).sum();
    edges / level.max(1e-12)
}

#[test]
fn the_trip_hop_document_plays_in_range_and_never_drops_out() {
    let l = load();
    let out = render(&l, l.steps, 4);
    assert!(out.iter().all(|s| s.is_finite()));
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(
        (0.2..=0.951).contains(&peak),
        "peak {peak}: the limiter holds it under its ceiling"
    );
    // Hats on every eighth and a bass line between them: no eighth is silent.
    let eighth = (step_len(&l.steps) * 2.0) as usize;
    for (n, start) in (0..out.len() / 2 - eighth).step_by(eighth).enumerate() {
        let e = energy(&out, start..start + eighth);
        assert!(e > 1e-5, "eighth {n} is silent ({e})");
    }
}

#[test]
fn the_drums_land_on_their_steps() {
    // The kit alone, against the same document with the kit muted: what
    // the kit adds starts on the swung grid, not before it.
    let l = load();
    let mut muted = l.steps;
    muted.kit.hits = [[0; shard_dsp::steps::STEPS]; shard_dsp::kit::VOICES];
    let with = render(&l, l.steps, 2);
    let without = render(&l, muted, 2);
    let added: Vec<f32> = with.iter().zip(&without).map(|(a, b)| a - b).collect();
    let latency = Engine::new(SR, 1).latency();
    let window = (0.015 * SR) as usize;
    let kit = &l.steps.kit;
    let mut checked = 0;
    for k in 0..kit.length as usize {
        // Accents only: a ghost under a kick's tail is felt, not measured.
        let loudest = (0..shard_dsp::kit::VOICES)
            .map(|v| kit.hits[v][k])
            .max()
            .unwrap_or(0);
        if loudest < 80 {
            continue;
        }
        let at = onset(&l.steps, k) + latency;
        // The loop's first step has nothing before it to compare with; the
        // second bar's first step stands in for it.
        if at < window {
            continue;
        }
        // Edges, not level: a hat right after a kick is quieter than the
        // kick's tail, but the tail is smooth and the hat is not.
        let before = edges(&added, at - window..at);
        let after = edges(&added, at..at + window);
        assert!(
            after > before * 2.0 && after > 1e-6,
            "step {k}: {after} after the onset against {before} before it"
        );
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} accents checked");
}

#[test]
fn the_bass_is_acid_its_filter_plucked_on_every_note() {
    // The bass alone. The mod envelope opens the resonant low-pass at each
    // note's start and lets it fall, so a long note is brighter at its start
    // than a step later. The drive brightens a note's loud start a little on
    // its own, so the same render with the link removed is the yardstick.
    let l = load();
    let mut bass = l.steps;
    bass.kit.hits = [[0; shard_dsp::steps::STEPS]; shard_dsp::kit::VOICES];
    let still = load();
    still.rig.session.unlink_param("filter.cutoff").unwrap();
    let plucked = render(&l, bass, 2);
    let unplucked = render(&still, bass, 2);
    let latency = Engine::new(SR, 1).latency();
    let ms = |t: f32| (t * 0.001 * SR) as usize;
    let fall = |out: &[f32], at: usize| {
        brightness(out, at + ms(5.0)..at + ms(45.0))
            / brightness(out, at + ms(200.0)..at + ms(240.0))
    };
    // The two-step notes, on the root and on the fifth.
    for k in [0usize, 10, 16, 26] {
        let at = onset(&bass, k) + latency;
        let (with, without) = (fall(&plucked, at), fall(&unplucked, at));
        assert!(
            with > without * 1.5,
            "step {k}: darkens {with}x plucked against {without}x without; the filter does not move"
        );
    }
}

#[test]
fn render_to_listen() {
    // Only when asked: `just e2e` sets the folder.
    let Ok(dir) = std::env::var("SHARD_RENDERS") else {
        return;
    };
    let l = load();
    let out = render(&l, l.steps, 8);
    let path = std::path::Path::new(&dir).join("trip-hop-acid.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).expect("the renders folder is writable");
    for s in out {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
    println!("wrote {}", path.display());
}

#[test]
fn the_drums_are_heard_over_the_bass() {
    // Georg, 2026-10-04, on the first render: "the wav doesn't have drums,
    // just a squeaky bassline". The kit was in it, 12 dB under a bass that
    // held the limiter down. What the drums add to the limited mix has to be
    // a good part of the bass's own level, or nobody hears them.
    let l = load();
    let mut bass = l.steps;
    bass.kit.hits = [[0; shard_dsp::steps::STEPS]; shard_dsp::kit::VOICES];
    let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
    let mix = render(&l, l.steps, 2);
    let alone = render(&l, bass, 2);
    let added: Vec<f32> = mix.iter().zip(&alone).map(|(x, y)| x - y).collect();
    let ratio = rms(&added) / rms(&alone);
    assert!(
        ratio > 0.7,
        "the drums add {ratio:.2} of the bass's level to the mix"
    );
}

#[test]
fn check_a_shard_file() {
    // `just shard-check <file>`: any document, measured the way the
    // references in `evals/agent/references/` were tuned. Whether it loads
    // clean, how loud it is, and how the drums sit against the track; then
    // eight bars rendered to listen to.
    let Ok(path) = std::env::var("SHARD_FILE") else {
        return;
    };
    let (l, report) = open(&path);
    println!(
        "loads: {} nodes, {} unknown, {} refused, material missing: {}",
        report.applied,
        report.unknown.len(),
        report.refused.len(),
        report.sample_missing
    );
    let steps = l.steps;
    let mut track = steps;
    track.kit.hits = [[0; shard_dsp::steps::STEPS]; shard_dsp::kit::VOICES];
    let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
    let db = |x: f32| 20.0 * x.max(1e-9).log10();
    let mix = render(&l, steps, 4);
    let alone = render(&l, track, 4);
    let added: Vec<f32> = mix.iter().zip(&alone).map(|(x, y)| x - y).collect();
    let peak = mix.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    println!(
        "level: mix {:.1} dBFS RMS, peak {peak:.2}; track alone {:.1} dBFS; drums add {:.2} of the track",
        db(rms(&mix)),
        db(rms(&alone)),
        rms(&added) / rms(&alone).max(1e-9)
    );
    let Ok(dir) = std::env::var("SHARD_RENDERS") else {
        return;
    };
    let stem = std::path::Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("shard");
    let dest = std::path::Path::new(&dir).join(format!("{stem}.wav"));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&dest, spec).expect("the renders folder is writable");
    for s in render(&l, steps, 8) {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
    println!("wrote {}", dest.display());
}
