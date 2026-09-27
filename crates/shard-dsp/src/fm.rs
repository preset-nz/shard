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
//!
//! **Towards analogue** (Georg, 2026-09-27: *"something that pulls that from
//! math based fm to pseudo analog"*). Digital FM is clinical because it is
//! exact: perfect ratios, identical cycles. Two rows break that. **Drift**
//! lets each operator wander in pitch on its own, so the ratio is never quite
//! whole and the sidebands beat. **Type** chooses how the modulator moves the
//! carrier: its phase (clean, the default), its frequency linearly (through
//! zero), or its frequency exponentially, as a VCO's pitch input does. The
//! last goes sharp as the index rises, which is the "wrong" that makes analogue
//! cross-modulation sound the way it does.

use crate::rng::Rng;
use crate::smooth::OnePole;
use core::f32::consts::TAU;

/// Feedback at full scale, in radians of phase offset. Past about 1.5 the
/// modulator's own output stops being a saw and turns to noise, which is the
/// far end of the control and deliberate.
const FEEDBACK_SCALE: f32 = 1.6;

/// How far a full drift wanders, in cents either way. Enough to beat and to
/// pull a whole ratio out of true, not enough to read as out of tune.
const DRIFT_CENTS: f32 = 25.0;
/// The per-sample roughness under the wander at full drift, in cents.
const JITTER_CENTS: f32 = 1.5;
/// How long the wander holds a heading before choosing another, in seconds.
const WANDER_S: f32 = 0.35;

/// Exponential FM reads the index as octaves of swing: ten is three octaves
/// either way, which is as far as a VCO's pitch input usefully goes.
const OCTAVES_PER_INDEX: f32 = 0.3;

/// How the modulator moves the carrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmType {
    /// The carrier's phase. Pitch holds still at any index.
    Phase,
    /// The carrier's frequency, linearly, through zero. The same sidebands as
    /// phase at no feedback; with feedback, the modulator's offset moves the
    /// pitch.
    Linear,
    /// The carrier's frequency in octaves, as a VCO's pitch input. The pitch
    /// rises with the index.
    Exponential,
}

