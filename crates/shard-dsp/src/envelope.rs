//! An amplitude envelope over the sample.
//!
//! Attack, decay, sustain, release, running once per pass through the trimmed
//! window. The loop point is the trigger and the window length is the
//! duration, so there is nothing to set for either.
//!
//! No state. The shape is a pure function of how far into the pass you are,
//! which makes it trivially testable and means changing the sample, the trim
//! or the transport cannot leave the envelope out of step with playback.
//!
//! When a sequencer arrives this stays as it is and the caller passes note
//! age instead of loop position. That is the whole reason it takes elapsed
//! time as an argument rather than counting internally.

#[derive(Debug, Clone, Copy)]
pub struct EnvParams {
    /// Depth. At one you get the shape as drawn; at zero the envelope is flat.
    /// Between, it dips rather than closes, which is what makes the same
    /// settings usable as a gentle swell and as a hard gate.
    pub amount: f32,
    pub attack_ms: f32,
    pub decay_ms: f32,
    /// Level held after the decay, 0 to 1.
    pub sustain: f32,
    /// Fades to silence, finishing exactly at the end of the pass.
    pub release_ms: f32,
}

impl Default for EnvParams {
    /// Neutral: no attack, no decay, full sustain, no release. Multiplies by
    /// exactly one everywhere, so an untouched envelope changes nothing.
    fn default() -> Self {
        Self {
            amount: 1.0,
            attack_ms: 0.0,
            decay_ms: 0.0,
            sustain: 1.0,
            release_ms: 0.0,
        }
    }
}

impl EnvParams {
    /// True when this envelope cannot change the signal, so the caller can
    /// skip it entirely rather than multiplying by one.
    pub fn is_neutral(&self) -> bool {
        self.amount <= 0.0
            || (self.attack_ms <= 0.0
                && self.decay_ms <= 0.0
                && self.release_ms <= 0.0
                && self.sustain >= 1.0)
    }

    /// The gain at `elapsed` samples into a pass of `length` samples.
    ///
    /// Stages are fitted to the pass rather than allowed to overrun it. Set an
    /// attack longer than the sample and you get a sample-long attack, not
    /// silence — the alternative is controls that silently do nothing on short
    /// material, which reads as a fault.
    pub fn gain_at(&self, elapsed: f32, length: f32, sample_rate: f32) -> f32 {
        if length < 2.0 {
            return 1.0;
        }
        if self.is_neutral() {
            return 1.0;
        }

        let ms = sample_rate * 0.001;
        let sustain = self.sustain.clamp(0.0, 1.0);
        let mut attack = (self.attack_ms.max(0.0) * ms).min(length);
        let mut decay = (self.decay_ms.max(0.0) * ms).min(length);
        let mut release = (self.release_ms.max(0.0) * ms).min(length);

        // Attack, decay and release cannot together exceed the pass. Scale
        // them down in proportion when they do, so the shape is preserved.
        let total = attack + decay + release;
        if total > length {
            let k = length / total;
            attack *= k;
            decay *= k;
            release *= k;
        }

        let t = elapsed.clamp(0.0, length);
        let release_start = length - release;

        let shape = if t >= release_start && release > 0.0 {
            // Release runs from wherever the envelope had got to, so a release
            // longer than the sustain stage does not jump up first.
            let level = if release_start <= attack && attack > 0.0 {
                release_start / attack
            } else if release_start <= attack + decay && decay > 0.0 {
                1.0 - (1.0 - sustain) * ((release_start - attack) / decay)
            } else {
                sustain
            };
            let x = ((t - release_start) / release).clamp(0.0, 1.0);
            level * (1.0 - x)
        } else if t < attack && attack > 0.0 {
            t / attack
        } else if t < attack + decay && decay > 0.0 {
            1.0 - (1.0 - sustain) * ((t - attack) / decay)
        } else {
            sustain
        };

        // Depth. Blends the shape toward flat rather than scaling it, so
        // lowering amount lifts the quiet parts instead of dropping the loud
        // ones. A half-depth envelope is a dip, not a quieter gate.
        let amount = self.amount.clamp(0.0, 1.0);
        1.0 - amount * (1.0 - shape.clamp(0.0, 1.0))
    }

    /// The shape, 0 to 1, `age` samples into a note that is held for `gate`
    /// samples, or for ever when `gate` is none. For the Loop and Hold
    /// lengths (`patch.length`), where a note has a time of its own rather
    /// than a pass through the sample.
    ///
    /// Stages are in real time and not fitted to anything: a two-second
    /// attack takes two seconds. The attack climbs from `from`, the level the
    /// last note had reached, so a retrigger mid-release does not click.
    /// Release runs from wherever the note had got to when the gate closed.
    pub fn shape_at_note(&self, age: f32, gate: Option<f32>, from: f32, sample_rate: f32) -> f32 {
        let ms = sample_rate * 0.001;
        let attack = self.attack_ms.max(0.0) * ms;
        let decay = self.decay_ms.max(0.0) * ms;
        let release = self.release_ms.max(0.0) * ms;
        let sustain = self.sustain.clamp(0.0, 1.0);
        let from = from.clamp(0.0, 1.0);
        let held = |t: f32| {
            if t < attack {
                from + (1.0 - from) * (t / attack)
            } else if t < attack + decay {
                1.0 - (1.0 - sustain) * ((t - attack) / decay)
            } else {
                sustain
            }
        };
        match gate {
            Some(g) if age >= g => {
                if release <= 0.0 {
                    0.0
                } else {
                    held(g) * (1.0 - (age - g) / release).max(0.0)
                }
            }
            _ => held(age.max(0.0)),
        }
    }

