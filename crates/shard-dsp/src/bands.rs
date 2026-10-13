//! Band Delay.
//!
//! The sound is split into low, mid and high, and each band gets its own
//! echo. With `harmonic` up the bands' times fall into a harmonic series: the
//! low band repeats at `time`, the mid at `time / (1 + harmonic)` and the high
//! at `time / (1 + 2 * harmonic)`, so at one they are `time`, `time / 2` and
//! `time / 3`, and a single hit scatters into a rhythm that falls apart by
//! register. At zero every band repeats at `time` and the effect is a plain
//! delay, with the crossover summing flat.
//!
//! The split is a pair of fourth-order Linkwitz-Riley crossovers, at `split_hz`
//! and three octaves above it. Each is two cascaded Butterworth sections, so a
//! low-pass and a high-pass at one frequency sum to an all-pass. The low band
//! is also run through the all-pass of the upper crossover (its own low-pass
//! plus high-pass, which is what the mid and high bands have been through), so
//! the three bands sum to an all-pass with a flat magnitude. Filter state is
//! kept in `f64`.
//!
//! Each band has its own line a side, read between samples with a cubic, as
//! `delay.rs` does. `time_ms` and `harmonic` glide over about a tenth of a
//! second and each read point is held to half a sample a sample at most, so
//! moving them bends the echoes in pitch, tape-style, rather than clicking.
//! The split moves smoothly too.
//!
//! Feedback stays in its band. The repeat is passed through a soft knee on the
//! way back round that does nothing below full scale and bends towards twice
//! full scale above, so the loop gain never exceeds `feedback` and loud
//! material at 95% rings without running away.
//!
//! `mix` crossfades, like every effect's: `dry * (1 - mix) + wet * mix`, with
//! the wet the sum of the three bands' repeats. At zero it is a bit-exact
//! bypass and the lines keep running, so switching on never plays stale audio.

use crate::smooth::OnePole;

/// The shortest and longest `time`, in milliseconds.
pub const TIME_MIN_MS: f32 = 10.0;
pub const TIME_MAX_MS: f32 = 2000.0;
/// The top of `bands.feedback`. Below one, so the repeats always die away.
pub const FEEDBACK_MAX: f32 = 0.95;
/// The range of `split_hz`: where low meets mid. Mid meets high at eight times.
pub const SPLIT_MIN_HZ: f32 = 100.0;
pub const SPLIT_MAX_HZ: f32 = 5_000.0;
/// The upper crossover sits this many times above the lower: three octaves.
const UPPER_RATIO: f32 = 8.0;
/// Crossovers are kept below this share of the sample rate.
const NYQUIST_SHARE: f32 = 0.45;
/// How long a change of time or harmonic takes to arrive, as a time constant.
const TIME_GLIDE_MS: f32 = 100.0;
/// The fastest a read point may move, in samples a sample.
const MAX_SLEW: f32 = 0.5;
/// Spare samples past the longest delay, for the interpolator's four points.
const MARGIN: usize = 8;
/// Below this a recirculating value is flushed to zero.
const FLUSH: f32 = 1e-15;
/// The same for filter memories, which are `f64`.
const FLUSH_F64: f64 = 1e-30;
/// A change of split smaller than this share of itself does not re-cut the filters.
const SPLIT_STEP: f32 = 1e-3;

#[derive(Debug, Clone, Copy)]
pub struct BandsParams {
    /// The low band's delay in milliseconds, `TIME_MIN_MS` to `TIME_MAX_MS`.
    pub time_ms: f32,
    /// 0: every band at `time_ms`. 1: low at `time`, mid at half, high at a third.
    pub harmonic: f32,
    /// How much of each band's repeat is fed back, 0 to `FEEDBACK_MAX`.
    pub feedback: f32,
    /// Where low meets mid, `SPLIT_MIN_HZ` to `SPLIT_MAX_HZ`.
    pub split_hz: f32,
    /// Crossfade from dry to the repeats, 0 to 1.
    pub mix: f32,
}

impl Default for BandsParams {
    fn default() -> Self {
        Self {
            time_ms: 375.0,
            harmonic: 1.0,
            feedback: 0.4,
            split_hz: 800.0,
            mix: 0.0,
        }
    }
}

