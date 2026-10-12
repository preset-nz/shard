//! The first architecture rule, held as a test: nothing the audio thread calls
//! touches the allocator.
//!
//! This file is its own test binary, which is what lets it install the guard
//! as the global allocator without changing any other test. Every engine call
//! the audio callback makes runs inside `no_alloc` here, against a parameter
//! sweep that drives every row of the table across its whole range, so a
//! branch that allocates only at an extreme setting still gets caught.

use std::hint::black_box;
use std::sync::Mutex;

use preset_math::Basis;
use shard_dsp::arrangement;
use shard_dsp::rt::{no_alloc, GuardedAlloc};
use shard_dsp::{
    adsr_curve, Engine, EnvSpec, Generator, GrainSpawn, LfoSpec, Mix, ModSet, ParamBank, Reading,
    Shape, StepParams, Taper, PARAMS, VOICES,
};

#[global_allocator]
static GUARD: GuardedAlloc = GuardedAlloc;

const SR: f32 = 48_000.0;

fn tone(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (core::f32::consts::TAU * 220.0 * i as f32 / SR).sin())
        .collect()
}

/// Every parameter at a different slow phase, reaching both ends of its range.
/// The arrangement's tape gates open and close along the way when it is
/// swept too.
fn sweep(bank: &ParamBank, block: usize) {
    for (i, p) in bank.defs().iter().enumerate() {
        let t = ((block * (i + 3)) % 97) as f32 / 96.0;
        bank.set_normalised(p.id, t);
    }
}

#[test]
fn the_guard_catches_an_allocation_and_its_free() {
    // Without this, every test below could pass because the guard is not
    // counting anything at all.
    let ((), caught) = no_alloc(|| {
        let v: Vec<u64> = black_box(Vec::with_capacity(64));
        drop(black_box(v));
    });
    assert!(
        caught >= 2,
        "expected an allocation and a free, caught {caught}"
    );
}

#[test]
fn a_block_never_touches_the_allocator() {
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(96_000));
    // The cloud reads a different material from the player.
    drop(e.swap_source(Generator::Grain, tone(30_000)));
    e.set_playing(true);
    let bank = ParamBank::new();
    // The arrangement's rows swept as well, and the patch heard alone and
    // arranged by turns, as the modes switch.
    let arr = ParamBank::for_table(arrangement::params());
    let mut out = vec![0.0f32; 2048];

    for block in 0..2_000 {
        sweep(&bank, block);
        sweep(&arr, block + 7);
        // Each generator's reading across its range too, ends crossed and all.
        let t = |k: usize| ((block * k) % 97) as f32 / 96.0;
        e.set_readings(
            Reading {
                octave: t(5) * 4.0 - 2.0,
                trim_start: t(7),
                trim_end: t(11),
            },
            Reading {
                octave: t(13) * 4.0 - 2.0,
                trim_start: t(17),
                trim_end: t(19),
            },
        );
        // Devices do not promise a fixed block size, so neither does this.
        let n = [64, 256, 512, 2048][block % 4];
        let ((), caught) = no_alloc(|| {
            e.set_arrangement(&arr, (block / 50) % 2 == 0);
            e.process_block(&mut out[..n], &bank)
        });
        assert_eq!(
            caught, 0,
            "block {block} touched the allocator {caught} time(s)"
        );
    }
}

#[test]
fn transport_and_audition_never_touch_the_allocator() {
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(48_000));
    let bank = ParamBank::new();
    // Effects start off; run a real cloud so stopping has grains to clear.
    bank.set_by_id("grain.on", 1.0);
    bank.set_by_id("grain.gain", 1.0);
    let mut out = vec![0.0f32; 512];
    let spawn = GrainSpawn {
        position: 0.3,
        rate: 1.0,
        len: 4_800.0,
        pan: 0.5,
        window: 0,
        seq: 1,
    };

    let ((), caught) = no_alloc(|| {
        e.set_playing(true);
        for _ in 0..50 {
            e.process_block(&mut out, &bank);
        }
        e.set_playing(false);
        e.audition(&spawn);
        for _ in 0..50 {
            e.process_block(&mut out, &bank);
        }
    });
    assert_eq!(
        caught, 0,
        "play, stop and audition touched the allocator {caught} time(s)"
    );
}

