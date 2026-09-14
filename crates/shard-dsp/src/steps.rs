//! The step clock: a grid that retriggers the patch.
//!
//! The first in-app trigger source (Georg, 2026-09-14: "being able to set
//! the 4/8/16 steps to trigger the patch"). A step that is on rewinds the
//! material's pass, which restarts the envelope with it, since the envelope
//! runs on the player's position. Nothing else changes: the cloud keeps
//! going, the effects keep their state. What a trigger should render down
//! and what stays live is still open, and this is the place to find out.
//!
//! **The steps belong to the tracker, the level above the patch** (Georg,
//! the same day). Tempo, swing and the pattern are not parameters: they do
//! not live in the table, a patch does not save them, and an LFO cannot
//! reach them. They cross to the audio thread through `StepBank` and into
//! the engine with `Engine::set_steps`, once a block.
//!
//! **A step is always a sixteenth; length shortens the loop.** Four steps
//! at 120 is one beat, half a second, not a bar of quarter notes. That keeps
//! swing meaning one thing, since swing belongs to sixteenths
//! (`design/drum-programming.md`), and leaves room for tracks of different
//! lengths running against each other later.
//!
//! Sample-accurate: the clock is advanced per frame inside the engine's
//! loop, so a trigger lands on the frame it is due, not at a block edge.
//! Swing delays every second step by a share of a step.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepParams {
    /// Whether the steps run at all.
    pub on: bool,
    pub tempo_bpm: f32,
    /// 4, 8 or 16.
    pub length: u32,
    /// 0 to 0.5 of a step, applied to the even-numbered steps (the second,
    /// fourth and so on). Drum machines count the same thing from 50 % to
    /// 75 %; the UI converts.
    pub swing: f32,
    /// One bit per step, bit 0 first.
    pub pattern: u32,
}

impl Default for StepParams {
    /// Off, and four on the floor, so switching on does something.
    fn default() -> Self {
        Self {
            on: false,
            tempo_bpm: 120.0,
            length: 16,
            swing: 0.0,
            pattern: 0b0001_0001_0001_0001,
        }
    }
}

/// The steps, shared between whatever edits the tracker and the audio thread.
///
/// The same idea as `ParamBank`, for five numbers that are not parameters.
/// Stored whole, read whole once a block. A read that lands between two
/// fields of a store plays one block with half the old tracker, which nobody
/// can hear and the next block corrects.
pub struct StepBank {
    on: AtomicBool,
    tempo_bpm: AtomicU32,
    length: AtomicU32,
    swing: AtomicU32,
    pattern: AtomicU32,
}

impl StepBank {
    pub fn new(p: StepParams) -> Self {
        Self {
            on: AtomicBool::new(p.on),
            tempo_bpm: AtomicU32::new(p.tempo_bpm.to_bits()),
            length: AtomicU32::new(p.length),
            swing: AtomicU32::new(p.swing.to_bits()),
            pattern: AtomicU32::new(p.pattern),
        }
    }

    pub fn store(&self, p: &StepParams) {
        self.on.store(p.on, Ordering::Relaxed);
        self.tempo_bpm
            .store(p.tempo_bpm.to_bits(), Ordering::Relaxed);
        self.length.store(p.length, Ordering::Relaxed);
        self.swing.store(p.swing.to_bits(), Ordering::Relaxed);
        self.pattern.store(p.pattern, Ordering::Relaxed);
    }

    #[inline]
    pub fn load(&self) -> StepParams {
        StepParams {
            on: self.on.load(Ordering::Relaxed),
            tempo_bpm: f32::from_bits(self.tempo_bpm.load(Ordering::Relaxed)),
            length: self.length.load(Ordering::Relaxed),
            swing: f32::from_bits(self.swing.load(Ordering::Relaxed)),
            pattern: self.pattern.load(Ordering::Relaxed),
        }
    }
}

impl Default for StepBank {
    fn default() -> Self {
        Self::new(StepParams::default())
    }
}

pub struct StepClock {
    sample_rate: f32,
    /// Samples into the pattern.
    pos: f32,
    /// The step the last frame was in, or none before the first.
    step: Option<u32>,
    running: bool,
}

