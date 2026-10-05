//! Bitcrush and sample-rate reduction.
//!
//! Two separate destructions that belong in one box because they are the same
//! gesture: throw away resolution. The bit crusher coarsens *amplitude* — how
//! finely a sample's value is described. The rate reducer coarsens *time* —
//! how often a new value is taken at all. Amplitude loss reads as grit and a
//! noise floor that moves with the signal; time loss reads as aliasing, the
//! metallic ring of early samplers.
//!
//! Deliberately not anti-aliased. The aliasing *is* the effect; filtering it
//! away would leave a quiet, polite version of a rude thing.
//!
//! `mix` arrives already smoothed by the chain, which is why nothing here
//! smooths it. See `Chain::front`.

use crate::smooth::OnePole;

/// The engine's own smoothing time for the two character controls. Sweeping
/// either without it steps audibly, because both change the shape of every
/// sample rather than just its level.
const SMOOTH_MS: f32 = 20.0;

#[derive(Debug, Clone, Copy)]
pub struct CrushParams {
    /// Word length in bits. Continuous rather than whole numbers so it can be
    /// swept and modulated; the audible steps come from the quantiser, not
    /// from the control.
    pub bits: f32,
    /// Sample-and-hold rate in hertz. At or above the engine's own rate this
    /// is transparent.
    pub rate: f32,
    /// Dry to crushed, 0 to 1. At zero the node is bit-exact bypass.
    pub mix: f32,
}

impl Default for CrushParams {
    /// Neutral: full word length, no holding, no mix. Multiplies by nothing
    /// and returns its input unchanged.
    fn default() -> Self {
        Self {
            bits: 16.0,
            rate: 48_000.0,
            mix: 0.0,
        }
    }
}

pub struct Crush {
    sample_rate: f32,
    /// Counts up by one reduced-rate period per output sample. A new value is
    /// taken each time it passes unity.
    phase: f32,
    held_l: f32,
    held_r: f32,
    bits: OnePole,
    rate: OnePole,
}

impl Crush {
    pub fn new(sample_rate: f32) -> Self {
        let mut bits = OnePole::new();
        let mut rate = OnePole::new();
        bits.set_time(SMOOTH_MS, sample_rate);
        rate.set_time(SMOOTH_MS, sample_rate);
        let d = CrushParams::default();
        bits.reset(d.bits);
        rate.reset(d.rate);
        Self {
            sample_rate,
            // Starts due, so the first sample through is taken rather than
            // held over from the zero the buffers start at.
            phase: 1.0,
            held_l: 0.0,
            held_r: 0.0,
            bits,
            rate,
        }
    }

    /// Forget the held samples, so nothing of the old signal is repeated. The
    /// hold clock is a timer, not audio, and keeps running.
    pub fn reset(&mut self) {
        self.held_l = 0.0;
        self.held_r = 0.0;
    }

    /// Both channels share one hold clock. Two would drift apart and smear the
    /// stereo image, the same reason the ring modulator shares its carrier.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &CrushParams) -> (f32, f32) {
        let bits = self.bits.process(p.bits.clamp(1.0, 16.0));
        let rate = self.rate.process(p.rate.clamp(1.0, 192_000.0));

