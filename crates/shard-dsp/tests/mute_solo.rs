//! Mute and solo (epic 28, story c): a change lands on the next bar line of
//! the shared transport, through a short gate, so it never clicks.
//! Silencing fades out over the gate's length before the line and is silent
//! from it, so the downbeat never leaks; bringing back fades in from it.
//! While stopped a change lands at once. Mute wins over solo.

use shard_dsp::arrangement;
use shard_dsp::mix::gate_frames;
use shard_dsp::steps::{StepParams, Transport};
use shard_dsp::{Engine, Mix, ParamBank, VOICES};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;
/// Two and a half bars at 120 bpm.
const BLOCKS: usize = 950;
/// The gate's length, in frames: 240 at 48 kHz.
fn fade() -> usize {
    gate_frames(SR)
}

const MUTE: Mix = Mix {
    mute: true,
    solo: false,
    solo_safe: false,
};
const SOLO: Mix = Mix {
    mute: false,
    solo: true,
    solo_safe: false,
};
const OPEN: Mix = Mix {
    mute: false,
    solo: false,
    solo_safe: false,
};

/// A whole number of cycles in a second, so the loop is seamless.
fn tone(hz: f32) -> Vec<f32> {
    (0..SR as usize)
        .map(|i| (core::f32::consts::TAU * hz * i as f32 / SR).sin())
        .collect()
}

fn transport() -> StepParams {
    StepParams {
        on: true,
        tempo_bpm: 120.0,
        ..Default::default()
    }
}

/// The engine with voices `active` looping free (their own steps off) on a
/// running transport, each its own tone, the kit as `kit`.
fn engine(active: &[usize], kit: bool) -> (Engine, ParamBank) {
    engine_with(active, kit, None)
}

/// The same, with every voice's steps as `steps` when given.
fn engine_with(active: &[usize], kit: bool, steps: Option<StepParams>) -> (Engine, ParamBank) {
    let mut e = Engine::new(SR, 64);
    for v in 0..VOICES {
        e.set_voice_source(v, tone(220.0 + 110.0 * v as f32));
        e.set_voice_steps(
            v,
            steps.unwrap_or(StepParams {
                on: false,
                ..transport()
            }),
        );
        e.set_voice_active(v, active.contains(&v));
    }
    e.set_transport(&transport());
    let arr = ParamBank::for_table(arrangement::params());
    arr.set_by_id("arrangement.kit.on", if kit { 1.0 } else { 0.0 });
    // Off, so renders with different voices in them compare to the bit:
    // near its ceiling the limiter's gain depends on the whole mix.
    arr.set_by_id("arrangement.amp.limit", 0.0);
    e.set_arrangement(&arr, false);
    let bank = ParamBank::new();
    bank.set_by_id("material.gain", 0.2);
    (e, bank)
}

/// The left channel of `BLOCKS` blocks, with `events` (block, voice, mix)
/// handed over before their block, as the host does.
fn render(active: &[usize], events: &[(usize, usize, Mix)]) -> Vec<f32> {
    render_with(active, events, None)
}

fn render_with(
    active: &[usize],
    events: &[(usize, usize, Mix)],
    steps: Option<StepParams>,
) -> Vec<f32> {
    let (mut e, bank) = engine_with(active, false, steps);
    e.set_playing(true);
    let mut out = vec![0.0; BLOCK * 2];
    let mut all = Vec::with_capacity(BLOCKS * BLOCK);
    for block in 0..BLOCKS {
        for (_, v, m) in events.iter().filter(|(b, _, _)| *b == block) {
            e.set_voice_mix(*v, *m);
        }
        e.process_block(&mut out, &bank);
        all.extend(out.iter().step_by(2));
    }
    all
}

/// Where bar `n` begins, in frames, counted the way the engine counts.
fn bar_start(n: usize) -> usize {
    let mut t = Transport::new(SR);
    t.set(&transport());
    t.begin_block();
    let (mut frame, mut bars) = (0, 0);
    while bars < n {
        if t.advance(1.0) {
            bars += 1;
        }
        frame += 1;
    }
    frame
}

/// The output is a look-ahead late.
fn latency() -> usize {
    Engine::new(SR, 64).latency()
}