/// One biquad's coefficients, normalised so `a0` is one.
#[derive(Debug, Clone, Copy, Default)]
struct Coef {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

/// A Butterworth low-pass and high-pass at one frequency. Two of each in
/// cascade make the Linkwitz-Riley fourth order pair.
#[derive(Debug, Clone, Copy, Default)]
struct Cut {
    lp: Coef,
    hp: Coef,
}

impl Cut {
    fn new(freq: f32, sample_rate: f32) -> Self {
        let w0 = core::f64::consts::TAU * freq as f64 / sample_rate as f64;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * core::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * c / a0;
        let a2 = (1.0 - alpha) / a0;
        let lp = (1.0 - c) / 2.0 / a0;
        let hp = (1.0 + c) / 2.0 / a0;
        Self {
            lp: Coef {
                b0: lp,
                b1: 2.0 * lp,
                b2: lp,
                a1,
                a2,
            },
            hp: Coef {
                b0: hp,
                b1: -2.0 * hp,
                b2: hp,
                a1,
                a2,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    z1: f64,
    z2: f64,
}

impl Biquad {
    #[inline]
    fn process(&mut self, x: f64, c: &Coef) -> f64 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        if self.z1.abs() < FLUSH_F64 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < FLUSH_F64 {
            self.z2 = 0.0;
        }
        y
    }
}

/// A Linkwitz-Riley fourth-order split: a cascade of two for each side.
#[derive(Debug, Clone, Copy, Default)]
struct Lr4 {
    lp: [Biquad; 2],
    hp: [Biquad; 2],
}

impl Lr4 {
    #[inline]
    fn split(&mut self, x: f64, cut: &Cut) -> (f64, f64) {
        let lo = self.lp[0].process(x, &cut.lp);
        let lo = self.lp[1].process(lo, &cut.lp);
        let hi = self.hp[0].process(x, &cut.hp);
        let hi = self.hp[1].process(hi, &cut.hp);
        (lo, hi)
    }
}

/// The three-band split of one channel.
#[derive(Debug, Clone, Copy, Default)]
struct Split {
    lower: Lr4,
    upper: Lr4,
    /// The upper crossover's all-pass, applied to the low band to match.
    align: Lr4,
}

pub struct Bands {
    /// One line a band and a side, sized once. `write` is the newest sample.
    lines: [[Vec<f32>; 2]; 3],
    write: usize,
    frames: usize,
    sample_rate: f32,
    splits: [Split; 2],
    lower: Cut,
    upper: Cut,
    /// The split the filters are cut for.
    cut_at: f32,
    split: OnePole,
    time: OnePole,
    harmonic: OnePole,
    feedback: OnePole,
    mix: OnePole,
    primed: bool,
    /// The delay each band is reading, in samples, chasing its goal.
    delay_now: [f32; 3],
}

/// Catmull-Rom between `p1` and `p2`, `f` of the way. Exact at `f == 0`.
#[inline]
fn cubic(p0: f32, p1: f32, p2: f32, p3: f32, f: f32) -> f32 {
    p1 + 0.5
        * f
        * (p2 - p0 + f * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + f * (3.0 * (p1 - p2) + p3 - p0)))
}

/// The loop's safety: identity up to full scale, then a bend towards 2.
#[inline]
fn knee(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.0 {
        x
    } else {
        (2.0 - 1.0 / a).copysign(x)
    }
}

impl Bands {
    pub fn new(sample_rate: f32) -> Self {
        let frames = (TIME_MAX_MS * 0.001 * sample_rate).ceil() as usize + MARGIN;
        let d = BandsParams::default();
        let smoother = |ms: f32, v: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        let line = || vec![0.0; frames];
        let (f1, f2) = Self::crossovers(d.split_hz, sample_rate);
        Self {
            lines: [[line(), line()], [line(), line()], [line(), line()]],
            write: 0,
            frames,
            sample_rate,
            splits: [Split::default(); 2],
            lower: Cut::new(f1, sample_rate),
            upper: Cut::new(f2, sample_rate),
            cut_at: d.split_hz,
            split: smoother(20.0, d.split_hz),
            time: smoother(TIME_GLIDE_MS, d.time_ms),
            harmonic: smoother(TIME_GLIDE_MS, d.harmonic),
            feedback: smoother(20.0, d.feedback),
            mix: smoother(20.0, 0.0),
            primed: false,
            delay_now: [2.0; 3],
        }
    }

