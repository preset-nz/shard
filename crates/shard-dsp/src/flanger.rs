//! Flanger.
//!
//! A very short delay, a fraction of a millisecond to a few, summed with the
//! dry signal. The two interfere: every frequency whose half-cycle fits the
//! delay arrives out of step with itself and cancels, which carves a comb of
//! notches into the spectrum, the first at 1 / (2 x delay) and then at odd
//! multiples of it. Sweep the delay and the whole comb slides up and down, a
//! hollow whoosh that rides through the sound like a jet passing.
//!
//! `manual_ms` sets where the sweep is centred. Short is bright and thin, with
//! the first notch far up the spectrum; long is low and throaty. The LFO moves
//! the delay around it, by `depth`, and does it in a geometric way: the delay
//! is the centre times two to the power of the LFO, so a quarter of the way
//! up the sweep is as many octaves for the notches as the quarter before it,
//! and the whoosh feels even, not rushed at one end. Full depth swings two
//! and a half octaves either side of the centre, and the delay is held between
//! 0.05 ms and 10 ms however the controls add up, so it never reaches zero or
//! goes negative. The LFO is a triangle with its corners rounded off (a cubic
//! easing), so it is steadier in the middle than a sine, which is what makes
//! the sweep read as constant, and it turns round without a lurch.
//!
//! Feedback sends the delayed signal back into the delay's input, so the comb
//! rings. Sign matters. Positive feedback reinforces the frequencies the delay
//! fits whole cycles of, at 1 / delay and its multiples, so the peaks between
//! the notches sharpen into a metallic, ringing, near-pitched tone. Negative
//! feedback reinforces the odd multiples of 1 / (2 x delay) instead, the
//! frequencies the dry signal cancels, so the notches fill in and the peaks
//! land where the notches were: a hollow, woody comb that rings an octave
//! lower. Either way the delayed signal passes through a soft limiter and a
//! denormal flush before it goes round again, so a loud input leans the sound
//! into a little saturation rather than into runaway, and the tail dies away
//! instead of lingering as a whisper of denormals.
//!
//! The right channel's LFO sits a quarter turn from the left's, so the two
//! sides' combs are never in the same place and a mono source opens out wide.
//!
//! Mix is dry and wet at equal level, so the notches are deep: at full mix
//! the output is half the dry plus half the delayed copy, and with no
//! feedback its first notch is exactly empty. Less mix lets more of the plain
//! dry through and shallows the notches. The sum is levelled for feedback, so
//! raising it adds ring without adding volume.
//!
//! At zero mix the node is a bit-exact bypass. The line keeps filling, so
//! switching on never plays stale audio.

use crate::smooth::OnePole;

/// The centre delay's range, in milliseconds. Exponential feel: a control
/// should map it on a log scale, so each stretch of travel is an equal
/// musical step for the notches.
pub const MANUAL_MIN_MS: f32 = 0.1;
pub const MANUAL_MAX_MS: f32 = 8.0;
/// The slowest and fastest LFO. A flanger sweeps slowly, once every few
/// seconds, but ten hertz is a warble worth having.
pub const MIN_RATE_HZ: f32 = 0.02;
pub const MAX_RATE_HZ: f32 = 10.0;
/// Feedback's reach either way. Under one, so the loop always dies away.
pub const MAX_FEEDBACK: f32 = 0.95;
/// The delay is never shorter or longer than this, whatever the sweep adds
/// up to. The floor keeps the read point clear of zero; the ceiling sizes the
/// line.
pub const DELAY_FLOOR_MS: f32 = 0.05;
pub const DELAY_CEIL_MS: f32 = 10.0;
/// The octaves the delay swings either side of its centre at full depth.
const SWEEP_OCTAVES: f32 = 2.5;
/// Where the feedback limiter tops out. The loop is linear well below this
/// and rounds off towards it.
const LIMIT: f32 = 1.5;
/// Below this a recirculating value is flushed to zero, so it never reaches
/// the denormal range, which is slow on some processors.
const FLUSH: f32 = 1e-18;

