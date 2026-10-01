//! Rise.
//!
//! An echo whose repeats climb. Each time the sound goes round the loop it
//! comes back a fixed interval higher than the last, so one note leaves a
//! staircase behind it: the original, then a fifth up, then a ninth, then a
//! thirteenth, thinning as it goes. Set the interval negative and the same
//! staircase descends. Ascending is the point, though, and the cartoon of it
//! is a shimmering run of repeats rising out of the note and out of hearing.
//!
//! The mechanism is a delay line with a pitch shifter in its feedback path.
//! The line is read `Time` less the shifter's own latency back, so the
//! shifter's delay counts towards the loop and the first repeat lands at
//! `Time` exactly. What comes out of the shifter is the wet signal, and a
//! scaled, saturated copy of it goes back into the line behind the input.
//! There is one line and one shifter a side, the shifter being the shared one
//! in `shift.rs`.
//!
//! Left alone, a loop that climbs would be a bright mess, and one at full
//! feedback could build without end, so the loop looks after itself in
//! three ways:
//!
//! - **Tone** is a low-pass after the shifter, inside the loop. Every pass
//!   through it takes the top off, so the higher repeats are also the darker
//!   ones, and they fade into the air rather than turning to glass.
//! - **Loss** grows with the interval. A pass that shifts by `s` semitones
//!   keeps `1 - 0.3 * |s| / 12` of its level, so the further each repeat
//!   climbs, the faster the staircase decays. It is a loss per pass, and the
//!   passes are what accumulate the shift, so the total loss follows the
//!   total shift. At twelve semitones and the most feedback the loop gain is
//!   about 0.63, which is a tail, not a runaway. At no shift there is no
//!   extra loss, and the feedback control alone sets the decay.
//! - **Saturation** is a soft knee on what goes back into the line, so a
//!   loud input that would otherwise stack up on itself leans against a
//!   ceiling instead of growing.
//!
//! Wobble is a slow random wander on the shift, up to 30 cents either way at
//! full, on a path that eases from one random point to the next and so never
//! turns a corner. It stops the repeats being a perfect machine. The left
//! and right lines wander on their own paths at slightly different speeds, so
//! a mono source comes out wide, and the two sides' repeats drift apart as
//! they climb.
//!
//! At zero mix the node is a bit-exact bypass. The lines keep running, so
//! switching on never plays stale audio.

use crate::rng::Rng;
use crate::shift::{semitones_to_ratio, PitchShifter};
use crate::smooth::{OnePole, Ramp};

/// The shortest and longest time to the first repeat, in milliseconds. The
/// shortest is well over the shifter's own 22 ms, which counts towards it.
pub const RISE_MIN_MS: f32 = 80.0;
pub const RISE_MAX_MS: f32 = 1_200.0;
/// The most feedback. A repeat is scaled by this before loss and saturation,
/// and a test holds the loop under it at the extremes.
pub const MAX_FEEDBACK: f32 = 0.9;
/// The most each repeat climbs or falls, in semitones.
pub const MAX_SHIFT_ST: f32 = 12.0;
/// The tone range, in hertz.
pub const TONE_MIN_HZ: f32 = 800.0;
pub const TONE_MAX_HZ: f32 = 14_000.0;
/// The widest the wobble strays at full, in cents, either way.
pub const WOBBLE_CENTS: f32 = 30.0;
/// How much level a repeat loses at the full interval, as a fraction.
const LOSS_AT_MAX_SHIFT: f32 = 0.3;
/// How hard the knee leans: the feedback is `x / (1 + KNEE * |x|)`. Gentle
/// below about a quarter, a ceiling of `1 / KNEE` far above.
const KNEE: f32 = 0.5;
/// The wobble's speeds, left and right, in hertz. Different, and slow.
const WANDER_HZ: [f32; 2] = [0.45, 0.62];
/// Margin on the line past the longest read, for the interpolator.
const GUARD: usize = 6;

#[derive(Debug, Clone, Copy)]
pub struct RiseParams {
    /// Time to the first repeat, `RISE_MIN_MS` to `RISE_MAX_MS`. Default 380.
    pub time_ms: f32,
    /// How much of each repeat goes round again, 0 to `MAX_FEEDBACK`. Default 0.6.
    pub feedback: f32,
    /// Semitones each repeat climbs, ±`MAX_SHIFT_ST`. Default +7, a fifth.
    pub shift_st: f32,
    /// Random wander on the shift, 0 to 1. Default 0.3.
    pub wobble: f32,
    /// The loop's low-pass, `TONE_MIN_HZ` to `TONE_MAX_HZ`. Default 6000.
    pub tone_hz: f32,
    /// Dry plus wet times this, 0 to 1. Default 0.4.
    pub mix: f32,
}

