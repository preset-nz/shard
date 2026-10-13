//! Dust, the fifth generator (row 59): sparse random clicks, from a single
//! tick now and then to dense vinyl crackle.
//!
//! **Clicks.** A Poisson process: each sample a click starts with probability
//! `density / sample_rate`, so the gaps are random and there is never a grid.
//! A click is an impulse into a one-pole decay, `c = exp(-1 / (decay * sr))`,
//! so it is an exponential with a time constant of Decay. Clicks add in the
//! decay's state, so overlapping ones need no voices and nothing is
//! allocated; the process runs on two states, left and right.
//!
//! **Amplitude.** Each click has a random sign and a magnitude of `v^3` for
//! `v` uniform in `[0, 1)`: the median click is an eighth of the loudest, a
//! tenth of them are above 0.73 and one in a hundred above 0.93. Most quiet,
//! some loud, like real dust. The random sign is what keeps the sum from
//! piling up one way. Its mean square is `1/7`, which the level below
//! folds into [`REF`].
//!
//! **Pan.** Each click draws its own place, scaled by Spread, and lands
//! there at constant power (`cos`, `sin` of a quarter turn). Spread 0 is
//! every click in the centre, at 0.707 a side.
//!
//! **Tone.** A one-pole low-pass at [`SPLIT_HZ`] splits each click into `low`
//! and `high = x - low`, and Tone mixes them: from 0 to 0.5 the high part
//! comes up from nothing to unity (all low, to the click as it is), and from
//! 0.5 to 1 the low part goes down to nothing (the click as it is, to all
//! high). A thud has only its low part; a tick, only the edge.
//!
//! **Level.** Density, Decay and Tone are not volume controls. A click's
//! energy `E` (summed squares, in samples) is worked out exactly: for the
//! decay `x[n] = c^n` and the split's pole `b`, the low part is
//! `a (c^(n+1) - b^(n+1)) / (c - b)` and its sums are geometric series
//! (`energy` below, in `f64`, since `1 - c^2` is tiny for a long decay).
//! The clicks are independent in sign, so their powers add, and the power
//! of a channel is half of `density * E * (1/7)`. Gain `g` is
//!
//! ```text
//! g = REF / sqrt(E / sr) / sqrt(max(1, density / 50))
//! ```
//!
//! so up to 50 clicks a second every click keeps its own loudness whatever
//! its length and tone (a click is as loud as any other at the same
//! amplitude), and above 50 the clicks come down as they crowd, holding the
//! RMS where it was at 50. Below 50 it falls with the density, because a
//! sparse tick should not be pumped up to a bed. `g` is held under
//! [`MAX_GAIN`]: at a tenth of a millisecond the energy norm would ask for
//! more than a loud click can be. The measured contract is in the tests.
//!
//! **No DC.** A click is one-signed, so its mean is not zero. A gentle DC
//! blocker ([`DC_HZ`]) after the clicks takes what is left.
//!
//! Seeded with the voice's salt (`rng::salt`), as Noise is: a render is the
//! same every time and two tracks do not crackle in step.

use crate::rng::Rng;
use crate::smooth::OnePole;

/// A click of amplitude one, 1 ms long at the middle Tone, comes out of the
/// generator at a peak of one: `sqrt(E_s)` for that click is `sqrt(0.5 ms)`.
const REF: f32 = 0.022_360_68;

/// The density up to which clicks keep their level, and above which the RMS
/// is held.
const KNEE: f32 = 50.0;

/// The most the level norm may raise a click: 0.1 ms clicks would ask for
/// three and a bit.
const MAX_GAIN: f32 = 2.5;

/// Where Tone splits a click into its dark and bright parts.
const SPLIT_HZ: f32 = 800.0;

/// The DC blocker's corner.
const DC_HZ: f32 = 8.0;

/// How long a turned control takes to glide to a new value.
const GLIDE_MS: f32 = 20.0;

/// How often, in samples, the level and the decay's coefficient are worked
/// out again from the gliding rows.
const EVERY: u32 = 32;

#[derive(Debug, Clone, Copy)]
pub struct DustParams {
    /// Clicks a second, on average. 0.1 to 2000.
    pub density: f32,
    /// 0 dark thuds, 1 bright ticks.
    pub tone: f32,
    /// Each click's time constant, in milliseconds. 0.1 to 50.
    pub decay_ms: f32,
    /// 0 every click in the centre, 1 each at its own place.
    pub spread: f32,
}

