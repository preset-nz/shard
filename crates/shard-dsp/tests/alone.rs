//! The patch on its own (Georg, 2026-10-13): `Alone::Loop` keeps sounding
//! whatever the Length, and `Alone::Once` plays one pass and then the engine
//! stops the transport itself.

use shard_dsp::{Alone, Engine, ParamBank};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;
/// A quarter of a second.
const SAMPLE: usize = 12_000;
const LOOP: f32 = 0.0;
const SAMPLE_LEN: f32 = 1.0;
const HOLD: f32 = 2.0;

fn tone() -> Vec<f32> {
    (0..SAMPLE)
        .map(|i| (core::f32::consts::TAU * 480.0 * i as f32 / SR).sin())
        .collect()
}

fn rig(length: f32, alone: Alone) -> (Engine, ParamBank) {
    let mut e = Engine::new(SR, 64);
    e.set_source(tone());
    e.set_alone(alone);
    let bank = ParamBank::new();
    bank.set_by_id("patch.length", length);
    bank.set_by_id("patch.hold", 100.0);
    (e, bank)
}

fn peak(out: &[f32]) -> f32 {
    out.iter().fold(0.0, |m, s| m.max(s.abs()))
}

/// Blocks until the engine stops itself, and the loudest of them, or `None`.
fn run_to_stop(e: &mut Engine, bank: &ParamBank, max_blocks: usize) -> Option<(usize, f32)> {
    let mut out = vec![0.0; BLOCK * 2];
    let mut loudest = 0.0f32;
    for b in 0..max_blocks {
        e.process_block(&mut out, bank);
        loudest = loudest.max(peak(&out));
        if !e.playing() {
            return Some((b + 1, loudest));
        }
    }
    None
}

#[test]
fn once_plays_one_pass_then_the_engine_stops_itself() {
    for length in [LOOP, SAMPLE_LEN, HOLD] {
        let (mut e, bank) = rig(length, Alone::Once);
        e.set_playing(true);
        let (blocks, loudest) = run_to_stop(&mut e, &bank, 2000)
            .unwrap_or_else(|| panic!("length {length}: never stopped"));
        assert!(loudest > 0.1, "length {length}: the pass was silent");
        let frames = blocks * BLOCK;
        // The sample is a quarter second; Hold is its 100 ms note and the
        // few milliseconds that let it go.
        let (lo, hi) = if length == HOLD {
            (4_000, 8_000)
        } else {
            (SAMPLE - BLOCK, SAMPLE + 2 * BLOCK)
        };
        assert!(
            (lo..=hi).contains(&frames),
            "length {length}: stopped after {frames} frames"
        );
        // Stopped means silence, and it stays so.
        let mut out = vec![1.0; BLOCK * 2];
        for _ in 0..20 {
            e.process_block(&mut out, &bank);
            assert!(
                out.iter().all(|s| *s == 0.0),
                "length {length}: sound after"
            );
        }
        // Play again starts the pass from the top, and ends the same way.
        e.set_playing(true);
        let (again, loudest) = run_to_stop(&mut e, &bank, 2000).expect("second pass stops");
        assert!(loudest > 0.1, "length {length}: the second pass was silent");
        assert_eq!(again, blocks, "length {length}: the passes differ");
    }
}

#[test]
fn once_ends_in_a_fade_not_a_cut_when_the_envelope_is_on() {
    let (mut e, bank) = rig(SAMPLE_LEN, Alone::Once);
    bank.set_by_id("env.on", 1.0);
    bank.set_by_id("env.attack", 5.0);
    bank.set_by_id("env.release", 50.0);
    e.set_playing(true);
    let mut out = vec![0.0; BLOCK * 2];
    let mut last = 0.0;
    while e.playing() {
        e.process_block(&mut out, &bank);
        last = out[out.len() - 2].abs().max(out[out.len() - 1].abs());
    }
    assert!(
        last < 0.05,
        "the last frame before the stop was still loud: {last}"
    );
}

#[test]
fn loop_keeps_sounding_past_the_sample_for_every_length() {
    for length in [LOOP, SAMPLE_LEN, HOLD] {
        let (mut e, bank) = rig(length, Alone::Loop);
        e.set_playing(true);
        let mut out = vec![0.0; BLOCK * 2];
        // Three seconds is twelve passes and thirty notes' hold.
        let blocks = 3 * SR as usize / BLOCK;
        let mut quietest = f32::MAX;
        for b in 0..blocks {
            e.process_block(&mut out, &bank);
            assert!(e.playing(), "length {length}: stopped itself");
            if b > 2 * SR as usize / BLOCK {
                quietest = quietest.min(peak(&out));
            }
        }
        assert!(quietest > 0.05, "length {length}: fell silent ({quietest})");
    }
}

#[test]
fn song_is_unchanged_by_the_new_state() {
    // Hold with the steps off retriggers when the note ends, as it did.
    let (mut e, bank) = rig(HOLD, Alone::Song);
    e.set_playing(true);
    let mut out = vec![0.0; BLOCK * 2];
    for _ in 0..(2 * SR as usize / BLOCK) {
        e.process_block(&mut out, &bank);
        assert!(e.playing());
    }
}