impl FmType {
    pub const ALL: [FmType; 3] = [FmType::Phase, FmType::Linear, FmType::Exponential];
    pub const NAMES: [&'static str; 3] = ["Phase", "Linear", "Exponential"];

    pub fn from_value(v: f32) -> FmType {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

/// A slow random walk from -1 to 1: a new heading every so often, glided to.
/// One per operator, so the two wander apart.
struct Wander {
    rng: Rng,
    heading: f32,
    left: u32,
    hold: u32,
    glide: OnePole,
}

impl Wander {
    fn new(seed: u32, sample_rate: f32) -> Self {
        let mut glide = OnePole::new();
        glide.set_time(WANDER_S * 1000.0, sample_rate);
        Self {
            rng: Rng::new(seed),
            heading: 0.0,
            left: 0,
            hold: (WANDER_S * sample_rate) as u32,
            glide,
        }
    }

    /// The wander this sample, and a bipolar white sample for the roughness.
    #[inline]
    fn process(&mut self) -> (f32, f32) {
        if self.left == 0 {
            self.heading = self.rng.next_bipolar();
            // Uneven holds, or the wander ticks at a rate you can hear.
            self.left = self.hold / 2 + (self.rng.next_f32() * self.hold as f32) as u32;
        }
        self.left -= 1;
        (self.glide.process(self.heading), self.rng.next_bipolar())
    }
}

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
    /// How far each operator wanders off its pitch, 0 to 1.
    pub drift: f32,
    pub ty: FmType,
}

impl Default for FmParams {
    fn default() -> Self {
        Self {
            freq: 110.0,
            ratio: 2.0,
            index: 1.5,
            feedback: 0.0,
            drift: 0.0,
            ty: FmType::Phase,
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
    drift: OnePole,
    carrier_wander: Wander,
    modulator_wander: Wander,
    /// The type last sample, to carry the carrier's phase across a switch.
    ty: FmType,
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
            drift: pole(d.drift),
            carrier_wander: Wander::new(0x5EED_0001, sample_rate),
            modulator_wander: Wander::new(0x5EED_0002, sample_rate),
            ty: d.ty,
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
        let drift = self.drift.process(p.drift.clamp(0.0, 1.0));

        // The wanders run whether drift is up or not, so turning it up picks
        // up a walk already in motion rather than one starting from centre.
        // At zero both detunes are exactly one, and the voice is untouched.
        let (cw, cn) = self.carrier_wander.process();
        let (mw, mn) = self.modulator_wander.process();
        let detune = |wander: f32, noise: f32| {
            if drift > 0.0 {
                (drift * (DRIFT_CENTS * wander + JITTER_CENTS * noise) / 1200.0).exp2()
            } else {
                1.0
            }
        };
        let step = freq / self.sample_rate;
        let carrier_step = step * detune(cw, cn);
        let modulator_step = step * ratio * detune(mw, mn);

        let fb = feedback * FEEDBACK_SCALE * 0.5 * (self.last[0] + self.last[1]);
        let m = (TAU * self.modulator + fb).sin();
        self.last = [m, self.last[0]];

        // Phase type reads the carrier with the modulation added; the other
        // two have it already in the carrier's phase. Switching between them
        // moves that offset across, so the wave carries on where it was
        // rather than jumping by the index.
        if p.ty != self.ty {
            let offset = index * m / TAU;
            match (self.ty, p.ty) {
                (FmType::Phase, _) => self.carrier = wrap(self.carrier + offset),
                (_, FmType::Phase) => self.carrier = wrap(self.carrier - offset),
                _ => {}
            }
            self.ty = p.ty;
        }
        let (out, carrier_step) = match p.ty {
            FmType::Phase => ((TAU * self.carrier + index * m).sin(), carrier_step),
            // Frequency deviation is the index times the modulator's rate,
            // which is what makes the index mean the same thing as in phase.
            FmType::Linear => (
                (TAU * self.carrier).sin(),
                carrier_step + index * modulator_step * m,
            ),
            FmType::Exponential => (
                (TAU * self.carrier).sin(),
                carrier_step * (index * OCTAVES_PER_INDEX * m).exp2(),
            ),
        };

        // Wrapped rather than left to grow, as the ring modulator's is: an
        // f32 phase counter loses precision audibly within minutes. Linear
        // runs through zero, so the phase can step backwards; `wrap` holds it
        // in range either way.
        self.carrier = wrap(self.carrier + carrier_step);
        self.modulator = wrap(self.modulator + modulator_step);
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
            ..FmParams::default()
        };
        let plain = render(p(0.0), 48_000);
        let bright = render(p(3.0), 48_000);
        assert!(energy_at(&plain, 800.0) < 1e-3);
        assert!(energy_at(&bright, 800.0) > 0.1);
    }

    /// Carrier cycles in `x`, from its rising zero crossings. Every type
    /// reads the carrier as a sine of one phase, so this is its mean pitch.
    fn cycles(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn drift_at_zero_holds_the_pitch_true() {
        // Ten seconds, so a wander left running would have somewhere to go.
        // A count can land either side of its edge by one.
        let p = FmParams {
            index: 0.0,
            ..FmParams::default()
        };
        let n = cycles(&render(p, 480_000)) as i32;
        assert!(
            (n - 1_100).abs() <= 1,
            "{n} cycles in ten seconds of 110 Hz"
        );
    }

    #[test]
    fn drift_pulls_the_pitch_off_true_and_moves_it() {
        // A second at a time, the cycle count wanders about 110 Hz rather
        // than sitting on it, and never by more than the range.
        let p = FmParams {
            index: 0.0,
            drift: 1.0,
            ..FmParams::default()
        };
        let x = render(p, 48_000 * 4);
        let counts: Vec<usize> = x.chunks(48_000).map(cycles).collect();
        let hz = |n: usize| n as f32;
        assert!(
            counts.iter().any(|&n| n != 110),
            "drift left the pitch alone: {counts:?}"
        );
        let most = 110.0 * (DRIFT_CENTS * 1.1 / 1200.0).exp2();
        assert!(
            counts
                .iter()
                .all(|&n| hz(n) < most + 1.0 && hz(n) > 110.0 * 110.0 / most - 1.0),
            "drifted further than its range: {counts:?}"
        );
    }

    #[test]
    fn exponential_goes_sharp_as_the_index_rises_and_the_others_hold_pitch() {
        // The analogue signature: modulating a VCO's pitch input raises its
        // mean pitch. Phase and linear keep the carrier on 110.
        let at = |ty, index| {
            cycles(&render(
                FmParams {
                    ratio: 0.5,
                    index,
                    ty,
                    ..FmParams::default()
                },
                48_000,
            )) as i32
        };
        for ty in [FmType::Phase, FmType::Linear] {
            assert!((at(ty, 3.0) - 110).abs() <= 2, "{ty:?} moved the pitch");
        }
        assert!(
            // The mean of 2^(0.9 sin) is about 1.1, so 121 Hz.
            at(FmType::Exponential, 3.0) > 116,
            "exponential held its pitch at {}",
            at(FmType::Exponential, 3.0)
        );
    }

    #[test]
    fn output_never_leaves_the_valid_range() {
        let mut fm = Fm::new(SR);
        let p = FmParams {
            freq: 20_000.0,
            ratio: 16.0,
            index: 20.0,
            feedback: 1.0,
            drift: 1.0,
            ty: FmType::Phase,
        };
        for ty in FmType::ALL {
            for _ in 0..48_000 {
                let v = fm.process(&FmParams { ty, ..p }, 8.0);
                assert!(v.is_finite() && v.abs() <= 1.0);
            }
        }
    }

    #[test]
    fn a_parameter_sweep_produces_no_discontinuity() {
        // A pure sine at 110 Hz moves at most about 0.015 a sample. Jerking
        // every knob end to end each 256 samples, and switching the type,
        // must stay near that: the smoothing turns a jump into a glide, and a
        // switch carries the carrier's phase across.
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
                drift: if high { 0.0 } else { 1.0 },
                ty: FmType::ALL[block / 2 % 3],
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
