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

use crate::smooth::{OnePole, Ramp};

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

/// How steeply the filter falls past its cutoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterSlope {
    /// One stage, 12 dB per octave.
    Db12,
    /// Two stages in series, 24 dB per octave.
    Db24,
}

impl FilterSlope {
    pub const NAMES: [&'static str; 2] = ["12 dB", "24 dB"];

    /// From the stepped parameter's value.
    pub fn from_value(v: f32) -> FilterSlope {
        if v.round() >= 1.0 {
            FilterSlope::Db24
        } else {
            FilterSlope::Db12
        }
    }

    fn target(self) -> f32 {
        match self {
            FilterSlope::Db12 => 0.0,
            FilterSlope::Db24 => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FilterParams {
    pub cutoff_hz: f32,
    /// 0 to 1. Zero is a Butterworth-ish slope, one rings hard.
    pub resonance: f32,
    pub kind: FilterType,
    pub slope: FilterSlope,
    /// Dry to wet, 0 to 1.
    pub mix: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 20_000.0,
            resonance: 0.2,
            kind: FilterType::LowPass,
            slope: FilterSlope::Db12,
            mix: 0.0,
        }
    }
}

/// The lowest Q, where resonance is zero. Just under critical damping, so a
/// closed low-pass has no bump at the corner.
const Q_MIN: f32 = 0.5;
/// The highest Q. Loud, but bounded: it will not run away.
const Q_MAX: f32 = 20.0;
/// The first stage of the 24 dB slope, fixed. A stage's gain at its cutoff is
/// its Q, so with the second stage at `Q_MIN` the pair sits at 0.5 there, the
/// same as one stage at `Q_MIN`: flipping the slope at resonance zero does
/// not move the corner. The second stage takes the resonance alone, so the
/// peak at full is the 12 dB filter's, and resonance does not stack.
const Q_FIRST: f32 = 1.0;
/// How long the crossfade between the slopes takes.
const SLOPE_MS: f32 = 10.0;

#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    ic1eq: f32,
    ic2eq: f32,
}

/// One stage's coefficients, shared by both channels.
#[derive(Clone, Copy)]
struct Coeffs {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Coeffs {
    #[inline]
    fn new(g: f32, k: f32) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        Self { k, a1, a2, a3 }
    }
}

impl Channel {
    #[inline]
    fn tick(&mut self, v0: f32, c: Coeffs, kind: FilterType) -> f32 {
        let Coeffs { k, a1, a2, a3 } = c;
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

/// One channel's stages. `single` is the 12 dB filter; `first` then `last`
/// are the 24 dB pair. All three run whenever the filter does, so the pair is
/// warm when the slope is turned up.
#[derive(Debug, Clone, Copy, Default)]
struct Stages {
    single: Channel,
    first: Channel,
    last: Channel,
}

impl Stages {
    /// The 12 dB and 24 dB outputs for one input.
    #[inline]
    fn tick(&mut self, x: f32, single: Coeffs, first: Coeffs, kind: FilterType) -> (f32, f32) {
        let w12 = self.single.tick(x, single, kind);
        let mid = self.first.tick(x, first, kind);
        // The last stage has the resonance, so it shares the 12 dB filter's
        // coefficients.
        let w24 = self.last.tick(mid, single, kind);
        (w12, w24)
    }
}

pub struct Filter {
    sample_rate: f32,
    left: Stages,
    right: Stages,
    cutoff: OnePole,
    resonance: OnePole,
    mix: OnePole,
    /// 0 is 12 dB, 1 is 24 dB; the crossfade between them.
    slope: Ramp,
}

impl Filter {
    pub fn new(sample_rate: f32) -> Self {
        let d = FilterParams::default();
        let mut cutoff = OnePole::new();
        let mut resonance = OnePole::new();
        let mut mix = OnePole::new();
        let mut slope = Ramp::new(d.slope.target());
        slope.set_time(SLOPE_MS, sample_rate);
        cutoff.set_time(20.0, sample_rate);
        resonance.set_time(20.0, sample_rate);
        mix.set_time(20.0, sample_rate);
        cutoff.reset(d.cutoff_hz);
        resonance.reset(d.resonance);
        mix.reset(d.mix);
        Self {
            sample_rate,
            left: Stages::default(),
            right: Stages::default(),
            cutoff,
            resonance,
            mix,
            slope,
        }
    }

    /// Nothing ringing, and the smoothers where `p` puts them, as if it had
    /// always been set so. For a voice coming back from idle.
    pub fn reset(&mut self, p: &FilterParams) {
        self.left = Stages::default();
        self.right = Stages::default();
        self.slope.set(p.slope.target());
        self.cutoff.reset(p.cutoff_hz);
        self.resonance.reset(p.resonance);
        self.mix.reset(p.mix);
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
        let single = Coeffs::new(g, k);
        let first = Coeffs::new(g, 1.0 / Q_FIRST);
        let (wl12, wl24) = self.left.tick(l, single, first, p.kind);
        let (wr12, wr24) = self.right.tick(r, single, first, p.kind);
        // Crossfade the slopes. The ramp lands on an exact zero, so 12 dB is
        // bit-exact with a filter that has no second stage; the stages keep
        // running either way.
        let slope = self.slope.process(p.slope.target());
        // Filters at high Q ring past unity; keep the state finite whatever
        // comes in, so a wild sweep cannot lock the filter into NaN.
        if !(wl12.is_finite() && wr12.is_finite() && wl24.is_finite() && wr24.is_finite()) {
            self.left = Stages::default();
            self.right = Stages::default();
            return (l, r);
        }
        let (wl, wr) = if slope == 0.0 {
            (wl12, wr12)
        } else {
            (wl12 + (wl24 - wl12) * slope, wr12 + (wr24 - wr12) * slope)
        };
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
                ..Default::default()
            };
            let x = (i as f32 * 0.37).sin() * 0.5 + (i as f32 * 0.011).sin() * 0.5;
            let (l, r) = f.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite(), "went non-finite at {i}");
            peak = peak.max(l.abs());
        }
        // Q of 20 is loud but it is not infinite.
        assert!(peak < 40.0, "resonance ran away: {peak}");
    }

