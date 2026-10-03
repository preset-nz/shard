//! The drum kit in the engine (`kit.rs`): beside the patch, on the
//! tracker's clock. The kit's own sound is tested in `kit.rs`; these hold
//! where it joins the engine, through its public face only.

use shard_dsp::arrangement;
use shard_dsp::{Engine, ParamBank, StepParams};

/// Steps running with no melody step set, so the drums sound alone, as
/// sound scaping hears the patch alone. `hits` are (voice, step) pairs.
fn drums_alone(hits: &[(usize, usize)], length: u32) -> StepParams {
    let mut kit = shard_dsp::kit::KitPattern {
        length,
        hits: [[0; shard_dsp::steps::STEPS]; shard_dsp::kit::VOICES],
    };
    for &(v, k) in hits {
        kit.hits[v][k] = shard_dsp::kit::HIT_MAX;
    }
    StepParams {
        on: true,
        pattern: 0,
        kit,
        ..Default::default()
    }
}

/// The arrangement with the Drums card switched on.
fn kit_on() -> ParamBank {
    let arr = ParamBank::for_table(arrangement::params());
    arr.set_by_id("arrangement.kit.on", 1.0);
    arr
}

/// An engine in tracker mode with the drums switched on.
fn drums_engine() -> Engine {
    let mut e = Engine::new(48_000.0, 64);
    e.set_arrangement(&kit_on(), false);
    e
}

fn render_steps(p: StepParams, frames: usize) -> Vec<f32> {
    render_with(p, frames, &kit_on())
}

fn render_with(p: StepParams, frames: usize, arr: &ParamBank) -> Vec<f32> {
    let mut e = Engine::new(48_000.0, 64);
    e.set_arrangement(arr, false);
    e.set_steps(p);
    e.set_playing(true);
    let bank = ParamBank::new();
    let mut out = vec![0.0; 512];
    let mut all = Vec::new();
    while all.len() < frames * 2 {
        e.process_block(&mut out, &bank);
        all.extend_from_slice(&out);
    }
    all
}

#[test]
fn a_kick_and_a_hat_on_one_step_both_sound() {
    use shard_dsp::kit::{HAT, KICK};
    // The whole point: the melody track is monophonic, the kit is not.
    let frames = 6_000;
    let kick = render_steps(drums_alone(&[(KICK, 0)], 16), frames);
    let hat = render_steps(drums_alone(&[(HAT, 0)], 16), frames);
    let both = render_steps(drums_alone(&[(KICK, 0), (HAT, 0)], 16), frames);
    let energy = |x: &[f32]| x.iter().map(|s| s * s).sum::<f32>();
    assert!(energy(&kick) > 1.0 && energy(&hat) > 0.1);
    // Neither cuts the other. The kit's glue saturates the sum, so together
    // is not exactly each added; but the hat's fizz (how much the signal
    // jumps sample to sample) and the kick's low end (a 2 ms moving average)
    // both come through.
    let fizz = |x: &[f32]| x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
    let low = |x: &[f32]| {
        let n = 96;
        energy(
            &x.windows(n)
                .map(|w| w.iter().sum::<f32>() / n as f32)
                .collect::<Vec<_>>(),
        )
    };
    assert!(fizz(&both) > fizz(&hat) * 0.6, "the kick cut the hat");
    assert!(low(&both) > low(&kick) * 0.6, "the hat cut the kick");
}

#[test]
fn a_kit_hit_lands_on_its_step() {
    use shard_dsp::kit::SNARE;
    // Step five at 120 bpm is four sixteenths in: 24,000 frames at 48 k,
    // then the limiter's look-ahead.
    let mut e = drums_engine();
    let latency = e.latency();
    e.set_steps(drums_alone(&[(SNARE, 4)], 16));
    e.set_playing(true);
    let bank = ParamBank::new();
    let mut out = vec![0.0; 512];
    let mut all = Vec::new();
    for _ in 0..120 {
        e.process_block(&mut out, &bank);
        all.extend_from_slice(&out);
    }
    let first = all.iter().position(|s| *s != 0.0).unwrap() / 2;
    assert_eq!(first, 24_000 + latency);
}

#[test]
fn the_kit_keeps_the_melodys_grid_through_a_tempo_change() {
    // Two lengths either way round, and the tempo moving under them.
    // Whatever each loop's length, both are on the same sixteenth of the
    // shorter one, every block.
    for (track, kit) in [(64u32, 16u32), (16, 64), (8, 32)] {
        let mut e = drums_engine();
        e.set_playing(true);
        let bank = ParamBank::new();
        let mut out = vec![0.0; 256];
        let mut p = drums_alone(&[], kit);
        p.length = track;
        let span = track.min(kit);
        for block in 0..4_000 {
            p.tempo_bpm = 90.0 + ((block / 300) % 4) as f32 * 40.0;
            e.set_steps(p);
            e.process_block(&mut out, &bank);
            let (Some(m), Some(k)) = (e.current_step(), e.current_kit_step()) else {
                panic!("block {block}: a clock is not running");
            };
            assert_eq!(
                k % span,
                m % span,
                "track {track}, kit {kit}, block {block}"
            );
        }
    }
}

#[test]
fn the_kit_is_silent_in_sound_scaping() {
    use shard_dsp::kit::KICK;
    // Sound scaping is the steps off and the patch alone. A kit switched
    // on, with a beat in it, must not leak in.
    let mut p = drums_alone(&[(KICK, 0), (KICK, 4), (KICK, 8)], 16);
    p.on = false;
    let mut e = Engine::new(48_000.0, 64);
    e.set_playing(true);
    e.set_steps(p);
    e.set_arrangement(&kit_on(), true);
    let bank = ParamBank::new();
    bank.set_by_id("material.on", 0.0);
    let mut out = vec![0.0; 512];
    for block in 0..200 {
        e.process_block(&mut out, &bank);
        assert!(out.iter().all(|s| *s == 0.0), "block {block}");
    }
}

#[test]
fn a_kit_switched_off_plays_nothing() {
    use shard_dsp::kit::KICK;
    // The arrangement at its defaults: the Drums card starts off.
    let p = drums_alone(&[(KICK, 0), (KICK, 4)], 16);
    let off = ParamBank::for_table(arrangement::params());
    assert!(render_with(p, 48_000, &off).iter().all(|s| *s == 0.0));
}

#[test]
fn the_default_beat_is_loud_enough_to_stand_beside_a_patch() {
    // Georg, 2026-10-04: "the drums are just too quiet, even with 200% gain
    // on drums and master". The voices peaked near full scale but fell away
    // so fast their level was -20 dBFS, and the limiter held the peaks, so
    // no fader could help. A drum loop sits nearer -12.
    let mut p = drums_alone(&[], 16);
    p.kit = shard_dsp::kit::KitPattern::default();
    let out = render_steps(p, 96_000);
    let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
    let db = 20.0 * rms.log10();
    assert!(db > -15.0, "the default beat is {db:.1} dBFS");
    assert!(out.iter().all(|s| s.abs() <= 1.0));
}
