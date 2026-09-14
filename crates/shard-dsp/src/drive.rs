//! Overdrive, distortion, fuzz and folding.
//!
//! One box with four curves, because a pedal is a curve plus the filters
//! around it. What separates the pedals on a shelf is mostly three things:
//!
//! - **The curve.** Soft (a tanh, the way an op-amp with diodes in its
//!   feedback loop rounds off) or hard (diodes to ground after the gain,
//!   which slice). Soft keeps the shape of the wave and adds harmonics
//!   gently; hard flattens the top and adds them all at once.
//! - **Symmetry.** A curve that treats the top and bottom of the wave the
//!   same makes only odd harmonics, which is the overdrive sound. Push the
//!   wave off centre first and the even harmonics appear: warmer, thicker,
//!   what a single transistor stage does, and what fuzz is.
//! - **The filters.** A high-pass before the curve keeps the bass from
//!   turning to mud when it clips, and it is where the famous mid hump comes
//!   from. A low-pass after it is the tone knob.
//!
//! The fourth curve here is not a pedal: a wavefolder, which turns the wave
//! back on itself when it passes the limit rather than flattening it.
//!
//! Everything runs at twice the sample rate, because clipping makes
//! harmonics past Nyquist and those fold back down as aliasing, which the
//! crusher wants and this does not. Up by zero-stuffing, down through the
//! same half-band filter. The filter is sized once at construction.

use crate::smooth::OnePole;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveType {
    /// Soft, symmetric, a high-pass in front. A Tube Screamer's shape.
    Overdrive,
    /// Hard, symmetric. A RAT or a DS-1: gain, then diodes that slice.
    Distortion,
    /// Soft, pushed off centre, more gain. A Fuzz Face's shape, even
    /// harmonics and a gated tail.
    Fuzz,
    /// Turns the wave back on itself past the limit. Not a pedal.
    Fold,
}