#[derive(Debug, Clone, Copy)]
pub struct FlangerParams {
    /// The centre delay in milliseconds, `MANUAL_MIN_MS` to `MANUAL_MAX_MS`.
    pub manual_ms: f32,
    /// LFO rate in hertz, `MIN_RATE_HZ` to `MAX_RATE_HZ`.
    pub rate_hz: f32,
    /// How far the delay sweeps round the centre, 0 to 1.
    pub depth: f32,
    /// Feedback, -`MAX_FEEDBACK` to `MAX_FEEDBACK`. Positive rings at the
    /// comb's peaks, negative gives a hollow comb with the peaks where the
    /// notches were. Zero is none.
    pub feedback: f32,
    /// Dry to wet, 0 to 1. Full is dry and wet at equal level.
    pub mix: f32,
}

impl Default for FlangerParams {
    fn default() -> Self {
        Self {
            manual_ms: 1.5,
            rate_hz: 0.25,
            depth: 0.7,
            feedback: 0.5,
            mix: 0.5,
        }
    }
}

pub struct Flanger {
    /// Interleaved left and right frames, sized once. `write` is the newest.
    line: Vec<f32>,
    write: usize,
    frames: usize,
    /// The left LFO's place in the turn. The right's is a quarter further.
    phase: f32,
    sample_rate: f32,
    manual: OnePole,
    rate: OnePole,
    depth: OnePole,
    feedback: OnePole,
    mix: OnePole,
}

/// The LFO shape: a triangle in, easing out. `turn` is the place in the cycle;
/// the result runs -1 to 1 with a flat spot at each turnaround.
#[inline]
fn lfo(turn: f32) -> f32 {
    let t = turn - turn.floor();
    let tri = 4.0 * (t - 0.5).abs() - 1.0;
    // 1.5x - 0.5x^3: slope 1.5 through the middle, zero at both ends.
    tri * (1.5 - 0.5 * tri * tri)
}

/// A soft limit with unit slope at zero that holds a recirculating signal to
/// `LIMIT`: a rational tanh, the same shape from either side.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let u = (x / LIMIT).clamp(-3.0, 3.0);
    LIMIT * u * (27.0 + u * u) / (27.0 + 9.0 * u * u)
}

impl Flanger {
    pub fn new(sample_rate: f32) -> Self {
        let frames = (DELAY_CEIL_MS * 0.001 * sample_rate).ceil() as usize + 4;
        let d = FlangerParams::default();
        let smoother = |ms: f32, v: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        Self {
            line: vec![0.0; frames * 2],
            write: 0,
            frames,
            phase: 0.0,
            sample_rate,
            // The centre delay moves the read point, so a step in it is a
            // pitch jump on the wet. Slower than the house 20 ms for that.
            manual: smoother(30.0, d.manual_ms),
            rate: smoother(50.0, d.rate_hz),
            depth: smoother(50.0, d.depth),
            feedback: smoother(20.0, d.feedback),
            // Starts closed, so a fresh node at zero mix is exact at once.
            mix: smoother(20.0, 0.0),
        }
    }

