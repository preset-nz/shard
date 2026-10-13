//! Peaks: two narrow, boosted bands.
//!
//! Two peaking (bell) filters in series, Low and High, each a boost of up to
//! 36 dB. On noise or a dense sound they ring like resonators; swept by an
//! LFO they whistle and squeal. The dry sound stays underneath: this is a
//! peaking EQ, not a band-pass.
//!
//! Each band is Cytomic's state-variable bell on a trapezoidal (TPT)
//! integrator pair: `out = in + k (A^2 - 1) * bandpass`, with `A = 10^(dB/40)`
//! and `k = 1 / (Q A)`. The gain at the centre is exactly `A^2`, the boost.
//! A TPT filter is stable for any coefficients, so a centre driven round the
//! spectrum at LFO speed (or a coefficient step) cannot blow up; and its state
//! is the integrators' memory, which stays continuous when the coefficients
//! move, so a sweep does not click.
//!
//! **Width** is one control for both bands: Q runs exponentially from
//! [`Q_NARROW`] (30, a whistle about a thirtieth of the centre wide) at 0 to
//! [`Q_BROAD`] (0.7, a tone colour) at 1: `Q = 0.7 * (30 / 0.7)^(1 - width)`.
//!
//! **Make-up.** A narrow boost adds little power to broadband sound and a broad
//! one a lot, so the boost is not made up in full (that would take the other
//! half of the spectrum down by the boost). The analytic noise-power gain of a
//! bell, `1 + pi f0 (A^4 - 1) / (Q A sr)` (the Lorentzian approximation of its
//! band-pass), is worked out for both bands and the output scaled by the
//! fourth root of it: half the rise in decibels is taken back. Contract, held
//! by a test: white noise at the default width changes by no more than 6 dB
//! in RMS from Boost 0 to 36 dB. A sine on a band's centre still gains
//! `Boost - make-up`.
//!
//! **Limiter.** The wet path ends in a soft limiter: exactly linear up to
//! 1.0, then a tanh curve with matched slope to a ceiling of 2.0 (+6 dBFS), so
//! a narrow full boost on loud noise cannot pass +6 dBFS and ordinary levels
//! pass untouched.

use crate::smooth::OnePole;

/// Lowest band centre, in hertz.
pub const FREQ_MIN_HZ: f32 = 40.0;
/// Highest band centre, in hertz. Clamped lower at low sample rates.
pub const FREQ_MAX_HZ: f32 = 12_000.0;
/// The most a band can stand out, in decibels.
pub const BOOST_MAX_DB: f32 = 36.0;
/// Q at Width 0: narrow, ringing, whistling.
pub const Q_NARROW: f32 = 30.0;
/// Q at Width 1: broad, a tone colour.
pub const Q_BROAD: f32 = 0.7;
/// Where the limiter starts to bend, and where it stops.
const KNEE: f32 = 1.0;
const CEILING: f32 = 2.0;
/// Centres never pass this fraction of the sample rate.
const MAX_FRACTION_OF_SR: f32 = 0.45;

#[derive(Debug, Clone, Copy)]
pub struct PeaksParams {
    /// First band's centre in hertz.
    pub low_hz: f32,
    /// Second band's centre in hertz.
    pub high_hz: f32,
    /// Boost of both bands in decibels, 0 to 36.
    pub boost_db: f32,
    /// 0 narrow to 1 broad.
    pub width: f32,
    /// Dry to wet, 0 to 1.
    pub mix: f32,
}

impl Default for PeaksParams {
    fn default() -> Self {
        Self {
            low_hz: 400.0,
            high_hz: 2_400.0,
            boost_db: 18.0,
            width: 0.2,
            mix: 0.0,
        }
    }
}

/// Q for a width in 0 to 1.
pub fn q_for(width: f32) -> f32 {
    Q_BROAD * (Q_NARROW / Q_BROAD).powf(1.0 - width.clamp(0.0, 1.0))
}

/// Analytic noise-power gain of one bell.
fn band_power(f0: f32, boost_db: f32, q: f32, sr: f32) -> f32 {
    let a = 10f32.powf(boost_db / 40.0);
    let a4 = 10f32.powf(boost_db / 10.0);
    1.0 + core::f32::consts::PI * f0 * (a4 - 1.0) / (q * a * sr)
}

