//! The step clock: a grid that retriggers the patch.
//!
//! The first in-app trigger source (Georg, 2026-09-14: "being able to set
//! the 4/8/16 steps to trigger the patch"). A step that is on rewinds the
//! material's pass, which restarts the envelope with it, since the envelope
//! runs on the player's position. Nothing else changes: the cloud keeps
//! going, the effects keep their state. What a trigger should render down
//! and what stays live is still open, and this is the place to find out.
//!
//! **A step is always a sixteenth; length shortens the loop.** Four steps
//! at 120 is one beat, half a second, not a bar of quarter notes. That keeps
//! swing meaning one thing, since swing belongs to sixteenths
//! (`design/drum-programming.md`), and leaves room for lanes of different
//! lengths running against each other later.
//!
//! Sample-accurate: the clock is advanced per frame inside the engine's
//! loop, so a trigger lands on the frame it is due, not at a block edge.
//! Swing delays every second step by a share of a step.

#[derive(Debug, Clone, Copy)]
pub struct StepParams {
    pub tempo_bpm: f32,
    /// 4, 8 or 16.
    pub length: u32,
    /// 0 to 0.75 of a step, applied to the even-numbered steps (the second,
    /// fourth and so on).
    pub swing: f32,
    /// One bit per step, bit 0 first.
    pub pattern: u32,
}

impl Default for StepParams {
    fn default() -> Self {
        Self {
            tempo_bpm: 120.0,
            length: 16,
            swing: 0.0,
            pattern: 0,
        }
    }
}

/// 4, 8 or 16 from the stepped parameter's value 0, 1 or 2.
pub fn length_from_value(v: f32) -> u32 {
    match v.round() as i32 {
        i32::MIN..=0 => 4,
        1 => 8,
        _ => 16,
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
        let swing = p.swing.clamp(0.0, 0.75);
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
    fn lengths_come_from_the_stepped_value() {
        assert_eq!(length_from_value(0.0), 4);
        assert_eq!(length_from_value(1.0), 8);
        assert_eq!(length_from_value(2.0), 16);
        assert_eq!(length_from_value(-3.0), 4);
    }
}
