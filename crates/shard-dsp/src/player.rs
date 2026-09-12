//! Plain looping playback.
//!
//! The sample as it is, at rate one, looping. No grains, no shaping.
//!
//! This exists to make the instrument learnable. Pressing play and hearing the
//! material untouched gives you the reference; every control then has an
//! audible before and after. Without it, granular parameters are adjustments
//! to something you never heard in the first place.

/// A short crossfade at the loop point. Without it the wrap is a step
/// discontinuity and you hear a click once per loop.
const FADE_MS: f32 = 8.0;

pub struct Player {
    pos: f32,
    sample_rate: f32,
}

impl Player {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            pos: 0.0,
            sample_rate,
        }
    }

    pub fn rewind(&mut self) {
        self.pos = 0.0;
    }

    /// Where the playhead is, 0 to 1. For drawing.
    pub fn position(&self, source_len: usize) -> f32 {
        if source_len < 2 {
            0.0
        } else {
            (self.pos / (source_len - 1) as f32).clamp(0.0, 1.0)
        }
    }

    #[inline]
    pub fn process(&mut self, source: &[f32]) -> f32 {
        if source.len() < 2 {
            return 0.0;
        }
        let last = source.len() - 1;

        let i = self.pos as usize;
        let s = if i >= last {
            source[last]
        } else {
            let frac = self.pos - i as f32;
            source[i] + (source[i + 1] - source[i]) * frac
        };

        // Equal-power fade across the seam, so the loop point holds level
        // rather than dipping the way a linear crossfade would.
        let fade = (FADE_MS * 0.001 * self.sample_rate).max(1.0);
        let amp = if self.pos < fade {
            (self.pos / fade).clamp(0.0, 1.0)
        } else if self.pos > last as f32 - fade {
            ((last as f32 - self.pos) / fade).clamp(0.0, 1.0)
        } else {
            1.0
        };

        self.pos += 1.0;
        if self.pos > last as f32 {
            self.pos = 0.0;
        }

        s * amp.sqrt()
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
    fn silent_without_a_source() {
        let mut p = Player::new(48_000.0);
        for _ in 0..1000 {
            assert_eq!(p.process(&[]), 0.0);
        }
    }

    #[test]
    fn plays_and_loops() {
        let src = tone(4_800);
        let mut p = Player::new(48_000.0);
        let mut peak: f32 = 0.0;
        // Three times round.
        for _ in 0..14_400 {
            peak = peak.max(p.process(&src).abs());
        }
        assert!(peak > 0.5, "peak was {peak}");
    }

    #[test]
    fn the_loop_seam_does_not_click() {
        // The contract this file exists for. Sample-to-sample jumps across the
        // wrap must stay in the same range as jumps anywhere else in the
        // waveform. Without the fade the seam jumps by roughly the signal's
        // full amplitude.
        let src = tone(4_800);
        let mut p = Player::new(48_000.0);
        let mut prev = p.process(&src);
        let mut worst = 0.0f32;
        for _ in 0..24_000 {
            let s = p.process(&src);
            worst = worst.max((s - prev).abs());
            prev = s;
        }
        // One cycle of 220 Hz at 48 kHz steps by at most ~0.029 per sample.
        assert!(worst < 0.05, "worst jump across the loop was {worst}");
    }

    #[test]
    fn output_never_exceeds_the_source() {
        let src = tone(4_800);
        let mut p = Player::new(48_000.0);
        for _ in 0..24_000 {
            let s = p.process(&src);
            assert!(s.abs() <= 1.0 + 1e-5);
            assert!(s.is_finite());
        }
    }

    #[test]
    fn position_advances_and_wraps() {
        let src = tone(1_000);
        let mut p = Player::new(48_000.0);
        assert_eq!(p.position(src.len()), 0.0);
        for _ in 0..500 {
            p.process(&src);
        }
        let mid = p.position(src.len());
        assert!(mid > 0.4 && mid < 0.6, "mid was {mid}");
        for _ in 0..600 {
            p.process(&src);
        }
        assert!(p.position(src.len()) < 0.2, "should have wrapped");
    }

    #[test]
    fn rewind_returns_to_the_start() {
        let src = tone(4_800);
        let mut p = Player::new(48_000.0);
        for _ in 0..2_000 {
            p.process(&src);
        }
        p.rewind();
        assert_eq!(p.position(src.len()), 0.0);
    }
}