#[test]
fn modulation_and_swapping_its_set_never_touch_the_allocator() {
    // Every continuous parameter follows an LFO, and the set is swapped for a
    // smaller one and back mid-stream, as adding and removing LFOs will. The
    // sets are built out here, off the audio thread, which is the point.
    let specs: Vec<LfoSpec> = (0..8u64)
        .map(|i| LfoSpec {
            id: i + 1,
            rate_hz: 0.5 + i as f32 * 2.3,
            shape: Shape::ALL[i as usize % Shape::ALL.len()],
            phase: i as f32 * 0.13,
        })
        .collect();
    // Modulation envelopes too, retriggered by the steps below, following
    // every third row. Ids clear of the LFOs', as the document keeps them.
    let envs: Vec<EnvSpec> = (0..4u64)
        .map(|i| {
            let mut curve = adsr_curve(
                i as f32 * 0.007,
                0.02 + i as f32 * 0.03,
                i as f32 * 0.3,
                i as f32 * 0.011,
            );
            // A curve with a smooth basis, and one with no sustain mark.
            if i == 2 {
                curve
                    .points
                    .iter_mut()
                    .for_each(|p| p.basis = Basis::CatmullRom);
            }
            if i == 3 {
                curve.sustain = None;
            }
            EnvSpec { id: 100 + i, curve }
        })
        .collect();
    let build = |lfos: &[LfoSpec], envs: &[EnvSpec]| {
        let mut set = ModSet::with_envelopes(lfos, envs);
        for (i, p) in PARAMS
            .iter()
            .enumerate()
            .filter(|(_, p)| !matches!(p.taper, Taper::Stepped(_)))
        {
            let (lo, hi) = if i % 2 == 0 { (0.2, 0.9) } else { (0.9, 0.2) };
            let source = if i % 3 == 0 {
                envs[i % envs.len()].id
            } else {
                lfos[i % lfos.len()].id
            };
            set.link(p.id, source, lo, hi).unwrap();
        }
        set
    };
    let (first, smaller, bigger) = (
        build(&specs, &envs),
        build(&specs[..3], &envs[..1]),
        build(&specs, &envs),
    );

    let mut e = Engine::new(SR, 256);
    e.set_source(tone(96_000));
    let bank = ParamBank::new();
    for id in [
        "grain.on",
        "fx.2.ring.on",
        "fx.1.crush.on",
        "filter.on",
        "fx.0.drive.on",
        "fx.3.chorus.on",
        "fx.7.delay.on",
        "fx.10.reverb.on",
        "fx.4.overtone.on",
        "fx.9.rise.on",
        "fx.8.echo.on",
        "fx.6.wear.on",
        "fx.5.flanger.on",
    ] {
        bank.set_by_id(id, 1.0);
    }
    bank.set_by_id("grain.gain", 1.0);
    // The first copy of every effect in the chain, then the second copies
    // until it is full.
    let mut chain = shard_dsp::fx::Order::EMPTY;
    for n in 0..shard_dsp::fx::INSTANCES {
        chain.push(n);
    }
    shard_dsp::fx::set_order(&bank, &chain);
    // Every step at the top tempo, so retriggers and their tails run in
    // nearly every block. Handed in per block, as the callback does.
    let steps = StepParams {
        on: true,
        tempo_bpm: 240.0,
        pattern: 0xFFFF,
        ..Default::default()
    };
    e.set_playing(true);
    drop(e.set_modulation(first));

    let mut out = vec![0.0f32; 512];
    let heard = ParamBank::new();
    let mut run = |e: &mut Engine, from: usize| {
        let ((), caught) = no_alloc(|| {
            for block in from..from + 300 {
                sweep(&bank, block);
                e.set_steps(steps);
                e.process_block(&mut out, &bank);
                // The app publishes what the engine heard after every block.
                e.publish_heard(&bank, &heard);
            }
        });
        assert_eq!(
            caught, 0,
            "modulated blocks touched the allocator {caught} time(s)"
        );
    };

    run(&mut e, 0);
    let (old, caught) = no_alloc(|| e.set_modulation(smaller));
    assert_eq!(caught, 0, "swapping in a smaller set touched the allocator");
    drop(old);
    run(&mut e, 300);
    let (old, caught) = no_alloc(|| e.set_modulation(bigger));
    assert_eq!(caught, 0, "swapping in a bigger set touched the allocator");
    drop(old);
    run(&mut e, 600);
}

