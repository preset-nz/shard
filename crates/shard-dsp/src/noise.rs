//! Noise, the fourth generator (epic 48).
//!
//! One row picks the kind of noise: **Colour**, from brown at -1 through pink,
//! white and blue to violet at 1, so an LFO or an envelope can sweep it.
//! Filtering it further is the patch filter's and the Ladder's job, after the
//! generators.
//!
//! Every colour is made from the same white source on every sample, and each
//! is scaled to the same RMS, so Colour crossfades between neighbouring
//! colours that are already running. Nothing restarts when it moves, the
//! output is a continuous function of it, and it is not a volume control.
//!
//! - **White**: a uniform sample from the voice's own seeded `Rng`. Flat.
//! - **Brown**: white through a leaky integrator, -6 dB an octave above
//!   [`BROWN_CORNER_HZ`]. Leaky rather than a plain sum, so it never drifts
//!   off at DC.
//! - **Pink**: Paul Kellet's refined filter, seven one-poles summed, -3 dB an
//!   octave to within half a dB. Its coefficients are for 44.1 kHz; at 48 kHz
//!   the slope is the same and only the bottom corner moves, a few hertz.
//! - **Blue**: pink, differentiated. +3 dB an octave.
//! - **Violet**: white, differentiated. +6 dB an octave.
//!
//! **Band** crossfades from the plain noise to a resonant band-pass of it at
//! **Frequency**, **Width** wide: wind, a whistle, a pitched hiss. A step's
//! pitch moves Frequency after its glide, as it moves FM's, so a pitched
//! band follows the notes. The filter is a Cytomic (TPT) state-variable
//! band-pass, one per side, whose coefficients are recomputed every sample, so
//! it stays stable and click-free while Frequency is modulated at audio rate;
//! Frequency is held under 0.45 of the sample rate.
//!
//! - **Width** maps to Q exponentially, `Q = 80 * (2/3 / 80)^width`: 0 is
//!   Q 80 (a whistle, about a hundredth of an octave wide), 1 is Q 2/3 (two
//!   octaves between the half-power points). Exponential because the ear
//!   hears Q in ratios, so the middle of the slider is a useful Q of about 7.
//! - **Level.** Band is not a volume control. A band-pass of unit peak gain
//!   passes only the fraction `h` of the noise's power that lies in the band,
//!   so the band is made up by `1/sqrt(h)` to the plain noise's RMS. `h` is
//!   not measured (a narrow band's power wanders far too slowly to follow) but
//!   computed from the noise's known spectrum at the current Colour,
//!   integrated against the filter's response by a 64-point quadrature
//!   spaced in the filter's own width, close together at the band and
//!   geometrically further out along both skirts, so a Q of 80 is resolved
//!   as well as a Q of 2/3 and a skirt that matters (violet's) is counted. It is refreshed every 64 samples and glided. The crossfade then
//!   divides by the level the plain and the band would sum to, given that
//!   they are correlated by `sqrt(h)` (the band is a part of the plain), as
//!   Colour's crossfade does for its neighbours, so Band 0 to 1 holds level.
//! - **Band 0 is the plain noise, bit for bit.** Once Band has glided to zero
//!   the filter is skipped and held at rest.
//!
//! **Stereo, independent sides.** The cloud is stereo and FM is mono; noise
//! takes two sources, one a side, seeded apart, so it is wide rather than a
//! point in the middle. A mono noise is the pan of a later story if one is
//! wanted.
//!
//! Seeded with the voice's salt (`rng::salt`), as FM's drift is: a render is
//! the same every time and eight voices on one patch do not play the same
//! noise.

use crate::rng::Rng;
use crate::smooth::OnePole;

/// The RMS every colour is scaled to, -12 dBFS. Chosen by the peaks: at full
/// Gain the loudest colours, brown and pink, which are near Gaussian, peak
/// a little over 1 in ten seconds, as an FM sine peaks at 1. White peaks at
/// 0.43. So at the same Gain noise's RMS sits about 9 dB under FM's.
const LEVEL: f32 = 0.25;