    /// Forgets the repeats and the filters' memory, without allocating. The
    /// smoothers keep their place: they follow controls, not signal.
    pub fn reset(&mut self) {
        for band in &mut self.lines {
            for line in band {
                line.fill(0.0);
            }
        }
        self.splits = [Split::default(); 2];
    }

    /// The two crossover frequencies for a split, kept clear of Nyquist and
    /// of each other.
    fn crossovers(split_hz: f32, sample_rate: f32) -> (f32, f32) {
        let top = sample_rate * NYQUIST_SHARE;
        let f1 = split_hz.clamp(SPLIT_MIN_HZ, SPLIT_MAX_HZ).min(top / 2.0);
        (f1, (f1 * UPPER_RATIO).min(top))
    }

    fn cut(&mut self, split_hz: f32) {
        let (f1, f2) = Self::crossovers(split_hz, self.sample_rate);
        self.lower = Cut::new(f1, self.sample_rate);
        self.upper = Cut::new(f2, self.sample_rate);
        self.cut_at = split_hz;
    }

    fn max_delay(&self) -> f32 {
        (self.frames - MARGIN) as f32
    }

    /// One side's input split into low, mid and high, with the low band
    /// aligned so the three sum to a flat all-pass.
    #[inline]
    fn bands_of(&mut self, ch: usize, x: f32) -> [f32; 3] {
        let (lower, upper) = (self.lower, self.upper);
        let s = &mut self.splits[ch];
        let (lo, rest) = s.lower.split(x as f64, &lower);
        let (mid, high) = s.upper.split(rest, &upper);
        let (a, b) = s.align.split(lo, &upper);
        [(a + b) as f32, mid as f32, high as f32]
    }

    #[inline]
    fn read(&self, band: usize, ch: usize, age: f32) -> f32 {
        let a = age as usize;
        let f = age - a as f32;
        let line = &self.lines[band][ch];
        let at = |k: usize| line[(self.write + self.frames - k) % self.frames];
        cubic(at(a - 1), at(a), at(a + 1), at(a + 2), f)
    }

    pub fn process(&mut self, l: f32, r: f32, p: &BandsParams) -> (f32, f32) {
        let time_target = p.time_ms.clamp(TIME_MIN_MS, TIME_MAX_MS);
        let harmonic_target = p.harmonic.clamp(0.0, 1.0);
        let feedback_target = p.feedback.clamp(0.0, FEEDBACK_MAX);
        let split_target = p.split_hz.clamp(SPLIT_MIN_HZ, SPLIT_MAX_HZ);
        if !self.primed {
            self.time.reset(time_target);
            self.harmonic.reset(harmonic_target);
            self.feedback.reset(feedback_target);
            self.split.reset(split_target);
            self.mix.reset(p.mix.clamp(0.0, 1.0));
            self.cut(split_target);
            for (b, d) in self.delay_now.iter_mut().enumerate() {
                let k = 1.0 + b as f32 * harmonic_target;
                *d = (time_target / k * 0.001 * self.sample_rate)
                    .clamp(2.0, (self.frames - MARGIN) as f32);
            }
            self.primed = true;
        }
        let time = self.time.process(time_target);
        let harmonic = self.harmonic.process(harmonic_target);
        let feedback = self.feedback.process(feedback_target);
        let split = self.split.process(split_target);
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };
        if (split - self.cut_at).abs() > SPLIT_STEP * self.cut_at {
            self.cut(split);
        }

        let ins = [self.bands_of(0, l), self.bands_of(1, r)];

        let max = self.max_delay();
        let mut wet = [0.0f32; 2];
        let mut sent = [[0.0f32; 2]; 3];
        for b in 0..3 {
            let goal =
                (time / (1.0 + b as f32 * harmonic) * 0.001 * self.sample_rate).clamp(2.0, max);
            self.delay_now[b] += (goal - self.delay_now[b]).clamp(-MAX_SLEW, MAX_SLEW);
            let age = self.delay_now[b] - 1.0;
            for ch in 0..2 {
                let heard = self.read(b, ch, age);
                wet[ch] += heard;
                let v = ins[ch][b] + feedback * knee(heard);
                sent[b][ch] = if v.abs() < FLUSH { 0.0 } else { v };
            }
        }
        self.write = (self.write + 1) % self.frames;
        for (band, sent) in self.lines.iter_mut().zip(sent) {
            for (line, v) in band.iter_mut().zip(sent) {
                line[self.write] = v;
            }
        }