    /// One channel's read at `delay` frames behind the frame being written,
    /// through a four-point Hermite curve. A swept read point passes halfway
    /// between samples constantly, and linear interpolation would dull the
    /// top and wobble the comb's depth as it did.
    #[inline]
    fn tap(&self, ch: usize, delay: f32) -> f32 {
        let d = delay.clamp(2.0, (self.frames - 3) as f32);
        let whole = d.floor();
        let t = d - whole;
        let n = self.frames;
        let at = |back: usize| self.line[((self.write + n - back) % n) * 2 + ch];
        let w = whole as usize;
        let (xm1, x0, x1, x2) = (at(w - 1), at(w), at(w + 1), at(w + 2));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &FlangerParams) -> (f32, f32) {
        let manual = self
            .manual
            .process(p.manual_ms.clamp(MANUAL_MIN_MS, MANUAL_MAX_MS));
        let rate = self.rate.process(p.rate_hz.clamp(MIN_RATE_HZ, MAX_RATE_HZ));
        let depth = self.depth.process(p.depth.clamp(0.0, 1.0));
        let fb = self
            .feedback
            .process(p.feedback.clamp(-MAX_FEEDBACK, MAX_FEEDBACK));
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Snap the last of a fade to a true zero, as the chorus does: a
        // section switched off has to be bit-exact with no flanger at all.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        self.phase += rate / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
        self.write = (self.write + 1) % self.frames;

        // The positive-feedback comb is the dry plus the delayed copy raised
        // by (1 + fb), which keeps the first notch empty however hard it
        // rings. A negative one has no notch to keep: its peaks have moved
        // there. Either comb is levelled to the power the plain one has on
        // noise (worked out from the mean of its squared response), so it
        // rings without getting louder.
        let wet_gain = 1.0 + fb.max(0.0);
        let level = if fb >= 0.0 {
            (1.0 - fb).sqrt()
        } else {
            (2.0 * (1.0 - fb * fb) / (2.0 - fb * fb)).sqrt()
        };
        let floor = (DELAY_FLOOR_MS * 0.001 * self.sample_rate).max(2.0);
        let ceil = DELAY_CEIL_MS * 0.001 * self.sample_rate;
        let centre = manual * 0.001 * self.sample_rate;

        let mut out = [0.0; 2];
        for (ch, out) in out.iter_mut().enumerate() {
            let dry = if ch == 0 { l } else { r };
            let delay = (centre
                * (SWEEP_OCTAVES * depth * lfo(self.phase + 0.25 * ch as f32)).exp2())
            .clamp(floor, ceil);
            let wet = self.tap(ch, delay);
            // What goes round again: the new input plus the limited,
            // flushed echo.
            let echo = soft_limit(fb * wet);
            let fed = dry + echo;
            self.line[self.write * 2 + ch] = if fed.abs() < FLUSH { 0.0 } else { fed };

            *out = dry * (1.0 - mix) + mix * level * 0.5 * (dry + wet_gain * wet);
        }
        if mix == 0.0 {
            return (l, r);
        }
        (out[0], out[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn tone(i: usize) -> f32 {
        (core::f32::consts::TAU * 220.0 * i as f32 / SR).sin() * 0.5
    }

    /// White noise from a fixed seed, so a test hears every frequency.
    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn wet(manual_ms: f32, depth: f32, feedback: f32) -> FlangerParams {
        FlangerParams {
            manual_ms,
            rate_hz: 1.0,
            depth,
            feedback,
            mix: 1.0,
        }
    }

    /// Music-like and busy: a few partials, a slow envelope, a little noise.
    fn music(i: usize, seed: &mut u32) -> f32 {
        let t = i as f32 / SR;
        let env = 0.6 + 0.4 * (core::f32::consts::TAU * 3.0 * t).sin();
        let tones = [(110.0, 0.4), (330.0, 0.25), (1_870.0, 0.15)]
            .iter()
            .map(|(f, a)| a * (core::f32::consts::TAU * f * t).sin())
            .sum::<f32>();
        env * tones + 0.02 * noise(seed)
    }

    /// Average power of `x` at each frequency, Hann-windowed segments.
    fn spectrum(x: &[f32], sr: f32, freqs: &[f32]) -> Vec<f64> {
        const SEG: usize = 4096;
        let window: Vec<f64> = (0..SEG)
            .map(|n| 0.5 - 0.5 * (core::f64::consts::TAU * n as f64 / SEG as f64).cos())
            .collect();
        freqs
            .iter()
            .map(|&f| {
                let w = core::f64::consts::TAU * f as f64 / sr as f64;
                let mut total = 0.0;
                let mut segs = 0.0;
                for seg in x.chunks_exact(SEG) {
                    let (mut re, mut im) = (0.0, 0.0);
                    for (n, (&s, &h)) in seg.iter().zip(&window).enumerate() {
                        re += s as f64 * h * (w * n as f64).cos();
                        im += s as f64 * h * (w * n as f64).sin();
                    }
                    total += re * re + im * im;
                    segs += 1.0;
                }
                total / segs
            })
            .collect()
    }

    /// Noise through a static flanger, the settled part, left channel.
    fn render_noise(sr: f32, p: &FlangerParams, seconds: f32) -> Vec<f32> {
        let mut f = Flanger::new(sr);
        let mut seed = 0x9E37_79B9;
        let warm = (0.5 * sr) as usize;
        let mut out = Vec::new();
        for i in 0..warm + (seconds * sr) as usize {
            let x = noise(&mut seed) * 0.3;
            let (l, _) = f.process(x, x, p);
            if i >= warm {
                out.push(l);
            }
        }
        out
    }

    fn lowest_bin(freqs: &[f32], power: &[f64]) -> f32 {
        let i = (0..power.len())
            .min_by(|&a, &b| power[a].total_cmp(&power[b]))
            .unwrap();
        freqs[i]
    }

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        let mut f = Flanger::new(SR);
        let on = FlangerParams {
            mix: 1.0,
            ..Default::default()
        };
        let off = FlangerParams {
            mix: 0.0,
            ..Default::default()
        };
        for i in 0..4_800 {
            f.process(tone(i), tone(i), &on);
        }
        for i in 0..48_000 {
            f.process(tone(i), tone(i), &off);
        }
        for _ in 0..1_000 {
            assert_eq!(f.process(0.5, -0.25, &off), (0.5, -0.25));
        }
        // A fresh node at zero mix is exact from the first frame.
        let mut f = Flanger::new(SR);
        assert_eq!(f.process(0.3, -0.7, &off), (0.3, -0.7));
    }

    #[test]
    fn the_line_keeps_running_at_zero_mix() {
        // Switching on after a stretch at zero mix must play the audio that
        // went by meanwhile, not silence.
        let p_on = FlangerParams {
            manual_ms: MANUAL_MAX_MS,
            depth: 0.0,
            feedback: 0.0,
            mix: 1.0,
            ..Default::default()
        };
        let p_off = FlangerParams { mix: 0.0, ..p_on };
        let mut kept = Flanger::new(SR);
        let mut fresh = Flanger::new(SR);
        // Let the centre delay settle at 8 ms (384 frames) in both.
        for _ in 0..20_000 {
            kept.process(0.0, 0.0, &p_off);
            fresh.process(0.0, 0.0, &p_off);
        }
        for i in 0..2_000 {
            kept.process(tone(i), tone(i), &p_off);
        }
        // Both switch on and hear the next 100 frames; only the kept one has
        // anything 384 frames back.
        let mut apart = 0.0f32;
        for i in 2_000..2_100 {
            let (a, _) = kept.process(tone(i), tone(i), &p_on);
            let (b, _) = fresh.process(tone(i), tone(i), &p_on);
            apart = apart.max((a - b).abs());
        }
        assert!(apart > 1e-3, "history was not kept: {apart}");
    }

    #[test]
    fn silence_in_silence_out() {
        for fb in [-MAX_FEEDBACK, 0.0, MAX_FEEDBACK] {
            let mut f = Flanger::new(SR);
            let p = wet(1.5, 1.0, fb);
            for _ in 0..10_000 {
                assert_eq!(f.process(0.0, 0.0, &p), (0.0, 0.0));
            }
        }
    }

    #[test]
    fn full_feedback_stays_bounded_and_dies_away() {
        for fb in [MAX_FEEDBACK, -MAX_FEEDBACK] {
            for manual_ms in [MANUAL_MIN_MS, 1.5, MANUAL_MAX_MS] {
                for depth in [0.0, 1.0] {
                    let mut f = Flanger::new(SR);
                    let p = FlangerParams {
                        rate_hz: MAX_RATE_HZ,
                        ..wet(manual_ms, depth, fb)
                    };
                    let mut seed = 0xBEEF_1234;
                    let mut worst = 0.0f32;
                    for _ in 0..48_000 {
                        let x = noise(&mut seed);
                        let (l, r) = f.process(x, -x, &p);
                        assert!(l.is_finite() && r.is_finite());
                        worst = worst.max(l.abs()).max(r.abs());
                    }
                    let mut tail = 0.0f32;
                    for i in 0..(6 * 48_000) {
                        let (l, r) = f.process(0.0, 0.0, &p);
                        assert!(l.is_finite() && r.is_finite());
                        worst = worst.max(l.abs()).max(r.abs());
                        if i >= 5 * 48_000 {
                            tail = tail.max(l.abs()).max(r.abs());
                        }
                    }
                    assert!(
                        worst <= 4.0,
                        "fb {fb} delay {manual_ms} depth {depth}: reached {worst}"
                    );
                    assert!(
                        tail < 1e-3,
                        "fb {fb} delay {manual_ms} depth {depth}: still {tail} after 5 s"
                    );
                }
            }
        }
    }

    #[test]
    fn sweeping_every_control_makes_no_click() {
        // Every control moves smoothly over a second of music, and back. No
        // output step may beat a few times the input's own largest, which
        // also catches a read point that jumps.
        for fb_range in [(0.0, 0.0), (-0.9, 0.9)] {
            let mut f = Flanger::new(SR);
            let mut seed = 7;
            let mut prev_in = 0.0;
            let mut prev_out = 0.0;
            let (mut in_step, mut out_step) = (0.0f32, 0.0f32);
            let n = SR as usize;
            for i in 0..(2 * n) {
                let k = (i % n) as f32 / n as f32;
                let k = if i < n { k } else { 1.0 - k };
                let p = FlangerParams {
                    manual_ms: MANUAL_MIN_MS * (MANUAL_MAX_MS / MANUAL_MIN_MS).powf(k),
                    rate_hz: MIN_RATE_HZ + (MAX_RATE_HZ - MIN_RATE_HZ) * k,
                    depth: k,
                    feedback: fb_range.0 + (fb_range.1 - fb_range.0) * k,
                    mix: k,
                };
                let x = music(i, &mut seed);
                let (y, _) = f.process(x, x, &p);
                if i > 100 {
                    in_step = in_step.max((x - prev_in).abs());
                    out_step = out_step.max((y - prev_out).abs());
                }
                prev_in = x;
                prev_out = y;
            }
            assert!(
                out_step < 4.0 * in_step,
                "fb {fb_range:?}: step {out_step} against input's {in_step}"
            );
        }
    }

    #[test]
    fn every_sample_rate_sizes_its_line() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut f = Flanger::new(sr);
            let mut seed = 99;
            for &(manual, fb) in &[(MANUAL_MIN_MS, 0.9), (MANUAL_MAX_MS, -0.9)] {
                let p = FlangerParams {
                    rate_hz: MAX_RATE_HZ,
                    ..wet(manual, 1.0, fb)
                };
                for _ in 0..(sr as usize) {
                    let x = noise(&mut seed);
                    let (l, r) = f.process(x, x, &p);
                    assert!(l.is_finite() && r.is_finite() && l.abs() < 4.0 && r.abs() < 4.0);
                }
            }
        }
    }