#[test]
fn one_shot_steps_never_touch_the_allocator() {
    // Under the steps a pass plays once and stops, the cloud stops throwing
    // grains between passes, and switching the steps on hands the loop to a
    // tail. A short source and sparse steps, so every pass plays out before
    // the next, with the reel reversing along the way. Then again with nothing
    // wired into Sample, where the cloud's window is the clock.
    for wired in [true, false] {
        let mut e = Engine::new(SR, 256);
        e.set_source(tone(2_000));
        if !wired {
            drop(e.swap_source(Generator::Player, Vec::new()));
        }
        let bank = ParamBank::new();
        bank.set_by_id("grain.on", 1.0);
        let arr = ParamBank::for_table(arrangement::params());
        e.set_playing(true);
        let mut out = vec![0.0f32; 512];

        let ((), caught) = no_alloc(|| {
            for block in 0..1_000 {
                let reverse = if block % 400 < 200 { 0.0 } else { 1.0 };
                assert!(arr.set_by_id("arrangement.tape.reverse", reverse));
                e.set_arrangement(&arr, false);
                // Pitched to both ends of the range, so each pass reads at a
                // different ratio from the tail it hands over.
                let mut pitches = [0; shard_dsp::steps::STEPS];
                pitches[0] = 24;
                pitches[2] = -24;
                e.set_steps(StepParams {
                    on: block % 100 >= 20,
                    pattern: 0b0101,
                    pitches,
                    ..Default::default()
                });
                e.process_block(&mut out, &bank);
            }
        });
        assert_eq!(
            caught, 0,
            "one-shot steps touched the allocator {caught} time(s), Sample wired: {wired}"
        );
    }
}

#[test]
fn the_callback_hand_offs_never_touch_the_allocator() {
    // The app's callback takes new sources and auditions from shared slots
    // with `try_lock`, and gives retired buffers back the same way.
    //
    // A mutex's *first* lock can allocate: on macOS the standard library
    // builds the underlying pthread mutex lazily, on first use. Found by this
    // test on 2026-09-13 (Rust 1.96), when an unprimed pair caught exactly two
    // calls. So the app locks every slot once before the stream starts, and
    // this holds that a primed slot is then free to hand off from.
    //
    // One slot per generator, as the app holds them: a source for the player,
    // a source for the cloud, or both.
    let swap: Mutex<[Option<Vec<f32>>; 2]> = Mutex::new([Some(tone(1_000)), Some(tone(500))]);
    let retired: Mutex<[Option<Vec<f32>>; 2]> = Mutex::new([None, None]);
    drop(swap.lock());
    drop(retired.lock());

    let ((), caught) = no_alloc(|| {
        let taken = swap
            .try_lock()
            .ok()
            .map(|mut slot| std::mem::take(&mut *slot));
        if let (Ok(mut slot), Some(taken)) = (retired.try_lock(), taken) {
            *slot = taken;
        }
    });
    assert_eq!(
        caught, 0,
        "a hand-off touched the allocator {caught} time(s)"
    );
    let lens = retired
        .lock()
        .map(|s| s.each_ref().map(|b| b.as_ref().map(Vec::len)))
        .ok();
    assert_eq!(lens, Some([Some(1_000), Some(500)]));
}

