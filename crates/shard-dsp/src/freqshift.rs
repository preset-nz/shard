//! Frequency shifter (row 48).
//!
//! Every partial moves by the same number of hertz, so a harmonic sound turns
//! inharmonic: 100, 200, 300 Hz shifted by 50 becomes 150, 250, 350. It is not
//! `shift.rs`, which multiplies and so keeps the ratios.
//!
//! **Single sideband.** A Hilbert transformer makes two copies of the input
//! 90 degrees apart at every frequency, and a quadrature oscillator rotates
//! them: `wet = I * cos(phi) - Q * sin(phi)`. That cancels one sideband, so a
//! positive Shift moves the sound up with no mirror image below it, and a
//! negative Shift moves it down. (A ring modulator is the same multiply
//! without the quadrature pair, which is why it leaves both.)
//!
//! **The Hilbert pair** is Olli Niemitalo's design: two chains of four
//! all-pass sections in `z^-2`, the first delayed a sample, with the
//! published coefficients (`A1`, `A2`). Both chains are all-pass, so the pair
//! keeps the level exactly; only the phase difference matters. The design is in
//! normalised frequency, so it is the same shape at every rate: the two
//! outputs stay within 0.7 degrees of quadrature from 0.002 to 0.998 of the
//! half-rate, a sideband rejection of 44 dB or better (measured). In hertz that is
//! 22 Hz to 22 kHz at 44.1 kHz, 24 Hz to 24 kHz at 48 kHz, 48 Hz to 48 kHz at
//! 96 kHz and 96 Hz to 96 kHz at 192 kHz. Below that corner the rejection
//! falls away (measured: 20 Hz at 96 kHz is 15 dB clean, 50 Hz at 192 kHz 20 dB). Nothing
//! musical sits there. State is held in f64: the sections have poles near
//! 0.9987, which f32 would round noisily.
//!
//! **The oscillator** is a phase accumulator in f64, in turns, wrapped each
//! sample, so it does not drift however long it runs. One per channel.
//!
//! **Shift** is -2000 to +2000 Hz. The table's `Bipolar` taper is linear, so
//! the fine travel near zero is that of a 4000 Hz-wide slider (one pixel in
//! 400 is 10 Hz); a finer centre would need a new taper, which is a table
//! decision and not made here. The shift is smoothed over 20 ms and the
//! oscillator integrates it, so a sweep or an LFO has no zipper noise.
//!
//! **Spread** pulls the two sides apart: the left is shifted `spread * 20 Hz`
//! further up than Shift and the right the same further down. At Shift 0 a
//! mono sound becomes a left that rises and a right that falls, 0 to 40 Hz
//! apart, which beats against itself and widens. 20 Hz is the largest because
//! past that the sides stop beating and just sound like two different pitches.
//! (A choice of this module, not a ruling.)
//!
//! **Feedback** returns the shifted sound to the input through a fixed 10 ms
//! delay and a `tanh`, so every trip round shifts it again and a sustained
//! sound climbs (or falls) in steps of Shift: the endless spiral. The delay is
//! what makes it a pitched staircase and not a smear; the `tanh` is what keeps
//! 95% from running away. It is near-linear for quiet sound and bends loud
//! sound towards 1 on the way back. The return can never exceed `feedback`, so
//! the loop cannot grow without bound.
//!
//! `mix` crossfades, `dry * (1 - mix) + wet * mix`, and is a bit-exact bypass
//! at zero. The wet is phase-rotated against the dry (no latency is added), so
//! a half mix of a small shift is a phaser-like sweep, and that is the sound.

use crate::smooth::OnePole;

/// The reach of Shift, in hertz, either way.
pub const SHIFT_MAX_HZ: f32 = 2000.0;
/// The top of Spread: how far each side moves from Shift, in hertz.
pub const SPREAD_MAX_HZ: f32 = 20.0;
/// The top of `freqshift.feedback`. Below one, so the spiral always dies.
pub const FEEDBACK_MAX: f32 = 0.95;
/// The feedback delay, in milliseconds.
const FEEDBACK_DELAY_MS: f32 = 10.0;
/// Below this a recirculating value is flushed to zero, so a dying tail never
/// reaches the denormals.
const FLUSH: f64 = 1e-25;
/// Which sideband a positive Shift keeps: +1 up, -1 would be down. Fixed by
/// `a_positive_shift_moves_up_and_leaves_no_image_below`.
const SIDE: f64 = 1.0;

