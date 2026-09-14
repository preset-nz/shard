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

use shard_dsp::rt::{no_alloc, GuardedAlloc};
use shard_dsp::{Engine, GrainSpawn, LfoSpec, ModSet, ParamBank, Shape, Taper, PARAMS};

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
    for (i, p) in PARAMS.iter().enumerate() {
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
    e.set_playing(true);
    let bank = ParamBank::new();
    let mut out = vec![0.0f32; 2048];

    for block in 0..2_000 {
        sweep(&bank, block);
        // Devices do not promise a fixed block size, so neither does this.
        let n = [64, 256, 512, 2048][block % 4];
        let ((), caught) = no_alloc(|| e.process_block(&mut out[..n], &bank));
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
    let build = |lfos: &[LfoSpec]| {
        let mut set = ModSet::new(lfos);
        for (i, p) in PARAMS
            .iter()
            .enumerate()
            .filter(|(_, p)| !matches!(p.taper, Taper::Stepped(_)))
        {
            let (lo, hi) = if i % 2 == 0 { (0.2, 0.9) } else { (0.9, 0.2) };
            set.link(p.id, lfos[i % lfos.len()].id, lo, hi).unwrap();
        }
        set
    };
    let (first, smaller, bigger) = (build(&specs), build(&specs[..3]), build(&specs));

    let mut e = Engine::new(SR, 256);
    e.set_source(tone(96_000));
    let bank = ParamBank::new();
    for id in ["grain.on", "ring.on", "crush.on", "filter.on"] {
        bank.set_by_id(id, 1.0);
    }
    bank.set_by_id("grain.gain", 1.0);
    e.set_playing(true);
    drop(e.set_modulation(first));

    let mut out = vec![0.0f32; 512];
    let heard = ParamBank::new();
    let mut run = |e: &mut Engine, from: usize| {
        let ((), caught) = no_alloc(|| {
            for block in from..from + 300 {
                sweep(&bank, block);
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
fn the_callback_hand_offs_never_touch_the_allocator() {
    // The app's callback takes new sources and auditions from shared slots
    // with `try_lock`, and gives retired buffers back the same way.
    //
    // A mutex's *first* lock can allocate: on macOS the standard library
    // builds the underlying pthread mutex lazily, on first use. Found by this
    // test on 2026-09-13 (Rust 1.96), when an unprimed pair caught exactly two
    // calls. So the app locks every slot once before the stream starts, and
    // this holds that a primed slot is then free to hand off from.
    let swap: Mutex<Option<Vec<f32>>> = Mutex::new(Some(tone(1_000)));
    let retired: Mutex<Option<Vec<f32>>> = Mutex::new(None);
    drop(swap.lock());
    drop(retired.lock());

    let ((), caught) = no_alloc(|| {
        let taken = swap.try_lock().ok().and_then(|mut slot| slot.take());
        if let Ok(mut slot) = retired.try_lock() {
            *slot = taken;
        }
    });
    assert_eq!(
        caught, 0,
        "a hand-off touched the allocator {caught} time(s)"
    );
    assert_eq!(
        retired
            .lock()
            .map(|s| s.as_ref().map(Vec::len))
            .ok()
            .flatten(),
        Some(1_000)
    );
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
fn swapping_the_source_hands_the_old_buffer_back_instead_of_freeing_it() {
    let mut e = Engine::new(SR, 256);
    e.set_source(tone(48_000));
    let next = tone(24_000);

    let (old, caught) = no_alloc(|| e.set_source(next));
    assert_eq!(caught, 0, "the swap freed something on the audio thread");
    assert_eq!(
        old.len(),
        48_000,
        "the old buffer should come back to the caller"
    );
    assert_eq!(e.source_len(), 24_000);
}
