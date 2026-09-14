//! The filter, on the master.
//!
//! A state-variable filter in the trapezoidal form (Simper, 2013). One
//! circuit gives low-pass, high-pass and band-pass at once, it stays stable
//! when the cutoff is swept fast, and it costs a handful of multiplies. Sits
//! after crush and ring, which add harmonics, so it is what takes them away
//! again (Georg, 2026-09-14: a filter on the master, not an EQ; an EQ waits
//! for several patches sounding at once).
//!
//! Cutoff and resonance are smoothed here, like the ring modulator's carrier,
//! because a zipper on a resonant filter is a click.

use crate::smooth::OnePole;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterType {
    LowPass,
    HighPass,
    BandPass,
}

impl FilterType {
    pub const ALL: [FilterType; 3] = [
        FilterType::LowPass,
        FilterType::HighPass,
        FilterType::BandPass,
    ];
    pub const NAMES: [&'static str; 3] = ["Low-pass", "High-pass", "Band-pass"];

    /// From the stepped parameter's value.
    pub fn from_value(v: f32) -> FilterType {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FilterParams {
    pub cutoff_hz: f32,
    /// 0 to 1. Zero is a Butterworth-ish slope, one rings hard.
    pub resonance: f32,
    pub kind: FilterType,
    /// Dry to wet, 0 to 1.
    pub mix: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 20_000.0,
            resonance: 0.2,
            kind: FilterType::LowPass,
            mix: 0.0,
        }
    }
}

/// The lowest Q, where resonance is zero. Just under critical damping, so a
/// closed low-pass has no bump at the corner.
const Q_MIN: f32 = 0.5;
/// The highest Q. Loud, but bounded: it will not run away.
const Q_MAX: f32 = 20.0;

#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    ic1eq: f32,
    ic2eq: f32,
}

impl Channel {
    #[inline]
    fn tick(&mut self, v0: f32, g: f32, k: f32, kind: FilterType) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = v0 - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        match kind {
            FilterType::LowPass => v2,
            FilterType::BandPass => v1,
            FilterType::HighPass => v0 - k * v1 - v2,
        }
    }
}

pub struct Filter {
    sample_rate: f32,
    left: Channel,
    right: Channel,
    cutoff: OnePole,
    resonance: OnePole,
    mix: OnePole,
}

impl Filter {
    pub fn new(sample_rate: f32) -> Self {
        let d = FilterParams::default();
        let mut cutoff = OnePole::new();
        let mut resonance = OnePole::new();
        let mut mix = OnePole::new();
        cutoff.set_time(20.0, sample_rate);
        resonance.set_time(20.0, sample_rate);
        mix.set_time(20.0, sample_rate);
        cutoff.reset(d.cutoff_hz);
        resonance.reset(d.resonance);
        mix.reset(d.mix);
        Self {
            sample_rate,
            left: Channel::default(),
            right: Channel::default(),
            cutoff,
            resonance,
            mix,
        }
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &FilterParams) -> (f32, f32) {
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Land on a true zero, as the ring modulator does, so "off" is
        // bit-exact dry. The state keeps running so switching back in does
        // not start from a thump.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };
        // Below a fifth of Nyquist the tangent is tame; above it the filter
        // is still stable but the shape flattens, so cap the cutoff there.
        let max_hz = self.sample_rate * 0.45;
        let hz = self.cutoff.process(p.cutoff_hz.clamp(5.0, max_hz));
        let res = self.resonance.process(p.resonance.clamp(0.0, 1.0));
        let g = (core::f32::consts::PI * hz / self.sample_rate).tan();
        let q = Q_MIN + (Q_MAX - Q_MIN) * res * res;
        let k = 1.0 / q;
        let wl = self.left.tick(l, g, k, p.kind);
        let wr = self.right.tick(r, g, k, p.kind);
        // Filters at high Q ring past unity; keep the state finite whatever
        // comes in, so a wild sweep cannot lock the filter into NaN.
        if !(wl.is_finite() && wr.is_finite()) {
            self.left = Channel::default();
            self.right = Channel::default();
            return (l, r);
        }
        (l + (wl - l) * mix, r + (wr - r) * mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn run(f: &mut Filter, p: &FilterParams, hz: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = (core::f32::consts::TAU * hz * i as f32 / SR).sin();
                f.process(x, x, p).0
            })
            .collect()
    }