fn worst_jump(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

fn first_difference(a: &[f32], b: &[f32]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

#[test]
fn a_bar_is_where_the_test_thinks_it_is() {
    // 120 bpm at 48 kHz: a sixteenth is 6000 samples, a bar 96 000.
    assert_eq!(bar_start(1), 96_000);
    assert_eq!(bar_start(2), 192_000);
}

#[test]
fn a_mute_pressed_mid_bar_lands_on_the_next_bar_line() {
    let reference = render(&[0], &[]);
    // Pressed a quarter of the way into the first bar.
    let muted = render(&[0], &[(100, 0, MUTE)]);
    let at = bar_start(1) + latency();
    assert_eq!(
        first_difference(&reference, &muted),
        Some(at - fade()),
        "the mute should start fading a gate's length before the bar line"
    );
    // Silent from the bar line on.
    assert!(
        muted[at..].iter().all(|s| *s == 0.0),
        "the muted voice still sounds on the bar"
    );
    // And it fades rather than cuts: no jump at the bar beyond what the tone
    // itself makes.
    let steady = worst_jump(&reference[BLOCK * 20..]);
    let landing = worst_jump(&muted[at - fade() - 100..at + 100]);
    assert!(
        landing <= steady + 0.2 / fade() as f32 + 1e-6,
        "the mute jumped {landing} against a steady {steady}"
    );
}

#[test]
fn the_applied_mix_stays_until_the_bar() {
    let (mut e, bank) = engine(&[0], false);
    e.set_playing(true);
    let mut out = vec![0.0; BLOCK * 2];
    let bar = bar_start(1);
    for block in 0..BLOCKS {
        if block == 100 {
            e.set_voice_mix(0, MUTE);
        }
        e.process_block(&mut out, &bank);
        // The bar line is announced on the last frame before it, so a bar
        // starting right after this block has landed already.
        let end = (block + 1) * BLOCK;
        let landed = block >= 100 && end >= bar;
        assert_eq!(e.voice_mix(0).mute, landed, "block {block}");
        assert_eq!(e.voice_audible(0), !landed, "block {block}");
    }
}

#[test]
fn pressing_again_before_the_bar_cancels_the_press() {
    let reference = render(&[0], &[]);
    let twice = render(&[0], &[(100, 0, MUTE), (200, 0, OPEN)]);
    assert_eq!(first_difference(&reference, &twice), None);
}

#[test]
fn an_unmute_lands_on_the_bar_too() {
    let reference = render(&[0], &[]);
    // Muted while stopped (at once), unmuted mid bar one.
    let (mut e, bank) = engine(&[0], false);
    e.set_voice_mix(0, MUTE);
    e.set_playing(true);
    let mut out = vec![0.0; BLOCK * 2];
    let mut all = Vec::new();
    for block in 0..BLOCKS {
        if block == 450 {
            e.set_voice_mix(0, OPEN);
        }
        e.process_block(&mut out, &bank);
        all.extend(out.iter().step_by(2));
    }
    let at = bar_start(2) + latency();
    assert!(all[..at].iter().all(|s| *s == 0.0), "heard before the bar");
    assert!(all[at] != 0.0 || all[at + 1] != 0.0);
    // The voice kept its place while muted: past the fade it is the
    // reference, sample for sample.
    assert_eq!(
        first_difference(&reference[at + fade()..], &all[at + fade()..]),
        None
    );
    assert!(worst_jump(&all[at - 100..at + fade() + 100]) <= worst_jump(&reference) + 1e-3);
}

#[test]
fn while_stopped_a_mute_applies_at_once() {
    let (mut e, bank) = engine(&[0], false);
    e.set_voice_mix(0, MUTE);
    assert!(e.voice_mix(0).mute && !e.voice_audible(0));
    e.set_playing(true);
    let mut out = vec![0.0; BLOCK * 2];
    for _ in 0..50 {
        e.process_block(&mut out, &bank);
        assert!(
            out.iter().all(|s| *s == 0.0),
            "a mute set while stopped leaked"
        );
    }
    // And a mute still waiting when the transport stops lands with the stop.
    e.set_voice_mix(0, OPEN);
    assert!(e.voice_mix(0).mute, "queued while playing");
    e.set_playing(false);
    assert!(!e.voice_mix(0).mute);
}

#[test]
fn with_the_steps_off_there_is_no_bar_to_wait_for() {
    let (mut e, bank) = engine(&[0], false);
    e.set_transport(&StepParams {
        on: false,
        ..transport()
    });
    e.set_playing(true);
    let mut out = vec![0.0; BLOCK * 2];
    e.process_block(&mut out, &bank);
    e.set_voice_mix(0, MUTE);
    assert!(e.voice_mix(0).mute);
}

#[test]
fn placing_a_mix_applies_it_without_waiting() {
    // A track arriving on a voice brings its switches with it.
    let reference = render(&[0], &[]);
    let (mut e, bank) = engine(&[0], false);
    e.set_playing(true);
    let mut out = vec![0.0; BLOCK * 2];
    let mut all = Vec::new();
    for block in 0..200 {
        if block == 100 {
            e.place_voice_mix(0, MUTE);
        }
        e.process_block(&mut out, &bank);
        all.extend(out.iter().step_by(2));
    }
    let at = 100 * BLOCK + latency();
    assert_eq!(first_difference(&reference, &all), Some(at));
    assert!(all[at + fade()..].iter().all(|s| *s == 0.0));
}

#[test]
fn one_solo_silences_the_others_from_the_next_bar() {
    let all = render(&[0, 1, 2], &[]);
    let alone = render(&[1], &[]);
    let soloed = render(&[0, 1, 2], &[(100, 1, SOLO)]);
    let at = bar_start(1) + latency();
    assert_eq!(first_difference(&all, &soloed), Some(at - fade()));
    assert_eq!(
        first_difference(&soloed[at..], &alone[at..]),
        None,
        "the soloed track should be heard alone"
    );
}

#[test]
fn two_solos_are_both_heard() {
    let pair = render(&[1, 2], &[]);
    let soloed = render(&[0, 1, 2], &[(100, 1, SOLO), (120, 2, SOLO)]);
    let at = bar_start(1) + latency();
    assert_eq!(
        first_difference(&soloed[at..], &pair[at..]),
        None,
        "both soloed tracks should be heard"
    );
}

#[test]
fn unsoloing_brings_every_track_back_but_a_muted_one() {
    let all = render(&[0, 1, 2], &[]);
    let back = render(&[0, 1, 2], &[(100, 1, SOLO), (450, 1, OPEN)]);
    let second = bar_start(2) + latency();
    assert_eq!(
        first_difference(&all[second + fade()..], &back[second + fade()..]),
        None
    );

    // Track 1 muted throughout; a solo on track 2 comes and goes. Once the
    // solo is gone, tracks 2 and 3 play and track 1 stays muted.
    let rest = render(&[1, 2], &[]);
    let muted = render(&[0, 1, 2], &[(0, 0, MUTE), (100, 1, SOLO), (450, 1, OPEN)]);
    assert_eq!(
        first_difference(&muted[second + fade()..], &rest[second + fade()..]),
        None,
        "after the solo, tracks 2 and 3 should play and track 1 stay muted"
    );
}

#[test]
fn mute_wins_over_solo() {
    // The only soloed track is muted too: it is silent, and so is everyone
    // its solo silenced.
    let both = Mix {
        mute: true,
        solo: true,
        solo_safe: false,
    };
    let soloed = render(&[0, 1], &[(100, 1, both)]);
    let at = bar_start(1) + latency();
    assert!(soloed[at..].iter().all(|s| *s == 0.0));
}

#[test]
fn a_solo_safe_voice_plays_through_a_solo() {
    let safe = Mix {
        solo_safe: true,
        ..OPEN
    };
    let soloed = render(&[0, 1, 2], &[(0, 2, safe), (0, 1, MUTE), (100, 0, SOLO)]);
    // Voice 2 is safe, so after the bar it is voice 0 and voice 2.
    let pair = render(&[0, 2], &[]);
    let at = bar_start(1) + latency();
    assert_eq!(
        first_difference(&soloed[at..], &pair[at..]),
        None,
        "the safe voice was silenced"
    );
}

#[test]
fn any_solo_silences_the_kit() {
    let kit = |solo: bool| {
        // No voice sounding, only a solo on voice 0.
        let (mut e, bank) = engine(&[], true);
        if solo {
            e.set_voice_mix(0, SOLO);
        }
        e.set_playing(true);
        let mut out = vec![0.0; BLOCK * 2];
        let mut peak = 0.0f32;
        for block in 0..400 {
            e.process_block(&mut out, &bank);
            // Past voice 0 fading out: it starts switched in.
            if block >= 20 {
                peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
            }
        }
        (peak, e.kit_audible())
    };
    let (heard, audible) = kit(false);
    assert!(heard > 0.01 && audible, "the kit should play: {heard}");
    let (soloed, audible) = kit(true);
    assert!(
        soloed == 0.0 && !audible,
        "the kit played through a solo: {soloed}"
    );
}

#[test]
fn the_downbeat_never_sounds_through_a_mute_or_a_solo() {
    // Each voice plays one note a bar, on step one: the note a mute or a
    // solo landing on the bar meets head on. None of its attack may get
    // through the fade.
    let downbeat = StepParams {
        pattern: 1,
        ..transport()
    };
    let at = bar_start(1) + latency();
    let muted = render_with(&[0], &[(100, 0, MUTE)], Some(downbeat));
    let open = render_with(&[0], &[], Some(downbeat));
    assert!(
        open[at..at + fade()].iter().any(|s| s.abs() > 0.05),
        "the downbeat should sound unmuted"
    );
    assert!(
        muted[at..].iter().all(|s| *s == 0.0),
        "the downbeat leaked through the mute"
    );
    // A solo landing silences the other track's downbeat the same way.
    let alone = render_with(&[1], &[], Some(downbeat));
    let soloed = render_with(&[0, 1], &[(100, 1, SOLO)], Some(downbeat));
    assert_eq!(first_difference(&soloed[at..], &alone[at..]), None);
}
