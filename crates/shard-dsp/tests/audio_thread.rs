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

use shard_dsp::arrangement;
use shard_dsp::rt::{no_alloc, GuardedAlloc};
use shard_dsp::{
    Engine, EnvSpec, Generator, GrainSpawn, LfoSpec, ModSet, ParamBank, Reading, Shape, StepParams,
    Taper, PARAMS,
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
/// The gates (`tape.reverse`, `tape.brake`) open and close along the way.
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
        .map(|i| EnvSpec {
            id: 100 + i,
            attack_ms: i as f32 * 7.0,
            decay_ms: 20.0 + i as f32 * 30.0,
            sustain: i as f32 * 0.3,
            release_ms: i as f32 * 11.0,
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
        "ring.on",
        "crush.on",
        "filter.on",
        "drive.on",
        "chorus.on",
        "delay.on",
        "overtone.on",
        "rise.on",
        "echo.on",
        "wear.on",
        "flanger.on",
    ] {
        bank.set_by_id(id, 1.0);
    }
    bank.set_by_id("grain.gain", 1.0);
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
        e.set_playing(true);
        let mut out = vec![0.0f32; 512];

        let ((), caught) = no_alloc(|| {
            for block in 0..1_000 {
                let reverse = if block % 400 < 200 { 0.0 } else { 1.0 };
                bank.set_by_id("tape.reverse", reverse);
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