/// Niemitalo's coefficients. Each section is
/// `y[n] = a^2 * (x[n] + y[n-2]) - x[n-2]`.
const A1: [f64; 4] = [
    0.692_387_8,
    0.936_065_432_295_9,
    0.988_229_522_686_0,
    0.998_748_845_273_7,
];
const A2: [f64; 4] = [
    0.402_192_116_242_6,
    0.856_171_088_242_0,
    0.972_290_954_565_1,
    0.995_288_479_127_8,
];

#[derive(Debug, Clone, Copy)]
pub struct FreqShiftParams {
    /// Hertz, -`SHIFT_MAX_HZ` to `SHIFT_MAX_HZ`. Positive is up.
    pub shift_hz: f32,
    /// How much of the shifted sound returns, 0 to `FEEDBACK_MAX`.
    pub feedback: f32,
    /// 0 to 1: the left goes `spread * SPREAD_MAX_HZ` further up, the right
    /// the same further down.
    pub spread: f32,
    /// Dry to shifted, 0 to 1.
    pub mix: f32,
}

impl Default for FreqShiftParams {
    fn default() -> Self {
        Self {
            shift_hz: 0.0,
            feedback: 0.0,
            spread: 0.0,
            mix: 0.0,
        }
    }
}

/// Two all-pass chains whose outputs are a quarter turn apart.
#[derive(Debug, Clone)]
struct Hilbert {
    /// `[x[n-1], x[n-2]]` and `[y[n-1], y[n-2]]` of each section; the first
    /// four are chain one, the next four chain two.
    x: [[f64; 2]; 8],
    y: [[f64; 2]; 8],
    /// Chain one's one-sample delay.
    prev: f64,
}