    /// The gain for a shape, through the depth, as `gain_at` applies it.
    pub fn gain_of(&self, shape: f32) -> f32 {
        let amount = self.amount.clamp(0.0, 1.0);
        1.0 - amount * (1.0 - shape.clamp(0.0, 1.0))
    }

    /// The shape sampled across one pass, for drawing. `points` evenly spaced
    /// values from the start of the window to its end.
    pub fn curve(&self, points: usize, length: f32, sample_rate: f32) -> Vec<f32> {
        (0..points.max(2))
            .map(|i| {
                let t = i as f32 / (points.max(2) - 1) as f32 * length;
                self.gain_at(t, length, sample_rate)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    /// One second.
    const LEN: f32 = 48_000.0;

    #[test]
    fn neutral_by_default() {
        let p = EnvParams::default();
        assert!(p.is_neutral());
        for i in 0..1000 {
            let t = i as f32 / 1000.0 * LEN;
            assert_eq!(p.gain_at(t, LEN, SR), 1.0, "not unity at {t}");
        }
    }

    #[test]
    fn rises_over_the_attack() {
        let p = EnvParams {
            amount: 1.0,
            attack_ms: 200.0,
            ..Default::default()
        };
        assert!(p.gain_at(0.0, LEN, SR) < 0.01, "should start at silence");
        let half = p.gain_at(0.1 * SR, LEN, SR);
        assert!((half - 0.5).abs() < 0.01, "halfway through attack: {half}");
        assert!(
            p.gain_at(0.2 * SR, LEN, SR) >= 0.99,
            "should be open by 200 ms"
        );
    }

    #[test]
    fn decays_to_the_sustain_level_and_holds() {
        let p = EnvParams {
            amount: 1.0,
            attack_ms: 10.0,
            decay_ms: 200.0,
            sustain: 0.25,
            release_ms: 0.0,
        };
        let peak = p.gain_at(0.010 * SR, LEN, SR);
        assert!(peak > 0.95, "should reach full after attack: {peak}");
        let after = p.gain_at(0.4 * SR, LEN, SR);
        assert!((after - 0.25).abs() < 0.01, "sustain was {after}");
        // And it stays there.
        let later = p.gain_at(0.8 * SR, LEN, SR);
        assert!((later - 0.25).abs() < 0.01, "sustain drifted to {later}");
    }

    #[test]
    fn release_finishes_exactly_at_the_end_of_the_pass() {
        // The contract that makes this an envelope on the sample rather than
        // an oscillator: the fade lands on the loop point, whatever the
        // sample's length.
        for length in [SR * 0.5, SR, SR * 7.0] {
            let p = EnvParams {
                amount: 1.0,
                attack_ms: 5.0,
                decay_ms: 0.0,
                sustain: 1.0,
                release_ms: 250.0,
            };
            let end = p.gain_at(length, length, SR);
            assert!(end < 0.01, "len {length}: ended at {end}");
            let before = p.gain_at(length - 0.25 * SR - 1.0, length, SR);
            assert!(
                before > 0.95,
                "len {length}: release started early ({before})"
            );
        }
    }

    #[test]
    fn stays_within_zero_and_one_for_any_settings() {
        let cases = [
            (0.0, 0.0, 1.0, 0.0),
            (2000.0, 4000.0, 0.0, 4000.0),
            (1.0, 1.0, 0.5, 1.0),
            (100000.0, 100000.0, 1.0, 100000.0),
            (0.0, 500.0, 0.0, 0.0),
        ];
        for (a, d, s, r) in cases {
            let p = EnvParams {
                amount: 1.0,
                attack_ms: a,
                decay_ms: d,
                sustain: s,
                release_ms: r,
            };
            for length in [2.0, 100.0, SR, SR * 30.0] {
                for i in 0..=200 {
                    let t = i as f32 / 200.0 * length;
                    let g = p.gain_at(t, length, SR);
                    assert!(g.is_finite(), "{a},{d},{s},{r} at {t}/{length}");
                    assert!((0.0..=1.0).contains(&g), "{g} for {a},{d},{s},{r}");
                }
            }
        }
    }

    #[test]
    fn stages_are_fitted_to_a_short_sample_not_dropped() {
        // A one-second attack on a 200 ms sample should still be an attack
        // across that sample, not silence. Controls that silently do nothing
        // on short material read as a fault.
        let p = EnvParams {
            amount: 1.0,
            attack_ms: 1000.0,
            ..Default::default()
        };
        let short = 0.2 * SR;
        assert!(p.gain_at(0.0, short, SR) < 0.01);
        let end = p.gain_at(short, short, SR);
        assert!(end > 0.95, "should be open by the end of the sample: {end}");
    }

    #[test]
    fn release_does_not_jump_up_before_falling() {
        // A release longer than everything else starts mid-decay. It has to
        // fall from wherever the envelope actually was.
        let p = EnvParams {
            amount: 1.0,
            attack_ms: 10.0,
            decay_ms: 900.0,
            sustain: 0.0,
            release_ms: 800.0,
        };
        let length = SR;
        let mut prev = p.gain_at(0.02 * SR, length, SR);
        for i in 2..=100 {
            let t = i as f32 / 100.0 * length;
            let g = p.gain_at(t, length, SR);
            assert!(g <= prev + 0.02, "jumped from {prev} to {g} at {t}");
            prev = g;
        }
    }

    #[test]
    fn amount_blends_toward_flat() {
        // Half depth should dip, not close, and should lift the trough rather
        // than lower the peak.
        let shaped = |amount: f32| {
            let p = EnvParams {
                amount,
                attack_ms: 5.0,
                decay_ms: 0.0,
                sustain: 1.0,
                release_ms: 500.0,
            };
            let mut peak = 0.0f32;
            let mut trough = 1.0f32;
            for i in 0..=500 {
                let g = p.gain_at(i as f32 / 500.0 * LEN, LEN, SR);
                peak = peak.max(g);
                trough = trough.min(g);
            }
            (peak, trough)
        };
        let (full_peak, full_trough) = shaped(1.0);
        let (half_peak, half_trough) = shaped(0.5);
        assert!(full_trough < 0.05, "full depth should close: {full_trough}");
        assert!(
            half_trough > 0.4,
            "half depth should only dip: {half_trough}"
        );
        assert!(
            (half_peak - full_peak).abs() < 0.01,
            "depth must not lower the peak: {half_peak} vs {full_peak}"
        );
    }

    #[test]
    fn zero_amount_is_exactly_flat() {
        let p = EnvParams {
            amount: 0.0,
            attack_ms: 500.0,
            decay_ms: 500.0,
            sustain: 0.0,
            release_ms: 500.0,
        };
        assert!(p.is_neutral());
        for i in 0..=200 {
            assert_eq!(p.gain_at(i as f32 / 200.0 * LEN, LEN, SR), 1.0);
        }
    }

    #[test]
    fn curve_is_drawable() {
        let p = EnvParams {
            amount: 1.0,
            attack_ms: 100.0,
            decay_ms: 0.0,
            sustain: 1.0,
            release_ms: 200.0,
        };
        let c = p.curve(128, LEN, SR);
        assert_eq!(c.len(), 128);
        assert!(c.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(c[0] < 0.05, "starts closed: {}", c[0]);
        assert!(c[64] > 0.95, "open in the middle: {}", c[64]);
        assert!(c[127] < 0.05, "ends closed: {}", c[127]);
    }

    #[test]
    fn is_neutral_only_when_nothing_is_set() {
        assert!(EnvParams::default().is_neutral());
        assert!(!EnvParams {
            amount: 1.0,
            attack_ms: 1.0,
            ..Default::default()
        }
        .is_neutral());
        assert!(!EnvParams {
            sustain: 0.9,
            ..Default::default()
        }
        .is_neutral());
        assert!(!EnvParams {
            release_ms: 1.0,
            ..Default::default()
        }
        .is_neutral());
    }

    #[test]
    fn a_note_envelope_runs_in_real_time_and_releases_from_its_gate() {
        let sr = 48_000.0;
        let e = EnvParams {
            amount: 1.0,
            attack_ms: 100.0,
            decay_ms: 100.0,
            sustain: 0.5,
            release_ms: 100.0,
        };
        let at = |ms: f32, gate_ms: Option<f32>| {
            e.shape_at_note(ms * 48.0, gate_ms.map(|g| g * 48.0), 0.0, sr)
        };
        assert!((at(50.0, None) - 0.5).abs() < 1e-3, "halfway up the attack");
        assert!((at(100.0, None) - 1.0).abs() < 1e-3, "at the peak");
        assert!(
            (at(10_000.0, None) - 0.5).abs() < 1e-6,
            "held at sustain for ever"
        );
        // Gate closed at 50 ms, halfway up: the release starts from there.
        assert!((at(50.0, Some(50.0)) - 0.5).abs() < 1e-3);
        assert!((at(100.0, Some(50.0)) - 0.25).abs() < 1e-3);
        assert_eq!(at(151.0, Some(50.0)), 0.0, "and lands on silence");
    }

    #[test]
    fn a_retrigger_climbs_from_where_the_last_note_was() {
        let e = EnvParams {
            attack_ms: 10.0,
            ..EnvParams::default()
        };
        assert_eq!(e.shape_at_note(0.0, None, 0.7, 48_000.0), 0.7);
        assert_eq!(e.shape_at_note(0.0, None, 0.0, 48_000.0), 0.0);
    }
}