#[test]
fn handing_a_modulation_set_across_never_touches_the_allocator() {
    // The app's callback takes a rebuilt `ModSet` from a primed slot, swaps it
    // into the running engine, and gives the one it replaced back through a
    // second slot, as it does a source buffer. The old set holds LFOs, so
    // freeing it here instead would be caught.
    let lfos = |n: u64| -> Vec<LfoSpec> {
        (1..=n)
            .map(|id| LfoSpec {
                id,
                rate_hz: id as f32,
                shape: Shape::Sine,
                phase: 0.0,
            })
            .collect()
    };
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(48_000));
    e.set_playing(true);
    drop(e.set_modulation(ModSet::new(&lfos(4))));

    let swap: Mutex<Option<ModSet>> = Mutex::new(Some(ModSet::new(&lfos(2))));
    let retired: Mutex<Option<ModSet>> = Mutex::new(None);
    drop(swap.lock());
    drop(retired.lock());

    let bank = ParamBank::new();
    let mut out = vec![0.0f32; 512];
    let mut retiring: Option<ModSet> = None;
    let ((), caught) = no_alloc(|| {
        for _ in 0..3 {
            if let Some(old) = retiring.take() {
                match retired.try_lock() {
                    Ok(mut slot) if slot.is_none() => *slot = Some(old),
                    _ => retiring = Some(old),
                }
            }
            if retiring.is_none() {
                if let Ok(mut pending) = swap.try_lock() {
                    if let Some(set) = pending.take() {
                        retiring = Some(e.set_modulation(set));
                    }
                }
            }
            e.process_block(&mut out, &bank);
        }
    });
    assert_eq!(
        caught, 0,
        "the modulation hand-off touched the allocator {caught} time(s)"
    );
    let old = retired.lock().ok().and_then(|mut slot| slot.take());
    assert!(
        old.is_some_and(|set| set.lfo_value(4).is_some()),
        "the replaced set should be waiting in the retired slot"
    );
}

#[test]
fn swapping_a_source_hands_the_old_buffer_back_instead_of_freeing_it() {
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(48_000));

    for (which, len) in [(Generator::Player, 24_000), (Generator::Grain, 12_000)] {
        let next = tone(len);
        let (old, caught) = no_alloc(|| e.swap_source(which, next));
        assert_eq!(caught, 0, "the swap freed something on the audio thread");
        assert_eq!(
            old.len(),
            48_000,
            "the old buffer should come back to the caller"
        );
        assert_eq!(e.source_len(which), len);
    }
    assert_eq!(
        e.source_len(Generator::Player),
        24_000,
        "swapping the cloud's source must leave the player's alone"
    );
}

