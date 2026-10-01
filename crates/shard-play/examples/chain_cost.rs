//! How much of a core each effect costs, and a full chain:
//! `cargo run --release -p shard-play --example chain_cost`.
//!
//! Renders ten seconds of a looping tone offline through the engine with
//! `settings` in the chain, and reports the share of real time it took. A
//! share of 0.05 means one core could run twenty of that chain. This is the
//! number behind "how many tracks": the audio callback must finish each block
//! in the time the block lasts, with room to spare. Debug builds are far
//! slower; use `--release`.

use shard_dsp::fx::{self, Kind, Order, ORDER_LEN};
use shard_dsp::{Engine, ParamBank};
use std::time::Instant;

const SR: f32 = 48_000.0;
const SECONDS: usize = 10;
const BLOCK: usize = 256;

fn share(kinds: &[Kind]) -> f64 {
    let source: Vec<f32> = (0..SR as usize * 4)
        .map(|i| {
            let t = i as f32 / SR;
            0.4 * (std::f32::consts::TAU * 220.0 * t).sin()
                + 0.2 * (std::f32::consts::TAU * 1_330.0 * t).sin()
        })
        .collect();
    let mut engine = Engine::new(SR, 256);
    engine.set_source(source);
    engine.set_playing(true);
    let bank = ParamBank::new();
    let mut order = Order::EMPTY;
    for kind in kinds {
        let Some(n) = order.add(*kind) else { continue };
        bank.set_by_id(&format!("fx.{n}.{}.mix", kind.name()), 0.5);
    }
    fx::set_order(&bank, &order);

    let mut buf = vec![0.0f32; BLOCK * 2];
    // Warm up, so the first-block work (fades in, resets) is not counted.
    for _ in 0..200 {
        engine.process_block(&mut buf, &bank);
    }
    let blocks = SECONDS * SR as usize / BLOCK;
    let start = Instant::now();
    for _ in 0..blocks {
        engine.process_block(&mut buf, &bank);
        std::hint::black_box(&buf);
    }
    start.elapsed().as_secs_f64() / SECONDS as f64
}

fn main() {
    let base = share(&[]);
    println!("no effects: {:.2}% of real time", base * 100.0);
    for kind in Kind::ALL {
        let s = share(&[kind]);
        println!("{:9} +{:.2}%", kind.name(), ((s - base) * 100.0).max(0.0));
    }
    let all: Vec<Kind> = Kind::ALL.into_iter().cycle().take(ORDER_LEN).collect();
    let full = share(&all);
    println!(
        "full chain of {ORDER_LEN}: {:.2}% of real time, so about {:.0} such chains to a core",
        full * 100.0,
        1.0 / full
    );
}