impl Hilbert {
    fn new() -> Self {
        Self {
            x: [[0.0; 2]; 8],
            y: [[0.0; 2]; 8],
            prev: 0.0,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    #[inline]
    fn section(&mut self, k: usize, a: f64, x: f64) -> f64 {
        let c = a * a;
        let y = c * (x + self.y[k][1]) - self.x[k][1];
        self.x[k][1] = self.x[k][0];
        self.x[k][0] = x;
        self.y[k][1] = self.y[k][0];
        self.y[k][0] = if y.abs() < FLUSH { 0.0 } else { y };
        y
    }

    /// `(q, i)`: chain one, which carries the one-sample delay, and chain two.
    #[inline]
    fn process(&mut self, x: f64) -> (f64, f64) {
        let mut q = self.prev;
        self.prev = x;
        for (k, a) in A1.iter().enumerate() {
            q = self.section(k, *a, q);
        }
        let mut i = x;
        for (k, a) in A2.iter().enumerate() {
            i = self.section(4 + k, *a, i);
        }
        (q, i)
    }
}

#[derive(Debug, Clone)]
pub struct FreqShift {
    /// Boxed: about 500 bytes of state would make this the chain's largest variant.
    hilbert: Box<[Hilbert; 2]>,
    /// The oscillators' phases, in turns, 0 to 1.
    phase: [f64; 2],
    /// The feedback delay, a ring a side, and where it is read and written.
    line: [Vec<f64>; 2],
    at: usize,
    sample_rate: f32,
    shift: OnePole,
    feedback: OnePole,
    spread: OnePole,
    mix: OnePole,
    primed: bool,
}

impl FreqShift {
    pub fn new(sample_rate: f32) -> Self {
        let frames = ((FEEDBACK_DELAY_MS * 0.001 * sample_rate).round() as usize).max(1);
        let smoother = || {
            let mut p = OnePole::new();
            p.set_time(20.0, sample_rate);
            p
        };
        Self {
            hilbert: Box::new([Hilbert::new(), Hilbert::new()]),
            phase: [0.0; 2],
            line: [vec![0.0; frames], vec![0.0; frames]],
            at: 0,
            sample_rate,
            shift: smoother(),
            feedback: smoother(),
            spread: smoother(),
            mix: smoother(),
            primed: false,
        }
    }

    /// Forgets the sound it holds, without allocating. The smoothers and the
    /// oscillators keep their place: they follow controls, not signal.
    pub fn reset(&mut self) {
        for h in self.hilbert.iter_mut() {
            h.reset();
        }
        for l in &mut self.line {
            l.fill(0.0);
        }
    }

    pub fn process(&mut self, l: f32, r: f32, p: &FreqShiftParams) -> (f32, f32) {
        let shift_target = p.shift_hz.clamp(-SHIFT_MAX_HZ, SHIFT_MAX_HZ);
        let feedback_target = p.feedback.clamp(0.0, FEEDBACK_MAX);
        let spread_target = p.spread.clamp(0.0, 1.0);
        if !self.primed {
            self.shift.reset(shift_target);
            self.feedback.reset(feedback_target);
            self.spread.reset(spread_target);
            self.mix.reset(p.mix.clamp(0.0, 1.0));
            self.primed = true;
        }
        let shift = self.shift.process(shift_target);
        let feedback = self.feedback.process(feedback_target) as f64;
        let spread = self.spread.process(spread_target);
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        let apart = spread * SPREAD_MAX_HZ;
        let top = self.sample_rate * 0.45;
        let hz = [
            (shift + apart).clamp(-top, top) as f64,
            (shift - apart).clamp(-top, top) as f64,
        ];
        let input = [l as f64, r as f64];
        let mut wet = [0.0f32; 2];
        for ch in 0..2 {
            let back = feedback * self.line[ch][self.at].tanh();
            let (q, i) = self.hilbert[ch].process(input[ch] + back);
            let ph = self.phase[ch] * std::f64::consts::TAU;
            let (s, c) = ph.sin_cos();
            let w = i * c - SIDE * q * s;
            let ph = self.phase[ch] + hz[ch] / self.sample_rate as f64;
            self.phase[ch] = ph - ph.floor();
            self.line[ch][self.at] = if w.abs() < FLUSH { 0.0 } else { w };
            wet[ch] = w as f32;
        }
        self.at = (self.at + 1) % self.line[0].len();

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
    const TAU: f64 = std::f64::consts::TAU;

    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn wet(shift_hz: f32) -> FreqShiftParams {
        FreqShiftParams {
            shift_hz,
            feedback: 0.0,
            spread: 0.0,
            mix: 1.0,
        }
    }

    /// The amplitude of the component at `hz` in `x` (a whole number of
    /// periods, so there is no leakage to speak of).
    fn amp_at(x: &[f32], hz: f64, sr: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, v) in x.iter().enumerate() {
            let a = TAU * hz * n as f64 / sr;
            re += *v as f64 * a.cos();
            im += *v as f64 * a.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    /// One second of a unit sine through the shifter, the second half back.
    fn through(f: f64, p: &FreqShiftParams, sr: f32) -> Vec<f32> {
        let mut s = FreqShift::new(sr);
        let mut out = Vec::new();
        for n in 0..(sr as usize) {
            let x = (TAU * f * n as f64 / sr as f64).sin() as f32;
            let (a, _) = s.process(x, x, p);
            if n >= sr as usize / 2 {
                out.push(a);
            }
        }
        out
    }

    #[test]
    fn the_two_chains_stay_in_quadrature_and_keep_the_level() {
        // Relative phase of the chains, 50 Hz to 20 kHz at 48 kHz: within
        // a degree of a quarter turn, each chain at unit gain.
        for &f in &[50.0f64, 100.0, 440.0, 2_000.0, 8_000.0, 16_000.0, 20_000.0] {
            let mut h = Hilbert::new();
            let (mut q_c, mut q_s, mut i_c, mut i_s) = (0.0, 0.0, 0.0, 0.0);
            let n = 48_000;
            for k in 0..n {
                let a = TAU * f * k as f64 / SR as f64;
                let (q, i) = h.process(a.cos());
                if k >= 24_000 {
                    q_c += q * a.cos();
                    q_s += q * a.sin();
                    i_c += i * a.cos();
                    i_s += i * a.sin();
                }
            }
            let (gq, gi) = (
                q_c.hypot(q_s) * 2.0 / 24_000.0,
                i_c.hypot(i_s) * 2.0 / 24_000.0,
            );
            assert!(
                (gq - 1.0).abs() < 0.01 && (gi - 1.0).abs() < 0.01,
                "{f}: {gq} {gi}"
            );
            let diff = (q_s.atan2(q_c) - i_s.atan2(i_c)).to_degrees();
            let diff = (diff + 180.0).rem_euclid(360.0) - 180.0;
            assert!((diff.abs() - 90.0).abs() < 1.0, "{f} Hz: {diff} degrees");
        }
    }

    #[test]
    fn a_positive_shift_moves_up_and_leaves_no_image_below() {
        // 200 Hz to 10 kHz, at 48 kHz: the shifted line is there at full
        // level and the mirror image is at least 30 dB down.
        for &(f, s) in &[
            (200.0, 100.0),
            (440.0, 300.0),
            (1_000.0, 500.0),
            (1_000.0, 2_000.0),
            (5_000.0, 700.0),
            (10_000.0, 2_000.0),
        ] {
            let out = through(f, &wet(s as f32), SR);
            let up = amp_at(&out, f + s, SR as f64);
            let image = amp_at(&out, f - s, SR as f64);
            assert!((up - 1.0).abs() < 0.12, "{f}+{s}: {up}");
            assert!(
                20.0 * (up / image.max(1e-9)).log10() > 30.0,
                "{f}+{s}: image {image} against {up}"
            );
            let stayed = amp_at(&out, f, SR as f64);
            assert!(
                stayed < 0.03,
                "{f}+{s}: the original is still there ({stayed})"
            );
        }
    }

    #[test]
    fn a_negative_shift_moves_down() {
        for &(f, s) in &[
            (1_000.0, 500.0),
            (5_000.0, 2_000.0),
            (10_000.0, 300.0),
            (2_500.0, 2_000.0),
        ] {
            let out = through(f, &wet(-s as f32), SR);
            let down = amp_at(&out, f - s, SR as f64);
            let image = amp_at(&out, f + s, SR as f64);
            assert!((down - 1.0).abs() < 0.12, "{f}-{s}: {down}");
            assert!(
                20.0 * (down / image.max(1e-9)).log10() > 30.0,
                "{f}-{s}: {image}"
            );
        }
    }

    #[test]
    fn zero_shift_at_full_mix_keeps_the_level() {
        // All-pass phase only: broadband level within half a dB.
        let mut s = FreqShift::new(SR);
        let p = wet(0.0);
        let mut seed = 9u32;
        let (mut din, mut dout) = (0.0f64, 0.0f64);
        for n in 0..96_000 {
            let x = noise(&mut seed);
            let (a, _) = s.process(x, x, &p);
            if n >= 4_800 {
                din += (x as f64).powi(2);
                dout += (a as f64).powi(2);
            }
        }
        let db = 10.0 * (dout / din).log10();
        assert!(db.abs() < 0.5, "{db} dB");
    }

    #[test]
    fn zero_mix_is_a_bit_exact_bypass() {
        let mut s = FreqShift::new(SR);
        let p = FreqShiftParams {
            shift_hz: 500.0,
            feedback: FEEDBACK_MAX,
            spread: 1.0,
            mix: 0.0,
        };
        let mut seed = 7u32;
        for _ in 0..20_000 {
            let (l, r) = (noise(&mut seed), noise(&mut seed));
            assert_eq!(s.process(l, r, &p), (l, r));
        }
    }

    #[test]
    fn mix_faded_to_zero_ends_exactly_transparent() {
        let mut s = FreqShift::new(SR);
        let on = wet(300.0);
        let off = FreqShiftParams { mix: 0.0, ..on };
        let mut seed = 3u32;
        for _ in 0..20_000 {
            s.process(noise(&mut seed), noise(&mut seed), &on);
        }
        for _ in 0..20_000 {
            s.process(noise(&mut seed), noise(&mut seed), &off);
        }
        for _ in 0..100 {
            let (l, r) = (noise(&mut seed), noise(&mut seed));
            assert_eq!(s.process(l, r, &off), (l, r));
        }
    }

    #[test]
    fn mix_is_a_crossfade_and_the_wet_is_as_loud_as_the_dry() {
        // Full mix against the dry, over noise: the same level (within 0.5 dB)
        // at a real shift, and half mix is the average of the two paths.
        let p = wet(400.0);
        let mut s = FreqShift::new(SR);
        let mut seed = 5u32;
        let (mut din, mut dout) = (0.0f64, 0.0f64);
        for n in 0..96_000 {
            let x = noise(&mut seed);
            let (a, _) = s.process(x, x, &p);
            if n >= 4_800 {
                din += (x as f64).powi(2);
                dout += (a as f64).powi(2);
            }
        }
        assert!((10.0 * (dout / din).log10()).abs() < 0.5);
        let mut full = FreqShift::new(SR);
        let mut half = FreqShift::new(SR);
        let ph = FreqShiftParams { mix: 0.5, ..p };
        let mut seed = 5u32;
        for _ in 0..5_000 {
            let x = noise(&mut seed);
            let (w, _) = full.process(x, x, &p);
            let (h, _) = half.process(x, x, &ph);
            assert!((h - (0.5 * x + 0.5 * w)).abs() < 1e-4);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut s = FreqShift::new(SR);
        let p = FreqShiftParams {
            shift_hz: 700.0,
            feedback: 0.9,
            spread: 0.5,
            mix: 1.0,
        };
        for _ in 0..10_000 {
            assert_eq!(s.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn feedback_climbs_in_steps_of_the_shift() {
        // 1 kHz in, +100 Hz: with no feedback the sound is at 1100 and not
        // 1200; with feedback the second trip puts a line at 1200.
        let at = |fb: f32| {
            let p = FreqShiftParams {
                feedback: fb,
                ..wet(100.0)
            };
            let out = through(1_000.0, &p, SR);
            (
                amp_at(&out, 1_100.0, SR as f64),
                amp_at(&out, 1_200.0, SR as f64),
            )
        };
        let (_, none) = at(0.0);
        let (first, second) = at(0.8);
        assert!(none < 0.02, "{none}");
        assert!(second > 0.2 * first, "{second} against {first}");
    }

    #[test]
    fn no_runaway_at_maximum_feedback() {
        for &shift in &[0.0f32, 40.0, -700.0, 2_000.0] {
            for &mono in &[false, true] {
                let mut s = FreqShift::new(SR);
                let p = FreqShiftParams {
                    shift_hz: shift,
                    feedback: FEEDBACK_MAX,
                    spread: 1.0,
                    mix: 1.0,
                };
                let mut seed = 5u32;
                let mut peak = 0.0f32;
                for i in 0..(SR as usize * 6) {
                    let (x, y) = if i < SR as usize * 3 {
                        let a = noise(&mut seed);
                        (a, if mono { a } else { noise(&mut seed) })
                    } else {
                        (0.0, 0.0)
                    };
                    let (a, b) = s.process(x, y, &p);
                    assert!(a.is_finite() && b.is_finite());
                    peak = peak.max(a.abs()).max(b.abs());
                }
                assert!(peak < 12.0, "shift {shift} mono {mono}: peak {peak}");
            }
        }
    }

    #[test]
    fn the_tail_dies_when_the_input_stops() {
        let mut s = FreqShift::new(SR);
        let p = FreqShiftParams {
            shift_hz: 150.0,
            feedback: FEEDBACK_MAX,
            spread: 0.0,
            mix: 1.0,
        };
        let mut seed = 2u32;
        for _ in 0..48_000 {
            let x = noise(&mut seed);
            s.process(x, x, &p);
        }
        let mut last = 0.0f32;
        for i in 0..(SR as usize * 10) {
            let (a, _) = s.process(0.0, 0.0, &p);
            if i >= SR as usize * 9 {
                last = last.max(a.abs());
            }
        }
        assert!(last < 1e-3, "{last}");
    }

    #[test]
    fn sweeping_shift_does_not_click() {
        // A 440 Hz sine, Shift swept the whole range and back in a second:
        // no step beyond what a sine at the top of the sweep can make.
        let mut s = FreqShift::new(SR);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for n in 0..(SR as usize * 2) {
            let t = n as f32 / SR;
            let shift = SHIFT_MAX_HZ * (std::f32::consts::TAU * 1.0 * t).sin();
            let x = 0.5 * (TAU * 440.0 * n as f64 / SR as f64).sin() as f32;
            let p = wet(shift);
            let (a, _) = s.process(x, x, &p);
            if n > 4_800 {
                worst = worst.max((a - prev).abs());
            }
            prev = a;
        }
        // 0.5 * 2*pi * 2440 / 48000 = 0.16 at the extreme.
        assert!(worst < 0.2, "{worst}");
    }

    #[test]
    fn a_jump_in_shift_does_not_click() {
        let mut s = FreqShift::new(SR);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for n in 0..(SR as usize) {
            let shift = if n < 24_000 { -1_500.0 } else { 1_500.0 };
            let x = 0.5 * (TAU * 440.0 * n as f64 / SR as f64).sin() as f32;
            let (a, _) = s.process(x, x, &wet(shift));
            if n > 4_800 {
                worst = worst.max((a - prev).abs());
            }
            prev = a;
        }
        assert!(worst < 0.2, "{worst}");
    }

    #[test]
    fn spread_pulls_the_sides_apart_and_zero_keeps_them_together() {
        let mut s = FreqShift::new(SR);
        let mut seed = 4u32;
        for _ in 0..5_000 {
            let x = noise(&mut seed);
            let (a, b) = s.process(x, x, &wet(100.0));
            assert_eq!(a, b);
        }
        // At full Spread the left sits at 1020 and the right at 980 for a
        // 1 kHz sine and no Shift.
        let p = FreqShiftParams {
            spread: 1.0,
            ..wet(0.0)
        };
        let mut s = FreqShift::new(SR);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for n in 0..(SR as usize) {
            let x = (TAU * 1_000.0 * n as f64 / SR as f64).sin() as f32;
            let (a, b) = s.process(x, x, &p);
            if n >= SR as usize / 2 {
                l.push(a);
                r.push(b);
            }
        }
        assert!(amp_at(&l, 1_020.0, SR as f64) > 0.85);
        assert!(amp_at(&r, 980.0, SR as f64) > 0.85);
        assert!(amp_at(&l, 980.0, SR as f64) < 0.05);
    }

    #[test]
    fn works_at_each_rate_and_sizes_its_line() {
        for &sr in &[22_050.0f32, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut s = FreqShift::new(sr);
            let want = (0.010 * sr).round() as usize;
            assert_eq!(s.line[0].len(), want);
            let p = FreqShiftParams {
                shift_hz: -2_000.0,
                feedback: FEEDBACK_MAX,
                spread: 1.0,
                mix: 1.0,
            };
            let mut seed = 1u32;
            for _ in 0..(sr as usize / 2) {
                let (a, b) = s.process(noise(&mut seed), noise(&mut seed), &p);
                assert!(a.is_finite() && b.is_finite() && a.abs() < 12.0 && b.abs() < 12.0);
            }
        }
    }

    #[test]
    fn the_shift_is_right_at_other_rates() {
        for &sr in &[44_100.0f32, 96_000.0, 192_000.0] {
            let out = through(2_000.0, &wet(500.0), sr);
            let up = amp_at(&out, 2_500.0, sr as f64);
            let image = amp_at(&out, 1_500.0, sr as f64);
            assert!((up - 1.0).abs() < 0.12, "{sr}: {up}");
            assert!(
                20.0 * (up / image.max(1e-9)).log10() > 30.0,
                "{sr}: {image}"
            );
        }
    }

    #[test]
    fn extremes_are_safe() {
        let mut s = FreqShift::new(SR);
        let p = FreqShiftParams {
            shift_hz: 1e9,
            feedback: 9.0,
            spread: 9.0,
            mix: 9.0,
        };
        let mut seed = 6u32;
        for _ in 0..10_000 {
            let (a, b) = s.process(noise(&mut seed), noise(&mut seed), &p);
            assert!(a.is_finite() && b.is_finite());
        }
    }

    #[test]
    fn reset_forgets_the_sound() {
        let mut s = FreqShift::new(SR);
        let p = FreqShiftParams {
            shift_hz: 200.0,
            feedback: FEEDBACK_MAX,
            spread: 0.0,
            mix: 1.0,
        };
        let mut seed = 8u32;
        for _ in 0..48_000 {
            s.process(noise(&mut seed), noise(&mut seed), &p);
        }
        s.reset();
        // Longer than the delay line and the all-passes' memory is zeroed, so
        // the very first frame is already exact zeros.
        for _ in 0..2_000 {
            assert_eq!(s.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn the_oscillator_does_not_drift() {
        let mut s = FreqShift::new(SR);
        let p = wet(1_234.5);
        for _ in 0..(SR as usize * 20) {
            s.process(0.1, 0.1, &p);
        }
        assert!(s.phase[0] >= 0.0 && s.phase[0] < 1.0);
    }
}