/// The RMS of a uniform sample in `[-1, 1)`: 1/sqrt(3).
const WHITE_RMS: f32 = 0.577_350_3;

/// Brown's corner, in hertz. Above it brown falls at -6 dB an octave; below
/// it the leak wins and it is flat, which is what keeps it off DC.
const BROWN_CORNER_HZ: f32 = 40.0;

/// Kellet's pink, unscaled, has about this RMS for a white input of RMS
/// 1/sqrt(3). Measured over ten seconds; `colour_is_not_a_volume_control`
/// holds it.
const PINK_RMS: f32 = 1.766_7;

/// The RMS of differentiated pink, unscaled, likewise.
const BLUE_RMS: f32 = 1.046_6;

/// How alike each neighbouring pair of colours is (their correlation, which
/// is high, since all five come from one source). A crossfade divides by the
/// level the two would sum to, so the level holds between them as well as
/// at them. Brown-pink, pink-white, white-blue, blue-violet. Measured.
const NEIGHBOURS: [f32; 4] = [0.783, 0.538, 0.908, 0.906];

/// How long Colour takes to glide to a new value, as FM's rows do.
const GLIDE_MS: f32 = 20.0;

/// Width 0 and width 1 as a filter's Q. See the module docs.
const Q_NARROW: f32 = 80.0;
const Q_WIDE: f32 = 2.0 / 3.0;

/// The band's centre never goes above this share of the sample rate.
const FREQ_CEIL: f32 = 0.45;

/// Quadrature points across the filter's response, and how often the band's
/// level is worked out again.
const QUAD: usize = 64;
const LEVEL_EVERY: u32 = 64;

/// The floor on `sqrt(h)`, so the most the band is made up by is 80 dB. A
/// whistle in blue or violet at 100 Hz, where those colours have almost
/// nothing, needs 50 dB of it; the band is that much of the plain noise's
/// RMS, so its peaks are no higher than the plain noise's.
const MIN_BAND_SHARE: f32 = 1e-4;

/// How long the band's made-up level takes to follow the spectrum.
const LEVEL_GLIDE_MS: f32 = 10.0;

/// The colours: index 0 brown to 4 violet.
const COLOURS: usize = 5;

#[derive(Debug, Clone, Copy)]
pub struct NoiseParams {
    /// -1 brown, -0.5 pink, 0 white, 0.5 blue, 1 violet; between two, a
    /// crossfade of the pair.
    pub color: f32,
    /// 0 the plain noise, 1 the band alone; between, a crossfade at one level.
    pub band: f32,
    /// The band's centre in hertz, before a step's pitch.
    pub freq: f32,
    /// 0 a whistle, 1 two octaves wide.
    pub width: f32,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self {
            color: 0.0,
            band: 0.0,
            freq: 440.0,
            width: 0.3,
        }
    }
}

/// Cytomic's state-variable filter, read out as a band-pass of unit gain at
/// its centre.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    #[inline]
    fn band(&mut self, x: f32, a: &SvfCoeffs) -> f32 {
        let v3 = x - self.ic2;
        let v1 = a.a1 * self.ic1 + a.a2 * v3;
        let v2 = self.ic2 + a.a2 * self.ic1 + a.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        a.k * v1
    }
}

#[derive(Debug, Clone, Copy)]
struct SvfCoeffs {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

/// One side's source and the filters that colour it.
struct Side {
    rng: Rng,
    brown: f32,
    pink: [f32; 7],
    last_pink: f32,
    last_white: f32,
}

impl Side {
    fn new(seed: u32) -> Self {
        Self {
            rng: Rng::new(seed),
            brown: 0.0,
            pink: [0.0; 7],
            last_pink: 0.0,
            last_white: 0.0,
        }
    }

    fn reset(&mut self) {
        self.brown = 0.0;
        self.pink = [0.0; 7];
        self.last_pink = 0.0;
        self.last_white = 0.0;
    }