impl DriveType {
    pub const ALL: [DriveType; 4] = [
        DriveType::Overdrive,
        DriveType::Distortion,
        DriveType::Fuzz,
        DriveType::Fold,
    ];
    pub const NAMES: [&'static str; 4] = ["Overdrive", "Distortion", "Fuzz", "Fold"];

    pub fn from_value(v: f32) -> DriveType {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DriveParams {
    /// Gain into the curve, in dB. Zero is clean for a full-scale signal.
    pub amount_db: f32,
    /// The low-pass after the curve, in hertz.
    pub tone_hz: f32,
    pub kind: DriveType,
    /// Dry to wet, 0 to 1. At zero the node is bit-exact bypass.
    pub mix: f32,
}

impl Default for DriveParams {
    fn default() -> Self {
        Self {
            amount_db: 24.0,
            tone_hz: 8_000.0,
            kind: DriveType::Overdrive,
            mix: 0.0,
        }
    }
}

/// Half-band FIR length. Odd, so the centre tap is the one non-zero even
/// coefficient and the filter is exactly linear phase.
const TAPS: usize = 31;

/// A two-times up and down sampler around a per-sample curve.
struct Oversampler {
    coef: Vec<f32>,
    up: Vec<f32>,
    down: Vec<f32>,
    pos: usize,
}

impl Oversampler {
    fn new() -> Self {
        // Windowed sinc at a quarter of the doubled rate, which is the old
        // Nyquist. Blackman keeps the stop band around 60 dB down.
        let mid = (TAPS / 2) as f32;
        let mut coef: Vec<f32> = (0..TAPS)
            .map(|i| {
                let n = i as f32 - mid;
                let sinc = if n == 0.0 {
                    0.5
                } else {
                    (core::f32::consts::PI * 0.5 * n).sin() / (core::f32::consts::PI * n)
                };
                let w = 0.42 - 0.5 * (core::f32::consts::TAU * i as f32 / (TAPS - 1) as f32).cos()
                    + 0.08 * (2.0 * core::f32::consts::TAU * i as f32 / (TAPS - 1) as f32).cos();
                sinc * w
            })
            .collect();
        let sum: f32 = coef.iter().sum();
        for c in &mut coef {
            *c /= sum;
        }
        Self {
            coef,
            up: vec![0.0; TAPS],
            down: vec![0.0; TAPS],
            pos: 0,
        }
    }

    #[inline]
    fn fir(buf: &[f32], coef: &[f32], pos: usize) -> f32 {
        let n = buf.len();
        let mut acc = 0.0;
        for (k, c) in coef.iter().enumerate() {
            acc += c * buf[(pos + n - k) % n];
        }
        acc
    }

    /// One input sample in, one out, with `shape` run twice in between.
    #[inline]
    fn process(&mut self, x: f32, mut shape: impl FnMut(f32) -> f32) -> f32 {
        let n = TAPS;
        let mut out = 0.0;
        for phase in 0..2 {
            // Zero-stuff, then interpolate: the gain of two restores level.
            self.pos = (self.pos + 1) % n;
            self.up[self.pos] = if phase == 0 { 2.0 * x } else { 0.0 };
            let interpolated = Self::fir(&self.up, &self.coef, self.pos);
            let shaped = shape(interpolated);
            self.down[self.pos] = shaped;
            // Only the second phase's output is kept: decimate by two.
            if phase == 1 {
                out = Self::fir(&self.down, &self.coef, self.pos);
            }
        }
        out
    }
}

/// A one-pole high-pass, for the pre-filter and the DC blocker.
#[derive(Debug, Clone, Copy, Default)]
struct HighPass {
    x1: f32,
    y1: f32,
    r: f32,
}

impl HighPass {
    fn set(&mut self, hz: f32, sample_rate: f32) {
        self.r = 1.0 - core::f32::consts::TAU * hz / sample_rate;
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// One channel's state: the filters around the curve and its oversampler.
struct Lane {
    /// The overdrive's input corner, where its mid hump comes from.
    pre_od: HighPass,
    /// The distortion's, lower: a RAT thins the bass less than a Screamer.
    pre_ds: HighPass,
    dc: HighPass,
    /// The fuzz's wool: a fixed low-pass inside the box, before Tone.
    wool: f32,
    /// The fuzz's sag: how loud the driven signal has been lately. The
    /// bias follows it, so the two halves clip at unequal widths at any
    /// gain, which is where the even harmonics come from.
    sag: f32,
    tone: OnePole,
    over: Oversampler,
}

pub struct Drive {
    left: Lane,
    right: Lane,
    amount: OnePole,
    tone_hz: OnePole,
    mix: OnePole,
    sample_rate: f32,
}

impl Drive {
    pub fn new(sample_rate: f32) -> Self {
        let lane = || {
            let mut pre_od = HighPass::default();
            let mut pre_ds = HighPass::default();
            let mut dc = HighPass::default();
            // A Tube Screamer's input corner sits around 700 Hz. Lower here,
            // so a drone keeps some body, but high enough to hear it thin.
            pre_od.set(400.0, sample_rate);
            pre_ds.set(120.0, sample_rate);
            dc.set(10.0, sample_rate);
            Lane {
                pre_od,
                pre_ds,
                dc,
                wool: 0.0,
                sag: 0.0,
                tone: OnePole::new(),
                over: Oversampler::new(),
            }
        };
        let d = DriveParams::default();
        let mut amount = OnePole::new();
        let mut tone_hz = OnePole::new();
        let mut mix = OnePole::new();
        amount.set_time(20.0, sample_rate);
        tone_hz.set_time(20.0, sample_rate);
        mix.set_time(20.0, sample_rate);
        amount.reset(d.amount_db);
        tone_hz.reset(d.tone_hz);
        mix.reset(d.mix);
        Self {
            left: lane(),
            right: lane(),
            amount,
            tone_hz,
            mix,
            sample_rate,
        }
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &DriveParams) -> (f32, f32) {
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };
        if mix <= 0.0 {
            // Bit-exact bypass. The filters and oversampler idle; their
            // state is a few samples and refills on the way back in.
            return (l, r);
        }
        let amount = self.amount.process(p.amount_db.clamp(0.0, 60.0));
        // Each type drives its curve as hard as its pedal would. A fuzz's
        // transistor sees far more than an overdrive's op-amp, and that gap
        // is most of what separates them (Georg, 2026-09-14: the three were
        // too subtle at one gain).
        let gain = 10f32.powf(amount / 20.0) * type_gain(p.kind);
        let tone = self.tone_hz.process(p.tone_hz.clamp(200.0, 20_000.0));
        // The tone filter is a one-pole whose time is set from the cutoff;
        // set per sample because the cutoff smooths.
        let tone_coef = (-core::f32::consts::TAU * tone / self.sample_rate).exp();
        let wool_coef = (-core::f32::consts::TAU * 2_500.0 / self.sample_rate).exp();
        let sag_release = (-1.0 / (self.sample_rate * 0.05)).exp();
        let kind = p.kind;
        let run = |lane: &mut Lane, x: f32| -> f32 {
            let x = match kind {
                DriveType::Overdrive => lane.pre_od.process(x),
                DriveType::Distortion => lane.pre_ds.process(x),
                _ => x,
            };
            let driven = x * gain;
            let bias = match kind {
                DriveType::Fuzz => {
                    lane.sag = driven.abs().max(lane.sag * sag_release);
                    FUZZ_BIAS * lane.sag
                }
                _ => 0.0,
            };
            let shaped = lane.over.process(driven, |v| curve(kind, v, bias));
            let shaped = lane.dc.process(shaped);
            let shaped = match kind {
                DriveType::Fuzz => {
                    lane.wool = shaped + (lane.wool - shaped) * wool_coef;
                    lane.wool
                }
                _ => shaped,
            };
            // Tone: a one-pole low-pass, stepped by coefficient.
            let y = shaped + (lane.tone.value() - shaped) * tone_coef;
            lane.tone.reset(y);
            y
        };
        let wl = run(&mut self.left, l);
        let wr = run(&mut self.right, r);
        (l + (wl - l) * mix, r + (wr - r) * mix)
    }
}

/// How much harder each type drives its curve, on top of Amount.
#[inline]
fn type_gain(kind: DriveType) -> f32 {
    match kind {
        DriveType::Overdrive => 1.0,
        DriveType::Distortion => 4.0,
        DriveType::Fuzz => 6.0,
        DriveType::Fold => 1.0,
    }
}

/// How far off centre a fuzz sits, as a share of how loud it has been.
const FUZZ_BIAS: f32 = 0.45;

/// The curves. Each maps a driven sample to about ±1. `bias` is the
/// fuzz's offset into its curve; the others ignore it.
#[inline]
fn curve(kind: DriveType, x: f32, bias: f32) -> f32 {
    match kind {
        DriveType::Overdrive => x.tanh(),
        // A RAT-like edge: hard, with a small knee so it is not a pure
        // square at extreme gain, which reads as digital rather than diode.
        DriveType::Distortion => {
            let c = x.clamp(-1.0, 1.0);
            c - c * c * c * 0.1
        }
        DriveType::Fuzz => {
            // Off centre by a share of the recent level, so one half clips
            // wider than the other however hard it is driven. A fixed
            // offset would vanish into the gain. The DC is blocked after.
            let y = (x + bias).tanh();
            // A gate at the bottom, the spitting tail of a starved fuzz.
            if y.abs() < 0.03 {
                y * y.abs() * 33.0
            } else {
                y
            }
        }
        DriveType::Fold => {
            // Reflect off ±1 as many times as it takes.
            let mut v = x;
            for _ in 0..8 {
                if v > 1.0 {
                    v = 2.0 - v;
                } else if v < -1.0 {
                    v = -2.0 - v;
                } else {
                    break;
                }
            }
            v.clamp(-1.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn tone(hz: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (core::f32::consts::TAU * hz * i as f32 / SR).sin() * amp)
            .collect()
    }

    #[test]
    fn zero_mix_is_bit_exact_bypass() {
        let mut d = Drive::new(SR);
        let p = DriveParams::default();
        for i in 0..2_000 {
            let x = (i as f32 * 0.03).sin();
            assert_eq!(d.process(x, -x, &p), (x, -x));
        }
    }

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        let mut d = Drive::new(SR);
        let on = DriveParams {
            mix: 1.0,
            ..Default::default()
        };
        let off = DriveParams {
            mix: 0.0,
            ..Default::default()
        };
        for _ in 0..4_800 {
            d.process(0.5, 0.5, &on);
        }
        for _ in 0..48_000 {
            d.process(0.5, 0.5, &off);
        }
        for _ in 0..1_000 {
            assert_eq!(d.process(0.5, -0.25, &off), (0.5, -0.25));
        }
    }

    #[test]
    fn every_curve_is_finite_bounded_and_makes_a_difference() {
        for kind in DriveType::ALL {
            let mut d = Drive::new(SR);
            let p = DriveParams {
                amount_db: 30.0,
                tone_hz: 20_000.0,
                kind,
                mix: 1.0,
            };
            let input = tone(220.0, 48_000, 0.5);
            let out: Vec<f32> = input.iter().map(|&x| d.process(x, x, &p).0).collect();
            for (i, v) in out.iter().enumerate() {
                assert!(v.is_finite(), "{kind:?} went non-finite at {i}");
                assert!(v.abs() <= 1.5, "{kind:?} left the range at {i}: {v}");
            }
            let diff = rms(&out[24_000..]
                .iter()
                .zip(&input[24_000..])
                .map(|(a, b)| a - b)
                .collect::<Vec<_>>());
            assert!(diff > 0.1, "{kind:?} changed nothing: {diff}");
        }
    }

    #[test]
    fn fuzz_leaves_no_dc_behind() {
        let mut d = Drive::new(SR);
        let p = DriveParams {
            amount_db: 40.0,
            tone_hz: 20_000.0,
            kind: DriveType::Fuzz,
            mix: 1.0,
        };
        let out: Vec<f32> = tone(110.0, 96_000, 0.8)
            .iter()
            .map(|&x| d.process(x, x, &p).0)
            .collect();
        let mean = out[48_000..].iter().sum::<f32>() / 48_000.0;
        assert!(mean.abs() < 0.02, "dc of {mean}");
    }

    #[test]
    fn the_tone_control_takes_the_top_off() {
        let at = |hz: f32| {
            let mut d = Drive::new(SR);
            let p = DriveParams {
                amount_db: 24.0,
                tone_hz: hz,
                kind: DriveType::Distortion,
                mix: 1.0,
            };
            let out: Vec<f32> = tone(200.0, 48_000, 0.8)
                .iter()
                .map(|&x| d.process(x, x, &p).0)
                .collect();
            // Energy in the difference between neighbours: a proxy for highs.
            rms(&out[24_000..]
                .windows(2)
                .map(|w| w[1] - w[0])
                .collect::<Vec<_>>())
        };
        assert!(at(500.0) < at(12_000.0) * 0.5);
    }

    #[test]
    fn oversampling_keeps_aliases_down() {
        // A 5 kHz tone hard-clipped at 48 kHz. Its 9th harmonic sits at
        // 45 kHz and folds to 3 kHz without oversampling. With it, what
        // lands at 3 kHz should be well under what lands at the fundamental.
        let mut d = Drive::new(SR);
        let p = DriveParams {
            amount_db: 40.0,
            tone_hz: 20_000.0,
            kind: DriveType::Distortion,
            mix: 1.0,
        };
        let out: Vec<f32> = tone(5_000.0, 96_000, 0.8)
            .iter()
            .map(|&x| d.process(x, x, &p).0)
            .collect();
        let tail = &out[48_000..];
        let power_at = |hz: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, v) in tail.iter().enumerate() {
                let ph = core::f32::consts::TAU * hz * i as f32 / SR;
                re += v * ph.cos();
                im += v * ph.sin();
            }
            (re * re + im * im).sqrt() / tail.len() as f32
        };
        let fundamental = power_at(5_000.0);
        let alias = power_at(3_000.0);
        assert!(
            alias < fundamental * 0.05,
            "alias at 3 kHz is {alias} against {fundamental}"
        );
    }

    #[test]
    fn the_pedals_are_told_apart() {
        // Same amount, same tone, a mid-level tone in. The three pedal
        // curves must land audibly apart, and fuzz must be the one with an
        // even harmonic, which is what its bias is for.
        let render = |kind: DriveType| {
            let mut d = Drive::new(SR);
            let p = DriveParams {
                amount_db: 24.0,
                tone_hz: 8_000.0,
                kind,
                mix: 1.0,
            };
            let out: Vec<f32> = tone(220.0, 96_000, 0.3)
                .iter()
                .map(|&x| d.process(x, x, &p).0)
                .collect();
            out[48_000..].to_vec()
        };
        let harmonic = |v: &[f32], n: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, x) in v.iter().enumerate() {
                let ph = core::f32::consts::TAU * 220.0 * n * i as f32 / SR;
                re += x * ph.cos();
                im += x * ph.sin();
            }
            (re * re + im * im).sqrt() / v.len() as f32
        };
        let od = render(DriveType::Overdrive);
        let ds = render(DriveType::Distortion);
        let fz = render(DriveType::Fuzz);
        let apart = |a: &[f32], b: &[f32]| {
            rms(&a.iter().zip(b).map(|(x, y)| x - y).collect::<Vec<_>>()) / rms(a)
        };
        assert!(
            apart(&od, &ds) > 0.2,
            "overdrive and distortion: {}",
            apart(&od, &ds)
        );
        assert!(
            apart(&ds, &fz) > 0.2,
            "distortion and fuzz: {}",
            apart(&ds, &fz)
        );
        assert!(
            apart(&od, &fz) > 0.2,
            "overdrive and fuzz: {}",
            apart(&od, &fz)
        );
        // Even harmonic: fuzz has one, the symmetric pair barely do.
        let even = |v: &[f32]| harmonic(v, 2.0) / harmonic(v, 1.0);
        assert!(
            even(&fz) > 0.1,
            "fuzz has no second harmonic: {}",
            even(&fz)
        );
        assert!(
            even(&od) < even(&fz) * 0.3,
            "overdrive is not symmetric: {}",
            even(&od)
        );
        // Hard clipping is brighter than soft: more third harmonic.
        let third = |v: &[f32]| harmonic(v, 3.0) / harmonic(v, 1.0);
        assert!(
            third(&ds) > third(&od),
            "distortion is not harder than overdrive"
        );
    }

    #[test]
    fn silence_in_silence_out() {
        let mut d = Drive::new(SR);
        for kind in DriveType::ALL {
            let p = DriveParams {
                kind,
                mix: 1.0,
                ..Default::default()
            };
            for _ in 0..5_000 {
                let (l, r) = d.process(0.0, 0.0, &p);
                assert!(l.abs() < 1e-6 && r.abs() < 1e-6, "{kind:?}: {l}");
            }
        }
    }
}
