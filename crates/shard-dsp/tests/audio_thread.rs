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
use shard_dsp::{Engine, GrainSpawn, ParamBank, PARAMS};

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