impl Default for RiseParams {
    fn default() -> Self {
        Self {
            time_ms: 380.0,
            feedback: 0.6,
            shift_st: 7.0,
            wobble: 0.3,
            tone_hz: 6_000.0,
            mix: 0.4,
        }
    }
}

/// A random path: from one point to the next along a half cosine.
#[derive(Debug, Clone, Copy, Default)]
struct Wander {
    from: f32,
    to: f32,
    t: f32,
}

/// One side: its line, its shifter, its tone filter and its wobble.
struct Side {
    line: Vec<f32>,
    write: usize,
    shifter: PitchShifter,
    tone_state: f32,
    wander: Wander,
    rng: Rng,
}

impl Side {
    fn new(sample_rate: f32, seed: u32, len: usize) -> Self {
        let mut rng = Rng::new(seed);
        let wander = Wander {
            from: rng.next_bipolar(),
            to: rng.next_bipolar(),
            // Start the paths at different places so the sides never
            // begin together.
            t: rng.next_f32(),
        };
        Self {
            line: vec![0.0; len],
            write: 0,
            shifter: PitchShifter::new(sample_rate),
            tone_state: 0.0,
            wander,
            rng,
        }
    }

    /// Four-point Hermite read `delay` samples behind the sample about to be
    /// written.
    #[inline]
    fn tap(&self, delay: f32) -> f32 {
        let d = (delay - 1.0).clamp(1.0, (self.line.len() - GUARD) as f32);
        let whole = d.floor();
        let t = d - whole;
        let n = self.line.len();
        let at = |back: usize| self.line[(self.write + n - back) % n];
        let w = whole as usize;
        let (xm1, x0, x1, x2) = (at(w - 1), at(w), at(w + 1), at(w + 2));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    /// The wobble's value, -1 to 1, advanced one sample at `hz`.
    #[inline]
    fn wobble(&mut self, hz: f32, sample_rate: f32) -> f32 {
        let w = &mut self.wander;
        w.t += hz / sample_rate;
        if w.t >= 1.0 {
            w.t -= w.t.floor();
            w.from = w.to;
            w.to = self.rng.next_bipolar();
        }
        let ease = 0.5 - 0.5 * (core::f32::consts::PI * w.t).cos();
        w.from + (w.to - w.from) * ease
    }

    /// One sample round the loop: returns the wet, which is the newest
    /// repeat as the shifter delivers it.
    #[inline]
    fn step(&mut self, x: f32, delay: f32, ratio: f32, tone_coef: f32, gain: f32) -> f32 {
        let back = self.tap(delay);
        let shifted = self.shifter.process(back, ratio);
        self.tone_state += tone_coef * (shifted - self.tone_state);
        // Anything denormal in a loop that recirculates would slow it down.
        if self.tone_state.abs() < 1e-20 {
            self.tone_state = 0.0;
        }
        let wet = self.tone_state;
        let fed = gain * wet / (1.0 + KNEE * wet.abs());
        let sum = (x + fed).clamp(-2.0, 2.0);
        self.write = (self.write + 1) % self.line.len();
        self.line[self.write] = if sum.abs() < 1e-20 { 0.0 } else { sum };
        wet
    }
}

pub struct Rise {
    sides: [Side; 2],
    sample_rate: f32,
    /// The shifter's latency in samples, which is part of the loop.
    latency: f32,
    primed: bool,
    time: OnePole,
    feedback: OnePole,
    shift: OnePole,
    wobble: OnePole,
    tone: OnePole,
    mix: Ramp,
}

impl Rise {
    pub fn new(sample_rate: f32) -> Self {
        let len = (RISE_MAX_MS * 0.001 * sample_rate).ceil() as usize + GUARD + 4;
        let smoother = |ms: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p
        };
        let mut mix = Ramp::new(0.0);
        mix.set_time(20.0, sample_rate);
        let sides = [
            Side::new(sample_rate, 0x5EED_0001, len),
            Side::new(sample_rate, 0x5EED_0002, len),
        ];
        let latency = sides[0].shifter.latency_samples() as f32;
        Self {
            sides,
            sample_rate,
            latency,
            primed: false,
            // A moving delay time bends the pitch of everything in the
            // loop, as a tape delay's does. Slow, to keep that to a glide.
            time: smoother(150.0),
            feedback: smoother(20.0),
            shift: smoother(20.0),
            wobble: smoother(20.0),
            tone: smoother(20.0),
            mix,
        }
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &RiseParams) -> (f32, f32) {
        let time_ms = p.time_ms.clamp(RISE_MIN_MS, RISE_MAX_MS);
        let feedback = p.feedback.clamp(0.0, MAX_FEEDBACK);
        let shift_st = p.shift_st.clamp(-MAX_SHIFT_ST, MAX_SHIFT_ST);
        let wobble = p.wobble.clamp(0.0, 1.0);
        let tone_hz = p.tone_hz.clamp(TONE_MIN_HZ, TONE_MAX_HZ);
        let mix_target = p.mix.clamp(0.0, 1.0);
        if !self.primed {
            // A fresh node starts at its settings rather than gliding to
            // them from the defaults.
            self.primed = true;
            self.time.reset(time_ms);
            self.feedback.reset(feedback);
            self.shift.reset(shift_st);
            self.wobble.reset(wobble);
            self.tone.reset(tone_hz);
            self.mix = Ramp::new(mix_target);
            self.mix.set_time(20.0, self.sample_rate);
        }
        let time = self.time.process(time_ms);
        let feedback = self.feedback.process(feedback);
        let shift_st = self.shift.process(shift_st);
        let wobble = self.wobble.process(wobble);
        let tone_hz = self.tone.process(tone_hz);
        let mix = self.mix.process(mix_target);

        // The shifter's latency is part of the loop, so the line is read
        // that much sooner and the first repeat lands at `time`.
        let delay = (time * 0.001 * self.sample_rate - self.latency).max(2.0);
        let tone_coef = 1.0 - (-core::f32::consts::TAU * tone_hz / self.sample_rate).exp();
        // Loss per pass, rising with the interval.
        let loss = 1.0 - LOSS_AT_MAX_SHIFT * shift_st.abs() / MAX_SHIFT_ST;
        let gain = feedback * loss;

        let input = [l, r];
        let mut wet = [0.0; 2];
        for (ch, side) in self.sides.iter_mut().enumerate() {
            let cents = side.wobble(WANDER_HZ[ch], self.sample_rate) * WOBBLE_CENTS * wobble;
            let ratio = semitones_to_ratio(shift_st + cents * 0.01);
            wet[ch] = side.step(input[ch], delay, ratio, tone_coef, gain);
        }
        if mix == 0.0 {
            return (l, r);
        }
        (l + wet[0] * mix, r + wet[1] * mix)
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
        (*state >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }

    fn p(time_ms: f32, feedback: f32, shift_st: f32, wobble: f32, mix: f32) -> RiseParams {
        RiseParams {
            time_ms,
            feedback,
            shift_st,
            wobble,
            tone_hz: TONE_MAX_HZ,
            mix,
        }
    }

    /// The strongest frequency near `centre` over `y`, by a scanned DFT.
    fn measure(y: &[f32], sr: f32, centre: f32) -> f32 {
        let mut best = (0.0, 0.0);
        let mut f = centre * 0.85;
        while f < centre * 1.15 {
            let w = core::f64::consts::TAU * f as f64 / sr as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in y.iter().enumerate() {
                re += v as f64 * (w * i as f64).cos();
                im += v as f64 * (w * i as f64).sin();
            }
            let mag = re * re + im * im;
            if mag > best.0 {
                best = (mag, f);
            }
            f += 0.5;
        }
        best.1
    }

    #[test]
    fn zero_mix_is_a_bit_exact_bypass() {
        let mut rise = Rise::new(SR);
        let mut s = 1u32;
        let params = p(200.0, 0.8, 7.0, 0.5, 0.0);
        for i in 0..SR as usize {
            let (l, r) = (noise(&mut s), noise(&mut s));
            let (a, b) = rise.process(l, r, &params);
            assert_eq!((a, b), (l, r), "sample {i}");
        }
        // The lines kept running, so switching on is heard at once.
        let on = p(200.0, 0.8, 7.0, 0.5, 1.0);
        let mut heard = false;
        for _ in 0..SR as usize / 2 {
            let (a, _) = rise.process(0.0, 0.0, &on);
            heard |= a.abs() > 0.01;
        }
        assert!(heard);
    }

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        let mut rise = Rise::new(SR);
        let mut s = 3u32;
        let mut params = p(200.0, 0.8, 7.0, 0.5, 1.0);
        for _ in 0..SR as usize / 2 {
            rise.process(noise(&mut s), noise(&mut s), &params);
        }
        params.mix = 0.0;
        for _ in 0..4800 {
            rise.process(noise(&mut s), noise(&mut s), &params);
        }
        for _ in 0..1000 {
            let (l, r) = (noise(&mut s), noise(&mut s));
            assert_eq!(rise.process(l, r, &params), (l, r));
        }
    }

