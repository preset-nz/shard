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
        self.attack_ms <= 0.0
            && self.decay_ms <= 0.0
            && self.release_ms <= 0.0
            && self.sustain >= 1.0
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

        if t >= release_start && release > 0.0 {
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
        }
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
    fn is_neutral_only_when_nothing_is_set() {
        assert!(EnvParams::default().is_neutral());
        assert!(!EnvParams {
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
}