#[test]
fn adding_removing_and_reordering_effects_never_touches_the_allocator() {
    // Every effect switched on and audible, and the chain changed every few
    // blocks in the patch and the arrangement: filled, emptied, reversed,
    // rotated through the second copies, a pair swapped. Adding resets an
    // instance, removing fades it out, reordering ducks the chain; none of it
    // may allocate, and the largest reset (the reverb's) happens here.
    use shard_dsp::fx::{self, Order, INSTANCES, ORDER_LEN};

    let mut e = Engine::new(SR, 256);
    e.set_source(tone(96_000));
    let bank = ParamBank::new();
    let arr = ParamBank::for_table(arrangement::params());
    for n in 0..INSTANCES {
        let (kind, _) = fx::POOL[n];
        for (b, prefix) in [(&bank, ""), (&arr, arrangement::PREFIX)] {
            b.set_by_id(&format!("{prefix}fx.{n}.{}.on", kind.name()), 1.0);
            b.set_by_id(&format!("{prefix}fx.{n}.{}.mix", kind.name()), 1.0);
        }
    }
    e.set_playing(true);

    let orders = |step: usize| -> Order {
        let mut o = Order::EMPTY;
        match step % 6 {
            0 => (0..ORDER_LEN).for_each(|n| {
                o.push(n);
            }),
            1 => {}
            2 => (0..ORDER_LEN).rev().for_each(|n| {
                o.push(n);
            }),
            3 => (INSTANCES - ORDER_LEN..INSTANCES).for_each(|n| {
                o.push(n);
            }),
            4 => {
                o.push(10);
                o.push(3);
                o.push(7);
            }
            _ => {
                o.push(3);
                o.push(10);
                o.push(7);
            }
        }
        o
    };

    let mut out = vec![0.0f32; 512];
    for block in 0..600 {
        // The host changes the chain between blocks; that is not the callback.
        if block % 5 == 0 {
            fx::set_order(&bank, &orders(block / 5));
            fx::set_order(&arr, &orders(block / 5 + 2));
        }
        let ((), caught) = no_alloc(|| {
            e.set_arrangement(&arr, false);
            e.process_block(&mut out, &bank);
        });
        assert_eq!(
            caught, 0,
            "block {block} touched the allocator {caught} time(s)"
        );
        assert!(out.iter().all(|v| v.is_finite()), "block {block}");
    }
}

#[test]
fn key_tracking_never_touches_the_allocator() {
    // The filter's cutoff follows each step's pitch, per frame, as the
    // tracking switches between off, half and full and the pitches move.
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(48_000));
    let bank = ParamBank::new();
    bank.set_by_id("grain.on", 1.0);
    bank.set_by_id("filter.on", 1.0);
    bank.set_by_id("filter.mix", 1.0);
    bank.set_by_id("filter.cutoff", 400.0);
    e.set_playing(true);
    let mut out = vec![0.0f32; 512];

    let ((), caught) = no_alloc(|| {
        for block in 0..1_200 {
            bank.set_by_id("filter.tracking", (block / 50 % 3) as f32);
            let mut pitches = [0i8; shard_dsp::steps::STEPS];
            for (i, p) in pitches.iter_mut().enumerate() {
                *p = ((block / 10 + i * 5) % 49) as i8 - 24;
            }
            e.set_steps(StepParams {
                on: true,
                tempo_bpm: 240.0,
                pattern: 0xFFFF,
                pitches,
                ..Default::default()
            });
            e.process_block(&mut out, &bank);
        }
    });
    assert_eq!(
        caught, 0,
        "key tracking touched the allocator {caught} time(s)"
    );
}

#[test]
fn the_24_db_slope_never_touches_the_allocator() {
    // The filter's slope flips back and forth in the patch and the
    // arrangement, through its crossfade, with the resonance up, across all
    // three types and while the cutoff sweeps.
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(48_000));
    let bank = ParamBank::new();
    let arr = ParamBank::for_table(arrangement::params());
    bank.set_by_id("grain.on", 1.0);
    for (b, prefix) in [(&bank, ""), (&arr, arrangement::PREFIX)] {
        b.set_by_id(&format!("{prefix}filter.on"), 1.0);
        b.set_by_id(&format!("{prefix}filter.mix"), 1.0);
        b.set_by_id(&format!("{prefix}filter.resonance"), 0.9);
    }
    e.set_playing(true);
    let mut out = vec![0.0f32; 512];

    let ((), caught) = no_alloc(|| {
        for block in 0..1_200 {
            let cutoff = 100.0 + (block % 40) as f32 * 150.0;
            let kind = (block / 100 % 3) as f32;
            bank.set_by_id("filter.slope", (block / 3 % 2) as f32);
            bank.set_by_id("filter.cutoff", cutoff);
            bank.set_by_id("filter.type", kind);
            arr.set_by_id("arrangement.filter.slope", (block / 5 % 2) as f32);
            arr.set_by_id("arrangement.filter.cutoff", cutoff * 0.7);
            arr.set_by_id("arrangement.filter.type", kind);
            e.set_arrangement(&arr, block % 400 < 200);
            e.process_block(&mut out, &bank);
        }
    });
    assert_eq!(
        caught, 0,
        "the 24 dB slope touched the allocator {caught} time(s)"
    );
    assert!(out.iter().all(|v| v.is_finite()));
}

