//! Noise's Run (epic 48, story c): Notes is the generator as it always was,
//! Free sounds for as long as the transport plays, past the amplitude
//! envelope and Hold's fall to zero.

use shard_dsp::{Engine, ParamBank, StepParams};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;
/// A frame is two samples; blocks are `BLOCK` frames.
const PER_BLOCK: usize = BLOCK * 2;

/// A held note once every bar (a step in sixteen at 120 bpm is 125 ms, so the
/// bar is two seconds), 20 ms long with the default release: the other
/// 1.9 s are silence for anything that follows the notes.
fn render(
    blocks: usize,
    playing_until: usize,
    mut each: impl FnMut(usize, &ParamBank),
) -> Vec<f32> {
    let mut e = Engine::new(SR, 64);
    e.set_source(vec![0.0; 48_000]);
    e.set_steps(StepParams {
        on: true,
        pattern: 1,
        ..Default::default()
    });
    e.set_playing(true);
    let bank = ParamBank::new();
    bank.set_by_id("material.on", 0.0);
    bank.set_by_id("noise.on", 1.0);
    bank.set_by_id("patch.length", 2.0);
    bank.set_by_id("patch.hold", 20.0);
    let mut out = vec![0.0; PER_BLOCK];
    let mut all = Vec::new();
    for b in 0..blocks {
        if b == playing_until {
            e.set_playing(false);
        }
        each(b, &bank);
        e.process_block(&mut out, &bank);
        all.extend_from_slice(&out);
    }
    all
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

/// Blocks 100 to 250 are the middle of the silence after the first note.
const GAP: core::ops::Range<usize> = (100 * PER_BLOCK)..(250 * PER_BLOCK);

#[test]
fn notes_is_the_default_and_goes_quiet_between_steps() {
    let plain = render(300, usize::MAX, |_, _| {});
    let notes = render(300, usize::MAX, |_, bank| {
        bank.set_by_id("noise.run", 0.0);
    });
    assert!(plain == notes, "Notes should be the row's default");
    assert!(
        peak(&plain[..40 * PER_BLOCK]) > 0.05,
        "the note should sound"
    );
    assert_eq!(peak(&plain[GAP]), 0.0, "Notes is gated by the notes");
}

#[test]
fn free_sounds_between_steps_and_through_the_envelopes_zero() {
    let out = render(300, usize::MAX, |_, bank| {
        bank.set_by_id("noise.run", 1.0);
    });
    let gap = &out[GAP];
    assert!(rms(gap) > 0.05, "Free should sound between the notes");
    // Steady: the envelope's fall does not dip it.
    let (a, b) = gap.split_at(gap.len() / 2);
    assert!((rms(a) / rms(b) - 1.0).abs() < 0.3);
    assert!(out.iter().all(|s| s.is_finite()));
}

#[test]
fn free_is_silent_when_the_transport_is_stopped() {
    let out = render(300, 150, |_, bank| {
        bank.set_by_id("noise.run", 1.0);
    });
    assert!(rms(&out[100 * PER_BLOCK..140 * PER_BLOCK]) > 0.05);
    assert_eq!(peak(&out[160 * PER_BLOCK..]), 0.0, "stopped is silent");
}

#[test]
fn switching_between_notes_and_free_does_not_click() {
    // Brown noise is smooth, so a jump stands out from its own steepest step.
    let run = |free_from: usize, free_to: usize| {
        render(300, usize::MAX, move |b, bank| {
            bank.set_by_id("noise.color", -1.0);
            bank.set_by_id("noise.run", (free_from..free_to).contains(&b) as u8 as f32);
        })
    };
    let steepest = |x: &[f32]| {
        x.chunks(2)
            .zip(x.chunks(2).skip(1))
            .fold(0.0f32, |m, (a, b)| m.max((b[0] - a[0]).abs()))
    };
    let steady = steepest(&run(0, 300)[GAP]);
    assert!(steady > 0.0);
    // Notes to Free in the silence, and Free back to Notes in it.
    let both = run(120, 200);
    let into = steepest(&both[110 * PER_BLOCK..140 * PER_BLOCK]);
    let out_of = steepest(&both[190 * PER_BLOCK..230 * PER_BLOCK]);
    assert!(into < steady * 1.5, "into Free clicked: {into} vs {steady}");
    assert!(
        out_of < steady * 1.5,
        "out of Free clicked: {out_of} vs {steady}"
    );
    // And once Notes is back and its fade has landed, it is silent again.
    assert_eq!(peak(&both[230 * PER_BLOCK..250 * PER_BLOCK]), 0.0);
}

#[test]
fn free_is_shaped_by_the_filter_but_not_the_envelope() {
    // The master filter still acts on it: closed, there is little left.
    let open = render(300, usize::MAX, |_, bank| {
        bank.set_by_id("noise.run", 1.0);
    });
    let closed = render(300, usize::MAX, |_, bank| {
        bank.set_by_id("noise.run", 1.0);
        bank.set_by_id("filter.on", 1.0);
        bank.set_by_id("filter.mix", 1.0);
        bank.set_by_id("filter.cutoff", 40.0);
    });
    assert!(rms(&closed[GAP]) < rms(&open[GAP]) * 0.2);
}
