//! The drums tracks' kits in the engine (`kit.rs`): beside the patches, on
//! the transport's clock. The kit's own sound is tested in `kit.rs`; these hold
//! where it joins the engine, through its public face only.

use shard_dsp::arrangement;
use shard_dsp::kit::{KitParams, KitPattern};
use shard_dsp::{Engine, Mix, ParamBank, StepParams, KITS};

/// A grid of `hits`, (voice, step) pairs, at full velocity.
fn grid(hits: &[(usize, usize)], length: u32) -> KitPattern {
    let mut kit = KitPattern {
        length,
        hits: [[0; shard_dsp::steps::STEPS]; shard_dsp::kit::MAX_LANES],
    };
    for &(v, k) in hits {
        kit.hits[v][k] = shard_dsp::kit::HIT_MAX;
    }
    kit
}

/// Steps running with no melody step set, so the drums sound alone.
fn steps() -> StepParams {
    StepParams {
        on: true,
        pattern: 0,
        ..Default::default()
    }
}

/// A drums track switched on.
const ON: KitParams = KitParams {
    on: true,
    tune: 0.0,
    decay: 1.0,
};

/// An engine on a running transport, kit 0 playing `kit`.
fn drums_engine(kit: KitPattern) -> Engine {
    let mut e = Engine::new(48_000.0, 64);
    e.set_arrangement(&ParamBank::for_table(arrangement::params()), false);
    e.set_steps(steps());
    e.standard_kit_lanes(0);
    e.set_kit(0, kit, ON);
    e
}

fn render_kit(kit: KitPattern, frames: usize) -> Vec<f32> {
    render_with(kit, ON, frames)
}

fn render_with(kit: KitPattern, params: KitParams, frames: usize) -> Vec<f32> {
    let mut e = drums_engine(kit);
    e.set_kit(0, kit, params);
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
    let kick = render_kit(grid(&[(KICK, 0)], 16), frames);
    let hat = render_kit(grid(&[(HAT, 0)], 16), frames);
    let both = render_kit(grid(&[(KICK, 0), (HAT, 0)], 16), frames);
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
    let mut e = drums_engine(grid(&[(SNARE, 4)], 16));
    let latency = e.latency();
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
        let mut e = drums_engine(grid(&[], kit));
        e.set_playing(true);
        let bank = ParamBank::new();
        let mut out = vec![0.0; 256];
        let mut p = steps();
        p.length = track;
        let span = track.min(kit);
        for block in 0..4_000 {
            p.tempo_bpm = 90.0 + ((block / 300) % 4) as f32 * 40.0;
            e.set_steps(p);
            e.process_block(&mut out, &bank);
            let (Some(m), Some(k)) = (e.current_step(), e.kit_step(0)) else {
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
fn the_kit_is_silent_with_the_patch_alone() {
    use shard_dsp::kit::KICK;
    // The patch heard alone: a kit switched on, with a beat in it, must not
    // leak in.
    let mut e = drums_engine(grid(&[(KICK, 0), (KICK, 4), (KICK, 8)], 16));
    e.set_playing(true);
    e.set_arrangement(&ParamBank::for_table(arrangement::params()), true);
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
    // A drums track's switch starts off, as the Drums card's did.
    let p = grid(&[(KICK, 0), (KICK, 4)], 16);
    assert!(render_with(p, KitParams::default(), 48_000)
        .iter()
        .all(|s| *s == 0.0));
}

/// Kits `kits` on a running transport, each with its own grid, `events`
/// (block, kit, mix) handed over before their block. The left channel.
fn render_kits(kits: &[(usize, KitPattern)], events: &[(usize, usize, Mix)]) -> Vec<f32> {
    let mut e = Engine::new(48_000.0, 64);
    let arr = ParamBank::for_table(arrangement::params());
    arr.set_by_id("arrangement.amp.limit", 0.0);
    e.set_arrangement(&arr, false);
    e.set_steps(steps());
    e.set_kit_active(0, false);
    for &(k, p) in kits {
        e.standard_kit_lanes(k);
        e.set_kit(k, p, ON);
        e.set_kit_active(k, true);
    }
    e.set_playing(true);
    let bank = ParamBank::new();
    let mut out = vec![0.0; 512];
    let mut all = Vec::new();
    for block in 0..400 {
        for (_, k, m) in events.iter().filter(|(b, _, _)| *b == block) {
            e.set_kit_mix(*k, *m);
        }
        e.process_block(&mut out, &bank);
        all.extend(out.iter().step_by(2));
    }
    all
}

#[test]
fn two_drums_tracks_both_sound_and_differ() {
    use shard_dsp::kit::{HAT, KICK, SNARE};
    // Story d: each drums track plays its own kit. A kick on one and a snare
    // on another both come through, and the same grid on two kits is not
    // the same sound, since each kit's noise is salted by its place.
    let kick = grid(&[(KICK, 0)], 16);
    let snare = grid(&[(SNARE, 4)], 16);
    let both = render_kits(&[(0, kick), (2, snare)], &[]);
    let at = |frame: usize| {
        both[frame..frame + 4_800]
            .iter()
            .map(|s| s * s)
            .sum::<f32>()
    };
    assert!(at(0) > 1.0, "the kick on kit 0 is silent");
    assert!(at(24_000) > 1.0, "the snare on kit 2 is silent");
    let hats = grid(&[(HAT, 0)], 16);
    let one = render_kits(&[(1, hats)], &[]);
    let other = render_kits(&[(3, hats)], &[]);
    assert!(one.iter().any(|s| *s != 0.0));
    assert_ne!(one, other, "two kits on one grid are identical");
    // Every kit at once stays in range.
    let all = render_kits(&(0..KITS).map(|k| (k, kick)).collect::<Vec<_>>(), &[]);
    assert!(all.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
}

#[test]
fn a_drums_tracks_mute_lands_on_the_bar() {
    use shard_dsp::kit::HAT;
    // Hats on every sixteenth; a mute pressed early in the first bar. Up to
    // the gate before the bar line the kit plays as unmuted, and from the
    // line on it is silent.
    let hats = grid(&(0..16).map(|k| (HAT, k)).collect::<Vec<_>>(), 16);
    let mute = Mix {
        mute: true,
        ..Mix::default()
    };
    let open = render_kits(&[(1, hats)], &[]);
    let muted = render_kits(&[(1, hats)], &[(10, 1, mute)]);
    let latency = Engine::new(48_000.0, 1).latency();
    // A bar at 120 bpm is two seconds.
    let bar = 96_000 + latency;
    let fade = shard_dsp::mix::gate_frames(48_000.0);
    assert_eq!(open[..bar - fade], muted[..bar - fade]);
    assert!(muted[bar..].iter().all(|s| *s == 0.0));
    assert!(open[bar..].iter().any(|s| *s != 0.0));
}

#[test]
fn the_default_beat_is_loud_enough_to_stand_beside_a_patch() {
    // Georg, 2026-10-04: "the drums are just too quiet, even with 200% gain
    // on drums and master". The voices peaked near full scale but fell away
    // so fast their level was -20 dBFS, and the limiter held the peaks, so
    // no fader could help. A drum loop sits nearer -12.
    let out = render_kit(KitPattern::default(), 96_000);
    let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
    let db = 20.0 * rms.log10();
    assert!(db > -15.0, "the default beat is {db:.1} dBFS");
    assert!(out.iter().all(|s| s.abs() <= 1.0));
}
