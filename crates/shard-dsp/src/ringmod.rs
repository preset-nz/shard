//! Ring modulation.
//!
//! Multiply the signal by a sine. The output holds the sum and difference of
//! every input partial with the carrier, and none of the originals. Those sums
//! and differences are not harmonically related to the input, so the result
//! turns inharmonic and bell-like. That is the industrial clang, and it is
//! about ten lines of code.
//!
//! Low carrier frequencies read as tremolo. Push past roughly 30 Hz and the
//! sidebands separate into their own pitches.

use crate::smooth::OnePole;

#[derive(Debug, Clone, Copy)]
pub struct RingModParams {
    /// Carrier frequency in hertz.
    pub freq: f32,
    /// Dry to wet, 0 to 1.
    pub mix: f32,
}

impl Default for RingModParams {
    fn default() -> Self {
        Self {
            freq: 140.0,
            mix: 0.0,
        }
    }
}

pub struct RingMod {
    phase: f32,
    sample_rate: f32,
    /// Sweeping the carrier without smoothing steps audibly, because the
    /// sidebands move with it.
    freq: OnePole,
    mix: OnePole,
}

impl RingMod {
    pub fn new(sample_rate: f32) -> Self {
        let mut freq = OnePole::new();
        let mut mix = OnePole::new();
        freq.set_time(20.0, sample_rate);
        mix.set_time(20.0, sample_rate);
        let d = RingModParams::default();
        freq.reset(d.freq);
        mix.reset(d.mix);
        Self {
            phase: 0.0,
            sample_rate,
            freq,
            mix,
        }
    }

    /// Both channels share one carrier. Two would cause the stereo image to
    /// drift apart as the phases diverge.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &RingModParams) -> (f32, f32) {
        let freq = self.freq.process(p.freq.clamp(0.0, 20_000.0));
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Snap the last of a fade to a true zero. A one-pole only approaches
        // its target, and a section switched off in the engine has to be
        // bit-exact with no ring modulation at all, not a millionth of it. The
        // step this leaves is under 2e-5 of the signal.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        let carrier = (core::f32::consts::TAU * self.phase).sin();
        self.phase += freq / self.sample_rate;
        // Wrapping the phase rather than letting it grow keeps precision
        // steady. An f32 phase counter drifts audibly after a few minutes.
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }

        (
            l * (1.0 - mix) + l * carrier * mix,
            r * (1.0 - mix) + r * carrier * mix,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        // The engine's section switch relies on this: the smoothed mix has to
        // arrive at a true zero, or "off" leaves a trace of modulation behind.
        let mut rm = RingMod::new(48_000.0);
        let on = RingModParams {
            mix: 1.0,
            ..Default::default()
        };
        let off = RingModParams {
            mix: 0.0,
            ..Default::default()
        };
        for _ in 0..4_800 {
            rm.process(0.5, 0.5, &on);
        }
        for _ in 0..48_000 {
            rm.process(0.5, 0.5, &off);
        }
        for _ in 0..1_000 {
            assert_eq!(rm.process(0.5, -0.25, &off), (0.5, -0.25));
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn dry_at_zero_mix() {
        let mut rm = RingMod::new(48_000.0);
        let p = RingModParams {
            freq: 200.0,
            mix: 0.0,
        };
        // Let the smoothers settle first; they start at the defaults.
        for _ in 0..4_800 {
            rm.process(0.0, 0.0, &p);
        }
        for i in 0..1000 {
            let x = (i as f32 * 0.01).sin();
            let (l, r) = rm.process(x, x, &p);
            assert!((l - x).abs() < 1e-4, "expected dry, got {l} vs {x}");
            assert!((r - x).abs() < 1e-4);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut rm = RingMod::new(48_000.0);
        let p = RingModParams {
            freq: 300.0,
            mix: 1.0,
        };
        for _ in 0..10_000 {
            assert_eq!(rm.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn wet_changes_the_signal() {
        let mut rm = RingMod::new(48_000.0);
        let p = RingModParams {
            freq: 300.0,
            mix: 1.0,
        };
        for _ in 0..4_800 {
            rm.process(0.0, 0.0, &p);
        }
        let mut diff = 0.0f32;
        for i in 0..4_800 {
            let x = (core::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin();
            let (l, _) = rm.process(x, x, &p);
            diff = diff.max((l - x).abs());
        }
        assert!(
            diff > 0.1,
            "wet output should differ from dry, max diff {diff}"
        );
    }

    #[test]
    fn output_stays_bounded_by_the_input() {
        // Multiplying by a sine cannot make anything louder.
        let mut rm = RingMod::new(48_000.0);
        let p = RingModParams {
            freq: 1_000.0,
            mix: 1.0,
        };
        for i in 0..48_000 {
            let x = (i as f32 * 0.05).sin();
            let (l, r) = rm.process(x, x, &p);
            assert!(l.abs() <= x.abs() + 1e-5, "{l} exceeded {x}");
            assert!(r.abs() <= x.abs() + 1e-5);
        }
    }

    #[test]
    fn zero_hz_carrier_does_not_blow_up() {
        let mut rm = RingMod::new(48_000.0);
        let p = RingModParams {
            freq: 0.0,
            mix: 1.0,
        };
        for i in 0..48_000 {
            let x = (i as f32 * 0.05).sin();
            let (l, r) = rm.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite());
        }
    }

    #[test]
    fn phase_stays_stable_over_a_long_run() {
        // Ten minutes of audio. A phase counter that grows without wrapping
        // loses precision and the carrier drifts flat.
        let mut rm = RingMod::new(48_000.0);
        let p = RingModParams {
            freq: 440.0,
            mix: 1.0,
        };
        let mut early = Vec::new();
        let mut late = Vec::new();
        for i in 0..48_000 * 600 {
            let (l, _) = rm.process(1.0, 1.0, &p);
            if (48_000..48_000 + 2_000).contains(&i) {
                early.push(l);
            }
            if (48_000 * 599..48_000 * 599 + 2_000).contains(&i) {
                late.push(l);
            }
        }
        let (a, b) = (rms(&early), rms(&late));
        assert!(
            (a - b).abs() < 0.01,
            "carrier amplitude drifted: {a} then {b}"
        );
    }
}