    fn level(p: &FilterParams, hz: f32) -> f32 {
        let mut f = Filter::new(SR);
        settle(&mut f, p);
        rms(&run(&mut f, p, hz, 48_000)[24_000..]) * core::f32::consts::SQRT_2
    }

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    fn at_slope(kind: FilterType, slope: FilterSlope, resonance: f32) -> FilterParams {
        FilterParams {
            cutoff_hz: 1_000.0,
            resonance,
            kind,
            slope,
            mix: 1.0,
        }
    }

    #[test]
    fn the_slope_falls_twelve_or_twenty_four_db_an_octave() {
        // Low-pass well above the cutoff, high-pass well below it: each
        // octave further out is down by the slope. Tones at 4x and 8x the
        // cutoff (a quarter and an eighth, for the high-pass).
        for (slope, want) in [(FilterSlope::Db12, 12.0), (FilterSlope::Db24, 24.0)] {
            let lp = at_slope(FilterType::LowPass, slope, 0.0);
            let drop = db(level(&lp, 4_000.0)) - db(level(&lp, 8_000.0));
            println!("low-pass {slope:?}: {drop:.2} dB/oct");
            assert!(
                (drop - want).abs() < 1.5,
                "low-pass {slope:?}: {drop} dB/oct"
            );
            let hp = at_slope(FilterType::HighPass, slope, 0.0);
            let drop = db(level(&hp, 250.0)) - db(level(&hp, 125.0));
            println!("high-pass {slope:?}: {drop:.2} dB/oct");
            assert!(
                (drop - want).abs() < 1.5,
                "high-pass {slope:?}: {drop} dB/oct"
            );
        }
    }

