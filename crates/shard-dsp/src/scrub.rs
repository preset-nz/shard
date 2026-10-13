//! Scrubbing: the plain sample read by a head that chases a position.
//!
//! In Scrub the sample plays no passes. `material.position` says where in the
//! trimmed window the head should be, and the head follows it through a
//! one-pole smoother, so a mouse jump or a stepped LFO becomes a fast sweep
//! rather than a click. How fast the head moves is the tape's speed: it sets
//! the direction and the pitch, so a hand dragging forward plays forward and
//! a hand held still plays nothing. Octave does not apply (the hand is the
//! speed), and neither do steps or Length.
//!
//! Where the head runs slower than the tape's knee the level follows the
//! speed down to an exact zero, as a stopping reel's does (`tape_gain`), so a
//! held position is silent and carries no DC.
//!
//! Freeze, the other feel, needs none of this: it is the cloud at
//! `grain.position`.

use crate::voice::tape_gain;

/// How long the head takes to cover 63% of the way to the position it chases.
pub const SMOOTH_MS: f32 = 40.0;

#[derive(Clone, Copy)]
pub struct Scrub {
    /// Where the head is, 0 to 1 across the window. A fraction, so a trim
    /// moved mid-scrub keeps the head where it was relative to the window.
    ///
    /// f64, because the speed is the difference of two nearby positions and
    /// the smoother amplifies rounding by its time constant: in f32 a long
    /// material's head wanders by samples.
    head: f64,
    /// How much of the way to the position the head covers each sample.
    coef: f64,
}

impl Scrub {
    pub fn new(sample_rate: f32) -> Self {
        let coef = 1.0 - (-1.0 / (f64::from(SMOOTH_MS) * 0.001 * f64::from(sample_rate))).exp();
        Self { head: 0.0, coef }
    }

    /// Put the head on `position` without moving it through the window, for
    /// when nothing is listening to it.
    pub fn snap(&mut self, position: f32) {
        self.head = f64::from(clean(position));
    }

    /// The head's place, 0 to 1 across the window. For drawing.
    pub fn position(&self) -> f32 {
        self.head as f32
    }

    /// One sample read at the head, after it has taken a step towards
    /// `position` (0 to 1 across `source`, the trimmed window).
    #[inline]
    pub fn process(&mut self, source: &[f32], position: f32) -> f32 {
        if source.len() < 2 {
            return 0.0;
        }
        let last = (source.len() - 1) as f64;
        let to = f64::from(clean(position));
        // Close enough is there: a held position ends in an exact zero speed.
        let step = if (to - self.head).abs() < 1e-9 {
            to - self.head
        } else {
            (to - self.head) * self.coef
        };
        self.head += step;
        let speed = (step * last) as f32;

        let at = self.head * last;
        let i = (at as usize).min(source.len() - 2);
        let frac = (at - i as f64) as f32;
        let s = source[i] + (source[i + 1] - source[i]) * frac;
        s * tape_gain(speed)
    }
}

fn clean(position: f32) -> f32 {
    if position.is_finite() {
        position.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (core::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn a_held_position_decays_to_silence() {
        let src = tone(48_000);
        let mut s = Scrub::new(48_000.0);
        s.snap(0.0);
        for _ in 0..4_800 {
            s.process(&src, 0.5);
        }
        // Two seconds on, the head has arrived and nothing is moving it.
        for _ in 0..96_000 {
            s.process(&src, 0.5);
        }
        for _ in 0..1_000 {
            assert_eq!(s.process(&src, 0.5), 0.0);
        }
    }

    #[test]
    fn stays_finite_and_in_range_whatever_the_position_does() {
        let src = tone(9_600);
        let mut s = Scrub::new(48_000.0);
        let mut rng = 0x1234_5678u32;
        let mut pos = 0.0;
        for n in 0..96_000 {
            if n % 37 == 0 {
                rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                pos = (rng >> 8) as f32 / (1u32 << 24) as f32 * 1.4 - 0.2;
            }
            let v = s.process(&src, pos);
            assert!(v.is_finite() && v.abs() <= 1.0 + 1e-5, "{v} at {n}");
        }
        assert!(s.process(&src, f32::NAN).is_finite());
    }

    #[test]
    fn too_short_a_source_is_silent() {
        let mut s = Scrub::new(48_000.0);
        assert_eq!(s.process(&[], 0.5), 0.0);
        assert_eq!(s.process(&[1.0], 0.5), 0.0);
    }

    /// The tone at a fractional sample, to compare the head's reading with.
    fn tone_at(at: f32) -> f32 {
        (core::f32::consts::TAU * 220.0 * at / 48_000.0).sin()
    }

    /// The head sits `SMOOTH_MS` behind a moving position, the way a one-pole
    /// lags a ramp, so a ramp over the window's own duration is the forward
    /// pass delayed by that much.
    const LAG: f32 = SMOOTH_MS * 0.001 * 48_000.0;

    #[test]
    fn a_ramp_over_the_windows_own_duration_reproduces_the_forward_pass() {
        let n = 96_000;
        let src = tone(n);
        let mut s = Scrub::new(48_000.0);
        s.snap(0.0);
        let mut worst = 0.0f32;
        for i in 0..n {
            let v = s.process(&src, i as f32 / (n - 1) as f32);
            // Past the start-up the head moves at one sample a sample.
            if i > 12 * LAG as usize {
                worst = worst.max((v - tone_at(i as f32 - LAG)).abs());
            }
        }
        assert!(
            worst < 0.03,
            "forward ramp strayed from the pass by {worst}"
        );
    }

    #[test]
    fn the_ramp_reversed_plays_backwards() {
        let n = 96_000;
        let src = tone(n);
        let mut s = Scrub::new(48_000.0);
        s.snap(1.0);
        let mut worst = 0.0f32;
        for i in 0..n {
            let v = s.process(&src, 1.0 - i as f32 / (n - 1) as f32);
            if i > 12 * LAG as usize {
                worst = worst.max((v - tone_at((n - 1 - i) as f32 + LAG)).abs());
            }
        }
        assert!(
            worst < 0.03,
            "reverse ramp strayed from the pass by {worst}"
        );
    }
}
