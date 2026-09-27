//! Two-operator FM, built as phase modulation.
//!
//! Georg, 2026-09-27: *"I want generators for FM, AM, PM synthesis."* FM and
//! PM sound the same in digital, and the DX7's FM was PM. Modulating phase
//! rather than frequency keeps the pitch still as the index moves and cannot
//! drift off at DC, so this is PM and the UI calls it FM.
//!
//! One modulator with self-feedback drives one carrier. Two operators cover
//! bells, basses, brass and, with the feedback up, noise, in four rows. More
//! operators belong in an operator matrix, a node of its own, rather than
//! here. See `guidance/projects/shard/design/node-vocabulary.md`.
//!
//! Not band-limited. A high index on a high note folds sidebands back under
//! Nyquist, which is part of how digital FM has always sounded.

use crate::smooth::OnePole;
use core::f32::consts::TAU;

/// Feedback at full scale, in radians of phase offset. Past about 1.5 the
/// modulator's own output stops being a saw and turns to noise, which is the
/// far end of the control and deliberate.
const FEEDBACK_SCALE: f32 = 1.6;

#[derive(Debug, Clone, Copy)]
pub struct FmParams {
    /// The carrier's frequency in hertz, before a step's pitch.
    pub freq: f32,
    /// The modulator's frequency as a multiple of the carrier's. Whole
    /// numbers are harmonic; anything else is inharmonic, bell and metal.
    pub ratio: f32,
    /// Modulation depth, in radians of carrier phase.
    pub index: f32,
    /// The modulator's feedback on itself, 0 to 1.
    pub feedback: f32,
}

impl Default for FmParams {
    fn default() -> Self {
        Self {
            freq: 110.0,
            ratio: 2.0,
            index: 1.5,
            feedback: 0.0,
        }
    }
}

pub struct Fm {
    carrier: f32,
    modulator: f32,
    /// The modulator's last two outputs. Feedback reads their average, as the
    /// DX7 did, which damps the one-sample oscillation plain feedback falls
    /// into at high amounts.
    last: [f32; 2],
    sample_rate: f32,
    freq: OnePole,
    ratio: OnePole,
    index: OnePole,
    feedback: OnePole,
}

impl Fm {
    pub fn new(sample_rate: f32) -> Self {
        let d = FmParams::default();
        let pole = |v: f32| {
            let mut p = OnePole::new();
            p.set_time(20.0, sample_rate);
            p.reset(v);
            p
        };
        Self {
            carrier: 0.0,
            modulator: 0.0,
            last: [0.0; 2],
            sample_rate,
            freq: pole(d.freq),
            ratio: pole(d.ratio),
            index: pole(d.index),
            feedback: pole(d.feedback),
        }
    }

    /// Back to the start of a cycle with no feedback in flight.
    pub fn reset(&mut self) {
        self.carrier = 0.0;
        self.modulator = 0.0;
        self.last = [0.0; 2];
    }

    /// One sample. `pitch` multiplies the frequency after smoothing, so a
    /// step's note lands at once while a turned knob glides.
    #[inline]
    pub fn process(&mut self, p: &FmParams, pitch: f32) -> f32 {
        let freq = self.freq.process(p.freq.clamp(0.0, 20_000.0)) * pitch;
        let ratio = self.ratio.process(p.ratio.max(0.0));
        let index = self.index.process(p.index.max(0.0));
        let feedback = self.feedback.process(p.feedback.clamp(0.0, 1.0));

        let fb = feedback * FEEDBACK_SCALE * 0.5 * (self.last[0] + self.last[1]);
        let m = (TAU * self.modulator + fb).sin();
        self.last = [m, self.last[0]];
        let out = (TAU * self.carrier + index * m).sin();

        // Wrapped rather than left to grow, as the ring modulator's is: an
        // f32 phase counter loses precision audibly within minutes.
        let step = freq / self.sample_rate;
        self.carrier = wrap(self.carrier + step);
        self.modulator = wrap(self.modulator + step * ratio);
        out
    }
}

#[inline]
fn wrap(phase: f32) -> f32 {
    phase - phase.floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// `n` samples after the smoothers have landed on `p`, since they start
    /// at the defaults.
    fn render(p: FmParams, n: usize) -> Vec<f32> {
        let mut fm = Fm::new(SR);
        for _ in 0..SR as usize / 2 {
            fm.process(&p, 1.0);
        }
        (0..n).map(|_| fm.process(&p, 1.0)).collect()
    }

    /// Energy at `hz`, by correlating against a sine and a cosine.
    fn energy_at(x: &[f32], hz: f32) -> f32 {
        let (mut s, mut c) = (0.0f32, 0.0f32);
        for (i, v) in x.iter().enumerate() {
            let w = TAU * hz * i as f32 / SR;
            s += v * w.sin();
            c += v * w.cos();
        }
        (s * s + c * c).sqrt() / x.len() as f32
    }

    #[test]
    fn index_zero_is_a_plain_sine_at_the_frequency() {
        let x = render(
            FmParams {
                index: 0.0,
                ..FmParams::default()
            },
            48_000,
        );
        assert!(energy_at(&x, 110.0) > 0.45, "the carrier");
        assert!(energy_at(&x, 330.0) < 1e-3, "and nothing else");
    }

    #[test]
    fn the_index_adds_sidebands_at_the_ratio() {
        // Carrier 200, modulator 600: sidebands at 200 ± 600k. 800 Hz is the
        // first upper one, and no lower sideband folds onto it, which a ratio
        // of two would do (200 - 800 = -600, cancelling against +600).
        let p = |index| FmParams {
            freq: 200.0,
            ratio: 3.0,
            index,
            feedback: 0.0,
        };
        let plain = render(p(0.0), 48_000);
        let bright = render(p(3.0), 48_000);
        assert!(energy_at(&plain, 800.0) < 1e-3);
        assert!(energy_at(&bright, 800.0) > 0.1);
    }

    #[test]
    fn output_never_leaves_the_valid_range() {
        let mut fm = Fm::new(SR);
        let p = FmParams {
            freq: 20_000.0,
            ratio: 16.0,
            index: 20.0,
            feedback: 1.0,
        };
        for _ in 0..48_000 {
            let v = fm.process(&p, 8.0);
            assert!(v.is_finite() && v.abs() <= 1.0);
        }
    }

    #[test]
    fn a_parameter_sweep_produces_no_discontinuity() {
        // A pure sine at 110 Hz moves at most about 0.015 a sample. Jerking
        // every knob end to end each 256 samples must stay near that,
        // because the smoothing has to turn a jump into a glide.
        let mut fm = Fm::new(SR);
        let mut last = 0.0f32;
        let mut worst = 0.0f32;
        for block in 0..200 {
            let high = block % 2 == 0;
            let p = FmParams {
                freq: 110.0,
                ratio: if high { 1.0 } else { 1.5 },
                index: if high { 0.0 } else { 1.0 },
                feedback: if high { 0.0 } else { 0.2 },
            };
            for _ in 0..256 {
                let v = fm.process(&p, 1.0);
                worst = worst.max((v - last).abs());
                last = v;
            }
        }
        assert!(worst < 0.1, "a jump of {worst}");
    }

    #[test]
    fn a_steps_pitch_moves_the_note() {
        let mut fm = Fm::new(SR);
        let p = FmParams {
            index: 0.0,
            ..FmParams::default()
        };
        let x: Vec<f32> = (0..48_000).map(|_| fm.process(&p, 2.0)).collect();
        assert!(energy_at(&x, 220.0) > 0.45);
    }
}
