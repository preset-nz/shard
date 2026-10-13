//! Scrubbing through the engine (roadmap row 45): Off leaves the sample
//! exactly as it was, and On follows the position row.

use shard_dsp::{Engine, ParamBank};

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;

fn tone() -> Vec<f32> {
    (0..48_000)
        .map(|i| (core::f32::consts::TAU * 220.0 * i as f32 / SR).sin())
        .collect()
}

/// `blocks` blocks of output, with `each` setting the bank before every one.
fn render(blocks: usize, mut each: impl FnMut(usize, &ParamBank)) -> Vec<f32> {
    let mut e = Engine::new(SR, 64);
    e.set_source(tone());
    e.set_playing(true);
    let bank = ParamBank::new();
    let mut out = vec![0.0; BLOCK * 2];
    let mut all = Vec::new();
    for b in 0..blocks {
        each(b, &bank);
        e.process_block(&mut out, &bank);
        all.extend_from_slice(&out);
    }
    all
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn scrub_off_ignores_its_position_and_is_bit_exact() {
    let plain = render(300, |_, _| {});
    let moved = render(300, |b, bank| {
        bank.set_by_id("material.position", (b % 7) as f32 / 6.0);
    });
    assert!(
        plain == moved,
        "the position moved a sample that was not scrubbed"
    );
    assert!(peak(&plain) > 0.3);
}

#[test]
fn scrub_switched_on_and_off_again_leaves_nothing_behind() {
    let plain = render(400, |_, _| {});
    let toggled = render(400, |b, bank| {
        bank.set_by_id(
            "material.scrub",
            if (40..80).contains(&b) { 1.0 } else { 0.0 },
        );
        bank.set_by_id("material.position", (b % 5) as f32 / 4.0);
    });
    // The passes ran on underneath, so once the fade has landed it is the
    // same sample, bit for bit.
    let from = 120 * BLOCK * 2;
    assert!(plain[from..] == toggled[from..]);
}

#[test]
fn scrub_on_holds_silent_still_and_sounds_while_the_position_moves() {
    let out = render(600, |b, bank| {
        bank.set_by_id("material.scrub", 1.0);
        // Still for the first half, then a sweep across the window.
        let t = if b < 300 {
            0.5
        } else {
            0.5 + (b - 300) as f32 / 700.0
        };
        bank.set_by_id("material.position", t);
    });
    let held = &out[250 * BLOCK * 2..300 * BLOCK * 2];
    assert_eq!(peak(held), 0.0, "a held position should be silent");
    let moving = &out[320 * BLOCK * 2..];
    assert!(peak(moving) > 0.1, "a moving position should sound");
    assert!(out.iter().all(|s| s.is_finite()));
}

#[test]
fn a_random_jumping_position_stays_finite_and_in_range() {
    let mut rng = 0x9e37_79b9u32;
    let out = render(800, |_, bank| {
        rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        bank.set_by_id("material.scrub", 1.0);
        bank.set_by_id("material.position", (rng >> 8) as f32 / (1u32 << 24) as f32);
    });
    assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    assert!(peak(&out) > 0.05);
}