impl Default for DustParams {
    fn default() -> Self {
        Self {
            density: 8.0,
            tone: 0.5,
            decay_ms: 1.0,
            spread: 0.5,
        }
    }
}

/// The summed squares of one click, `x[n] = c^n`, after Tone: `gl` of the
/// split's low part and `gh` of the high one. `c` is the decay and `b` the
/// split's pole, `1 - a`.
fn energy(c: f64, a: f64, tone: f64) -> f64 {
    let b = 1.0 - a;
    // The closed form divides by `c - b`. A hair either side of it changes
    // the decay by a thousandth and keeps the sums out of cancellation.
    let c = if (c - b).abs() < 1e-3 {
        b + if c >= b { 1e-3 } else { -1e-3 }
    } else {
        c
    };
    let (gl, gh) = if tone < 0.5 {
        (1.0, 2.0 * tone)
    } else {
        (2.0 * (1.0 - tone), 1.0)
    };
    let d = c - b;
    let ex = 1.0 / (1.0 - c * c);
    let el = a * a / (d * d)
        * (c * c / (1.0 - c * c) + b * b / (1.0 - b * b) - 2.0 * c * b / (1.0 - c * b));
    let exl = a / d * (c / (1.0 - c * c) - b / (1.0 - c * b));
    let k = gl - gh;
    (gh * gh * ex + 2.0 * gh * k * exl + k * k * el).max(1e-12)
}

#[derive(Debug, Clone)]
pub struct Dust {
    rng: Rng,
    sample_rate: f32,
    density: OnePole,
    tone: OnePole,
    decay: OnePole,
    spread: OnePole,
    /// The decay's states, per side.
    s: [f32; 2],
    /// The split's low-pass states, per side.
    low: [f32; 2],
    /// The DC blocker's last input and output, per side.
    dc_x: [f32; 2],
    dc_y: [f32; 2],
    /// The split's coefficient `a` and the blocker's pole.
    split_a: f32,
    dc_r: f32,
    /// Refreshed every [`EVERY`] samples.
    decay_c: f32,
    gl: f32,
    gh: f32,
    gain: f32,
    countdown: u32,
    clicks: u64,
}

impl Dust {
    pub fn new(sample_rate: f32) -> Self {
        Self::seeded(sample_rate, 0)
    }

    /// Seeded with `salt` mixed in (`rng::salt`); zero is `new`.
    pub fn seeded(sample_rate: f32, salt: u32) -> Self {
        let pole = |v: f32| {
            let mut p = OnePole::new();
            p.set_time(GLIDE_MS, sample_rate);
            p.reset(v);
            p
        };
        let d = DustParams::default();
        let mut dust = Self {
            rng: Rng::new(0xD057_0001 ^ salt),
            sample_rate,
            density: pole(d.density),
            tone: pole(d.tone),
            decay: pole(d.decay_ms),
            spread: pole(d.spread),
            s: [0.0; 2],
            low: [0.0; 2],
            dc_x: [0.0; 2],
            dc_y: [0.0; 2],
            split_a: 1.0
                - (-core::f32::consts::TAU * SPLIT_HZ.min(0.4 * sample_rate) / sample_rate).exp(),
            dc_r: 1.0 - core::f32::consts::TAU * DC_HZ / sample_rate,
            decay_c: 0.0,
            gl: 1.0,
            gh: 1.0,
            gain: 1.0,
            countdown: 0,
            clicks: 0,
        };
        dust.refresh();
        dust
    }

    /// Clicks started since this was made. For tests and the inspector.
    pub fn clicks(&self) -> u64 {
        self.clicks
    }

    /// The decay's coefficient, the tone's gains and the level, from the
    /// rows as they now glide.
    fn refresh(&mut self) {
        let density = self.density.value().clamp(0.1, 2000.0);
        let tone = self.tone.value().clamp(0.0, 1.0);
        let tau = (self.decay.value().clamp(0.1, 50.0) * 0.001 * self.sample_rate).max(1.0);
        let c = (-1.0 / tau as f64).exp();
        self.decay_c = c as f32;
        (self.gl, self.gh) = if tone < 0.5 {
            (1.0, 2.0 * tone)
        } else {
            (2.0 * (1.0 - tone), 1.0)
        };
        let e = energy(c, self.split_a as f64, tone as f64) as f32;
        let crowd = (density / KNEE).max(1.0);
        self.gain = (REF / (e / self.sample_rate).sqrt() / crowd.sqrt()).min(MAX_GAIN);
    }

