//! Delay.
//!
//! The clean one. A line a channel, up to two seconds long, read back and fed
//! into itself. Every repeat is a faithful copy of the one before, quieter by
//! `feedback`, and the only thing allowed to change them is `tone`, a low-pass
//! in the loop that darkens each pass a little more than the last. There is no
//! wobble and no saturation here: a degraded, wandering voicing is a different
//! node's job, and this one stays out of its way.
//!
//! `time_ms` moves the read point, and the read point glides. A step in time
//! is smoothed over about a tenth of a second, and the read point is held to
//! half a sample a sample at most (an octave down, a fifth up). The line is read
//! between samples with a cubic, so turning Time bends the repeats up or down in
//! pitch the way a tape machine's speed does, instead of clicking. Sweep it
//! slowly for a gentle bend; jump it for a swoop.
//!
//! `cross` is the ping-pong. At zero each side repeats itself. At one the
//! two lines swap places on every trip round the loop: the left input lands
//! in the right line, its repeat comes back into the left, and so on, so the
//! repeats alternate sides. In between, a share `cross` of each side is sent
//! to the other. The two gains always add to one rather than holding equal
//! power. That keeps the loop gain at `feedback` or below for any source, so
//! a mono signal cannot build up however the two controls are set; the cost
//! is that uncorrelated left and right dip about 3 dB at half cross.
//!
//! `tone_hz` turns a one-pole low-pass in the feedback path only. The first
//! repeat is already filtered once, since it passes through the same loop as
//! the rest. The top of the range is a true bypass: the last quarter of the
//! range fades the filter out, so it is transparent at the maximum rather
//! than merely high.
//!
//! Repeats are passed through a soft knee on the way back round. It does
//! nothing at all below full scale; above, it bends towards twice full scale
//! and no further, so loud material at high feedback piles up to a ceiling
//! instead of running away.
//!
//! `mix` is how much of the repeats is added. The dry signal always stays at
//! full level: `out = dry + wet * mix`, unlike the modulation effects'
//! crossfade, because a delay that turned the original down as the repeats
//! came up would sound like it was ducking the source. At zero mix the node
//! is a bit-exact bypass, and the line keeps running, so switching on never
//! plays stale audio.

use crate::smooth::OnePole;

/// The shortest and longest delay, in milliseconds.
pub const DELAY_MIN_MS: f32 = 1.0;
pub const DELAY_MAX_MS: f32 = 2000.0;
/// The top of `delay.feedback`. Below one, so the repeats always die away.
pub const FEEDBACK_MAX: f32 = 0.95;
/// The feedback low-pass's range, in hertz. At the top it is bypassed.
pub const TONE_MIN_HZ: f32 = 500.0;
pub const TONE_MAX_HZ: f32 = 20_000.0;
/// Below this the tone filter is fully engaged; between this and
/// `TONE_MAX_HZ` it fades out.
const TONE_OPEN_FROM_HZ: f32 = 15_000.0;
/// How long a change of time takes to arrive, as a time constant.
const TIME_GLIDE_MS: f32 = 100.0;
/// The fastest the read point may move, in samples a sample. Half means a
/// big change of time bends the repeats at most an octave down or half an
/// octave up, however far it goes, and never stalls or runs backwards.
const MAX_SLEW: f32 = 0.5;
/// Spare samples past the longest delay, for the interpolator's four points.
const MARGIN: usize = 8;
/// Below this a recirculating value is flushed to zero, so a dying tail never
/// reaches the denormals.
const FLUSH: f32 = 1e-15;

#[derive(Debug, Clone, Copy)]
pub struct DelayParams {
    /// Delay time in milliseconds, `DELAY_MIN_MS` to `DELAY_MAX_MS`.
    pub time_ms: f32,
    /// How much of each repeat is fed back, 0 to `FEEDBACK_MAX`.
    pub feedback: f32,
    /// Where the feedback low-pass turns, `TONE_MIN_HZ` to `TONE_MAX_HZ`.
    /// At the top it is out of the circuit.
    pub tone_hz: f32,
    /// Ping-pong, 0 to 1. Zero keeps each side to itself; one swaps the sides
    /// on every repeat.
    pub cross: f32,
    /// How much of the repeats is added to the dry, 0 to 1.
    pub mix: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self {
            time_ms: 350.0,
            feedback: 0.4,
            tone_hz: 8_000.0,
            cross: 0.0,
            mix: 0.3,
        }
    }
}

