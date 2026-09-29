//! Chorus.
//!
//! Two short delay lines, one a channel, each read at a point an LFO sweeps
//! back and forth. A moving read point bends the pitch of the delayed copy
//! slightly, and a copy that is a little sharp and a little flat against the
//! original reads as several voices rather than one. The two LFOs sit a
//! quarter turn apart, so the channels never swing together and the wet
//! signal widens the image.
//!
//! The wet path is the delayed copies alone. Dry and wet are cross-faded by
//! `mix`, and at zero the node is a bit-exact bypass.

use crate::smooth::OnePole;

/// The centre of the sweep, in milliseconds. Long enough to comb the dry
/// signal audibly, short enough to stay a chorus and not an echo.
const CENTRE_MS: f32 = 12.0;
/// The widest the read point strays from the centre, at full depth.
const SWEEP_MS: f32 = 8.0;
/// Room for the centre plus the widest sweep, with a sample either side for
/// the interpolator.
const MAX_MS: f32 = CENTRE_MS + SWEEP_MS + 2.0;

#[derive(Debug, Clone, Copy)]
pub struct ChorusParams {
    /// LFO rate in hertz.
    pub rate: f32,
    /// How far the read point sweeps, 0 to 1.
    pub depth: f32,
    /// Dry to wet, 0 to 1.
    pub mix: f32,
}

impl Default for ChorusParams {
    fn default() -> Self {
        Self {
            rate: 0.8,
            depth: 0.5,
            mix: 0.0,
        }
    }
}

pub struct Chorus {
    /// Interleaved frames, sized once. `write` is the newest frame.
    line: Vec<f32>,
    write: usize,
    frames: usize,
    phase: f32,
    sample_rate: f32,
    rate: OnePole,
    depth: OnePole,
    mix: OnePole,
}

impl Chorus {
    pub fn new(sample_rate: f32) -> Self {
        let frames = (MAX_MS * 0.001 * sample_rate).ceil() as usize + 2;
        let d = ChorusParams::default();
        let smoother = |ms: f32, v: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        Self {
            line: vec![0.0; frames * 2],
            write: 0,
            frames,
            phase: 0.0,
            sample_rate,
            // Depth moves the read point, so a step in it is a pitch jump.
            rate: smoother(20.0, d.rate),
            depth: smoother(20.0, d.depth),
            mix: smoother(20.0, d.mix),
        }
    }

    /// One channel's read at `delay` frames behind the newest, linearly
    /// interpolated.
    #[inline]
    fn tap(&self, ch: usize, delay: f32) -> f32 {
        let d = delay.clamp(1.0, (self.frames - 2) as f32);
        let whole = d.floor();
        let frac = d - whole;
        let a = (self.write + self.frames - whole as usize) % self.frames;
        let b = (a + self.frames - 1) % self.frames;
        self.line[a * 2 + ch] * (1.0 - frac) + self.line[b * 2 + ch] * frac
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &ChorusParams) -> (f32, f32) {
        let rate = self.rate.process(p.rate.clamp(0.0, 20.0));
        let depth = self.depth.process(p.depth.clamp(0.0, 1.0));
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Snap the last of a fade to a true zero, as the ring modulator does:
        // a section switched off has to be bit-exact with no chorus at all.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        self.write = (self.write + 1) % self.frames;
        self.line[self.write * 2] = l;
        self.line[self.write * 2 + 1] = r;

        let sweep = SWEEP_MS * depth * 0.001 * self.sample_rate;
        let centre = CENTRE_MS * 0.001 * self.sample_rate;
        let lfo_l = (core::f32::consts::TAU * self.phase).sin();
        let lfo_r = (core::f32::consts::TAU * self.phase).cos();
        self.phase += rate / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }

        let wet_l = self.tap(0, centre + sweep * lfo_l);
        let wet_r = self.tap(1, centre + sweep * lfo_r);
        (l * (1.0 - mix) + wet_l * mix, r * (1.0 - mix) + wet_r * mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn tone(i: usize) -> f32 {
        (core::f32::consts::TAU * 220.0 * i as f32 / SR).sin() * 0.5
    }

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        let mut c = Chorus::new(SR);
        let on = ChorusParams {
            mix: 1.0,
            ..Default::default()
        };
        let off = ChorusParams {
            mix: 0.0,
            ..Default::default()
        };
        for i in 0..4_800 {
            c.process(tone(i), tone(i), &on);
        }
        for i in 0..48_000 {
            c.process(tone(i), tone(i), &off);
        }
        for _ in 0..1_000 {
            assert_eq!(c.process(0.5, -0.25, &off), (0.5, -0.25));
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut c = Chorus::new(SR);
        let p = ChorusParams {
            mix: 1.0,
            depth: 1.0,
            ..Default::default()
        };
        for _ in 0..10_000 {
            assert_eq!(c.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn wet_changes_the_signal() {
        let mut c = Chorus::new(SR);
        let p = ChorusParams {
            mix: 1.0,
            ..Default::default()
        };
        let mut diff = 0.0f32;
        for i in 0..9_600 {
            let x = tone(i);
            let (l, _) = c.process(x, x, &p);
            if i > 4_800 {
                diff = diff.max((l - x).abs());
            }
        }
        assert!(diff > 0.1, "wet should differ from dry, max diff {diff}");
    }

    #[test]
    fn the_channels_decorrelate() {
        // Quadrature LFOs: the same mono input must come out different on
        // each side, or the chorus is not widening anything.
        let mut c = Chorus::new(SR);
        let p = ChorusParams {
            mix: 1.0,
            depth: 1.0,
            rate: 2.0,
        };
        let mut apart = 0.0f32;
        for i in 0..48_000 {
            let x = tone(i);
            let (l, r) = c.process(x, x, &p);
            if i > 4_800 {
                apart = apart.max((l - r).abs());
            }
        }
        assert!(apart > 0.05, "left and right stayed together: {apart}");
    }

    #[test]
    fn output_stays_in_range_at_the_extremes() {
        // A delayed, interpolated copy of a bounded signal is bounded, so a
        // full-scale input at full depth and the fastest rate stays inside it.
        let mut c = Chorus::new(SR);
        let p = ChorusParams {
            mix: 1.0,
            depth: 1.0,
            rate: 20.0,
        };
        for i in 0..96_000 {
            let x = if i % 200 < 100 { 1.0 } else { -1.0 };
            let (l, r) = c.process(x, -x, &p);
            assert!(l.abs() <= 1.0 + 1e-5 && r.abs() <= 1.0 + 1e-5, "{l} {r}");
        }
    }

    #[test]
    fn a_swept_parameter_does_not_jump() {
        // Depth and rate move the read point, so an unsmoothed step in either
        // would click. A slow sine through a steady chorus is the baseline.
        let worst = |sweep: bool| {
            let mut c = Chorus::new(SR);
            let mut prev = 0.0f32;
            let mut worst = 0.0f32;
            for i in 0..96_000 {
                let (depth, rate) = if sweep && (i / 4_800) % 2 == 0 {
                    (1.0, 6.0)
                } else {
                    (0.1, 0.5)
                };
                let p = ChorusParams {
                    depth,
                    rate,
                    mix: 1.0,
                };
                let (l, _) = c.process(tone(i), tone(i), &p);
                if i > 4_800 {
                    worst = worst.max((l - prev).abs());
                }
                prev = l;
            }
            worst
        };
        let (steady, swept) = (worst(false), worst(true));
        assert!(
            swept < steady * 3.0 + 0.02,
            "a parameter step jumped by {swept} against a steady {steady}"
        );
    }

    #[test]
    fn works_at_other_sample_rates() {
        for sr in [22_050.0, 44_100.0, 96_000.0, 192_000.0] {
            let mut c = Chorus::new(sr);
            let p = ChorusParams {
                mix: 1.0,
                depth: 1.0,
                rate: 20.0,
            };
            for i in 0..10_000 {
                let x = (i as f32 * 0.05).sin();
                let (l, r) = c.process(x, x, &p);
                assert!(l.is_finite() && r.is_finite());
            }
        }
    }
}