impl StepClock {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            pos: 0.0,
            step: None,
            running: false,
        }
    }

    /// Start from the top, or stop. Starting always begins at step one.
    pub fn set_running(&mut self, running: bool) {
        if running && !self.running {
            self.pos = 0.0;
            self.step = None;
        }
        self.running = running;
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// The step the clock is in, for the UI. None while stopped.
    pub fn current(&self) -> Option<u32> {
        if self.running {
            self.step
        } else {
            None
        }
    }

    /// A sixteenth note, in samples.
    fn step_len(&self, p: &StepParams) -> f32 {
        self.sample_rate * 60.0 / p.tempo_bpm.clamp(20.0, 300.0) / 4.0
    }

    /// Where step `k` begins, with swing.
    fn onset(&self, k: u32, step_len: f32, swing: f32) -> f32 {
        let late = if k % 2 == 1 { swing * step_len } else { 0.0 };
        k as f32 * step_len + late
    }

    /// One frame on. True when a step that is on begins on this frame.
    #[inline]
    pub fn tick(&mut self, p: &StepParams) -> bool {
        if !self.running {
            return false;
        }
        let step_len = self.step_len(p);
        let length = p.length.clamp(1, 32);
        let total = length as f32 * step_len;
        let swing = p.swing.clamp(0.0, 0.5);
        // Which step contains `pos`: the last whose onset is at or before it.
        let mut k = ((self.pos / step_len) as u32).min(length - 1);
        while k > 0 && self.onset(k, step_len, swing) > self.pos {
            k -= 1;
        }
        let entered = self.step != Some(k);
        self.step = Some(k);
        self.pos += 1.0;
        if self.pos >= total {
            self.pos -= total;
        }
        entered && (p.pattern >> k) & 1 == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn triggers(p: &StepParams, frames: usize) -> Vec<usize> {
        let mut c = StepClock::new(SR);
        c.set_running(true);
        (0..frames).filter(|_| c.tick(p)).collect()
    }

    #[test]
    fn four_on_the_floor_at_120_is_every_half_second() {
        let p = StepParams {
            on: true,
            tempo_bpm: 120.0,
            length: 16,
            swing: 0.0,
            pattern: 0b0001_0001_0001_0001,
        };
        let t = triggers(&p, SR as usize * 4);
        // Steps 0, 4, 8, 12 of a 16-step bar at 120: every 0.5 s, 8 in 4 s.
        assert_eq!(t.len(), 8, "{t:?}");
        for (i, at) in t.iter().enumerate() {
            assert_eq!(*at, i * 24_000, "{t:?}");
        }
    }

    #[test]
    fn four_steps_loop_every_beat_not_every_bar() {
        let p = StepParams {
            on: true,
            tempo_bpm: 120.0,
            length: 4,
            swing: 0.0,
            pattern: 0b0001,
        };
        let t = triggers(&p, SR as usize);
        // Four sixteenths at 120 is half a second; step one fires twice.
        assert_eq!(t, vec![0, 24_000]);
    }

    #[test]
    fn swing_delays_the_even_steps_only() {
        let straight = StepParams {
            on: true,
            tempo_bpm: 120.0,
            length: 4,
            swing: 0.0,
            pattern: 0b1111,
        };
        let swung = StepParams {
            swing: 0.5,
            ..straight
        };
        // One beat: a sixteenth is 6,000 samples, and half of one is 3,000.
        let a = triggers(&straight, SR as usize / 2);
        let b = triggers(&swung, SR as usize / 2);
        assert_eq!(a, vec![0, 6_000, 12_000, 18_000]);
        assert_eq!(b, vec![0, 9_000, 12_000, 21_000]);
    }

    #[test]
    fn starting_begins_at_step_one_and_stopping_reports_none() {
        let p = StepParams {
            pattern: 0b1,
            length: 4,
            ..Default::default()
        };
        let mut c = StepClock::new(SR);
        assert!(!c.tick(&p));
        assert_eq!(c.current(), None);
        c.set_running(true);
        for _ in 0..1_000 {
            c.tick(&p);
        }
        assert_eq!(c.current(), Some(0));
        c.set_running(false);
        assert_eq!(c.current(), None);
        c.set_running(true);
        assert!(c.tick(&p), "restarting fires step one again");
    }

    #[test]
    fn the_bank_hands_back_exactly_what_it_was_given() {
        let p = StepParams {
            on: true,
            tempo_bpm: 87.5,
            length: 8,
            swing: 0.12,
            pattern: 0b1010_1010_1010_1010,
        };
        let bank = StepBank::default();
        assert_eq!(bank.load(), StepParams::default());
        bank.store(&p);
        assert_eq!(bank.load(), p);
    }
}