    #[test]
    fn silence_in_silence_out() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut rise = Rise::new(sr);
            let params = RiseParams {
                mix: 1.0,
                ..RiseParams::default()
            };
            for _ in 0..sr as usize {
                assert_eq!(rise.process(0.0, 0.0, &params), (0.0, 0.0));
            }
        }
    }

    #[test]
    fn the_loop_cannot_run_away() {
        for (time, shift) in [(380.0, 12.0), (380.0, -12.0), (80.0, 12.0), (1200.0, 0.0)] {
            let mut rise = Rise::new(SR);
            let params = RiseParams {
                time_ms: time,
                feedback: MAX_FEEDBACK,
                shift_st: shift,
                wobble: 1.0,
                tone_hz: TONE_MAX_HZ,
                mix: 1.0,
            };
            let mut s = 11u32;
            let mut peak_after = [0.0f32; 2];
            for i in 0..SR as usize * 7 {
                let x = if i < SR as usize { noise(&mut s) } else { 0.0 };
                let (a, b) = rise.process(x, -x, &params);
                assert!(
                    a.is_finite() && b.is_finite() && a.abs() <= 4.0 && b.abs() <= 4.0,
                    "time {time} shift {shift} sample {i}: {a} {b}"
                );
                if i >= SR as usize * 2 {
                    let half = (i >= SR as usize * 5) as usize;
                    peak_after[half] = peak_after[half].max(a.abs()).max(b.abs());
                }
            }
            // Bounded, and not growing: the late tail is no louder than
            // the early one. With an interval it is gone.
            assert!(peak_after[1] <= peak_after[0], "{peak_after:?}");
            if shift != 0.0 {
                assert!(peak_after[1] < 0.05, "tail {peak_after:?}");
            }
        }
    }

    fn music(i: usize, sr: f32) -> f32 {
        let t = i as f32 / sr;
        0.3 * (core::f32::consts::TAU * 220.0 * t).sin()
            + 0.2 * (core::f32::consts::TAU * 330.0 * t).sin()
            + 0.1 * (core::f32::consts::TAU * 1250.0 * t).sin()
    }

    #[test]
    fn a_swept_parameter_does_not_click() {
        let n = SR as usize;
        let in_step = (1..n).fold(0.0f32, |m, i| {
            m.max((music(i, SR) - music(i - 1, SR)).abs())
        });
        // Each parameter in turn, from one end of its range to the other,
        // the rest settled and the loop already full.
        for which in 0..6 {
            let mut rise = Rise::new(SR);
            let mut prev = 0.0;
            let mut worst = 0.0f32;
            for i in 0..SR as usize * 3 {
                let f = ((i as f32 - SR) / SR).clamp(0.0, 1.0);
                let mut params = p(300.0, 0.7, 5.0, 0.3, 0.5);
                params.tone_hz = 6_000.0;
                match which {
                    0 => params.time_ms = RISE_MIN_MS + f * (RISE_MAX_MS - RISE_MIN_MS),
                    1 => params.feedback = f * MAX_FEEDBACK,
                    2 => params.shift_st = -12.0 + 24.0 * f,
                    3 => params.wobble = f,
                    4 => params.tone_hz = TONE_MIN_HZ + f * (TONE_MAX_HZ - TONE_MIN_HZ),
                    _ => params.mix = f,
                }
                let (y, _) = rise.process(music(i, SR), music(i, SR), &params);
                if i > 4800 {
                    worst = worst.max((y - prev).abs());
                }
                prev = y;
            }
            // A swept shift or time bends the pitch up to an octave, which
            // doubles a tone's own steps; the rest add the wet.
            assert!(
                worst < in_step * 5.0,
                "param {which}: {worst}, input {in_step}"
            );
        }
    }

    /// A burst of a steady tone, then the rest silent.
    fn tone_burst(f: f32, i: usize, len: usize) -> f32 {
        if i < len {
            0.5 * (core::f32::consts::TAU * f * i as f32 / SR).sin()
        } else {
            0.0
        }
    }

    #[test]
    fn each_repeat_climbs_by_the_interval() {
        let mut rise = Rise::new(SR);
        let params = p(500.0, 0.8, 7.0, 0.0, 1.0);
        // A tone that fits the shifter's window, a burst a quarter second
        // long, repeats at 0.5 s and 1.0 s.
        let base = 500.0;
        let y: Vec<f32> = (0..SR as usize * 2)
            .map(|i| rise.process(tone_burst(base, i, 12_000), 0.0, &params).0)
            .collect();
        let ratio = semitones_to_ratio(7.0);
        for n in 1..=2 {
            let start = (SR * 0.5 * n as f32) as usize + 4800;
            let want = base * ratio.powi(n);
            let f = measure(&y[start..start + 7200], SR, want);
            assert!(
                (f - want).abs() / want < 0.02,
                "repeat {n}: {f} Hz, wanted {want}"
            );
        }
    }

    /// The centre of mass of the energy in `y[from..to]`, in samples.
    fn centroid(y: &[f32], from: usize, to: usize) -> f32 {
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for (i, &v) in y.iter().enumerate().take(to).skip(from) {
            num += i as f64 * (v * v) as f64;
            den += (v * v) as f64;
        }
        (num / den) as f32
    }

    #[test]
    fn the_first_repeat_lands_at_the_time() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for (shift, tol_ms) in [(0.0, 1.0), (7.0, 8.0), (-5.0, 8.0)] {
                let mut rise = Rise::new(sr);
                let params = p(400.0, 0.5, shift, 0.0, 1.0);
                let mut s = 5u32;
                let burst = (0.003 * sr) as usize;
                let y: Vec<f32> = (0..(sr * 0.9) as usize)
                    .map(|i| {
                        let x = if i < burst { noise(&mut s) } else { 0.0 };
                        rise.process(x, 0.0, &params).0 - x
                    })
                    .collect();
                // Only the wet is left. Its first burst, away from the second.
                let from = (sr * 0.3) as usize;
                let to = (sr * 0.62) as usize;
                let sent = centroid(&noise_burst(burst, 5), 0, burst);
                let ms = (centroid(&y, from, to) - sent) / sr * 1000.0;
                assert!(
                    (ms - 400.0).abs() < tol_ms,
                    "sr {sr} shift {shift}: repeat at {ms} ms"
                );
            }
        }
    }

    fn noise_burst(len: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..len).map(|_| noise(&mut s)).collect()
    }

    #[test]
    fn a_darker_tone_leaves_less_in_the_late_repeats() {
        let late_energy = |tone: f32| {
            let mut rise = Rise::new(SR);
            let mut params = p(200.0, 0.8, 7.0, 0.0, 1.0);
            params.tone_hz = tone;
            let mut s = 9u32;
            let mut e = 0.0f64;
            for i in 0..SR as usize * 2 {
                let x = if i < 2400 { noise(&mut s) } else { 0.0 };
                let (y, _) = rise.process(x, 0.0, &params);
                if i > SR as usize {
                    e += (y * y) as f64;
                }
            }
            e
        };
        assert!(late_energy(TONE_MIN_HZ) < late_energy(TONE_MAX_HZ) * 0.5);
    }

    #[test]
    fn the_sides_wander_apart() {
        let mut rise = Rise::new(SR);
        let params = p(250.0, 0.7, 7.0, 1.0, 1.0);
        let mut diff = 0.0f64;
        for i in 0..SR as usize * 3 {
            let x = 0.4 * (core::f32::consts::TAU * 440.0 * i as f32 / SR).sin();
            let (l, r) = rise.process(x, x, &params);
            if i > SR as usize {
                diff += ((l - r) * (l - r)) as f64;
            }
        }
        assert!(diff > 1.0, "{diff}");
    }

    #[test]
    fn a_mono_source_stays_finite_and_bounded() {
        let mut rise = Rise::new(SR);
        let params = RiseParams {
            feedback: MAX_FEEDBACK,
            shift_st: 12.0,
            wobble: 1.0,
            mix: 1.0,
            ..RiseParams::default()
        };
        for i in 0..SR as usize * 3 {
            let x = music(i, SR) * 3.0;
            let (l, r) = rise.process(x, x, &params);
            assert!(l.is_finite() && r.is_finite() && l.abs() <= 4.0 && r.abs() <= 4.0);
        }
    }

    #[test]
    fn works_at_other_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut rise = Rise::new(sr);
            let params = RiseParams {
                time_ms: RISE_MAX_MS,
                feedback: MAX_FEEDBACK,
                mix: 1.0,
                ..RiseParams::default()
            };
            let mut s = 2u32;
            let mut heard = false;
            for i in 0..(sr * 2.5) as usize {
                let x = if i < 4800 { noise(&mut s) } else { 0.0 };
                let (l, r) = rise.process(x, x, &params);
                assert!(l.is_finite() && r.is_finite() && l.abs() <= 4.0);
                heard |= i > (sr * 1.2) as usize && l.abs() > 0.01;
            }
            assert!(heard, "sr {sr}: no repeat");
        }
    }
}