#[test]
fn held_steps_never_touch_the_allocator() {
    // Under Hold each step gates its own note: a trigger latches a different
    // gate, release and end every time. The patch's Length is Hold, the
    // steps run fast, and the holds, pitches and tempo change along the way.
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(48_000));
    let bank = ParamBank::new();
    bank.set_by_id("patch.length", 2.0);
    bank.set_by_id("patch.hold", 80.0);
    bank.set_by_id("grain.on", 1.0);
    bank.set_by_id("fm.on", 1.0);
    e.set_playing(true);
    let mut out = vec![0.0f32; 512];

    let ((), caught) = no_alloc(|| {
        for block in 0..1_200 {
            let mut holds = [0u8; shard_dsp::steps::STEPS];
            for (i, h) in holds.iter_mut().enumerate() {
                *h = ((block / 40 + i) % 18) as u8;
            }
            // Early and late, every step a different amount, moving each block.
            let mut nudges = [0i8; shard_dsp::steps::STEPS];
            for (i, n) in nudges.iter_mut().enumerate() {
                *n = ((block + i * 7) % 101) as i8 - 50;
            }
            e.set_steps(StepParams {
                on: true,
                tempo_bpm: 120.0 + (block % 120) as f32,
                pattern: 0xFFFF,
                holds,
                nudges,
                ..Default::default()
            });
            e.process_block(&mut out, &bank);
        }
    });
    assert_eq!(
        caught, 0,
        "held steps touched the allocator {caught} time(s)"
    );
}

#[test]
fn every_drums_track_never_touches_the_allocator() {
    // Every kit of the pool (story d) on every step, so hits cut hits and
    // tails overlap; each kit's length, tune, decay and fader move along the
    // way, its switch goes off and on, kits are switched out (ringing out)
    // and back in mid-loop, and their mutes and solos land on the bars.
    use shard_dsp::kit::{KitParams, KitPattern, VOICES as DRUMS};
    use shard_dsp::steps::STEPS;
    use shard_dsp::KITS;
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(48_000));
    let bank = ParamBank::new();
    let arr = ParamBank::for_table(arrangement::params());
    for k in 0..KITS {
        e.set_kit_active(k, true);
    }
    e.set_playing(true);
    let mut out = vec![0.0f32; 512];

    let ((), caught) = no_alloc(|| {
        for block in 0..1_200 {
            e.set_arrangement(&arr, (block / 400) % 3 == 2);
            e.set_steps(StepParams {
                on: true,
                tempo_bpm: 120.0 + (block % 120) as f32,
                swing: (block % 50) as f32 * 0.01,
                pattern: 0xFFFF,
                ..Default::default()
            });
            for k in 0..KITS {
                e.set_kit(
                    k,
                    KitPattern {
                        length: [4, 8, 16, 32, 64][(block / 100 + k) % 5],
                        hits: [[100 - k as u8 * 10; STEPS]; DRUMS],
                    },
                    KitParams {
                        on: (block + k * 37) % 200 < 150,
                        tune: ((block + k) % 25) as f32 - 12.0,
                        decay: 0.25 + ((block + k) % 16) as f32 * 0.25,
                    },
                );
                e.set_kit_gain(k, ((block + k * 7) % 40) as f32 * 0.05);
                let m = Mix {
                    mute: (block / (17 + k)) % 3 == 0,
                    solo: (block / (31 + k)) % 5 == 0,
                    solo_safe: false,
                };
                if block % 170 == 60 + k {
                    e.place_kit_mix(k, m);
                } else {
                    e.set_kit_mix(k, m);
                }
                if block % 250 == 100 + k * 10 {
                    e.set_kit_active(k, false);
                }
                if block % 250 == 180 + k * 10 {
                    e.set_kit_active(k, true);
                }
                black_box(e.kit_mix(k));
                black_box(e.kit_audible(k));
                black_box(e.kit_step(k));
            }
            e.process_block(&mut out, &bank);
            assert!(out.iter().all(|v| v.is_finite()), "block {block}");
        }
    });
    assert_eq!(caught, 0, "the kits touched the allocator {caught} time(s)");
}