    /// The five colours this sample, brown to violet, each at [`LEVEL`].
    #[inline]
    fn process(&mut self, leak: f32, brown_in: f32) -> [f32; 5] {
        let white = self.rng.next_bipolar();

        self.brown = leak * self.brown + brown_in * white;

        // Kellet's refined pink ("pk3"), as published.
        let b = &mut self.pink;
        b[0] = 0.998_86 * b[0] + white * 0.055_517_9;
        b[1] = 0.993_32 * b[1] + white * 0.075_075_9;
        b[2] = 0.969_00 * b[2] + white * 0.153_852;
        b[3] = 0.866_50 * b[3] + white * 0.310_485_6;
        b[4] = 0.550_00 * b[4] + white * 0.532_952_2;
        b[5] = -0.761_6 * b[5] - white * 0.016_898_0;
        let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.536_2;
        b[6] = white * 0.115_926;

        let blue = pink - self.last_pink;
        let violet = white - self.last_white;
        self.last_pink = pink;
        self.last_white = white;

        const W: f32 = LEVEL / WHITE_RMS;
        [
            self.brown * W,
            pink * (LEVEL / PINK_RMS),
            white * W,
            blue * (LEVEL / BLUE_RMS),
            // Differencing two independent samples doubles the variance.
            violet * (W * core::f32::consts::FRAC_1_SQRT_2),
        ]
    }
}

pub struct Noise {
    left: Side,
    right: Side,
    color: OnePole,
    /// The brown integrator's leak, and what scales its input so that its
    /// RMS is white's: sqrt(1 - leak^2).
    leak: f32,
    brown_in: f32,
    sample_rate: f32,
    band: OnePole,
    freq: OnePole,
    width: OnePole,
    /// sqrt of the share of the noise's power inside the band, glided.
    share: OnePole,
    share_target: f32,
    svf: [Svf; 2],
    /// Whether the filter is running, or skipped and at rest.
    live: bool,
    countdown: u32,
    /// Each colour's spectrum is divided by its mean, so a shape is a share
    /// of the power. Worked out once.
    mean: [f32; COLOURS],
    /// The quadrature's midpoints across (-1, 1).
    grid: [f32; QUAD],
}

/// A colour's power spectrum, up to a constant, at `sin2` = sin^2(w/2) and
/// `f` hertz.
#[inline]
fn shape(colour: usize, sin2: f32, f: f32, leak: f32) -> f32 {
    let diff = 4.0 * sin2;
    match colour {
        0 => 1.0 / ((1.0 - leak) * (1.0 - leak) + 4.0 * leak * sin2),
        1 => 1.0 / (f + 8.0),
        2 => 1.0,
        3 => diff / (f + 8.0),
        _ => diff,
    }
}

impl Noise {
    pub fn new(sample_rate: f32) -> Self {
        Self::seeded(sample_rate, 0)
    }

    /// Both sides seeded with `salt` mixed in (`rng::salt`); zero is `new`.
    pub fn seeded(sample_rate: f32, salt: u32) -> Self {
        let mut color = OnePole::new();
        color.set_time(GLIDE_MS, sample_rate);
        color.reset(NoiseParams::default().color);
        let leak = (-core::f32::consts::TAU * BROWN_CORNER_HZ / sample_rate).exp();
        let pole = |v: f32, ms: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        let d = NoiseParams::default();
        let mut mean = [0.0f32; COLOURS];
        const N: usize = 8_192;
        for (c, m) in mean.iter_mut().enumerate() {
            let mut sum = 0.0f64;
            for j in 0..N {
                let w = (j as f32 + 0.5) * core::f32::consts::PI / N as f32;
                let sin = (0.5 * w).sin();
                let f = w * sample_rate / core::f32::consts::TAU;
                sum += shape(c, sin * sin, f, leak) as f64;
            }
            *m = (sum / N as f64) as f32;
        }
        let mut grid = [0.0f32; QUAD];
        for (j, t) in grid.iter_mut().enumerate() {
            *t = (j as f32 + 0.5) / QUAD as f32 * 2.0 - 1.0;
        }
        Self {
            left: Side::new(0x4015_E001 ^ salt),
            right: Side::new(0x4015_E002 ^ salt),
            color,
            leak,
            brown_in: (1.0 - leak * leak).sqrt(),
            sample_rate,
            band: pole(d.band, GLIDE_MS),
            freq: pole(d.freq, GLIDE_MS),
            width: pole(d.width, GLIDE_MS),
            share: pole(1.0, LEVEL_GLIDE_MS),
            share_target: 1.0,
            svf: [Svf::default(); 2],
            live: false,
            countdown: 0,
            mean,
            grid,
        }
    }