/// The make-up gain (a factor, at most 1) for the pair of bands.
pub(crate) fn makeup(low_hz: f32, high_hz: f32, boost_db: f32, width: f32, sr: f32) -> f32 {
    let q = q_for(width);
    let p = band_power(low_hz, boost_db, q, sr) * band_power(high_hz, boost_db, q, sr);
    p.powf(-0.25)
}

/// Linear to `KNEE`, then a tanh bend with the same slope to `CEILING`.
#[inline]
fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let room = CEILING - KNEE;
        (KNEE + room * ((a - KNEE) / room).tanh()).copysign(x)
    }
}

/// One bell's coefficients.
#[derive(Debug, Clone, Copy, Default)]
struct Coef {
    a1: f32,
    a2: f32,
    a3: f32,
    m1: f32,
}

impl Coef {
    fn new(f0: f32, boost_db: f32, q: f32, sr: f32) -> Self {
        let a = 10f32.powf(boost_db / 40.0);
        let g = (core::f32::consts::PI * f0 / sr).tan();
        let k = 1.0 / (q * a);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            m1: k * (a * a - 1.0),
        }
    }
}

/// Integrator memory of one band on one channel.
#[derive(Debug, Clone, Copy, Default)]
struct Mem {
    ic1: f32,
    ic2: f32,
}

impl Mem {
    #[inline]
    fn tick(&mut self, c: &Coef, x: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        x + c.m1 * v1
    }
}

pub struct Peaks {
    sample_rate: f32,
    /// Smoothed in the log domain: ln hertz, ln Q and decibels.
    ln_low: OnePole,
    ln_high: OnePole,
    ln_q: OnePole,
    boost: OnePole,
    mix: OnePole,
    low: [Mem; 2],
    high: [Mem; 2],
    coef: [Coef; 2],
    gain: f32,
    /// The smoothed values the coefficients were last made from, so a held
    /// setting costs no trigonometry.
    last: [f32; 4],
}

impl Peaks {
    pub fn new(sample_rate: f32) -> Self {
        let mut s = Self {
            sample_rate,
            ln_low: OnePole::new(),
            ln_high: OnePole::new(),
            ln_q: OnePole::new(),
            boost: OnePole::new(),
            mix: OnePole::new(),
            low: [Mem::default(); 2],
            high: [Mem::default(); 2],
            coef: [Coef::default(); 2],
            gain: 1.0,
            last: [f32::NAN; 4],
        };
        for o in [
            &mut s.ln_low,
            &mut s.ln_high,
            &mut s.ln_q,
            &mut s.boost,
            &mut s.mix,
        ] {
            o.set_time(20.0, sample_rate);
        }
        let d = PeaksParams::default();
        s.ln_low.reset(s.clamp_hz(d.low_hz).ln());
        s.ln_high.reset(s.clamp_hz(d.high_hz).ln());
        s.ln_q.reset(q_for(d.width).ln());
        s.boost.reset(d.boost_db);
        s.mix.reset(d.mix);
        s
    }

    fn clamp_hz(&self, hz: f32) -> f32 {
        hz.clamp(
            FREQ_MIN_HZ,
            FREQ_MAX_HZ.min(MAX_FRACTION_OF_SR * self.sample_rate),
        )
    }

    /// Zeroes the filters' memory. The smoothers stay where they are.
    pub fn reset(&mut self) {
        self.low = [Mem::default(); 2];
        self.high = [Mem::default(); 2];
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &PeaksParams) -> (f32, f32) {
        let ln_low = self.ln_low.process(self.clamp_hz(p.low_hz).ln());
        let ln_high = self.ln_high.process(self.clamp_hz(p.high_hz).ln());
        let ln_q = self.ln_q.process(q_for(p.width).ln());
        let boost = self.boost.process(p.boost_db.clamp(0.0, BOOST_MAX_DB));
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Snap the last of a fade to a true zero, so off is bit-exact.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        let now = [ln_low, ln_high, ln_q, boost];
        if now != self.last {
            self.last = now;
            let (fl, fh, q) = (ln_low.exp(), ln_high.exp(), ln_q.exp());
            self.coef = [
                Coef::new(fl, boost, q, self.sample_rate),
                Coef::new(fh, boost, q, self.sample_rate),
            ];
            self.gain = makeup(fl, fh, boost, q_to_width(q), self.sample_rate);
        }

        let mut out = [l, r];
        for (ch, x) in out.iter_mut().enumerate() {
            let wet = self.low[ch].tick(&self.coef[0], *x);
            let wet = self.high[ch].tick(&self.coef[1], wet);
            let wet = limit(wet * self.gain);
            *x = *x * (1.0 - mix) + wet * mix;
        }
        (out[0], out[1])
    }
}