    #[test]
    fn notches_follow_manual() {
        // Static delay, no feedback, dry and wet at equal level: the first
        // notch sits at 1 / (2 x delay).
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for ms in [0.5f32, 1.0, 2.0] {
                let expect = 500.0 / ms;
                let x = render_noise(sr, &wet(ms, 0.0, 0.0), 4.0);
                let freqs: Vec<f32> = (0..=80)
                    .map(|i| expect * (0.8 + 0.4 * i as f32 / 80.0))
                    .collect();
                let found = lowest_bin(&freqs, &spectrum(&x, sr, &freqs));
                assert!(
                    (found / expect - 1.0).abs() < 0.03,
                    "{sr} Hz, {ms} ms: notch at {found}, expected {expect}"
                );
            }
        }
    }

    #[test]
    fn a_positive_feedback_notch_stays_put() {
        let expect = 500.0;
        let x = render_noise(SR, &wet(1.0, 0.0, 0.7), 4.0);
        let freqs: Vec<f32> = (0..=80)
            .map(|i| expect * (0.8 + 0.4 * i as f32 / 80.0))
            .collect();
        let found = lowest_bin(&freqs, &spectrum(&x, SR, &freqs));
        assert!((found / expect - 1.0).abs() < 0.03, "notch at {found}");
    }

    #[test]
    fn feedback_deepens_the_comb() {
        // A comb is as deep as its peaks stand above its floor. Positive
        // feedback sharpens the peaks over the notches, so the spread between
        // the loudest and quietest points of the spectrum widens with it.
        // (Negative feedback fills the notches in, then rings at them: its
        // spread is not monotone, so it is not held to this.)
        let spread = |fb: f32| {
            let x = render_noise(SR, &wet(1.0, 0.0, fb), 4.0);
            let freqs: Vec<f32> = (0..=200).map(|i| 100.0 + 10.0 * i as f32).collect();
            let p = spectrum(&x, SR, &freqs);
            let hi = p.iter().cloned().fold(f64::MIN, f64::max);
            let lo = p.iter().cloned().fold(f64::MAX, f64::min);
            10.0 * (hi / lo).log10()
        };
        let (a, b, c) = (spread(0.2), spread(0.6), spread(0.9));
        assert!(a < b && b < c, "{a:.1} {b:.1} {c:.1} dB");
    }

    #[test]
    fn negative_feedback_moves_the_peaks_to_the_notches() {
        // Positive: the loudest point is at 1 / delay (1 kHz for 1 ms).
        // Negative: it is at 1 / (2 x delay), where the dry cancelled.
        let freqs = [500.0, 1_000.0];
        let pos = spectrum(&render_noise(SR, &wet(1.0, 0.0, 0.8), 4.0), SR, &freqs);
        let neg = spectrum(&render_noise(SR, &wet(1.0, 0.0, -0.8), 4.0), SR, &freqs);
        assert!(pos[1] > 10.0 * pos[0]);
        assert!(neg[0] > 3.0 * neg[1]);
    }

    #[test]
    fn a_swept_comb_stays_level() {
        // Noise through the LFO at full depth: the output should be about as
        // loud as the input (a few dB under at equal-level dry and wet, which
        // do not add in power, and a little more under at heavy feedback,
        // where white noise's top octave is worn down by the interpolator on
        // each trip round), and not run away or collapse with feedback.
        for fb in [-0.9f32, 0.0, 0.9] {
            let mut f = Flanger::new(SR);
            let mut seed = 5;
            let (mut i_p, mut o_p) = (0.0f64, 0.0f64);
            for i in 0..96_000 {
                let x = noise(&mut seed) * 0.3;
                let (l, _) = f.process(x, x, &wet(1.5, 1.0, fb));
                if i > 9_600 {
                    i_p += (x * x) as f64;
                    o_p += (l * l) as f64;
                }
            }
            let db = 10.0 * (o_p / i_p).log10();
            assert!(db.abs() < 6.0, "fb {fb}: {db:.1} dB off the input");
        }
    }

    #[test]
    fn the_sweep_stays_inside_the_line() {
        // Full depth from the shortest and longest centres runs the read
        // point to both clamps without a bad value.
        for manual in [MANUAL_MIN_MS, MANUAL_MAX_MS] {
            let mut f = Flanger::new(SR);
            let p = FlangerParams {
                rate_hz: MAX_RATE_HZ,
                ..wet(manual, 1.0, 0.0)
            };
            for i in 0..48_000 {
                let (l, r) = f.process(tone(i), tone(i), &p);
                assert!(l.is_finite() && r.is_finite() && l.abs() < 2.0 && r.abs() < 2.0);
            }
        }
        let mut peak = 0.0f32;
        for i in 0..1_000 {
            peak = peak.max(lfo(i as f32 / 1_000.0).abs());
        }
        assert!((peak - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_channels_decorrelate() {
        // The right's LFO is a quarter turn on, so a mono source must come
        // out different on each side.
        let mut f = Flanger::new(SR);
        let p = wet(1.5, 1.0, 0.3);
        let mut seed = 3;
        let mut apart = 0.0f32;
        for i in 0..48_000 {
            let x = music(i, &mut seed);
            let (l, r) = f.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite() && l.abs() < 4.0 && r.abs() < 4.0);
            if i > 4_800 {
                apart = apart.max((l - r).abs());
            }
        }
        assert!(apart > 0.05, "left and right stayed together: {apart}");
    }

    #[test]
    fn full_mix_is_dry_and_wet_at_equal_level() {
        // No feedback and no sweep: out = (x + x delayed) / 2, so an impulse
        // comes out as two halves, the delay apart (48 frames for 1 ms).
        let p = wet(1.0, 0.0, 0.0);
        let mut f = Flanger::new(SR);
        // The smoothed manual starts at its default, so settle first.
        for _ in 0..20_000 {
            f.process(0.0, 0.0, &p);
        }
        let mut out = Vec::new();
        for i in 0..2_000 {
            let x = if i == 1_000 { 1.0 } else { 0.0 };
            out.push(f.process(x, x, &p).0);
        }
        assert!((out[1_000] - 0.5).abs() < 1e-4, "dry half {}", out[1_000]);
        assert!((out[1_048] - 0.5).abs() < 0.02, "wet half {}", out[1_048]);
    }
}