    fn settle(f: &mut Filter, p: &FilterParams) {
        for _ in 0..SR as usize {
            f.process(0.0, 0.0, p);
        }
    }

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        let mut f = Filter::new(SR);
        let on = FilterParams {
            cutoff_hz: 300.0,
            mix: 1.0,
            ..Default::default()
        };
        let off = FilterParams { mix: 0.0, ..on };
        for _ in 0..4_800 {
            f.process(0.5, 0.5, &on);
        }
        for _ in 0..48_000 {
            f.process(0.5, 0.5, &off);
        }
        for _ in 0..1_000 {
            assert_eq!(f.process(0.5, -0.25, &off), (0.5, -0.25));
        }
    }

    #[test]
    fn low_pass_keeps_lows_and_loses_highs() {
        let p = FilterParams {
            cutoff_hz: 500.0,
            resonance: 0.0,
            kind: FilterType::LowPass,
            mix: 1.0,
        };
        let mut f = Filter::new(SR);
        settle(&mut f, &p);
        let low = rms(&run(&mut f, &p, 50.0, 48_000)[24_000..]);
        let mut f = Filter::new(SR);
        settle(&mut f, &p);
        let high = rms(&run(&mut f, &p, 8_000.0, 48_000)[24_000..]);
        let unity =
            rms(&run(&mut Filter::new(SR), &FilterParams::default(), 50.0, 48_000)[24_000..]);
        assert!(
            (low - unity).abs() < unity * 0.05,
            "lows changed: {low} vs {unity}"
        );
        assert!(high < unity * 0.02, "highs survived: {high}");
    }

    #[test]
    fn high_pass_is_the_mirror() {
        let p = FilterParams {
            cutoff_hz: 2_000.0,
            resonance: 0.0,
            kind: FilterType::HighPass,
            mix: 1.0,
        };
        let mut f = Filter::new(SR);
        settle(&mut f, &p);
        let low = rms(&run(&mut f, &p, 50.0, 48_000)[24_000..]);
        let mut f = Filter::new(SR);
        settle(&mut f, &p);
        let high = rms(&run(&mut f, &p, 12_000.0, 48_000)[24_000..]);
        assert!(
            low < high * 0.02,
            "lows survived a high-pass: {low} vs {high}"
        );
    }

    #[test]
    fn band_pass_prefers_the_centre() {
        let p = FilterParams {
            cutoff_hz: 1_000.0,
            resonance: 0.5,
            kind: FilterType::BandPass,
            mix: 1.0,
        };
        let at = |hz: f32| {
            let mut f = Filter::new(SR);
            settle(&mut f, &p);
            rms(&run(&mut f, &p, hz, 48_000)[24_000..])
        };
        let (lo, mid, hi) = (at(60.0), at(1_000.0), at(12_000.0));
        assert!(mid > lo * 4.0 && mid > hi * 4.0, "{lo} {mid} {hi}");
    }

    #[test]
    fn a_fast_resonant_sweep_stays_finite_and_bounded() {
        let mut f = Filter::new(SR);
        let mut peak = 0.0f32;
        for i in 0..SR as usize * 4 {
            let t = i as f32 / SR;
            let p = FilterParams {
                cutoff_hz: 20.0 * (1_000.0f32).powf((t * 7.0).sin() * 0.5 + 0.5),
                resonance: 1.0,
                kind: FilterType::ALL[(i / 7_000) % 3],
                mix: 1.0,
            };
            let x = (i as f32 * 0.37).sin() * 0.5 + (i as f32 * 0.011).sin() * 0.5;
            let (l, r) = f.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite(), "went non-finite at {i}");
            peak = peak.max(l.abs());
        }
        // Q of 20 is loud but it is not infinite.
        assert!(peak < 40.0, "resonance ran away: {peak}");
    }

    #[test]
    fn silence_in_silence_out() {
        let mut f = Filter::new(SR);
        let p = FilterParams {
            cutoff_hz: 800.0,
            resonance: 0.9,
            mix: 1.0,
            ..Default::default()
        };
        for _ in 0..10_000 {
            assert_eq!(f.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn the_type_is_read_from_the_stepped_value() {
        assert_eq!(FilterType::from_value(0.0), FilterType::LowPass);
        assert_eq!(FilterType::from_value(1.2), FilterType::HighPass);
        assert_eq!(FilterType::from_value(2.0), FilterType::BandPass);
        assert_eq!(FilterType::from_value(9.0), FilterType::BandPass);
        assert_eq!(FilterType::from_value(-1.0), FilterType::LowPass);
    }
}