    #[test]
    fn flipping_the_slope_leaves_the_corner_where_it_was() {
        // At resonance zero the level at the cutoff is within 1.5 dB between
        // the slopes, for each type; with resonance up the peak stays the
        // 12 dB filter's to within the same tolerance.
        for kind in FilterType::ALL {
            for res in [0.0, 0.5, 1.0] {
                let a = db(level(&at_slope(kind, FilterSlope::Db12, res), 1_000.0));
                let b = db(level(&at_slope(kind, FilterSlope::Db24, res), 1_000.0));
                println!("corner {kind:?} res {res}: 12 dB {a:.2}, 24 dB {b:.2}");
                assert!((a - b).abs() < 1.5, "{kind:?} res {res}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn twelve_db_is_the_single_stage_whatever_the_second_does() {
        // A filter that spends the middle of the run at 24 dB lands back on
        // the same bits as one that never left 12 dB, once the 10 ms
        // crossfade has finished.
        let twelve = at_slope(FilterType::LowPass, FilterSlope::Db12, 0.6);
        let twenty_four = FilterParams {
            slope: FilterSlope::Db24,
            ..twelve
        };
        let mut a = Filter::new(SR);
        let mut b = Filter::new(SR);
        for i in 0..96_000 {
            let x = (i as f32 * 0.05).sin() * 0.7 + (i as f32 * 0.9).sin() * 0.2;
            let pb = if (30_000..60_000).contains(&i) {
                &twenty_four
            } else {
                &twelve
            };
            let ya = a.process(x, -x, &twelve);
            let yb = b.process(x, -x, pb);
            if !(30_000..60_000 + 500).contains(&i) {
                assert_eq!(ya, yb, "sample {i}");
            }
        }
    }

    #[test]
    fn switching_the_slope_does_not_click() {
        let worst = |toggle: bool| {
            let mut f = Filter::new(SR);
            let mut prev = 0.0f32;
            let mut worst = 0.0f32;
            for i in 0..96_000 {
                let slope = if toggle && (i / 4_000) % 2 == 1 {
                    FilterSlope::Db24
                } else {
                    FilterSlope::Db12
                };
                let p = FilterParams {
                    cutoff_hz: 800.0,
                    resonance: 0.5,
                    kind: FilterType::LowPass,
                    slope,
                    mix: 1.0,
                };
                let x = (core::f32::consts::TAU * 220.0 * i as f32 / SR).sin() * 0.8
                    + (core::f32::consts::TAU * 3_000.0 * i as f32 / SR).sin() * 0.1;
                let (l, _) = f.process(x, x, &p);
                if i > 4_800 {
                    worst = worst.max((l - prev).abs());
                }
                prev = l;
            }
            worst
        };
        let steady = worst(false);
        let switched = worst(true);
        println!("slope toggle jump {switched}, steady {steady}");
        assert!(
            switched < steady * 1.5,
            "toggling the slope stepped {switched} vs {steady}"
        );
    }

    #[test]
    fn a_fast_resonant_sweep_stays_finite_and_bounded_at_24_db() {
        let mut f = Filter::new(SR);
        let mut peak = 0.0f32;
        for i in 0..SR as usize * 4 {
            let t = i as f32 / SR;
            let p = FilterParams {
                cutoff_hz: 20.0 * (1_000.0f32).powf((t * 7.0).sin() * 0.5 + 0.5),
                resonance: 1.0,
                kind: FilterType::ALL[(i / 7_000) % 3],
                slope: if (i / 11_000) % 3 == 0 {
                    FilterSlope::Db12
                } else {
                    FilterSlope::Db24
                },
                mix: 1.0,
            };
            let x = (i as f32 * 0.37).sin() * 0.5 + (i as f32 * 0.011).sin() * 0.5;
            let (l, r) = f.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite(), "went non-finite at {i}");
            peak = peak.max(l.abs());
        }
        assert!(peak < 40.0, "resonance ran away: {peak}");
    }

    #[test]
    fn the_slope_is_read_from_the_stepped_value() {
        assert_eq!(FilterSlope::from_value(0.0), FilterSlope::Db12);
        assert_eq!(FilterSlope::from_value(0.4), FilterSlope::Db12);
        assert_eq!(FilterSlope::from_value(1.0), FilterSlope::Db24);
        assert_eq!(FilterSlope::from_value(7.0), FilterSlope::Db24);
        assert_eq!(FilterSlope::from_value(-1.0), FilterSlope::Db12);
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
