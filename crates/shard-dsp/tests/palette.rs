//! The palette's audio contracts, through the engine.
//!
//! Changing the chain while it plays must stay inside the ordinary movement of
//! the signal: adding, removing and reordering effects do not click.

use shard_dsp::fx::{self, Kind, Order};
use shard_dsp::{Engine, ParamBank};

const SR: f32 = 48_000.0;

fn tone(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            0.6 * (core::f32::consts::TAU * 220.0 * t).sin()
                + 0.2 * (core::f32::consts::TAU * 1_330.0 * t).sin()
        })
        .collect()
}

fn instance(kind: Kind) -> usize {
    fx::POOL
        .iter()
        .position(|&(k, c)| k == kind && c == 0)
        .unwrap()
}

/// Every effect named here is on and fully mixed.
fn loud(bank: &ParamBank, kinds: &[Kind]) {
    for kind in kinds {
        let n = instance(*kind);
        let name = kind.name();
        assert!(bank.set_by_id(&format!("fx.{n}.{name}.on"), 1.0));
        assert!(bank.set_by_id(&format!("fx.{n}.{name}.mix"), 1.0));
    }
}

fn chain(bank: &ParamBank, kinds: &[Kind]) {
    let mut o = Order::EMPTY;
    for k in kinds {
        o.add(*k).unwrap();
    }
    fx::set_order(bank, &o);
}

/// The largest sample-to-sample step over blocks `from..`, with the chain set
/// by `script(block)` before each block.
fn worst_step(blocks: usize, from: usize, script: impl Fn(usize, &ParamBank)) -> f32 {
    let mut e = Engine::new(SR, 128);
    e.set_source(tone(96_000));
    e.set_playing(true);
    let bank = ParamBank::new();
    let mut out = vec![0.0; 256];
    let (mut prev, mut worst) = (0.0f32, 0.0f32);
    for block in 0..blocks {
        script(block, &bank);
        e.process_block(&mut out, &bank);
        for s in out.iter().step_by(2) {
            if block >= from {
                worst = worst.max((s - prev).abs());
            }
            prev = *s;
        }
    }
    worst
}

/// The kinds whose output does not wander at random, so a click is a click.
const STEADY: [Kind; 6] = [
    Kind::Drive,
    Kind::Ring,
    Kind::Chorus,
    Kind::Flanger,
    Kind::Delay,
    Kind::Reverb,
];

#[test]
fn an_empty_chain_is_bit_exact_with_no_effects_at_all() {
    let run = |all: bool| {
        let mut e = Engine::new(SR, 128);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        if all {
            // Every effect set up loud and on, none of them in the chain.
            for kind in Kind::ALL {
                loud(&bank, &[kind]);
            }
        }
        let mut out = vec![0.0; 256];
        let mut all_out = Vec::new();
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
            all_out.extend_from_slice(&out);
        }
        all_out
    };
    assert_eq!(run(true), run(false));
}

#[test]
fn adding_an_effect_does_not_click() {
    for kind in STEADY {
        let steady = worst_step(120, 20, |_, bank| {
            loud(bank, &[kind]);
            chain(bank, &[kind]);
        });
        let added = worst_step(120, 20, |block, bank| {
            loud(bank, &[kind]);
            if block == 40 {
                chain(bank, &[kind]);
            }
        });
        assert!(
            added < steady * 2.0 + 0.02,
            "adding {} stepped by {added} against a steady {steady}",
            kind.name()
        );
    }
}

#[test]
fn removing_an_effect_does_not_click() {
    for kind in STEADY {
        let steady = worst_step(120, 20, |_, bank| {
            loud(bank, &[kind]);
            chain(bank, &[kind]);
        });
        let removed = worst_step(120, 20, |block, bank| {
            loud(bank, &[kind]);
            if block < 40 {
                chain(bank, &[kind]);
            } else {
                chain(bank, &[]);
            }
        });
        assert!(
            removed < steady * 2.0 + 0.02,
            "removing {} stepped by {removed} against a steady {steady}",
            kind.name()
        );
    }
}

#[test]
fn reordering_effects_does_not_click() {
    // Drive then delay, and the other way round, against holding either.
    const PAIR: [Kind; 2] = [Kind::Drive, Kind::Delay];
    const SWAPPED: [Kind; 2] = [Kind::Delay, Kind::Drive];
    let hold = |kinds: &'static [Kind]| {
        worst_step(160, 20, move |_, bank| {
            loud(bank, &PAIR);
            chain(bank, kinds);
        })
    };
    let steady = hold(&PAIR).max(hold(&SWAPPED));
    let moved = worst_step(160, 20, |block, bank| {
        loud(bank, &PAIR);
        chain(bank, if block < 60 { &PAIR } else { &SWAPPED });
    });
    assert!(
        moved < steady * 2.0 + 0.02,
        "reordering stepped by {moved} against a steady {steady}"
    );
}

#[test]
fn the_arrangement_has_its_own_chain() {
    let render = |arranged: bool| {
        let mut e = Engine::new(SR, 128);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        let arr = ParamBank::for_table(shard_dsp::arrangement::params());
        let n = instance(Kind::Ring);
        arr.set_by_id(&format!("arrangement.fx.{n}.ring.on"), 1.0);
        arr.set_by_id(&format!("arrangement.fx.{n}.ring.mix"), 1.0);
        if arranged {
            let mut o = Order::EMPTY;
            o.add(Kind::Ring);
            fx::set_order(&arr, &o);
        }
        let mut out = vec![0.0; 256];
        let mut all = Vec::new();
        for _ in 0..100 {
            e.set_arrangement(&arr, false);
            e.process_block(&mut out, &bank);
            all.extend_from_slice(&out);
        }
        all
    };
    assert_ne!(
        render(true),
        render(false),
        "the arrangement's chain is silent"
    );
}