        if mix == 0.0 {
            return (l, r);
        }
        (
            l * (1.0 - mix) + wet[0] * mix,
            r * (1.0 - mix) + wet[1] * mix,
        )
    }
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

    fn music(i: usize, sr: f32) -> f32 {
        let t = i as f32 / sr;
        let tau = core::f32::consts::TAU;
        0.3 * (tau * 220.0 * t).sin()
            + 0.2 * (tau * 660.0 * t).sin()
            + 0.1 * (tau * 1_320.0 * t).sin()
    }

    fn wet(time_ms: f32, harmonic: f32, feedback: f32) -> BandsParams {
        BandsParams {
            time_ms,
            harmonic,
            feedback,
            split_hz: 800.0,
            mix: 1.0,
        }
    }

    #[test]
    fn zero_mix_is_a_bit_exact_bypass() {
        let mut d = Bands::new(SR);
        let p = BandsParams {
            mix: 0.0,
            feedback: FEEDBACK_MAX,
            ..BandsParams::default()
        };
        let mut s = 7u32;
        for _ in 0..20_000 {
            let (l, r) = (noise(&mut s), noise(&mut s));
            assert_eq!(d.process(l, r, &p), (l, r));
        }
    }

    #[test]
    fn mix_faded_to_zero_ends_exactly_transparent_and_keeps_running() {
        let mut d = Bands::new(SR);
        let on = BandsParams {
            mix: 1.0,
            ..BandsParams::default()
        };
        let off = BandsParams { mix: 0.0, ..on };
        let mut s = 3u32;
        for _ in 0..30_000 {
            d.process(noise(&mut s), noise(&mut s), &on);
        }
        for _ in 0..30_000 {
            d.process(noise(&mut s), noise(&mut s), &off);
        }
        for _ in 0..1_000 {
            let (l, r) = (noise(&mut s), noise(&mut s));
            assert_eq!(d.process(l, r, &off), (l, r));
        }
        let heard = (0..48_000)
            .map(|_| {
                let (a, b) = d.process(0.0, 0.0, &on);
                a.abs().max(b.abs())
            })
            .fold(0.0, f32::max);
        assert!(
            heard > 0.1,
            "lines should have kept recording, peak {heard}"
        );
    }

