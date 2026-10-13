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

#[derive(Debug, Clone, Copy)]
pub struct NoiseParams {
    /// -1 brown, -0.5 pink, 0 white, 0.5 blue, 1 violet; between two, a
    /// crossfade of the pair.
    pub color: f32,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self { color: 0.0 }
    }
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
        Self {
            left: Side::new(0x4015_E001 ^ salt),
            right: Side::new(0x4015_E002 ^ salt),
            color,
            leak,
            brown_in: (1.0 - leak * leak).sqrt(),
        }
    }

    /// Every filter back to rest. The sources run on, so a voice switched off
    /// and on again does not repeat the noise it played before.
    pub fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }

    /// Reset, with Colour already at `p`, so nothing glides from a value left
    /// over from before. For a voice coming back from idle.
    pub fn settle(&mut self, p: &NoiseParams) {
        self.reset();
        self.color.reset(p.color.clamp(-1.0, 1.0));
    }

    /// One stereo sample.
    #[inline]
    pub fn process(&mut self, p: &NoiseParams) -> (f32, f32) {
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
        (ga * l[i] + gb * l[i + 1], ga * r[i] + gb * r[i + 1])
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
        noise.settle(&NoiseParams { color });
        (0..n)
            .map(|_| noise.process(&NoiseParams { color }))
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
                let (l, _) = noise.process(&NoiseParams { color });
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
                .map(|_| noise.process(&NoiseParams { color: -0.3 }))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(0), run(0));
        assert_ne!(run(0), run(crate::rng::salt(1)));
        // And the two sides are not one signal.
        let x = run(0);
        assert!(x.iter().filter(|s| s.0 != s.1).count() > 4_700);
    }
}