pub struct Delay {
    /// One line a side, sized once. `write` is the newest sample.
    lines: [Vec<f32>; 2],
    write: usize,
    frames: usize,
    sample_rate: f32,
    /// The feedback low-pass's state, a side each.
    lows: [f32; 2],
    /// Delay time in milliseconds, glided.
    time: OnePole,
    feedback: OnePole,
    tone: OnePole,
    cross: OnePole,
    mix: OnePole,
    /// Whether the smoothers have been set to the first parameters seen, so
    /// the first block does not glide in from the defaults.
    primed: bool,
    /// The delay actually being read, in samples: `time` chased at a limited
    /// speed.
    delay_now: f32,
}

/// Catmull-Rom between `p1` and `p2`, `f` of the way. Exact at `f == 0`.
#[inline]
fn cubic(p0: f32, p1: f32, p2: f32, p3: f32, f: f32) -> f32 {
    p1 + 0.5
        * f
        * (p2 - p0 + f * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + f * (3.0 * (p1 - p2) + p3 - p0)))
}

/// The loop's safety. Identity up to full scale, then a smooth bend towards
/// 2 that never arrives (`2 - 1/a`, slope one where it starts). Programme
/// inside the range, which is all that a repeat normally is, passes bit for
/// bit; it only acts when loud material and high feedback pile up on each
/// other, and it keeps that from running away.
#[inline]
fn knee(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.0 {
        x
    } else {
        (2.0 - 1.0 / a).copysign(x)
    }
}

impl Delay {
    pub fn new(sample_rate: f32) -> Self {
        let frames = (DELAY_MAX_MS * 0.001 * sample_rate).ceil() as usize + MARGIN;
        let d = DelayParams::default();
        let smoother = |ms: f32, v: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        Self {
            lines: [vec![0.0; frames], vec![0.0; frames]],
            write: 0,
            frames,
            sample_rate,
            lows: [0.0; 2],
            time: smoother(TIME_GLIDE_MS, d.time_ms),
            feedback: smoother(20.0, d.feedback),
            tone: smoother(20.0, d.tone_hz),
            cross: smoother(20.0, d.cross),
            mix: smoother(20.0, 0.0),
            primed: false,
            delay_now: 2.0,
        }
    }

    fn max_delay(&self) -> f32 {
        (self.frames - MARGIN) as f32
    }

    /// Both lines, read `age` samples back from the newest, `age >= 1`, with
    /// a fractional part. Age zero is the sample written last frame, which is
    /// a delay of one.
    #[inline]
    fn read(&self, ch: usize, age: f32) -> f32 {
        let a = age as usize;
        let f = age - a as f32;
        let line = &self.lines[ch];
        let at = |k: usize| line[(self.write + self.frames - k) % self.frames];
        // `a - 1` is the newer neighbour, `a + 2` the oldest of the four.
        cubic(at(a - 1), at(a), at(a + 1), at(a + 2), f)
    }