#[test]
fn every_voice_playing_never_touches_the_allocator() {
    // The whole pool at once (epic 28): every voice active, each on its own
    // bank with its own chain, cloud and FM, its own steps on the shared
    // transport, and its own LFOs. Mid-stream, sources and modulation sets
    // are swapped on several voices, voices are switched out and back in
    // (each wake resets the effects its chain names), and the tape brakes
    // and rewinds.
    use shard_dsp::fx::{self, Order, INSTANCES, POOL};

    let mut e = Engine::new(SR, 256);
    let banks: Vec<ParamBank> = (0..VOICES).map(|_| ParamBank::new()).collect();
    for (i, bank) in banks.iter().enumerate() {
        e.set_voice_source(i, tone(24_000 + i * 1_000));
        e.set_voice_active(i, true);
        bank.set_by_id("grain.on", 1.0);
        bank.set_by_id("fm.on", 1.0);
        bank.set_by_id("fm.drift", 0.5);
        // A different run of the palette on every voice, the second copies
        // included, so between them every instance in the pool is used.
        let mut order = Order::EMPTY;
        for k in 0..6 {
            let n = (i * 5 + k * 3) % INSTANCES;
            let (kind, _) = POOL[n];
            bank.set_by_id(&format!("fx.{n}.{}.on", kind.name()), 1.0);
            bank.set_by_id(&format!("fx.{n}.{}.mix", kind.name()), 0.7);
            order.push(n);
        }
        fx::set_order(bank, &order);
        e.set_voice_steps(
            i,
            StepParams {
                on: true,
                length: [4, 8, 16, 32][i % 4],
                pattern: 0x5555_5555 >> (i % 3),
                ..Default::default()
            },
        );
        drop(e.set_voice_modulation(
            i,
            ModSet::new(&[LfoSpec {
                id: 1,
                rate_hz: 1.0 + i as f32,
                shape: Shape::SmoothRandom,
                phase: 0.0,
            }]),
        ));
    }
    let arr = ParamBank::for_table(arrangement::params());
    // Every kit playing too, under the voices.
    for k in 0..shard_dsp::KITS {
        e.set_kit(
            k,
            shard_dsp::KitPattern::default(),
            shard_dsp::KitParams {
                on: true,
                ..shard_dsp::KitParams::default()
            },
        );
        e.set_kit_active(k, true);
    }
    e.set_transport(&StepParams {
        on: true,
        tempo_bpm: 180.0,
        swing: 0.2,
        ..Default::default()
    });
    e.set_playing(true);
    let refs: [&ParamBank; VOICES] = std::array::from_fn(|i| &banks[i]);

    // Built out here, off the audio thread; the replaced ones come back.
    let mut sources: Vec<Vec<f32>> = (0..12).map(|i| tone(10_000 + i * 700)).collect();
    let mut sets: Vec<ModSet> = (0..12)
        .map(|i| {
            let mut set = ModSet::new(&[LfoSpec {
                id: 1 + (i % 2) as u64,
                rate_hz: 3.0,
                shape: Shape::Sine,
                phase: 0.0,
            }]);
            set.link("grain.position", 1 + (i % 2) as u64, 0.1, 0.9)
                .unwrap();
            set
        })
        .collect();
    let mut retired_sources = Vec::with_capacity(16);
    let mut retired_sets = Vec::with_capacity(16);
    let mut out = vec![0.0f32; 512];
    let heard = ParamBank::new();

    for block in 0..900 {
        let ((), caught) = no_alloc(|| {
            if block % 75 == 10 {
                let i = (block / 75) % VOICES;
                if let Some(buf) = sources.pop() {
                    let g = if block % 150 == 10 {
                        Generator::Player
                    } else {
                        Generator::Grain
                    };
                    retired_sources.push(e.swap_voice_source(i, g, buf));
                }
                if let Some(set) = sets.pop() {
                    retired_sets.push(e.set_voice_modulation((i + 3) % VOICES, set));
                }
            }
            // Voices out and back in, a few at a time.
            if block % 60 == 30 {
                e.set_voice_active((block / 60) % VOICES, false);
            }
            if block % 60 == 50 {
                e.set_voice_active((block / 60) % VOICES, true);
            }
            // The per-block hand-offs on a couple of voices: steps,
            // transport and readings, all moving.
            for i in [2, 5] {
                e.set_voice_steps(
                    i,
                    StepParams {
                        on: true,
                        length: [4, 8, 16, 32, 64][(block / 20 + i) % 5],
                        pattern: 0xFFFF_FFFF_FFFF_FFFF >> (block % 7),
                        ..Default::default()
                    },
                );
                let t = ((block * (i + 3)) % 97) as f32 / 96.0;
                e.set_voice_readings(
                    i,
                    Reading {
                        octave: t * 4.0 - 2.0,
                        trim_start: t * 0.5,
                        trim_end: 1.0 - t * 0.3,
                    },
                    Reading::default(),
                );
            }
            e.set_transport(&StepParams {
                on: true,
                tempo_bpm: 120.0 + (block % 120) as f32,
                swing: (block % 50) as f32 * 0.01,
                ..Default::default()
            });
            arr.set_by_id(
                "arrangement.tape.brake",
                if block % 300 > 200 { 1.0 } else { 0.0 },
            );
            arr.set_by_id(
                "arrangement.tape.reverse",
                if block % 400 > 330 { 1.0 } else { 0.0 },
            );
            // Mutes and solos pressed, cancelled and landing on the bars,
            // a track's switches placed when it moves voice, and the applied
            // mix read back for the scene view.
            for i in 0..VOICES {
                let m = Mix {
                    mute: (block / (13 + i)) % 3 == 0,
                    solo: (block / (29 + i)) % 5 == 0,
                    solo_safe: i == VOICES - 1,
                };
                if block % 170 == 90 + i {
                    e.place_voice_mix(i, m);
                } else {
                    e.set_voice_mix(i, m);
                }
                black_box(e.voice_mix(i));
                black_box(e.voice_audible(i));
            }
            for k in 0..shard_dsp::KITS {
                e.set_kit_mix(
                    k,
                    Mix {
                        mute: (block / (19 + k)) % 4 == 0,
                        solo: (block / (37 + k)) % 6 == 0,
                        solo_safe: false,
                    },
                );
                black_box(e.kit_audible(k));
            }
            // Every voice's fader moving, and what the shown voice heard,
            // where it is and how many grains it has, as the host reads them.
            for i in 0..VOICES {
                e.set_voice_gain(i, ((block + i * 11) % 40) as f32 * 0.05);
            }
            e.set_arrangement(&arr, false);
            e.process_voices(&mut out, &refs);
            let shown = (block / 90) % VOICES;
            e.publish_voice_heard(shown, refs[shown], &heard);
            black_box(e.voice_play_position(shown));
            black_box(e.voice_active_grains(shown));
            black_box(e.voice_step(shown));
        });
        assert_eq!(
            caught, 0,
            "block {block} with every voice touched the allocator {caught} time(s)"
        );
        assert!(out.iter().all(|v| v.is_finite()), "block {block}");
    }
    assert!(retired_sources.len() >= 10 && retired_sets.len() >= 10);
}