    /// Every state back to rest. The source runs on, so a voice switched off
    /// and on again does not repeat the clicks it played before.
    pub fn reset(&mut self) {
        self.s = [0.0; 2];
        self.low = [0.0; 2];
        self.dc_x = [0.0; 2];
        self.dc_y = [0.0; 2];
    }

    /// Reset, with every row already at `p`, so nothing glides from a value
    /// left over from before. For a voice coming back from idle.
    pub fn settle(&mut self, p: &DustParams) {
        self.reset();
        self.density.reset(p.density.clamp(0.1, 2000.0));
        self.tone.reset(p.tone.clamp(0.0, 1.0));
        self.decay.reset(p.decay_ms.clamp(0.1, 50.0));
        self.spread.reset(p.spread.clamp(0.0, 1.0));
        self.refresh();
        self.countdown = 0;
    }

    /// One stereo sample.
    #[inline]
    pub fn process(&mut self, p: &DustParams) -> (f32, f32) {
        let density = self.density.process(p.density.clamp(0.1, 2000.0));
        self.tone.process(p.tone.clamp(0.0, 1.0));
        self.decay.process(p.decay_ms.clamp(0.1, 50.0));
        let spread = self.spread.process(p.spread.clamp(0.0, 1.0));
        if self.countdown == 0 {
            self.refresh();
            self.countdown = EVERY;
        }
        self.countdown -= 1;

        // A click: a Bernoulli trial a sample, which at these densities is
        // a Poisson process to well within what the ear can tell.
        if self.rng.next_f32() < (density / self.sample_rate).min(0.5) {
            let v = self.rng.next_f32();
            let sign = if self.rng.next_u32() & 0x8000_0000 != 0 {
                1.0
            } else {
                -1.0
            };
            let amp = sign * v * v * v;
            let place = self.rng.next_bipolar() * spread;
            let angle = (place + 1.0) * core::f32::consts::FRAC_PI_4;
            self.s[0] += amp * angle.cos();
            self.s[1] += amp * angle.sin();
            self.clicks += 1;
        }

        let mut out = [0.0f32; 2];
        for (i, o) in out.iter_mut().enumerate() {
            let x = self.s[i];
            self.s[i] = x * self.decay_c;
            self.low[i] += self.split_a * (x - self.low[i]);
            let toned = self.gh * x + (self.gl - self.gh) * self.low[i];
            let y = toned - self.dc_x[i] + self.dc_r * self.dc_y[i];
            self.dc_x[i] = toned;
            self.dc_y[i] = y;
            *o = y * self.gain;
        }
        (out[0], out[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn params(density: f32, tone: f32, decay_ms: f32, spread: f32) -> DustParams {
        DustParams {
            density,
            tone,
            decay_ms,
            spread,
        }
    }

    /// `secs` of dust at `p`, settled there first, and how many clicks it made.
    fn render(sr: f32, salt: u32, p: &DustParams, secs: f32) -> (Vec<f32>, Vec<f32>, u64) {
        let mut d = Dust::seeded(sr, salt);
        d.settle(p);
        let n = (sr * secs) as usize;
        let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for _ in 0..n {
            let (a, b) = d.process(p);
            l.push(a);
            r.push(b);
        }
        (l, r, d.clicks())
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    #[test]
    fn density_sets_how_many_clicks() {
        // Poisson: the count over ten seconds is within five standard
        // deviations (the root of the mean) of density times ten.
        for density in [0.5, 8.0, 50.0, 200.0, 1000.0, 2000.0] {
            let (_, _, n) = render(SR, 0, &params(density, 0.5, 1.0, 0.5), 10.0);
            let mean = density as f64 * 10.0;
            let off = (n as f64 - mean).abs();
            assert!(
                off <= 5.0 * mean.sqrt() + 1.0,
                "density {density}: {n} clicks against {mean}"
            );
        }
    }

    #[test]
    fn a_denser_dust_has_more_clicks() {
        let few = render(SR, 0, &params(20.0, 0.5, 1.0, 0.5), 5.0).2;
        let many = render(SR, 0, &params(400.0, 0.5, 1.0, 0.5), 5.0).2;
        assert!(many > 10 * few, "{few} against {many}");
    }

    #[test]
    fn the_gaps_are_random_not_a_grid() {
        // A grid would give one gap length. Count the gaps between
        // successive clicks at 100 a second, with each click found as the
        // sample the decay state jumps up.
        let p = params(100.0, 0.5, 1.0, 0.0);
        let mut d = Dust::seeded(SR, 0);
        d.settle(&p);
        let (mut last, mut gaps) = (0u64, Vec::new());
        for i in 0..(SR as u64 * 10) {
            d.process(&p);
            if d.clicks() != last {
                last = d.clicks();
                gaps.push(i);
            }
        }
        let g: Vec<f64> = gaps.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
        let mean = g.iter().sum::<f64>() / g.len() as f64;
        let var = g.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / g.len() as f64;
        // An exponential's standard deviation is its mean.
        assert!(
            (var.sqrt() / mean - 1.0).abs() < 0.15,
            "gap sd {} against mean {mean}",
            var.sqrt()
        );
    }

    #[test]
    fn loudness_holds_from_fifty_to_two_thousand_a_second() {
        // The contract: density is how many clicks, not how loud. Twenty
        // seconds each, for a few decays and tones, within 4 dB end to end.
        for (tone, decay) in [(0.5, 1.0), (0.0, 1.0), (1.0, 1.0), (0.5, 8.0), (0.5, 0.2)] {
            let levels: Vec<f32> = [50.0, 100.0, 250.0, 500.0, 1000.0, 2000.0]
                .iter()
                .map(|&density| {
                    let (l, r, _) = render(SR, 1, &params(density, tone, decay, 0.5), 20.0);
                    db(rms(&[l, r].concat()))
                })
                .collect();
            let (lo, hi) = levels
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            assert!(
                hi - lo <= 4.0,
                "tone {tone} decay {decay}: {levels:?} spans {} dB",
                hi - lo
            );
        }
    }

    #[test]
    fn a_sparse_tick_is_not_pumped_up_to_a_bed() {
        // Under the knee the clicks keep their level, so fewer of them is
        // quieter, not the same.
        let sparse = rms(&render(SR, 1, &params(5.0, 0.5, 1.0, 0.5), 20.0).0);
        let busy = rms(&render(SR, 1, &params(50.0, 0.5, 1.0, 0.5), 20.0).0);
        assert!(db(busy) - db(sparse) > 6.0, "{} {}", db(sparse), db(busy));
    }

    #[test]
    fn the_energy_is_that_of_a_click() {
        // The closed form against summing the click through the split.
        let a = 1.0 - (-core::f32::consts::TAU * SPLIT_HZ / SR).exp();
        let b = 1.0 - a;
        for decay_ms in [0.1, 1.0, 10.0, 50.0] {
            for tone in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let c = (-1.0 / (decay_ms * 0.001 * SR as f64)).exp();
                let (gl, gh) = if tone < 0.5 {
                    (1.0, 2.0 * tone)
                } else {
                    (2.0 * (1.0 - tone), 1.0)
                };
                let (mut x, mut low, mut sum) = (1.0f64, 0.0f64, 0.0f64);
                for _ in 0..(SR as usize * 2) {
                    low += a as f64 * (x - low);
                    let y = gh * x + (gl - gh) * low;
                    sum += y * y;
                    x *= c;
                }
                let e = energy(c, a as f64, tone);
                assert!(
                    (e / sum - 1.0).abs() < 0.01,
                    "decay {decay_ms} tone {tone}: {e} against {sum} (b {b})"
                );
            }
        }
    }

    #[test]
    fn there_is_no_dc() {
        for (density, decay) in [(8.0, 1.0), (300.0, 4.0), (2000.0, 50.0), (2.0, 50.0)] {
            let (l, r, _) = render(SR, 2, &params(density, 0.5, decay, 1.0), 10.0);
            for x in [l, r] {
                let mean = x.iter().sum::<f32>() / x.len() as f32;
                assert!(
                    mean.abs() < 0.02 * rms(&x).max(1e-6),
                    "density {density} decay {decay}: mean {mean}, rms {}",
                    rms(&x)
                );
            }
        }
    }

    #[test]
    fn it_is_finite_and_bounded_at_the_extremes_and_every_rate() {
        for sr in [22_050.0, 32_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for density in [0.1, 2000.0] {
                for tone in [0.0, 0.5, 1.0] {
                    for decay in [0.1, 50.0] {
                        for spread in [0.0, 1.0] {
                            let p = params(density, tone, decay, spread);
                            let (l, r, _) = render(sr, 3, &p, 1.0);
                            for v in l.iter().chain(&r) {
                                assert!(v.is_finite() && v.abs() < 8.0, "sr {sr} {p:?}: {v}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_loudness_holds_at_other_rates_too() {
        for sr in [22_050.0, 96_000.0, 192_000.0] {
            let at = |density| {
                let (l, r, _) = render(sr, 1, &params(density, 0.5, 1.0, 0.5), 20.0);
                db(rms(&[l, r].concat()))
            };
            let (a, b) = (at(50.0), at(2000.0));
            assert!((a - b).abs() <= 4.0, "{sr}: {a} against {b}");
        }
    }

    #[test]
    fn it_is_seeded_and_salted() {
        let p = params(300.0, 0.5, 1.0, 1.0);
        let a = render(SR, 0, &p, 1.0);
        let b = render(SR, 0, &p, 1.0);
        let c = render(SR, 1, &p, 1.0);
        assert_eq!(a.0, b.0, "the same seed repeats");
        assert_eq!(a.1, b.1);
        assert!(a.0 != c.0, "a different voice does not crackle in step");
    }

    #[test]
    fn spread_places_clicks() {
        let (l, r, _) = render(SR, 0, &params(200.0, 0.5, 1.0, 0.0), 2.0);
        for (a, b) in l.iter().zip(&r) {
            assert!(
                (a - b).abs() <= 1e-4 * a.abs().max(b.abs()) + 1e-9,
                "{a} {b}"
            );
        }
        let (l, r, _) = render(SR, 0, &params(200.0, 0.5, 1.0, 1.0), 2.0);
        let diff = rms(&l.iter().zip(&r).map(|(a, b)| a - b).collect::<Vec<_>>());
        assert!(diff > 0.3 * rms(&l), "wide dust differs between sides");
        // Constant power: the sides together are as loud as at the centre.
        let (cl, cr, _) = render(SR, 0, &params(200.0, 0.5, 1.0, 0.0), 2.0);
        let wide = (rms(&l).powi(2) + rms(&r).powi(2)).sqrt();
        let mid = (rms(&cl).powi(2) + rms(&cr).powi(2)).sqrt();
        assert!((db(wide) - db(mid)).abs() < 2.0, "{} {}", db(wide), db(mid));
    }

    #[test]
    fn tone_runs_from_thuds_to_ticks() {
        // The share of a signal's energy in its first difference rises with
        // its brightness.
        let bright = |tone| {
            let (l, _, _) = render(SR, 0, &params(100.0, tone, 4.0, 0.5), 5.0);
            let d: Vec<f32> = l.windows(2).map(|w| w[1] - w[0]).collect();
            rms(&d) / rms(&l)
        };
        let (dark, mid, bright_) = (bright(0.0), bright(0.5), bright(1.0));
        assert!(dark < mid && mid < bright_, "{dark} {mid} {bright_}");
        assert!(bright_ > 5.0 * dark, "{dark} against {bright_}");
    }

    #[test]
    fn most_clicks_are_quiet_and_some_are_loud() {
        // The clicks of a sparse dust, found by their first sample's size.
        let p = params(20.0, 0.5, 5.0, 0.0);
        let mut d = Dust::seeded(SR, 0);
        d.settle(&p);
        let (mut last, mut sizes) = (0, Vec::new());
        for _ in 0..(SR as usize * 120) {
            let before: f32 = d.s[0].abs();
            d.process(&p);
            if d.clicks() != last {
                last = d.clicks();
                sizes.push(
                    (d.s[0].abs() - before * d.decay_c).abs() / core::f32::consts::FRAC_1_SQRT_2,
                );
            }
        }
        sizes.sort_by(f32::total_cmp);
        let median = sizes[sizes.len() / 2];
        let top = sizes[sizes.len() * 99 / 100];
        assert!(median < 0.25, "median click {median}");
        assert!(top > 0.8, "loud clicks {top}");
    }
}