    /// The filter at `freq` hertz and `width`: `g` and its coefficients.
    #[inline]
    fn coeffs(&self, freq: f32, width: f32) -> (f32, SvfCoeffs) {
        let f = freq.clamp(1.0, FREQ_CEIL * self.sample_rate);
        let g = (core::f32::consts::PI * f / self.sample_rate).tan();
        let q = Q_NARROW * (Q_WIDE / Q_NARROW).powf(width.clamp(0.0, 1.0));
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        (
            g,
            SvfCoeffs {
                k,
                a1,
                a2: g * a1,
                a3: g * g * a1,
            },
        )
    }

    /// sqrt of the share `h` of the current colour's power that the band
    /// passes: `h` is the mean over frequency of the colour's spectrum, as a
    /// share, times the band-pass's power response. With the warped frequency
    /// `Omega = e^x`, that response is `1 / (1 + tau^2)` where
    /// `tau = 2 sinh(x) / k`, and with `tau = sinh(xi)` the integral over
    /// frequency becomes a sum over evenly spaced `xi`. That puts the points
    /// closely where the band is and geometrically further out along both
    /// skirts, out to where the skirt has reached the top of the spectrum.
    fn band_share(&self, g: f32, k: f32, i: usize, ga: f32, gb: f32, rho: f32) -> f32 {
        let (ma, mb) = (self.mean[i], self.mean[i + 1]);
        // The skirt is followed until u = g * Omega is about 300 (within a
        // thousandth of the sample rate of Nyquist) and as far down.
        let xm = (300.0 / g).ln();
        let xi_m = (2.0 * xm.sinh() / k).asinh();
        let mut sum = 0.0f32;
        for &s in &self.grid {
            let xi = xi_m * s;
            let e = xi.exp();
            let (sinh, cosh_xi) = (0.5 * (e - 1.0 / e), 0.5 * (e + 1.0 / e));
            let y = 0.5 * k * sinh;
            let cosh_x = (1.0 + y * y).sqrt();
            let u = g * (y + cosh_x);
            let u2 = u * u;
            let sin2 = u2 / (1.0 + u2);
            let f = self.sample_rate * u.atan() / core::f32::consts::PI;
            let sa = shape(i, sin2, f, self.leak) / ma;
            let sb = shape(i + 1, sin2, f, self.leak) / mb;
            let sp = ga * ga * sa + gb * gb * sb + 2.0 * ga * gb * rho * (sa * sb).sqrt();
            sum += sp * u / ((1.0 + u2) * cosh_x * cosh_xi);
        }
        let h = sum * k * (2.0 * xi_m / QUAD as f32) / core::f32::consts::PI;
        h.clamp(1e-8, 1.0).sqrt()
    }

    /// Every filter back to rest. The sources run on, so a voice switched off
    /// and on again does not repeat the noise it played before.
    pub fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.svf = [Svf::default(); 2];
    }

    /// Reset, with Colour already at `p`, so nothing glides from a value left
    /// over from before. For a voice coming back from idle.
    pub fn settle(&mut self, p: &NoiseParams) {
        self.reset();
        self.color.reset(p.color.clamp(-1.0, 1.0));
        self.band.reset(p.band.clamp(0.0, 1.0));
        self.freq.reset(p.freq.clamp(1.0, 20_000.0));
        self.width.reset(p.width.clamp(0.0, 1.0));
        self.live = false;
    }

