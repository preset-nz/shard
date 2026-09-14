//! Plain playback.
//!
//! The sample as it is, at rate one, looping. No grains, no shaping. Under
//! the steps it plays once instead: a step starts a pass, the pass stops at
//! the edge of the window, and it stays silent until the next step.
//!
//! This exists to make the instrument learnable. Pressing play and hearing the
//! material untouched gives you the reference; every control then has an
//! audible before and after. Without it, granular parameters are adjustments
//! to something you never heard in the first place.

/// A short crossfade at the loop point. Without it the wrap is a step
/// discontinuity and you hear a click once per loop.
const FADE_MS: f32 = 8.0;

/// Copy, so a retrigger can hand the pass in flight to a tail that fades out
/// while a new pass fades in. Three fields; nothing to allocate.
#[derive(Clone, Copy)]
pub struct Player {
    pos: f32,
    sample_rate: f32,
    /// A one-shot pass has reached the edge of the window. Silent until the
    /// next rewind, or until it is asked to loop again.
    played_out: bool,
}

impl Player {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            pos: 0.0,
            sample_rate,
            played_out: false,
        }
    }

    /// How long the fade at the start of a pass is, in samples. A tail
    /// fading out over the same time makes a retrigger an equal-power
    /// crossfade.
    pub fn seam_samples(&self) -> f32 {
        (FADE_MS * 0.001 * self.sample_rate).max(1.0)
    }

    /// Start a new pass from the top.
    pub fn rewind(&mut self) {
        self.pos = 0.0;
        self.played_out = false;
    }

    /// Start a new pass from the far end, for a tape running backwards. From
    /// the top it would leave the window on its first sample.
    pub fn rewind_to_end(&mut self, source_len: usize) {
        self.pos = source_len.saturating_sub(1) as f32;
        self.played_out = false;
    }

    /// Silence the pass where it is, as though it had played out.
    pub fn finish(&mut self) {
        self.played_out = true;
    }

    /// Whether a pass is sounding: always while looping, and under the steps
    /// from a rewind to the edge of the window.
    pub fn sounding(&self) -> bool {
        !self.played_out
    }

    /// Samples elapsed into the current pass. The envelope's clock.
    pub fn elapsed(&self) -> f32 {
        self.pos
    }

    /// Where the playhead is, 0 to 1. For drawing.
    pub fn position(&self, source_len: usize) -> f32 {
        if source_len < 2 {
            0.0
        } else {
            (self.pos / (source_len - 1) as f32).clamp(0.0, 1.0)
        }
    }

    /// `speed` is a multiplier on tape time: 1 is normal, 0 is stopped, and a
    /// negative value runs the reel backwards. Fractional values are the whole
    /// point — a tape slowing to a halt spends most of its time between the
    /// two, and that is where the pitch drops and the sound garbles.
    ///
    /// Not `looping`, the pass ends where it leaves the window, at either
    /// edge, and returns silence from then on. Both edges already fade to
    /// zero, so the end of a pass needs no fade of its own.
    #[inline]
    pub fn process(&mut self, source: &[f32], speed: f32, looping: bool) -> f32 {
        if source.len() < 2 {
            return 0.0;
        }
        if self.played_out {
            if !looping {
                return 0.0;
            }
            // Looping again picks up from the edge it stopped at, where the
            // seam's fade is at zero, so it wraps in without a click.
            self.played_out = false;
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

        // Wrap by subtracting the span rather than snapping to zero, so a
        // fractional speed does not lose its remainder at every loop and drift
        // out of step with the envelope.
        let span = last as f32;
        self.pos += speed;
        if self.pos > span || self.pos < 0.0 {
            if !looping {
                self.played_out = true;
            } else if self.pos > span {
                self.pos -= span;
            } else {
                self.pos += span;
            }
        }
        // A speed large enough to jump the whole buffer in one sample would
        // leave the position outside it; clamp rather than trust the caller.
        self.pos = self.pos.clamp(0.0, span);

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
            assert_eq!(p.process(&[], 1.0, true), 0.0);
        }
    }

    #[test]
    fn plays_and_loops() {
        let src = tone(4_800);
        let mut p = Player::new(48_000.0);
        let mut peak: f32 = 0.0;
        // Three times round.
        for _ in 0..14_400 {
            peak = peak.max(p.process(&src, 1.0, true).abs());
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
        let mut prev = p.process(&src, 1.0, true);
        let mut worst = 0.0f32;
        for _ in 0..24_000 {
            let s = p.process(&src, 1.0, true);
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
            let s = p.process(&src, 1.0, true);
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
            p.process(&src, 1.0, true);
        }
        let mid = p.position(src.len());
        assert!(mid > 0.4 && mid < 0.6, "mid was {mid}");
        for _ in 0..600 {
            p.process(&src, 1.0, true);
        }
        assert!(p.position(src.len()) < 0.2, "should have wrapped");
    }

    #[test]
    fn rewind_returns_to_the_start() {
        let src = tone(4_800);
        let mut p = Player::new(48_000.0);
        for _ in 0..2_000 {
            p.process(&src, 1.0, true);
        }
        p.rewind();
        assert_eq!(p.position(src.len()), 0.0);
    }

    #[test]
    fn a_one_shot_pass_plays_once_then_stays_silent() {
        let src = tone(4_800);
        for (speed, backwards) in [(1.0, false), (-1.0, true)] {
            let mut p = Player::new(48_000.0);
            if backwards {
                p.rewind_to_end(src.len());
            }
            let mut peak = 0.0f32;
            for _ in 0..4_700 {
                peak = peak.max(p.process(&src, speed, false).abs());
            }
            assert!(peak > 0.5, "the pass never sounded: peak {peak}");
            assert!(p.sounding());
            // Past the edge, and well past where a loop would have wrapped.
            for _ in 0..10_000 {
                p.process(&src, speed, false);
            }
            assert!(!p.sounding(), "speed {speed} should have played out");
            assert_eq!(p.process(&src, speed, false), 0.0);
            // A rewind starts the next pass, and looping again resumes.
            p.rewind();
            assert!(p.sounding());
            p.finish();
            p.process(&src, 1.0, true);
            assert!(p.sounding(), "looping picks a played-out pass back up");
        }
    }
}
