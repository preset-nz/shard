//! Per-parameter drift.
//!
//! Any parameter can be handed to a slow oscillator instead of a hand. That
//! is the whole feature: a toggle per control, not one global switch.
//!
//! This is the modulation matrix in embryo. Today every drifting parameter
//! gets one internal LFO at a fixed rate; the matrix generalises it to any
//! source driving any destination with a depth and a curve. The shape is
//! already right, which is the point of doing it this way now.
//!
//! Runs on its own thread and writes the same atomic bank the UI writes to.
//! A parameter with drift off is never touched here, so it stays yours.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use shard_dsp::params::{Taper, PARAMS};
use shard_dsp::ParamBank;

/// Which parameters drift on startup. Chosen because these five are what make
/// a static cloud sound alive, and because hearing them move is the fastest
/// way to understand what each one does.
const ON_BY_DEFAULT: &[&str] = &[
    "grain.position",
    "grain.size",
    "grain.density",
    "grain.jitter",
    "ring.freq",
];

/// One flag per parameter, indexed the same as `PARAMS`.
pub struct DriftState {
    enabled: Vec<AtomicBool>,
    /// Master switch. Off means nothing drifts, whatever the per-parameter
    /// flags say, and flipping it back restores them rather than clearing.
    master: AtomicBool,
}

impl DriftState {
    pub fn new() -> Self {
        Self {
            enabled: PARAMS
                .iter()
                .map(|p| AtomicBool::new(ON_BY_DEFAULT.contains(&p.id)))
                .collect(),
            master: AtomicBool::new(true),
        }
    }

    /// Stepped parameters are excluded. Drifting an enum means walking through
    /// window shapes at random, which is a different feature and not this one.
    pub fn can_drift(index: usize) -> bool {
        !matches!(PARAMS[index].taper, Taper::Stepped(_))
    }

    pub fn is_on(&self, index: usize) -> bool {
        self.enabled[index].load(Ordering::Relaxed)
    }

    pub fn set(&self, index: usize, on: bool) {
        if Self::can_drift(index) {
            self.enabled[index].store(on, Ordering::Relaxed);
        }
    }

    pub fn master(&self) -> bool {
        self.master.load(Ordering::Relaxed)
    }

    pub fn set_master(&self, on: bool) {
        self.master.store(on, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Vec<bool> {
        (0..PARAMS.len()).map(|i| self.is_on(i)).collect()
    }
}

impl Default for DriftState {
    fn default() -> Self {
        Self::new()
    }
}

/// Each parameter gets its own rate and starting phase, spread so no two land
/// in step. Without this every drifting control moves together and the whole
/// thing pulses instead of wandering.
fn rate_for(index: usize) -> f32 {
    0.007 + 0.0037 * index as f32
}

fn phase_for(index: usize) -> f32 {
    (index as f32 * 0.37).fract()
}

/// How far a drifting parameter travels, as a fraction of its full range,
/// centred. Full range sends density to nearly nothing and grain size to five
/// milliseconds, both of which read as a fault rather than as movement.
const DEPTH: f32 = 0.7;

pub fn spawn(bank: Arc<ParamBank>, state: Arc<DriftState>) {
    thread::spawn(move || {
        let start = Instant::now();
        loop {
            if state.master() {
                let t = start.elapsed().as_secs_f32();
                for (i, def) in PARAMS.iter().enumerate() {
                    if !state.is_on(i) || !DriftState::can_drift(i) {
                        continue;
                    }
                    let phase = rate_for(i) * t + phase_for(i);
                    let osc = 0.5 + 0.5 * (core::f32::consts::TAU * phase).sin();
                    let pos = 0.5 + (osc - 0.5) * DEPTH;
                    bank.set(i, def.denormalise(pos));
                }
            }
            // 60 Hz, well under every smoothing time in the table, so the
            // steps between updates are inaudible.
            thread::sleep(Duration::from_millis(16));
        }
    });
}
