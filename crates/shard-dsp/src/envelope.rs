//! A looping attack-decay envelope.
//!
//! There is no note input yet, so this retriggers on its own period rather
//! than on a gate. That turns a constant cloud into something with shape:
//! swells, pulses, or hard percussive hits depending on how you set it.
//!
//! When the sequencer arrives this becomes the same envelope driven by an
//! event instead of a timer, which is why the attack and decay live here and
//! not in whatever is currently doing the triggering.

use crate::smooth::OnePole;

#[derive(Debug, Clone, Copy)]
pub struct EnvParams {
    /// Depth. At zero the envelope is bypassed entirely and costs nothing.
    pub amount: f32,
    /// Seconds between retriggers.
    pub period: f32,
    pub attack_ms: f32,
    pub decay_ms: f32,
}

impl Default for EnvParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            period: 1.0,
            attack_ms: 10.0,
            decay_ms: 400.0,
        }
    }
}

pub struct Envelope {
    sample_rate: f32,
    /// Samples since the last retrigger.
    age: f32,
    period_samples: f32,
    /// Smooths the final gain, so changing attack or decay mid-flight does not
    /// step. The envelope shape itself is not smoothed — a fast attack has to
    /// stay fast.
    out: OnePole,
}

impl Envelope {
    pub fn new(sample_rate: f32) -> Self {
        let mut out = OnePole::new();
        out.set_time(2.0, sample_rate);
        out.reset(1.0);
        Self {
            sample_rate,
            age: 0.0,
            period_samples: sample_rate,
            out,
        }
    }

    pub fn retrigger(&mut self) {
        self.age = 0.0;
    }

    /// A multiplier, 0 to 1. At `amount` zero this is always exactly 1, so the
    /// envelope is genuinely bypassed rather than nearly so.
    #[inline]
    pub fn process(&mut self, p: &EnvParams) -> f32 {
        let amount = p.amount.clamp(0.0, 1.0);
        if amount <= 0.0 {
            self.age = 0.0;
            self.out.reset(1.0);
            return 1.0;
        }

        self.period_samples = (p.period.clamp(0.01, 60.0) * self.sample_rate).max(2.0);
        let attack = (p.attack_ms.clamp(0.0, 20_000.0) * 0.001 * self.sample_rate).max(1.0);
        let decay = (p.decay_ms.clamp(0.0, 60_000.0) * 0.001 * self.sample_rate).max(1.0);

        let shape = if self.age < attack {
            self.age / attack
        } else {
            // Exponential decay, floored so a long decay against a short
            // period does not retrigger from an audible step.
            let t = (self.age - attack) / decay;
            (-4.0 * t).exp()
        };

        self.age += 1.0;
        if self.age >= self.period_samples {
            self.age = 0.0;
        }

        // Depth: at amount 1 the envelope closes fully, below that it only
        // dips. That makes it a swell control at low settings and a gate at
        // high ones, from one knob.
        let gain = 1.0 - amount * (1.0 - shape.clamp(0.0, 1.0));
        self.out.process(gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypassed_at_zero_amount() {
        let mut e = Envelope::new(48_000.0);
        let p = EnvParams::default();
        for _ in 0..48_000 {
            assert_eq!(e.process(&p), 1.0, "must be exactly unity when off");
        }
    }

    #[test]
    fn stays_within_zero_and_one() {
        let mut e = Envelope::new(48_000.0);
        let p = EnvParams {
            amount: 1.0,
            period: 0.25,
            attack_ms: 1.0,
            decay_ms: 100.0,
        };
        for _ in 0..48_000 * 4 {
            let g = e.process(&p);
            assert!((0.0..=1.0).contains(&g), "gain out of range: {g}");
            assert!(g.is_finite());
        }
    }

    #[test]
    fn opens_and_closes_within_a_period() {
        let mut e = Envelope::new(48_000.0);
        let p = EnvParams {
            amount: 1.0,
            period: 1.0,
            attack_ms: 5.0,
            decay_ms: 150.0,
        };
        let mut peak = 0.0f32;
        let mut trough = 1.0f32;
        for _ in 0..48_000 {
            let g = e.process(&p);
            peak = peak.max(g);
            trough = trough.min(g);
        }
        assert!(peak > 0.9, "never opened, peak {peak}");
        assert!(trough < 0.2, "never closed, trough {trough}");
    }

    #[test]
    fn amount_scales_the_depth() {
        // Half amount should dip, not close. This is what makes one control
        // cover both a swell and a gate.
        let level = |amount: f32| {
            let mut e = Envelope::new(48_000.0);
            let p = EnvParams {
                amount,
                period: 0.5,
                attack_ms: 5.0,
                decay_ms: 100.0,
            };
            let mut trough = 1.0f32;
            for _ in 0..48_000 {
                trough = trough.min(e.process(&p));
            }
            trough
        };
        let full = level(1.0);
        let half = level(0.5);
        assert!(half > full, "half {half} should dip less than full {full}");
        assert!(half > 0.3, "half amount should not close, got {half}");
    }

    #[test]
    fn retriggers_on_its_period() {
        let mut e = Envelope::new(48_000.0);
        let p = EnvParams {
            amount: 1.0,
            period: 0.1,
            attack_ms: 2.0,
            decay_ms: 30.0,
        };
        // Count upward crossings of the halfway mark. Per-sample slope is too
        // shallow to detect directly: a 2 ms attack rises about 1% per sample
        // and the output smoother flattens it further.
        let mut prev = e.process(&p);
        let mut crossings = 0;
        for _ in 0..48_000 {
            let g = e.process(&p);
            if prev < 0.5 && g >= 0.5 {
                crossings += 1;
            }
            prev = g;
        }
        // Ten periods of 100 ms in one second.
        assert!(
            (8..=12).contains(&crossings),
            "expected ~10 retriggers, got {crossings}"
        );
    }

    #[test]
    fn a_very_short_period_does_not_blow_up() {
        let mut e = Envelope::new(48_000.0);
        let p = EnvParams {
            amount: 1.0,
            period: 0.01,
            attack_ms: 500.0,
            decay_ms: 2_000.0,
        };
        for _ in 0..48_000 {
            let g = e.process(&p);
            assert!((0.0..=1.0).contains(&g), "{g}");
        }
    }
}