/// The width that gives a Q (the inverse of `q_for`), so the make-up can be
/// made from the smoothed Q.
fn q_to_width(q: f32) -> f32 {
    1.0 - (q / Q_BROAD).ln() / (Q_NARROW / Q_BROAD).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn wet(p: PeaksParams) -> PeaksParams {
        PeaksParams { mix: 1.0, ..p }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    fn settle(pk: &mut Peaks, p: &PeaksParams) {
        for _ in 0..(SR as usize / 2) {
            pk.process(0.0, 0.0, p);
        }
    }

    /// RMS gain of a sine through the settled effect, in decibels.
    fn sine_gain_db(p: PeaksParams, hz: f32, amp: f32, sr: f32) -> f32 {
        let mut pk = Peaks::new(sr);
        let n = sr as usize;
        let mut out = Vec::new();
        for i in 0..n {
            let x = amp * (core::f32::consts::TAU * hz * i as f32 / sr).sin();
            let (o, _) = pk.process(x, x, &p);
            if i > n / 2 {
                out.push(o);
            }
        }
        db(rms(&out) / (amp / 2f32.sqrt()))
    }

    #[test]
    fn a_mix_at_zero_is_exactly_transparent() {
        let mut pk = Peaks::new(SR);
        let on = wet(PeaksParams::default());
        let off = PeaksParams::default();
        for _ in 0..4_800 {
            pk.process(0.5, 0.5, &on);
        }
        for _ in 0..48_000 {
            pk.process(0.5, 0.5, &off);
        }
        let mut s = 1u32;
        for _ in 0..1_000 {
            let (a, b) = (noise(&mut s), noise(&mut s));
            assert_eq!(pk.process(a, b, &off), (a, b));
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut pk = Peaks::new(SR);
        let p = wet(PeaksParams::default());
        for _ in 0..10_000 {
            assert_eq!(pk.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn reset_leaves_exact_silence() {
        let mut pk = Peaks::new(SR);
        let p = wet(PeaksParams {
            boost_db: 36.0,
            width: 0.0,
            ..Default::default()
        });
        let mut s = 7u32;
        for _ in 0..48_000 {
            pk.process(noise(&mut s), noise(&mut s), &p);
        }
        pk.reset();
        for _ in 0..48_000 {
            assert_eq!(pk.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn boost_zero_at_full_mix_is_dry() {
        for width in [0.0, 0.2, 1.0] {
            let p = wet(PeaksParams {
                boost_db: 0.0,
                width,
                ..Default::default()
            });
            for hz in [100.0, 400.0, 1_000.0, 2_400.0, 8_000.0] {
                let g = sine_gain_db(p, hz, 0.5, SR);
                assert!(g.abs() < 0.1, "width {width} at {hz} Hz: {g} dB");
            }
        }
    }

    #[test]
    fn a_sine_at_a_band_is_boosted_by_the_boost_and_an_octave_away_much_less() {
        let boost = 18.0;
        for width in [0.0, 0.2, 0.6] {
            let p = wet(PeaksParams {
                low_hz: 500.0,
                high_hz: 8_000.0,
                boost_db: boost,
                width,
                ..Default::default()
            });
            let m = db(makeup(500.0, 8_000.0, boost, width, SR));
            // 0.05 is far under the limiter's knee, which is linear anyway.
            let at = sine_gain_db(p, 500.0, 0.05, SR) - m;
            assert!(
                (at - boost).abs() < 0.5,
                "width {width}: {at} dB at the band"
            );
            let away = sine_gain_db(p, 1_000.0, 0.05, SR) - m;
            let below = sine_gain_db(p, 250.0, 0.05, SR) - m;
            assert!(
                away < boost - 3.0 && below < boost - 3.0,
                "width {width}: {away} dB above, {below} below"
            );
            if width == 0.0 {
                assert!(away < 6.0 && below < 6.0, "{away} and {below} dB");
            }
        }
    }

    #[test]
    fn the_bands_are_independent() {
        let base = PeaksParams {
            low_hz: 300.0,
            high_hz: 5_000.0,
            boost_db: 20.0,
            width: 0.0,
            mix: 1.0,
        };
        let m = db(makeup(300.0, 5_000.0, 20.0, 0.0, SR));
        let low = sine_gain_db(base, 300.0, 0.05, SR) - m;
        let high = sine_gain_db(base, 5_000.0, 0.05, SR) - m;
        assert!((low - 20.0).abs() < 0.5 && (high - 20.0).abs() < 0.5);
        // Move the high band: the low band's gain does not change, and the
        // old high spot falls back to flat.
        let moved = PeaksParams {
            high_hz: 9_000.0,
            ..base
        };
        let m2 = db(makeup(300.0, 9_000.0, 20.0, 0.0, SR));
        let low2 = sine_gain_db(moved, 300.0, 0.05, SR) - m2;
        let old = sine_gain_db(moved, 5_000.0, 0.05, SR) - m2;
        assert!((low2 - low).abs() < 0.2, "{low} became {low2}");
        assert!(old.abs() < 1.0, "old high spot still at {old} dB");
    }

    #[test]
    fn white_noise_level_barely_moves_as_boost_rises() {
        // The contract: at the default width, Boost 0 to 36 dB changes the
        // RMS of white noise by no more than 6 dB.
        let run = |boost_db: f32| {
            let mut pk = Peaks::new(SR);
            let p = wet(PeaksParams {
                boost_db,
                ..Default::default()
            });
            let mut s = 99u32;
            let mut out = Vec::new();
            for i in 0..SR as usize * 2 {
                let x = 0.1 * noise(&mut s);
                let (o, _) = pk.process(x, x, &p);
                if i > SR as usize / 2 {
                    out.push(o);
                }
            }
            db(rms(&out))
        };
        let flat = run(0.0);
        let mut worst = 0.0f32;
        for b in [6.0, 12.0, 18.0, 24.0, 30.0, 36.0] {
            worst = worst.max((run(b) - flat).abs());
        }
        assert!(worst <= 6.0, "noise moved {worst} dB");
    }

    #[test]
    fn full_boost_on_full_scale_noise_stays_bounded() {
        for width in [0.0, 0.2, 1.0] {
            let mut pk = Peaks::new(SR);
            let p = wet(PeaksParams {
                low_hz: 200.0,
                high_hz: 3_000.0,
                boost_db: 36.0,
                width,
                ..Default::default()
            });
            let mut s = 3u32;
            let mut peak = 0.0f32;
            for _ in 0..SR as usize * 2 {
                let (a, b) = pk.process(noise(&mut s), noise(&mut s), &p);
                assert!(a.is_finite() && b.is_finite());
                peak = peak.max(a.abs()).max(b.abs());
            }
            assert!(peak <= CEILING + 1e-4, "width {width}: peak {peak}");
        }
    }

    #[test]
    fn the_limiter_is_linear_below_the_knee_and_smooth_through_it() {
        for x in [-1.0, -0.3, 0.0, 0.7, 1.0] {
            assert_eq!(limit(x), x);
        }
        assert!(limit(100.0) <= CEILING && limit(-100.0) >= -CEILING);
        let e = 1e-3;
        let slope = (limit(1.0 + e) - limit(1.0 - e)) / (2.0 * e);
        assert!((slope - 1.0).abs() < 1e-2, "slope {slope}");
    }

    #[test]
    fn works_from_22k_to_192k() {
        for sr in [22_050.0f32, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut pk = Peaks::new(sr);
            let p = wet(PeaksParams {
                low_hz: 40.0,
                high_hz: 12_000.0,
                boost_db: 36.0,
                width: 0.0,
                ..Default::default()
            });
            let mut s = 5u32;
            let mut peak = 0.0f32;
            for _ in 0..sr as usize {
                let (a, b) = pk.process(noise(&mut s), noise(&mut s), &p);
                assert!(a.is_finite() && b.is_finite(), "{sr}");
                peak = peak.max(a.abs()).max(b.abs());
            }
            assert!(peak <= CEILING + 1e-4, "{sr}: {peak}");
            // The band follows the sine at its centre.
            let q = PeaksParams {
                low_hz: 1_000.0,
                high_hz: 3_000.0,
                boost_db: 12.0,
                width: 0.3,
                mix: 1.0,
            };
            let m = db(makeup(1_000.0, 3_000.0, 12.0, 0.3, sr));
            let g = sine_gain_db(q, 1_000.0, 0.05, sr) - m;
            assert!((g - 12.0).abs() < 0.7, "{sr}: {g} dB");
        }
    }

    #[test]
    fn centres_past_the_top_are_held_below_nyquist() {
        let mut pk = Peaks::new(22_050.0);
        let p = wet(PeaksParams {
            low_hz: 12_000.0,
            high_hz: 12_000.0,
            boost_db: 36.0,
            width: 0.0,
            ..Default::default()
        });
        let mut s = 8u32;
        for _ in 0..22_050 {
            let (a, _) = pk.process(noise(&mut s), 0.0, &p);
            assert!(a.is_finite() && a.abs() <= CEILING + 1e-4);
        }
    }

    #[test]
    fn sweeping_both_centres_at_lfo_speed_does_not_click() {
        // Both centres run the whole range, in opposite senses, at 8 Hz, with
        // the boost and width at their harshest. The output's step must stay
        // in step with a plain loud input's.
        let mut pk = Peaks::new(SR);
        let n = SR as usize * 2;
        let mut max_out = 0.0f32;
        let mut last = 0.0f32;
        let mut max_in = 0.0f32;
        let mut last_in = 0.0f32;
        for i in 0..n {
            let t = i as f32 / SR;
            let lfo = 0.5 + 0.5 * (core::f32::consts::TAU * 8.0 * t).sin();
            let p = wet(PeaksParams {
                low_hz: 40.0 * (12_000.0f32 / 40.0).powf(lfo),
                high_hz: 40.0 * (12_000.0f32 / 40.0).powf(1.0 - lfo),
                boost_db: 36.0,
                width: 0.0,
                ..Default::default()
            });
            let x = 0.3 * (core::f32::consts::TAU * 220.0 * t).sin()
                + 0.3 * (core::f32::consts::TAU * 1_700.0 * t).sin();
            let (o, _) = pk.process(x, x, &p);
            if i > 4_800 {
                max_out = max_out.max((o - last).abs());
                max_in = max_in.max((x - last_in).abs());
            }
            last = o;
            last_in = x;
        }
        // The boosted sines are as loud as the limiter lets them be (2.0), so
        // allow the output's step the ceiling over the input's peak.
        assert!(
            max_out < 4.0 * max_in * (CEILING / 0.6),
            "step {max_out} vs {max_in}"
        );
    }

    #[test]
    fn a_jump_in_a_centre_does_not_click() {
        let mut pk = Peaks::new(SR);
        let a = wet(PeaksParams {
            low_hz: 200.0,
            ..Default::default()
        });
        let b = wet(PeaksParams {
            low_hz: 8_000.0,
            ..Default::default()
        });
        let mut s = 21u32;
        let mut worst = 0.0f32;
        let mut last = 0.0f32;
        for i in 0..SR as usize * 2 {
            let p = if (i / 4_800) % 2 == 0 { &a } else { &b };
            let (o, _) = pk.process(0.2 * noise(&mut s), 0.0, p);
            if i > 4_800 {
                worst = worst.max((o - last).abs());
            }
            last = o;
        }
        // Noise at 0.2 steps by up to 0.4; the boosted output may be several
        // times that but never anything like the ceiling's full swing.
        assert!(worst < 2.0 * CEILING, "step {worst}");
    }

    #[test]
    fn a_held_setting_settles_the_coefficients() {
        let mut pk = Peaks::new(SR);
        let p = wet(PeaksParams::default());
        settle(&mut pk, &p);
        let before = pk.last;
        pk.process(0.1, 0.1, &p);
        assert_eq!(before, pk.last);
    }
}