    /// One stereo sample. `pitch` multiplies the band's frequency after
    /// smoothing, so a step's note lands at once while a turned knob glides.
    #[inline]
    pub fn process(&mut self, p: &NoiseParams, pitch: f32) -> (f32, f32) {
        let c = self.color.process(p.color.clamp(-1.0, 1.0));
        // -1..1 onto the five colours, 0..4: which pair, and how far along.
        let at = (c + 1.0) * 2.0;
        let i = (at.floor() as usize).min(3);
        let t = at - i as f32;
        let (a, b) = (1.0 - t, t);
        // The level the pair sums to, given how alike they are; divided out
        // so the crossfade holds level. Exactly one at either end.
        let rho = NEIGHBOURS[i];
        let norm = 1.0 / (a * a + b * b + 2.0 * a * b * rho).sqrt();
        let (ga, gb) = (a * norm, b * norm);

        let l = self.left.process(self.leak, self.brown_in);
        let r = self.right.process(self.leak, self.brown_in);
        let plain = (ga * l[i] + gb * l[i + 1], ga * r[i] + gb * r[i + 1]);

        let freq = self.freq.process(p.freq.clamp(1.0, 20_000.0)) * pitch;
        let width = self.width.process(p.width.clamp(0.0, 1.0));
        let target = p.band.clamp(0.0, 1.0);
        let mut band = self.band.process(target);
        // The glide never quite arrives; below this it is zero, and the
        // filter is skipped.
        if target == 0.0 && band < 1e-5 {
            band = 0.0;
            self.band.reset(0.0);
        }
        if band == 0.0 {
            if self.live {
                self.live = false;
                self.svf = [Svf::default(); 2];
            }
            return plain;
        }

        let (g, coeffs) = self.coeffs(freq, width);
        if self.countdown == 0 || !self.live {
            let share = self.band_share(g, coeffs.k, i, ga, gb, rho);
            if !self.live {
                self.share.reset(share);
                self.live = true;
            }
            self.countdown = LEVEL_EVERY;
            self.share_target = share;
        }
        self.countdown -= 1;
        let share = self.share.process(self.share_target).max(MIN_BAND_SHARE);

        let (a, b) = (1.0 - band, band);
        let norm = 1.0 / (a * a + b * b + 2.0 * a * b * share).sqrt();
        let (gp, gband) = (a * norm, b * norm / share);
        (
            gp * plain.0 + gband * self.svf[0].band(plain.0, &coeffs),
            gp * plain.1 + gband * self.svf[1].band(plain.1, &coeffs),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn render(color: f32, n: usize) -> Vec<(f32, f32)> {
        render_at(SR, color, n)
    }

    fn render_at(sample_rate: f32, color: f32, n: usize) -> Vec<(f32, f32)> {
        let mut noise = Noise::new(sample_rate);
        noise.settle(&NoiseParams {
            color,
            ..Default::default()
        });
        (0..n)
            .map(|_| {
                noise.process(
                    &NoiseParams {
                        color,
                        ..Default::default()
                    },
                    1.0,
                )
            })
            .collect()
    }

    fn rms(x: impl Iterator<Item = f32>) -> f32 {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for v in x {
            sum += (v as f64) * (v as f64);
            n += 1;
        }
        (sum / n as f64).sqrt() as f32
    }

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    #[test]
    fn colour_is_not_a_volume_control() {
        // The five colours and the four midpoints between them, where the
        // crossfade's correction for how alike a pair is does its work.
        // Ten seconds each, both sides, at both common rates: brown's leak
        // and how alike it is to pink depend on the rate.
        let mut levels = Vec::new();
        for (sample_rate, color) in [44_100.0, SR]
            .into_iter()
            .flat_map(|sr| [-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0].map(|c| (sr, c)))
        {
            let x = render_at(sample_rate, color, 480_000);
            let l = rms(x.iter().map(|s| s.0));
            let r = rms(x.iter().map(|s| s.1));
            assert!(
                db(l / LEVEL).abs() < 1.0 && db(r / LEVEL).abs() < 1.0,
                "colour {color} at {sample_rate}: {} dB, {} dB against the level",
                db(l / LEVEL),
                db(r / LEVEL)
            );
            levels.push(l);
        }
        let lo = levels.iter().copied().fold(f32::MAX, f32::min);
        let hi = levels.iter().copied().fold(0.0, f32::max);
        assert!(
            db(hi / lo) < 3.0,
            "{} dB from quietest to loudest",
            db(hi / lo)
        );
    }

    #[test]
    fn colour_is_continuous_across_each_join() {
        // Where the crossfade changes pair (pink, white, blue), a hair either
        // side must sound the same: the same seed, so any difference is the
        // gains jumping, not the noise.
        for join in [-0.5f32, 0.0, 0.5] {
            let below = render(join - 1e-3, 4_800);
            let above = render(join + 1e-3, 4_800);
            let worst = below
                .iter()
                .zip(&above)
                .map(|(a, b)| (a.0 - b.0).abs().max((a.1 - b.1).abs()))
                .fold(0.0f32, f32::max);
            assert!(worst < 0.01, "a jump of {worst} at {join}");
        }
    }

    #[test]
    fn a_colour_sweep_produces_no_discontinuity() {
        // Colour slammed end to end every 256 samples. The glide turns each
        // jump into a sweep, so no sample-to-sample step is larger than the
        // largest the colours make standing still (violet's), and no
        // stretch is louder or quieter than they are.
        let steady = [-1.0f32, -0.5, 0.0, 0.5, 1.0]
            .iter()
            .map(|&c| {
                let x = render(c, 48_000);
                x.windows(2)
                    .map(|w| (w[1].0 - w[0].0).abs())
                    .fold(0.0f32, f32::max)
            })
            .fold(0.0f32, f32::max);
        let mut noise = Noise::new(SR);
        let mut last = 0.0f32;
        let mut worst = 0.0f32;
        for block in 0..200 {
            let color = if block % 2 == 0 { -1.0 } else { 1.0 };
            let mut window = [0.0f32; 256];
            for v in window.iter_mut() {
                let (l, _) = noise.process(
                    &NoiseParams {
                        color,
                        ..Default::default()
                    },
                    1.0,
                );
                worst = worst.max((l - last).abs());
                last = l;
                *v = l;
            }
            let level = rms(window.iter().copied());
            assert!(
                db(level / LEVEL).abs() < 6.0,
                "block {block} at {} dB",
                db(level / LEVEL)
            );
        }
        assert!(worst <= steady * 1.1, "a jump of {worst} against {steady}");
    }

    #[test]
    fn output_stays_in_range() {
        // Near Gaussian, so there is no hard bound; ten seconds of every
        // colour peaks a little over 1. Seeded, so this cannot flake.
        for color in [-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0] {
            for (l, r) in render(color, 480_000) {
                assert!(l.is_finite() && r.is_finite());
                assert!(l.abs() <= 1.5 && r.abs() <= 1.5, "colour {color}: {l}, {r}");
            }
        }
    }

    #[test]
    fn brown_does_not_drift_off_at_dc() {
        // A minute of brown. Below its 40 Hz corner it is flat, so a second's
        // mean wobbles by about 0.02 either way and no more, the last second
        // as much as the first, and the minute's mean is near zero. A plain
        // sum would wander off without bound.
        let x = render(-1.0, 60 * 48_000);
        let mut total = 0.0f64;
        for (s, second) in x.chunks(48_000).enumerate() {
            let sum = second.iter().map(|v| v.0 as f64).sum::<f64>();
            let mean = sum / second.len() as f64;
            assert!(mean.abs() < 0.1, "second {s}: mean {mean}");
            total += sum;
        }
        let mean = total / x.len() as f64;
        assert!(mean.abs() < 0.015, "the minute's mean is {mean}");
    }

    #[test]
    fn the_same_seed_is_the_same_noise_and_a_salt_is_not() {
        let run = |salt: u32| {
            let mut noise = Noise::seeded(SR, salt);
            (0..4_800)
                .map(|_| {
                    noise.process(
                        &NoiseParams {
                            color: -0.3,
                            ..Default::default()
                        },
                        1.0,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(run(0), run(0));
        assert_ne!(run(0), run(crate::rng::salt(1)));
        // And the two sides are not one signal.
        let x = run(0);
        assert!(x.iter().filter(|s| s.0 != s.1).count() > 4_700);
    }

    // ---- Band -------------------------------------------------------------

    fn band_params(color: f32, band: f32, freq: f32, width: f32) -> NoiseParams {
        NoiseParams {
            color,
            band,
            freq,
            width,
        }
    }

    fn render_band(sample_rate: f32, p: NoiseParams, pitch: f32, n: usize) -> Vec<(f32, f32)> {
        let mut noise = Noise::new(sample_rate);
        noise.settle(&p);
        (0..n).map(|_| noise.process(&p, pitch)).collect()
    }

    fn level_db(x: &[(f32, f32)]) -> f32 {
        let l = rms(x.iter().map(|s| s.0));
        let r = rms(x.iter().map(|s| s.1));
        db(((l * l + r * r) * 0.5).sqrt() / LEVEL)
    }

    /// Power of `x` at `hz`, by Goertzel.
    fn goertzel(x: impl Iterator<Item = f32>, hz: f32, sample_rate: f32) -> f32 {
        let c = 2.0 * (core::f32::consts::TAU * hz / sample_rate).cos();
        let (mut a, mut b, mut n) = (0.0f64, 0.0f64, 0usize);
        for v in x {
            let next = v as f64 + c as f64 * a - b;
            b = a;
            a = next;
            n += 1;
        }
        ((a * a + b * b - a * b * c as f64) / (n as f64 * n as f64)) as f32
    }

    #[test]
    fn band_is_not_a_volume_control() {
        // Band at 1, white, across the width of the slider and the useful
        // range of Frequency: within 3 dB of the plain noise. Twenty
        // seconds, because a whistle's power wanders slowly.
        for width in [0.0, 0.25, 0.5, 0.75, 1.0] {
            for freq in [100.0, 300.0, 1_000.0, 3_000.0, 5_000.0] {
                let x = render_band(SR, band_params(0.0, 1.0, freq, width), 1.0, 960_000);
                let d = level_db(&x);
                assert!(d.abs() < 3.0, "width {width} at {freq} Hz: {d} dB");
            }
        }
    }

    #[test]
    fn band_holds_level_in_every_colour() {
        // The make-up follows the colour's spectrum, so a band in brown or
        // violet is as loud as the brown or violet it came from.
        for color in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            for (freq, width) in [(200.0, 0.3), (1_000.0, 0.0), (4_000.0, 0.6)] {
                let x = render_band(SR, band_params(color, 1.0, freq, width), 1.0, 960_000);
                let d = level_db(&x);
                assert!(
                    d.abs() < 3.0,
                    "colour {color}, {freq} Hz, width {width}: {d} dB"
                );
            }
        }
    }

    #[test]
    fn band_crossfade_holds_level() {
        for (freq, width) in [(150.0, 0.0), (1_000.0, 0.3), (4_000.0, 1.0)] {
            for band in [0.1, 0.25, 0.5, 0.75, 0.9] {
                let x = render_band(SR, band_params(0.0, band, freq, width), 1.0, 960_000);
                let d = level_db(&x);
                assert!(
                    d.abs() < 3.0,
                    "band {band}, {freq} Hz, width {width}: {d} dB"
                );
            }
        }
    }

    #[test]
    fn the_bands_energy_peaks_at_frequency() {
        for freq in [200.0f32, 1_000.0, 4_000.0] {
            let x = render_band(SR, band_params(0.0, 1.0, freq, 0.2), 1.0, 96_000);
            let at = goertzel(x.iter().map(|s| s.0), freq, SR);
            for other in [freq * 0.5, freq * 2.0] {
                let away = goertzel(x.iter().map(|s| s.0), other, SR);
                assert!(
                    db(at / away) > 10.0,
                    "{freq} Hz is only {} dB over {other} Hz",
                    db(at / away)
                );
            }
        }
    }

    #[test]
    fn a_steps_pitch_moves_the_band() {
        // An octave up: the energy that was at 500 is at 1000.
        let p = band_params(0.0, 1.0, 500.0, 0.2);
        let x = render_band(SR, p, 2.0, 96_000);
        let up = goertzel(x.iter().map(|s| s.0), 1_000.0, SR);
        let was = goertzel(x.iter().map(|s| s.0), 500.0, SR);
        assert!(db(up / was) > 10.0, "{} dB", db(up / was));
    }

    #[test]
    fn a_frequency_sweep_produces_no_discontinuity() {
        // Frequency slammed between 100 Hz and 8 kHz every 256 samples, at
        // two widths. The glide turns each jump into a sweep. No sample step
        // is larger than the largest the band makes standing still, and no
        // stretch strays far from the level.
        for width in [0.2, 0.6] {
            let steady = [100.0, 400.0, 1_600.0, 6_400.0, 8_000.0]
                .iter()
                .map(|&f| {
                    let x = render_band(SR, band_params(0.0, 1.0, f, width), 1.0, 96_000);
                    x.windows(2)
                        .map(|w| (w[1].0 - w[0].0).abs())
                        .fold(0.0f32, f32::max)
                })
                .fold(0.0f32, f32::max);
            let mut noise = Noise::new(SR);
            let (mut last, mut worst) = (0.0f32, 0.0f32);
            for block in 0..400 {
                let freq = if block % 2 == 0 { 100.0 } else { 8_000.0 };
                let p = band_params(0.0, 1.0, freq, width);
                let mut window = [0.0f32; 256];
                for v in window.iter_mut() {
                    let (l, _) = noise.process(&p, 1.0);
                    worst = worst.max((l - last).abs());
                    last = l;
                    *v = l;
                }
                if block > 4 {
                    let d = db(rms(window.iter().copied()) / LEVEL);
                    assert!(d.abs() < 9.0, "width {width}, block {block}: {d} dB");
                }
            }
            assert!(
                worst <= steady * 1.25,
                "width {width}: a jump of {worst} against {steady}"
            );
        }
    }

    #[test]
    fn band_is_finite_at_the_extremes() {
        for sample_rate in [22_050.0, 44_100.0, SR, 96_000.0] {
            for color in [-1.0, 0.0, 1.0] {
                for (freq, width, pitch) in [
                    (20.0, 0.0, 0.0),
                    (20.0, 1.0, 1.0),
                    (8_000.0, 0.0, 16.0),
                    (8_000.0, 1.0, 1_000.0),
                    (20_000.0, 0.0, 1.0),
                    (0.0, 0.0, 1.0),
                    (440.0, 2.0, -3.0),
                ] {
                    let p = band_params(color, 1.0, freq, width);
                    for (l, r) in render_band(sample_rate, p, pitch, 48_000) {
                        assert!(
                            l.is_finite() && r.is_finite() && l.abs() < 20.0 && r.abs() < 20.0,
                            "{sample_rate} Hz, colour {color}, {freq} Hz, width {width}, \
                             pitch {pitch}: {l}, {r}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn band_zero_is_the_plain_noise_bit_for_bit() {
        // Frequency and Width mean nothing at Band 0, and a band that was
        // there and has glided away leaves the same noise as one never there
        // (the sources run on either way).
        let plain = render_band(SR, band_params(-0.3, 0.0, 440.0, 0.3), 1.0, 60_000);
        let elsewhere = render_band(SR, band_params(-0.3, 0.0, 5_000.0, 0.9), 3.0, 60_000);
        assert_eq!(plain, elsewhere);

        let mut noise = Noise::new(SR);
        let on = band_params(-0.3, 1.0, 900.0, 0.1);
        let off = band_params(-0.3, 0.0, 900.0, 0.1);
        noise.settle(&off);
        let mut both = Vec::new();
        for i in 0..60_000 {
            both.push(noise.process(if i < 1_000 { &on } else { &off }, 1.0));
        }
        assert_eq!(&both[30_000..], &plain[30_000..]);
    }
}