        // Sample and hold. The accumulator advances by one output sample's
        // worth of the reduced rate, so at or above the engine's rate the step
        // is at least one and every sample is new — transparent rather than
        // switched off, which keeps the control continuous at the top.
        self.phase += rate / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.held_l = l;
            self.held_r = r;
        }

        let mix = p.mix.clamp(0.0, 1.0);
        if mix <= 0.0 {
            // Exactly bypassed, not nearly. The null test depends on this, and
            // so does the promise that a sample "starts clean".
            return (l, r);
        }

        // Quantise the held value, so the coarse value persists across the
        // hold rather than being recomputed into something slightly different
        // each sample. That is the order real hardware imposes and it is the
        // one that sounds like it.
        let step = 2.0 / bits.exp2();
        let q = |s: f32| (s / step).round() * step;

        // Written as two scaled terms rather than `l + (crushed - l) * mix`,
        // which looks equivalent and is not: the lerp form is exact at mix
        // zero but rounds at mix one, leaving every held sample slightly
        // different from its neighbour. That quietly destroys the one property
        // the rate reducer exists to have.
        (
            l * (1.0 - mix) + q(self.held_l) * mix,
            r * (1.0 - mix) + q(self.held_r) * mix,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn tone(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (core::f32::consts::TAU * 220.0 * i as f32 / SR).sin())
            .collect()
    }

    #[test]
    fn neutral_is_bit_exact_bypass() {
        // The null test the roadmap asks of every effect: at neutral settings
        // the node must cancel against a bypass path exactly, not nearly. A
        // near-miss means the dry path has picked up a delay or a gain.
        let mut c = Crush::new(SR);
        let p = CrushParams::default();
        for s in tone(4_800) {
            let (l, r) = c.process(s, -s, &p);
            assert_eq!(l, s, "left changed at neutral");
            assert_eq!(r, -s, "right changed at neutral");
        }
    }

    #[test]
    fn a_mix_of_zero_bypasses_whatever_the_other_controls_say() {
        // Mix is the gate, so an envelope on it can hold the node open at
        // zero while bits and rate sit at their most destructive.
        let mut c = Crush::new(SR);
        let p = CrushParams {
            bits: 1.0,
            rate: 200.0,
            mix: 0.0,
        };
        for s in tone(4_800) {
            assert_eq!(c.process(s, s, &p).0, s);
        }
    }

    /// Run the node with these settings until its smoothers have settled, so a
    /// measurement afterwards sees the effect rather than the ramp into it.
    /// Half a second is roughly twenty-five time constants.
    fn settled(p: &CrushParams) -> Crush {
        let mut c = Crush::new(SR);
        for s in tone(24_000) {
            c.process(s, s, p);
        }
        c
    }

    #[test]
    fn reducing_the_rate_holds_values() {
        // 1 kHz against 48 kHz is a run of 48 identical samples, so a second
        // of audio should carry about a thousand distinct values rather than
        // forty-eight thousand.
        let p = CrushParams {
            bits: 16.0,
            rate: 1_000.0,
            mix: 1.0,
        };
        let mut c = settled(&p);
        let mut runs = 0;
        let mut prev = f32::NAN;
        for s in tone(48_000) {
            let (l, _) = c.process(s, s, &p);
            if l != prev {
                runs += 1;
                prev = l;
            }
        }
        assert!(
            (800..1_300).contains(&runs),
            "expected ~1000 held values, got {runs}"
        );
    }

    #[test]
    fn reducing_the_bits_reduces_the_distinct_values() {
        let p = CrushParams {
            bits: 2.0,
            rate: 192_000.0,
            mix: 1.0,
        };
        let mut c = settled(&p);
        let mut seen: Vec<f32> = Vec::new();
        for s in tone(48_000) {
            let (l, _) = c.process(s, s, &p);
            // A loose comparison on purpose. `bits` is continuous and
            // smoothed, so the quantiser's step is still creeping in the last
            // decimal place long after the level count has settled.
            if !seen.iter().any(|v| (v - l).abs() < 1e-3) {
                seen.push(l);
            }
        }
        // Two bits over a signal that never reaches full scale: a handful of
        // levels, nothing like a continuous waveform.
        assert!(seen.len() <= 8, "two bits gave {} levels", seen.len());
    }

    #[test]
    fn output_stays_finite_and_bounded() {
        let mut c = Crush::new(SR);
        for (bits, rate, mix) in [
            (1.0, 1.0, 1.0),
            (16.0, 192_000.0, 1.0),
            (1.0, 200.0, 0.5),
            (0.0, 0.0, 1.0),
            (99.0, 1e9, 2.0),
        ] {
            let p = CrushParams { bits, rate, mix };
            for s in tone(4_800) {
                let (l, r) = c.process(s, s, &p);
                assert!(
                    l.is_finite() && r.is_finite(),
                    "non-finite at {bits}/{rate}"
                );
                assert!(l.abs() <= 2.0 && r.abs() <= 2.0, "runaway at {bits}/{rate}");
            }
        }
    }

    #[test]
    fn both_channels_hold_together() {
        // One shared clock, so a stereo signal cannot smear apart.
        let mut c = Crush::new(SR);
        let p = CrushParams {
            bits: 8.0,
            rate: 800.0,
            mix: 1.0,
        };
        for s in tone(24_000) {
            let (l, r) = c.process(s, s, &p);
            assert_eq!(l, r, "channels diverged");
        }
    }

    #[test]
    fn reset_forgets_the_held_samples() {
        // A very low rate makes the hold last as long as possible: the held
        // value would otherwise sound for the whole period.
        let p = CrushParams {
            bits: 4.0,
            rate: 2.0,
            mix: 1.0,
        };
        let mut c = Crush::new(SR);
        for s in tone(48_000) {
            c.process(s, -s, &p);
        }
        c.reset();
        // Silence is the held value too, whenever the clock next fires.
        for i in 0..48_000 {
            assert_eq!(c.process(0.0, 0.0, &p), (0.0, 0.0), "at {i}");
        }
    }

    #[test]
    fn reset_on_a_fresh_crush_changes_nothing() {
        let p = CrushParams {
            bits: 6.0,
            rate: 3_000.0,
            mix: 1.0,
        };
        let mut fresh = Crush::new(SR);
        let mut reset = Crush::new(SR);
        reset.reset();
        for s in tone(4_800) {
            assert_eq!(fresh.process(s, s, &p), reset.process(s, s, &p));
        }
    }
}