    #[test]
    fn silence_in_silence_out() {
        let mut d = Bands::new(SR);
        let p = BandsParams {
            mix: 1.0,
            feedback: FEEDBACK_MAX,
            ..BandsParams::default()
        };
        for _ in 0..50_000 {
            assert_eq!(d.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    /// The three bands of a sine, summed, against the sine: the ratio of
    /// steady-state RMS in decibels.
    fn flatness_db(sr: f32, split_hz: f32, freq: f32) -> f32 {
        let mut d = Bands::new(sr);
        d.cut(split_hz);
        let n = sr as usize;
        let (mut e_in, mut e_out) = (0.0f64, 0.0f64);
        for i in 0..n {
            let x = (core::f32::consts::TAU * freq * (i as f32 / sr)).sin();
            let b = d.bands_of(0, x);
            if i > n / 2 {
                e_in += (x * x) as f64;
                let s = b[0] + b[1] + b[2];
                e_out += (s * s) as f64;
            }
        }
        10.0 * (e_out / e_in).log10() as f32
    }

    #[test]
    fn the_crossover_sums_flat() {
        for &split in &[100.0, 800.0, 2_500.0, 5_000.0] {
            for &freq in &[
                30.0, 50.0, 100.0, 200.0, 400.0, 800.0, 1_000.0, 2_000.0, 3_000.0, 6_400.0,
                8_000.0, 10_000.0, 15_000.0,
            ] {
                let db = flatness_db(SR, split, freq);
                assert!(db.abs() < 0.5, "split {split}, {freq} Hz: {db} dB");
            }
        }
    }

    #[test]
    fn the_crossover_separates_the_registers() {
        let mut d = Bands::new(SR);
        // Steady state of a tone, the share of its energy in each band.
        let share = |d: &mut Bands, freq: f32| {
            let mut e = [0.0f64; 3];
            for i in 0..SR as usize {
                let x = (core::f32::consts::TAU * freq * (i as f32 / SR)).sin();
                let b = d.bands_of(0, x);
                if i > 4_800 {
                    for k in 0..3 {
                        e[k] += (b[k] * b[k]) as f64;
                    }
                }
            }
            let t: f64 = e.iter().sum();
            [e[0] / t, e[1] / t, e[2] / t]
        };
        let low = share(&mut d, 100.0);
        assert!(low[0] > 0.99, "{low:?}");
        d.reset();
        let mid = share(&mut d, 2_000.0);
        assert!(mid[1] > 0.99, "{mid:?}");
        d.reset();
        let high = share(&mut d, 12_000.0);
        assert!(high[2] > 0.99, "{high:?}");
    }

    /// A Hann-windowed burst of a sine, `ms` long.
    fn burst(i: usize, freq: f32, ms: f32, sr: f32) -> f32 {
        let n = (ms * 0.001 * sr) as usize;
        if i >= n {
            return 0.0;
        }
        let w = 0.5 - 0.5 * (core::f32::consts::TAU * i as f32 / n as f32).cos();
        w * (core::f32::consts::TAU * freq * (i as f32 / sr)).sin()
    }

    /// Where the energy of a burst's echoes lands, in milliseconds, in
    /// windows of 1 ms: returns (total energy, energy per window).
    fn echoes_of(freq: f32, ms: f32, p: &BandsParams, len_ms: f32) -> (f64, Vec<f64>) {
        let mut d = Bands::new(SR);
        let n = (len_ms * 0.001 * SR) as usize;
        let win = (0.001 * SR) as usize;
        let mut per = vec![0.0f64; n / win + 1];
        let mut total = 0.0;
        for i in 0..n {
            let x = burst(i, freq, ms, SR);
            let (o, _) = d.process(x, 0.0, p);
            let e = (o * o) as f64;
            per[i / win] += e;
            total += e;
        }
        (total, per)
    }

    fn input_energy(freq: f32, ms: f32) -> f64 {
        (0..(ms * 0.001 * SR) as usize + 1)
            .map(|i| burst(i, freq, ms, SR) as f64)
            .map(|v| v * v)
            .sum()
    }

    /// The share of energy within `[from_ms, to_ms)`.
    fn share_between(per: &[f64], total: f64, from_ms: usize, to_ms: usize) -> f64 {
        per[from_ms..to_ms].iter().sum::<f64>() / total
    }

    #[test]
    fn at_full_harmonic_each_register_echoes_at_its_own_time() {
        let p = wet(300.0, 1.0, 0.0);
        // Low tone at Time, mid at Time/2, high at Time/3. The burst is 30 ms.
        for &(freq, at_ms) in &[(150.0, 300usize), (2_000.0, 150), (12_000.0, 100)] {
            let (total, per) = echoes_of(freq, 30.0, &p, 400.0);
            let e_in = input_energy(freq, 30.0);
            let ratio = total / e_in;
            assert!(
                (0.8..1.25).contains(&ratio),
                "{freq} Hz: echo has {ratio} of the burst's energy"
            );
            let near = share_between(&per, total, at_ms - 2, at_ms + 36);
            assert!(
                near > 0.95,
                "{freq} Hz: {near} of the energy near {at_ms} ms"
            );
        }
    }

    #[test]
    fn at_zero_harmonic_every_band_echoes_at_time() {
        let p = wet(300.0, 0.0, 0.0);
        for &freq in &[150.0, 2_000.0, 12_000.0] {
            let (total, per) = echoes_of(freq, 30.0, &p, 400.0);
            let near = share_between(&per, total, 298, 336);
            assert!(near > 0.95, "{freq} Hz: {near} of the energy near 300 ms");
        }
        // And an impulse comes out whole: nothing earlier, the same energy.
        let mut d = Bands::new(SR);
        let n = (0.4 * SR) as usize;
        let t = (0.3 * SR) as usize;
        let (mut early, mut late) = (0.0f64, 0.0f64);
        for i in 0..n {
            let (o, _) = d.process(if i == 0 { 1.0 } else { 0.0 }, 0.0, &p);
            let e = (o * o) as f64;
            if i < t - 2 {
                early += e;
            } else {
                late += e;
            }
        }
        assert!(early < 1e-9, "energy before Time: {early}");
        assert!(
            (late - 1.0).abs() < 0.02,
            "an all-pass keeps the energy: {late}"
        );
    }

    #[test]
    fn harmonic_scatters_an_impulse_into_three_times() {
        let mut d = Bands::new(SR);
        let p = wet(300.0, 1.0, 0.0);
        let n = (0.4 * SR) as usize;
        let mut per = vec![0.0f64; 400];
        for i in 0..n {
            let (o, _) = d.process(if i == 0 { 1.0 } else { 0.0 }, 0.0, &p);
            per[i / 48] += (o * o) as f64;
        }
        let near = |ms: usize| per[ms - 3..ms + 15].iter().sum::<f64>();
        for ms in [100, 150, 300] {
            assert!(near(ms) > 0.01, "nothing near {ms} ms: {}", near(ms));
        }
        let all: f64 = per.iter().sum();
        let three = near(100) + near(150) + near(300);
        assert!(three > 0.9 * all, "{three} of {all}");
    }

    #[test]
    fn each_repeat_is_feedback_times_the_last() {
        // A low tone burst repeats at Time with feedback 0.5: the second echo
        // is half the first, in amplitude.
        let p = wet(100.0, 1.0, 0.5);
        let (_, per) = echoes_of(150.0, 30.0, &p, 400.0);
        let first: f64 = per[98..136].iter().sum();
        let second: f64 = per[198..236].iter().sum();
        let r = (second / first).sqrt();
        assert!((r - 0.5).abs() < 0.03, "second over first {r}");
    }

    #[test]
    fn no_runaway_at_maximum_feedback() {
        for &harmonic in &[0.0, 0.5, 1.0] {
            for &mono in &[false, true] {
                let mut d = Bands::new(SR);
                let p = wet(120.0, harmonic, FEEDBACK_MAX);
                let mut s = 5u32;
                for i in 0..(SR as usize * 6) {
                    let (x, y) = if i < SR as usize {
                        let a = noise(&mut s);
                        (a, if mono { a } else { noise(&mut s) })
                    } else {
                        (0.0, 0.0)
                    };
                    let (a, b) = d.process(x, y, &p);
                    assert!(a.is_finite() && b.is_finite());
                    assert!(
                        a.abs() <= 8.0 && b.abs() <= 8.0,
                        "harmonic {harmonic}: {a} {b} at {i}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_tail_decays() {
        let mut d = Bands::new(SR);
        let p = wet(120.0, 1.0, FEEDBACK_MAX);
        let mut s = 5u32;
        let mut late = 0.0f32;
        for i in 0..(SR as usize * 12) {
            let x = if i < SR as usize / 2 {
                noise(&mut s)
            } else {
                0.0
            };
            let (a, _) = d.process(x, 0.0, &p);
            if i > SR as usize * 11 {
                late = late.max(a.abs());
            }
        }
        assert!(late < 0.05, "tail still at {late}");
    }

    fn worst_step(sweep: impl Fn(f32) -> BandsParams) -> (f32, f32) {
        let n = SR as usize;
        let mut d = Bands::new(SR);
        let settle = sweep(0.0);
        for i in 0..n {
            d.process(music(i, SR), music(i, SR), &settle);
        }
        let (mut max_in, mut max_out) = (0.0f32, 0.0f32);
        let (mut li, mut lo) = (0.0, 0.0);
        for i in 0..n {
            let t = i as f32 / n as f32;
            let x = music(n + i, SR);
            let (o, _) = d.process(x, x, &sweep(t));
            if i > 0 {
                max_in = max_in.max((x - li).abs());
                max_out = max_out.max((o - lo).abs());
            }
            li = x;
            lo = o;
        }
        (max_in, max_out)
    }

    #[test]
    fn sweeping_time_does_not_click() {
        let (i, o) = worst_step(|t| wet(100.0 + 500.0 * t, 1.0, 0.4));
        assert!(o < 4.0 * i, "out step {o} against in {i}");
    }

    #[test]
    fn jumping_time_glides_rather_than_clicks() {
        let mut d = Bands::new(SR);
        let (a, b) = (wet(300.0, 1.0, 0.0), wet(900.0, 1.0, 0.0));
        let (mut max_in, mut max_out) = (0.0f32, 0.0f32);
        let (mut li, mut lo) = (0.0, 0.0);
        for i in 0..SR as usize * 2 {
            let x = music(i, SR);
            let (o, _) = d.process(x, x, if i < SR as usize { &a } else { &b });
            if i > 0 {
                max_in = max_in.max((x - li).abs());
                max_out = max_out.max((o - lo).abs());
            }
            li = x;
            lo = o;
        }
        assert!(
            max_out < 4.0 * max_in,
            "out step {max_out} against in {max_in}"
        );
    }

    #[test]
    fn sweeping_harmonic_split_and_everything_does_not_click() {
        let (i, o) = worst_step(|t| BandsParams {
            time_ms: 100.0 + 400.0 * t,
            harmonic: t,
            feedback: 0.2 + 0.7 * t,
            split_hz: 200.0 + 3_000.0 * t,
            mix: 0.2 + 0.7 * t,
        });
        assert!(o < 4.0 * i, "out step {o} against in {i}");
    }

    #[test]
    fn works_and_sizes_itself_at_each_rate() {
        for &sr in &[22_050.0f32, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut d = Bands::new(sr);
            assert!(d.frames as f32 >= 2.0 * sr, "{sr}: {} frames", d.frames);
            assert!(d.frames as f32 <= 2.0 * sr + 16.0);
            for &split in &[SPLIT_MIN_HZ, 800.0, SPLIT_MAX_HZ] {
                let p = BandsParams {
                    split_hz: split,
                    ..wet(TIME_MAX_MS, 1.0, FEEDBACK_MAX)
                };
                let mut s = 1u32;
                for _ in 0..(sr as usize / 4) {
                    let (a, b) = d.process(noise(&mut s), noise(&mut s), &p);
                    assert!(a.is_finite() && b.is_finite() && a.abs() < 40.0);
                }
            }
        }
    }

    #[test]
    fn extremes_of_every_range_are_safe() {
        let mut d = Bands::new(SR);
        let mut s = 9u32;
        for &(t, h, sp) in &[
            (0.0, -1.0, 0.0),
            (TIME_MIN_MS, 0.0, SPLIT_MIN_HZ),
            (TIME_MAX_MS, 1.0, SPLIT_MAX_HZ),
            (1.0e9, 9.0, 1.0e9),
            (-5.0, 0.5, -5.0),
        ] {
            let p = BandsParams {
                time_ms: t,
                harmonic: h,
                feedback: 2.0,
                split_hz: sp,
                mix: 2.0,
            };
            for _ in 0..20_000 {
                let (a, b) = d.process(noise(&mut s), noise(&mut s), &p);
                assert!(a.is_finite() && b.is_finite() && a.abs() < 40.0 && b.abs() < 40.0);
            }
        }
    }

    #[test]
    fn the_default_is_inside_every_range() {
        let p = BandsParams::default();
        assert!((TIME_MIN_MS..=TIME_MAX_MS).contains(&p.time_ms));
        assert!((0.0..=1.0).contains(&p.harmonic));
        assert!((0.0..=FEEDBACK_MAX).contains(&p.feedback));
        assert!((SPLIT_MIN_HZ..=SPLIT_MAX_HZ).contains(&p.split_hz));
        assert_eq!(p.mix, 0.0);
    }

    #[test]
    fn reset_forgets_the_tail() {
        let mut x = Bands::new(48_000.0);
        let p = wet(2000.0, 1.0, FEEDBACK_MAX);
        let mut state = 0x9E37_79B9u32;
        for _ in 0..48_000 {
            let v = noise(&mut state);
            x.process(v, -v, &p);
        }
        x.reset();
        for _ in 0..(48_000 * 3) {
            assert_eq!(x.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn reset_on_a_fresh_node_changes_nothing() {
        let p = wet(2000.0, 1.0, FEEDBACK_MAX);
        let mut a = Bands::new(48_000.0);
        let mut b = Bands::new(48_000.0);
        b.reset();
        for i in 0..4_800 {
            let v = ((i % 211) as f32 / 211.0) - 0.5;
            assert_eq!(a.process(v, v * 0.5, &p), b.process(v, v * 0.5, &p));
        }
    }
}