    pub fn process(&mut self, l: f32, r: f32, p: &DelayParams) -> (f32, f32) {
        let time_target = p.time_ms.clamp(DELAY_MIN_MS, DELAY_MAX_MS);
        let feedback_target = p.feedback.clamp(0.0, FEEDBACK_MAX);
        let tone_target = p.tone_hz.clamp(TONE_MIN_HZ, TONE_MAX_HZ);
        let cross_target = p.cross.clamp(0.0, 1.0);
        if !self.primed {
            self.time.reset(time_target);
            self.feedback.reset(feedback_target);
            self.tone.reset(tone_target);
            self.cross.reset(cross_target);
            self.mix.reset(p.mix.clamp(0.0, 1.0));
            self.delay_now = (time_target * 0.001 * self.sample_rate).clamp(2.0, self.max_delay());
            self.primed = true;
        }
        let time = self.time.process(time_target);
        let feedback = self.feedback.process(feedback_target);
        let tone = self.tone.process(tone_target);
        let cross = self.cross.process(cross_target);
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Settle an exact bypass once the mix has faded out, since a one-pole
        // never quite arrives.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        // A delay of `d` samples is an age of `d - 1`, since age zero is last
        // frame's write. Two samples is the shortest the cubic can reach.
        let goal = (time * 0.001 * self.sample_rate).clamp(2.0, self.max_delay());
        self.delay_now += (goal - self.delay_now).clamp(-MAX_SLEW, MAX_SLEW);
        let d = self.delay_now;
        let age = d - 1.0;
        let wet = [self.read(0, age), self.read(1, age)];

        // The loop's low-pass, faded out over the top of its range.
        let fc = tone.min(self.sample_rate * 0.45);
        let g = 1.0 - (-core::f32::consts::TAU * fc / self.sample_rate).exp();
        let open = ((tone - TONE_OPEN_FROM_HZ) / (TONE_MAX_HZ - TONE_OPEN_FROM_HZ)).clamp(0.0, 1.0);
        let mut back = [0.0f32; 2];
        for ch in 0..2 {
            let low = &mut self.lows[ch];
            *low += g * (wet[ch] - *low);
            if low.abs() < FLUSH {
                *low = 0.0;
            }
            back[ch] = knee(*low + open * (wet[ch] - *low));
        }

        let into = [l + feedback * back[0], r + feedback * back[1]];
        let sent = [
            (1.0 - cross) * into[0] + cross * into[1],
            cross * into[0] + (1.0 - cross) * into[1],
        ];
        self.write = (self.write + 1) % self.frames;
        for (line, v) in self.lines.iter_mut().zip(sent) {
            line[self.write] = if v.abs() < FLUSH { 0.0 } else { v };
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
        *state as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    /// Three partials, a steady tone with some harmonic content.
    fn music(i: usize, sr: f32) -> f32 {
        let t = i as f32 / sr;
        let tau = core::f32::consts::TAU;
        0.3 * (tau * 220.0 * t).sin()
            + 0.2 * (tau * 660.0 * t).sin()
            + 0.1 * (tau * 1_320.0 * t).sin()
    }

    fn wet(time_ms: f32, feedback: f32) -> DelayParams {
        DelayParams {
            time_ms,
            feedback,
            tone_hz: TONE_MAX_HZ,
            cross: 0.0,
            mix: 1.0,
        }
    }

    /// Impulse in the left at frame zero; the output frames for `n` frames.
    fn impulse_response(p: &DelayParams, n: usize, sr: f32) -> Vec<(f32, f32)> {
        let mut d = Delay::new(sr);
        (0..n)
            .map(|i| d.process(if i == 0 { 1.0 } else { 0.0 }, 0.0, p))
            .collect()
    }

    #[test]
    fn zero_mix_is_a_bit_exact_bypass() {
        let mut d = Delay::new(SR);
        let p = DelayParams {
            mix: 0.0,
            feedback: FEEDBACK_MAX,
            ..DelayParams::default()
        };
        let mut s = 7u32;
        for _ in 0..20_000 {
            let (l, r) = (noise(&mut s), noise(&mut s));
            assert_eq!(d.process(l, r, &p), (l, r));
        }
    }

    #[test]
    fn mix_faded_to_zero_ends_exactly_transparent_and_keeps_running() {
        let mut d = Delay::new(SR);
        let on = DelayParams {
            mix: 1.0,
            ..DelayParams::default()
        };
        let off = DelayParams { mix: 0.0, ..on };
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
        // The line kept filling while off: switching on plays a repeat at once.
        let heard = (0..48_000)
            .map(|_| {
                let (a, b) = d.process(0.0, 0.0, &on);
                a.abs().max(b.abs())
            })
            .fold(0.0, f32::max);
        assert!(heard > 0.1, "line should have kept recording, peak {heard}");
    }

    #[test]
    fn silence_in_silence_out() {
        let mut d = Delay::new(SR);
        let p = DelayParams {
            mix: 1.0,
            feedback: FEEDBACK_MAX,
            ..DelayParams::default()
        };
        for _ in 0..50_000 {
            assert_eq!(d.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn the_echo_arrives_at_the_stated_time() {
        for &(sr, ms) in &[
            (44_100.0, 250.0),
            (48_000.0, 350.0),
            (96_000.0, 123.0),
            (48_000.0, 2_000.0),
        ] {
            let (sr, ms): (f32, f32) = (sr, ms);
            let want = (ms * 0.001 * sr).round() as usize;
            let out = impulse_response(&wet(ms, 0.0), want + 20, sr);
            let peak = out
                .iter()
                .enumerate()
                .skip(1)
                .max_by(|a, b| a.1 .0.abs().total_cmp(&b.1 .0.abs()))
                .unwrap()
                .0;
            assert!(
                (peak as i64 - want as i64).abs() <= 1,
                "{ms} ms at {sr}: wanted frame {want}, heard {peak}"
            );
            assert!(
                out[peak].0 > 0.99,
                "the copy is faithful, got {}",
                out[peak].0
            );
            assert_eq!(out[peak].1, 0.0, "no cross, so nothing on the right");
        }
    }

    #[test]
    fn a_fractional_time_lands_between_samples() {
        // 100.5 samples of delay: the energy splits across two neighbours.
        let ms = 100.5 / SR * 1000.0;
        let out = impulse_response(&wet(ms, 0.0), 200, SR);
        let sum: f32 = out.iter().skip(1).map(|f| f.0).sum();
        assert!(
            (sum - 1.0).abs() < 0.05,
            "gain of an interpolated copy, {sum}"
        );
        assert!(out[100].0 > 0.4 && out[101].0 > 0.4, "{:?}", &out[98..104]);
    }

    #[test]
    fn each_repeat_is_feedback_times_the_last() {
        let ms = 100.0;
        let n = (ms * 0.001 * SR).round() as usize;
        let out = impulse_response(&wet(ms, 0.5), 4 * n, SR);
        let first = out[n].0;
        let second = out[2 * n].0;
        let third = out[3 * n].0;
        assert!((first - 1.0).abs() < 1e-3);
        assert!((second - 0.5).abs() < 1e-3, "second {second}");
        assert!((third - 0.25).abs() < 1e-3, "third {third}");
    }

    #[test]
    fn tone_darkens_repeats_and_the_top_is_transparent() {
        let ms = 50.0;
        let n = (ms * 0.001 * SR).round() as usize;
        let rough = |tone: f32| {
            let mut d = Delay::new(SR);
            let p = DelayParams {
                tone_hz: tone,
                ..wet(ms, 0.8)
            };
            let mut s = 11u32;
            let mut prev = 0.0;
            let mut energy = 0.0;
            let mut level = 0.0;
            for i in 0..SR as usize {
                let x = if i < n { noise(&mut s) } else { 0.0 };
                let (o, _) = d.process(x, 0.0, &p);
                // Second repeat onward only.
                if i > 2 * n {
                    energy += (o - prev) * (o - prev);
                    level += o * o;
                }
                prev = o;
            }
            energy / level
        };
        let open = rough(TONE_MAX_HZ);
        let dark = rough(800.0);
        assert!(dark < 0.3 * open, "dark {dark} against open {open}");
        // Transparent at the top: a second repeat is the impulse times fb^2.
        let out = impulse_response(&wet(ms, 0.5), 3 * n + 4, SR);
        for (i, f) in out.iter().enumerate().skip(1) {
            let want = if i == n {
                1.0
            } else if i == 2 * n {
                0.5
            } else if i == 3 * n {
                0.25
            } else {
                0.0
            };
            assert!(
                (f.0 - want).abs() < 1e-5,
                "frame {i}: {} against {want}",
                f.0
            );
        }
    }

    #[test]
    fn full_cross_alternates_the_sides() {
        let ms = 100.0;
        let n = (ms * 0.001 * SR).round() as usize;
        let p = DelayParams {
            cross: 1.0,
            ..wet(ms, 0.5)
        };
        let out = impulse_response(&p, 4 * n, SR);
        // Left in; first repeat right, second left, third right.
        assert!((out[n].1 - 1.0).abs() < 1e-3 && out[n].0.abs() < 1e-6);
        assert!((out[2 * n].0 - 0.5).abs() < 1e-3 && out[2 * n].1.abs() < 1e-6);
        assert!((out[3 * n].1 - 0.25).abs() < 1e-3 && out[3 * n].0.abs() < 1e-6);
    }

    #[test]
    fn half_cross_splits_a_repeat_across_both_sides() {
        let ms = 100.0;
        let n = (ms * 0.001 * SR).round() as usize;
        let p = DelayParams {
            cross: 0.5,
            ..wet(ms, 0.0)
        };
        let out = impulse_response(&p, n + 4, SR);
        assert!((out[n].0 - 0.5).abs() < 1e-3 && (out[n].1 - 0.5).abs() < 1e-3);
    }

    #[test]
    fn no_runaway_at_maximum_feedback() {
        for &cross in &[0.0, 0.5, 1.0] {
            for &mono in &[false, true] {
                let mut d = Delay::new(SR);
                let p = DelayParams {
                    time_ms: 120.0,
                    feedback: FEEDBACK_MAX,
                    tone_hz: TONE_MAX_HZ,
                    cross,
                    mix: 1.0,
                };
                let mut s = 5u32;
                let mut peak = 0.0f32;
                for i in 0..(SR as usize * 6) {
                    let (x, y) = if i < SR as usize {
                        let a = noise(&mut s);
                        (a, if mono { a } else { noise(&mut s) })
                    } else {
                        (0.0, 0.0)
                    };
                    let (a, b) = d.process(x, y, &p);
                    assert!(a.is_finite() && b.is_finite());
                    if i >= SR as usize {
                        peak = peak.max(a.abs()).max(b.abs());
                        assert!(
                            a.abs() <= 4.0 && b.abs() <= 4.0,
                            "cross {cross}: {a} {b} at {i}"
                        );
                    }
                }
                let _ = peak;
            }
        }
    }

    #[test]
    fn the_tail_decays() {
        let mut d = Delay::new(SR);
        let p = wet(120.0, FEEDBACK_MAX);
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
        // 0.95 per 120 ms over ten seconds is far down.
        assert!(late < 0.05, "tail still at {late}");
    }

    #[test]
    fn sweeping_every_parameter_does_not_click() {
        let n = SR as usize;
        let mut d = Delay::new(SR);
        let mut max_in = 0.0f32;
        let mut max_out = 0.0f32;
        let (mut last_in, mut last_out) = (0.0, 0.0);
        // Let the line fill with the music first.
        let settle = DelayParams {
            time_ms: 100.0,
            feedback: 0.2,
            tone_hz: 2_000.0,
            cross: 0.0,
            mix: 0.2,
        };
        for i in 0..n {
            d.process(music(i, SR), music(i, SR), &settle);
        }
        for i in 0..n {
            let t = i as f32 / n as f32;
            let p = DelayParams {
                time_ms: 100.0 + 500.0 * t,
                feedback: 0.2 + 0.7 * t,
                tone_hz: 2_000.0 + 18_000.0 * t,
                cross: t,
                mix: 0.2 + 0.7 * t,
            };
            let x = music(n + i, SR);
            let (o, _) = d.process(x, x, &p);
            if i > 0 {
                max_in = max_in.max((x - last_in).abs());
                max_out = max_out.max((o - last_out).abs());
            }
            last_in = x;
            last_out = o;
        }
        assert!(
            max_out < 4.0 * max_in,
            "out step {max_out} against in {max_in}"
        );
    }

    #[test]
    fn a_jump_in_time_glides_rather_than_clicks() {
        let mut d = Delay::new(SR);
        let a = wet(300.0, 0.0);
        let b = wet(900.0, 0.0);
        let mut max_in = 0.0f32;
        let mut max_out = 0.0f32;
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
        // The read point is swept, never jumped. The bend gives a few times
        // the input's slope at most.
        assert!(
            max_out < 4.0 * max_in,
            "out step {max_out} against in {max_in}"
        );
    }

    #[test]
    fn works_and_sizes_itself_at_each_rate() {
        for &sr in &[44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            let d = Delay::new(sr);
            assert!(d.frames as f32 >= 2.0 * sr, "{sr}: {} frames", d.frames);
            assert!(d.frames as f32 <= 2.0 * sr + 16.0);
            let mut d = d;
            let p = wet(DELAY_MAX_MS, FEEDBACK_MAX);
            let mut s = 1u32;
            for _ in 0..(sr as usize / 2) {
                let (a, b) = d.process(noise(&mut s), noise(&mut s), &p);
                assert!(a.is_finite() && b.is_finite());
            }
        }
    }

    #[test]
    fn extremes_of_every_range_are_safe() {
        let mut d = Delay::new(SR);
        let mut s = 9u32;
        for &(t, tone) in &[
            (0.0, 0.0),
            (DELAY_MIN_MS, TONE_MIN_HZ),
            (DELAY_MAX_MS, TONE_MAX_HZ),
            (1.0e9, 1.0e9),
            (-5.0, -5.0),
        ] {
            let p = DelayParams {
                time_ms: t,
                feedback: 2.0,
                tone_hz: tone,
                cross: 3.0,
                mix: 2.0,
            };
            for _ in 0..20_000 {
                let (a, b) = d.process(noise(&mut s), noise(&mut s), &p);
                assert!(a.is_finite() && b.is_finite() && a.abs() < 40.0 && b.abs() < 40.0);
            }
        }
    }

    #[test]
    fn a_mono_source_stays_bounded_and_finite() {
        let mut d = Delay::new(SR);
        let p = DelayParams {
            cross: 0.5,
            feedback: FEEDBACK_MAX,
            mix: 1.0,
            ..DelayParams::default()
        };
        for i in 0..SR as usize * 3 {
            let x = music(i, SR);
            let (a, b) = d.process(x, x, &p);
            assert!(a.is_finite() && b.is_finite());
            assert_eq!(a, b, "a mono source stays mono at half cross");
            assert!(a.abs() < 8.0);
        }
    }

    #[test]
    fn the_default_is_inside_every_range() {
        let p = DelayParams::default();
        assert!((DELAY_MIN_MS..=DELAY_MAX_MS).contains(&p.time_ms));
        assert!((0.0..=FEEDBACK_MAX).contains(&p.feedback));
        assert!((TONE_MIN_HZ..=TONE_MAX_HZ).contains(&p.tone_hz));
        assert!((0.0..=1.0).contains(&p.cross));
        assert!((0.0..=1.0).contains(&p.mix));
    }
}
