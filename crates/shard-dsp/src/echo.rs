//! Echo.
//!
//! A tape-voiced echo, where every repeat comes back a little worse than the
//! one before. The first is a close copy; by the fourth the top has gone, the
//! edges are soft, and the pitch is leaning and drifting as if the tape were
//! old. That decay is the point: a clean delay repeats a sound, and this one
//! wears it out.
//!
//! The mechanism is one delay line a channel with its feedback path running
//! through a small chain, so everything that happens to a repeat happens once
//! more on every trip round:
//!
//! - A gentle high-pass, about 80 Hz, so bass does not pile up under the
//!   saturation. Without it a low note feeds itself and swells.
//! - A soft saturator, `tanh(g x) / g`. Small signals pass at unity whatever
//!   `grit` is, so quiet tails stay clean; loud ones are rounded off, harder
//!   the higher `grit`. The division is the makeup: the stage never has more
//!   than unity gain, so with the filters and a feedback under one the loop
//!   is always losing energy and cannot run away. Even at no grit there is a
//!   gentle ceiling, so a hot input at full feedback stays within a few
//!   times full scale.
//! - A low-pass, two poles, at `tone`. Each trip loses more top, because
//!   each trip filters what the last one left.
//!
//! `wobble` moves the read point, and a moving read point bends pitch. Two
//! motions are summed: a slow wow near half a hertz, whose rate wanders at
//! random so it never settles into a cycle, and a smaller, faster flutter near
//! seven hertz. At full wobble the pitch strays by a little over a third of a
//! percent either way, which is a worn machine rather than a seasick one.
//! Because the wobble is inside the loop the later repeats carry the sum of
//! every trip's wander, and they smear.
//!
//! The two channels are two lines with their wobbles a different way round the
//! turn, so a mono source comes out wide. There is no ping-pong: left stays
//! left.
//!
//! Changing `time_ms` glides the read point, which bends the pitch while it
//! moves, the way turning the delay knob on a tape machine does.
//!
//! The output crossfades, `dry * (1 - mix) + wet * mix`, so at full mix only
//! the repeats are heard. At zero mix the node is a bit-exact bypass. The line keeps filling, so
//! switching on never plays stale audio.

use crate::rng::Rng;
use crate::smooth::OnePole;

/// The shortest and longest delay, in milliseconds.
pub const ECHO_MIN_MS: f32 = 20.0;
pub const ECHO_MAX_MS: f32 = 1_500.0;
/// The most feedback. Under one, so the repeats always die away.
pub const MAX_FEEDBACK: f32 = 0.95;
/// The tone control's range: the loop low-pass's corner, in hertz.
pub const TONE_MIN_HZ: f32 = 400.0;
pub const TONE_MAX_HZ: f32 = 12_000.0;
/// Defaults, for `params.rs` to borrow later.
pub const DEFAULT_TIME_MS: f32 = 300.0;
pub const DEFAULT_FEEDBACK: f32 = 0.45;
pub const DEFAULT_TONE_HZ: f32 = 3_500.0;
pub const DEFAULT_MIX: f32 = 0.35;

/// The loop's high-pass corner, in hertz.
const LOOP_HIGH_PASS_HZ: f32 = 80.0;
/// Wow: the slow drift. Its rate in hertz, how far the rate wanders either
/// way as a fraction, and its pitch deviation at full wobble.
pub const WOW_HZ: f32 = 0.5;
const WOW_WANDER: f32 = 0.25;
const WOW_DEVIATION: f32 = 0.0025;
/// Flutter: the fast shiver, smaller.
pub const FLUTTER_HZ: f32 = 7.0;
const FLUTTER_WANDER: f32 = 0.15;
const FLUTTER_DEVIATION: f32 = 0.001;
/// The widest the pitch strays at full wobble, as a fraction, with both
/// motions at their fastest and in step. A test holds the line to it.
pub const MAX_DEVIATION: f32 =
    WOW_DEVIATION * (1.0 + WOW_WANDER) + FLUTTER_DEVIATION * (1.0 + FLUTTER_WANDER);
/// How far each motion moves the read point at full wobble, in milliseconds.
/// A sine of amplitude `a` at `f` hertz bends pitch by `2 pi f a`.
const WOW_MS: f32 = 1_000.0 * WOW_DEVIATION / (core::f32::consts::TAU * WOW_HZ);
const FLUTTER_MS: f32 = 1_000.0 * FLUTTER_DEVIATION / (core::f32::consts::TAU * FLUTTER_HZ);
/// How often the wow's rate picks a new place to wander to, in seconds.
const WANDER_EVERY_S: f32 = 0.8;
/// Saturation drive at `grit` 0 and 1. Even the bottom is a soft ceiling at
/// `1 / SAT_MIN` times full scale; small signals are untouched at any drive.
const SAT_MIN: f32 = 0.5;
const SAT_MAX: f32 = 4.0;
/// The delay time's glide, as a time constant in milliseconds.
const TIME_GLIDE_MS: f32 = 200.0;
/// Worst-case read point reach beyond the longest time, in milliseconds: the
/// wobble, a margin for the glide's overshoot and the interpolator's points.
const HEADROOM_MS: f32 = 6.0;

#[derive(Debug, Clone, Copy)]
pub struct EchoParams {
    /// Delay, `ECHO_MIN_MS` to `ECHO_MAX_MS`.
    pub time_ms: f32,
    /// How much of each repeat is fed back, 0 to `MAX_FEEDBACK`.
    pub feedback: f32,
    /// The loop low-pass's corner in hertz, `TONE_MIN_HZ` to `TONE_MAX_HZ`.
    pub tone_hz: f32,
    /// Wow and flutter on the read point, 0 to 1.
    pub wobble: f32,
    /// Soft saturation in the loop, 0 to 1.
    pub grit: f32,
    /// Crossfade from dry to the repeats, 0 to 1: at 1 only the repeats are heard.
    pub mix: f32,
}

impl Default for EchoParams {
    fn default() -> Self {
        Self {
            time_ms: DEFAULT_TIME_MS,
            feedback: DEFAULT_FEEDBACK,
            tone_hz: DEFAULT_TONE_HZ,
            wobble: 0.3,
            grit: 0.3,
            mix: DEFAULT_MIX,
        }
    }
}

/// One channel's wow and flutter.
#[derive(Debug, Clone, Copy)]
struct Wobble {
    wow: f32,
    flutter: f32,
    /// Where the rate is wandering to, and the smoothed path towards it.
    target: f32,
    walk: OnePole,
    /// Samples until the next target.
    countdown: u32,
    rng: Rng,
}

impl Wobble {
    fn new(seed: u32, wow: f32, flutter: f32, sample_rate: f32) -> Self {
        let mut walk = OnePole::new();
        walk.set_time(400.0, sample_rate);
        Self {
            wow,
            flutter,
            target: 0.0,
            walk,
            countdown: 0,
            rng: Rng::new(seed),
        }
    }

    /// Advance one sample. Returns the read point's offset in milliseconds at
    /// full wobble.
    #[inline]
    fn step(&mut self, sample_rate: f32) -> f32 {
        if self.countdown == 0 {
            self.target = self.rng.next_bipolar();
            self.countdown = (WANDER_EVERY_S * sample_rate) as u32;
        }
        self.countdown -= 1;
        let walk = self.walk.process(self.target);
        self.wow = (self.wow + WOW_HZ * (1.0 + WOW_WANDER * walk) / sample_rate).fract();
        self.flutter =
            (self.flutter + FLUTTER_HZ * (1.0 + FLUTTER_WANDER * walk) / sample_rate).fract();
        WOW_MS * (core::f32::consts::TAU * self.wow).sin()
            + FLUTTER_MS * (core::f32::consts::TAU * self.flutter).sin()
    }
}

/// Keep a recirculating state out of the denormal range.
#[inline]
fn flush(v: f32) -> f32 {
    if v.abs() < 1e-20 {
        0.0
    } else {
        v
    }
}

pub struct Echo {
    /// Interleaved left, right frames, sized once. `write` is the newest.
    line: Vec<f32>,
    write: usize,
    frames: usize,
    sample_rate: f32,
    wobbles: [Wobble; 2],
    /// The loop's filters, a channel each: the high-pass's low-pass, and the
    /// two poles of the tone low-pass.
    hp: [f32; 2],
    lp1: [f32; 2],
    lp2: [f32; 2],
    hp_coef: f32,
    /// The delay in milliseconds, smoothed slowly so a change glides. Double
    /// precision, because a slow single-precision one-pole stalls short of
    /// its target by `ulp / (1 - a)`, which at a second's delay is a quarter
    /// of a millisecond: a dozen samples off the stated time.
    time: f64,
    time_coef: f64,
    feedback: OnePole,
    tone: OnePole,
    wobble: OnePole,
    grit: OnePole,
    mix: OnePole,
}

impl Echo {
    /// Forgets the repeats it holds, without allocating. The wobble and the smoothers keep their place: they follow controls, not signal.
    pub fn reset(&mut self) {
        self.line.fill(0.0);
        self.hp = [0.0; 2];
        self.lp1 = [0.0; 2];
        self.lp2 = [0.0; 2];
    }

    pub fn new(sample_rate: f32) -> Self {
        let frames = ((ECHO_MAX_MS + HEADROOM_MS) * 0.001 * sample_rate).ceil() as usize + 4;
        let d = EchoParams::default();
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
            sample_rate,
            // The sides start at different places in both motions.
            wobbles: [
                Wobble::new(0x0EC0_0001, 0.0, 0.0, sample_rate),
                Wobble::new(0x0EC0_0002, 0.31, 0.57, sample_rate),
            ],
            hp: [0.0; 2],
            lp1: [0.0; 2],
            lp2: [0.0; 2],
            hp_coef: 1.0 - (-core::f32::consts::TAU * LOOP_HIGH_PASS_HZ / sample_rate).exp(),
            // A step in time slides the read point, and the slide is the
            // pitch bend. Slow, to keep that bend to a few semitones.
            time: d.time_ms as f64,
            time_coef: (-1.0 / (TIME_GLIDE_MS as f64 * 0.001 * sample_rate as f64)).exp(),
            feedback: smoother(20.0, d.feedback),
            tone: smoother(20.0, d.tone_hz),
            wobble: smoother(50.0, d.wobble),
            grit: smoother(20.0, d.grit),
            mix: smoother(20.0, 0.0),
        }
    }

    /// One channel's read at `delay` frames behind the newest, through a
    /// four-point Hermite curve, for the same reason as the chorus's: a read
    /// point that moves passes between samples constantly, and linear
    /// interpolation would dull the top by a varying amount as it went.
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
    pub fn process(&mut self, l: f32, r: f32, p: &EchoParams) -> (f32, f32) {
        let target = p.time_ms.clamp(ECHO_MIN_MS, ECHO_MAX_MS) as f64;
        self.time = target + self.time_coef * (self.time - target);
        let time_ms = self.time as f32;
        let feedback = self.feedback.process(p.feedback.clamp(0.0, MAX_FEEDBACK));
        let tone = self.tone.process(p.tone_hz.clamp(TONE_MIN_HZ, TONE_MAX_HZ));
        let wobble = self.wobble.process(p.wobble.clamp(0.0, 1.0));
        let grit = self.grit.process(p.grit.clamp(0.0, 1.0));
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Snap the last of a fade to a true zero: a node switched off has to
        // be bit-exact with no echo at all.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        let ms = 0.001 * self.sample_rate;
        let lp_coef = 1.0 - (-core::f32::consts::TAU * tone / self.sample_rate).exp();
        let drive = SAT_MIN + (SAT_MAX - SAT_MIN) * grit;
        let ins = [l, r];

        // Read both sides before writing: the newest frame is the previous
        // one, so a delay of `d` is read `d - 1` back.
        let mut wet = [0.0f32; 2];
        for (ch, w) in wet.iter_mut().enumerate() {
            let wob = self.wobbles[ch].step(self.sample_rate) * wobble;
            *w = self.tap(ch, (time_ms + wob) * ms - 1.0);
        }

        self.write = (self.write + 1) % self.frames;
        let mut out = [0.0f32; 2];
        for ch in 0..2 {
            let mut x = ins[ch] + feedback * wet[ch];
            // High-pass: the signal less its own low-pass.
            self.hp[ch] = flush(self.hp[ch] + self.hp_coef * (x - self.hp[ch]));
            x -= self.hp[ch];
            // Soft saturation, unity for small signals.
            x = (drive * x).tanh() / drive;
            // Tone: two one-pole low-passes.
            self.lp1[ch] = flush(self.lp1[ch] + lp_coef * (x - self.lp1[ch]));
            self.lp2[ch] = flush(self.lp2[ch] + lp_coef * (self.lp1[ch] - self.lp2[ch]));
            self.line[self.write * 2 + ch] = flush(self.lp2[ch]);
            out[ch] = ins[ch] * (1.0 - mix) + wet[ch] * mix;
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

    /// A plain wet setting with the character off, for measuring the loop.
    fn clean(time_ms: f32, feedback: f32) -> EchoParams {
        EchoParams {
            time_ms,
            feedback,
            tone_hz: TONE_MAX_HZ,
            wobble: 0.0,
            grit: 0.0,
            mix: 1.0,
        }
    }

    fn maxed() -> EchoParams {
        EchoParams {
            time_ms: 300.0,
            feedback: MAX_FEEDBACK,
            tone_hz: TONE_MAX_HZ,
            wobble: 1.0,
            grit: 1.0,
            mix: 1.0,
        }
    }

    /// Run a mono signal through, returning the left output. Three seconds of
    /// silence go first, so the smoothers have arrived at the settings.
    fn run(sr: f32, p: &EchoParams, input: &[f32]) -> Vec<f32> {
        let mut e = Echo::new(sr);
        for _ in 0..(3.0 * sr) as usize {
            e.process(0.0, 0.0, p);
        }
        input.iter().map(|&x| e.process(x, x, p).0).collect()
    }

    #[test]
    fn a_mix_at_zero_is_transparent() {
        let mut e = Echo::new(SR);
        let on = maxed();
        let off = EchoParams { mix: 0.0, ..on };
        // Fresh node, mix already zero.
        for _ in 0..100 {
            assert_eq!(e.process(0.5, -0.25, &off), (0.5, -0.25));
        }
        // After running wet, and a fade to zero.
        for i in 0..4_800 {
            e.process(tone(i), tone(i), &on);
        }
        for i in 0..48_000 {
            e.process(tone(i), tone(i), &off);
        }
        for _ in 0..1_000 {
            assert_eq!(e.process(0.5, -0.25, &off), (0.5, -0.25));
        }
    }

    #[test]
    fn the_line_keeps_running_while_mix_is_zero() {
        let mut e = Echo::new(SR);
        let off = EchoParams {
            mix: 0.0,
            ..clean(100.0, 0.5)
        };
        let on = EchoParams { mix: 1.0, ..off };
        for i in 0..9_600 {
            e.process(tone(i), tone(i), &off);
        }
        // Switch on after the input stops: the echo of what went by is there.
        let mut energy = 0.0;
        for _ in 0..4_800 {
            let (l, _) = e.process(0.0, 0.0, &on);
            energy += l * l;
        }
        assert!(energy > 1.0, "nothing was waiting in the line: {energy}");
    }

    #[test]
    fn silence_in_silence_out() {
        for p in [EchoParams::default(), maxed()] {
            let mut e = Echo::new(SR);
            for _ in 0..20_000 {
                assert_eq!(e.process(0.0, 0.0, &p), (0.0, 0.0));
            }
        }
    }

    #[test]
    fn full_feedback_on_a_loud_input_does_not_run_away() {
        for grit in [0.0, 1.0] {
            for time_ms in [ECHO_MIN_MS, 300.0, ECHO_MAX_MS] {
                let p = EchoParams {
                    time_ms,
                    grit,
                    ..maxed()
                };
                let mut e = Echo::new(SR);
                let mut seed = 1u32;
                let mut peak = 0.0f32;
                for _ in 0..(5 * 48_000) {
                    let x = noise(&mut seed);
                    let (l, r) = e.process(x, -x, &p);
                    assert!(l.is_finite() && r.is_finite());
                    peak = peak.max(l.abs()).max(r.abs());
                }
                let mut first = 0.0f64;
                let mut last = 0.0f64;
                for i in 0..(10 * 48_000) {
                    let (l, r) = e.process(0.0, 0.0, &p);
                    assert!(l.is_finite() && r.is_finite());
                    peak = peak.max(l.abs()).max(r.abs());
                    let power = (l * l + r * r) as f64;
                    if i < 48_000 {
                        first += power;
                    } else if i >= 9 * 48_000 {
                        last += power;
                    }
                }
                assert!(peak <= 4.0, "grit {grit} time {time_ms}: peak {peak}");
                assert!(
                    last < first,
                    "grit {grit} time {time_ms}: tail did not decay ({first} then {last})"
                );
            }
        }
    }

    #[test]
    fn the_first_repeat_arrives_at_the_stated_time() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for time_ms in [ECHO_MIN_MS, 123.0, 300.0, 1_000.0, ECHO_MAX_MS] {
                let p = clean(time_ms, 0.5);
                let mut input = vec![0.0; (sr * (time_ms * 0.001 + 0.1)) as usize];
                input[100] = 1.0;
                let out = run(sr, &p, &input);
                // Skip the dry impulse itself.
                let (at, _) = out
                    .iter()
                    .enumerate()
                    .skip(110)
                    .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                    .unwrap();
                let expected = 100.0 + time_ms * 0.001 * sr;
                // The loop filters delay the peak by a sample or so.
                assert!(
                    (at as f32 - expected).abs() <= 2.5,
                    "sr {sr} time {time_ms}: repeat at {at}, wanted {expected}"
                );
            }
        }
    }

    /// Energy in the first difference over total energy: a stand-in for the
    /// spectral centroid that needs no transform. Falls as the top goes.
    fn brightness(x: &[f32]) -> f64 {
        let total: f64 = x.iter().map(|&v| (v * v) as f64).sum();
        let diff: f64 = x.windows(2).map(|w| ((w[1] - w[0]).powi(2)) as f64).sum();
        diff / total.max(1e-30)
    }

    #[test]
    fn every_repeat_is_darker_than_the_last() {
        let time_ms = 300.0;
        let p = EchoParams {
            tone_hz: 3_500.0,
            ..clean(time_ms, 0.8)
        };
        let step = (time_ms * 0.001 * SR) as usize;
        let burst = 4_800;
        let mut seed = 9u32;
        let mut input = vec![0.0; step * 5 + burst];
        for v in input.iter_mut().take(burst) {
            *v = noise(&mut seed) * 0.5;
        }
        let out = run(SR, &p, &input);
        let b: Vec<f64> = (0..5)
            .map(|k| brightness(&out[k * step..k * step + burst]))
            .collect();
        for k in 1..5 {
            assert!(
                b[k] < b[k - 1] * 0.9,
                "repeat {k} was not clearly darker: {b:?}"
            );
        }
    }

    #[test]
    fn tone_sets_how_fast_the_top_goes() {
        let measure = |tone_hz: f32| {
            let p = EchoParams {
                tone_hz,
                ..clean(200.0, 0.7)
            };
            let step = (0.2 * SR) as usize;
            let mut seed = 5u32;
            let mut input = vec![0.0; step * 3 + 2_400];
            for v in input.iter_mut().take(2_400) {
                *v = noise(&mut seed) * 0.5;
            }
            let out = run(SR, &p, &input);
            brightness(&out[2 * step..2 * step + 2_400])
        };
        assert!(measure(800.0) < measure(8_000.0) * 0.5);
    }

    #[test]
    fn wobble_bends_pitch_within_its_stated_reach() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut e = Echo::new(sr);
            let ms = 0.001 * sr;
            let mut prev = 0.0f32;
            let mut worst = 0.0f32;
            for i in 0..(30.0 * sr) as usize {
                // The read point's speed: how far its delay moved this sample.
                let d = e.wobbles[0].step(sr) * ms;
                if i > 0 {
                    worst = worst.max((d - prev).abs());
                }
                prev = d;
            }
            assert!(worst <= MAX_DEVIATION * 1.02, "sr {sr}: deviation {worst}");
            assert!(worst >= 0.002, "sr {sr}: wobble barely moves ({worst})");
        }
    }

    #[test]
    fn no_wobble_means_no_pitch_bend() {
        // A steady tone comes back as a steady tone with wobble off: after
        // taking out the one frequency (the high-pass shifts its phase and
        // level a little, which is not the point), nothing is left over.
        let p = clean(100.0, 0.0);
        let input: Vec<f32> = (0..24_000).map(tone).collect();
        let out = run(SR, &p, &input);
        let echo: Vec<f64> = (18_000..22_000)
            .map(|i| (out[i] - (1.0 - p.mix) * input[i]) as f64)
            .collect();
        let w = core::f64::consts::TAU * 220.0 / SR as f64;
        let n = echo.len() as f64;
        let (mut s, mut c) = (0.0, 0.0);
        for (k, &e) in echo.iter().enumerate() {
            let t = (k + 18_000) as f64 * w;
            s += e * t.sin() * 2.0 / n;
            c += e * t.cos() * 2.0 / n;
        }
        let total: f64 = echo.iter().map(|e| e * e).sum();
        let tone_part = (s * s + c * c) / 2.0 * n;
        assert!(total > 100.0, "no repeat to measure");
        assert!(
            (total - tone_part) / total < 0.001,
            "repeat is not a pure tone: {} of its energy is elsewhere",
            (total - tone_part) / total
        );
    }

    #[test]
    fn the_channels_decorrelate_with_wobble() {
        let p = EchoParams {
            wobble: 1.0,
            ..clean(200.0, 0.5)
        };
        let mut e = Echo::new(SR);
        let mut diff = 0.0f64;
        let mut total = 0.0f64;
        for i in 0..(4 * 48_000) {
            let x = tone(i);
            let (l, r) = e.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite());
            if i > 48_000 {
                diff += ((l - r) * (l - r)) as f64;
                total += (l * l) as f64;
            }
        }
        assert!(diff > 0.01 * total, "a mono source stayed mono");
    }

    #[test]
    fn there_is_no_ping_pong() {
        let p = clean(100.0, 0.7);
        let mut e = Echo::new(SR);
        for i in 0..48_000 {
            let (_, r) = e.process(if i < 2_400 { tone(i) } else { 0.0 }, 0.0, &p);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn grit_rounds_off_loud_repeats_and_leaves_quiet_ones() {
        let level = |amp: f32, grit: f32| {
            let p = EchoParams {
                grit,
                ..clean(100.0, 0.0)
            };
            let input: Vec<f32> = (0..9_600).map(|i| tone(i) * 2.0 * amp).collect();
            let out = run(SR, &p, &input);
            let mut peak = 0.0f32;
            for i in 7_000..9_600 {
                peak = peak.max((out[i] - (1.0 - p.mix) * input[i]).abs());
            }
            peak / amp
        };
        // Quiet: the same gain whatever the grit.
        let (q0, q1) = (level(0.01, 0.0), level(0.01, 1.0));
        assert!(
            (q0 - q1).abs() < 0.02 * q0,
            "quiet repeat changed: {q0} {q1}"
        );
        // Loud: held down by grit.
        assert!(level(1.0, 1.0) < level(1.0, 0.0) * 0.8);
    }

    #[test]
    fn grit_adds_harmonics_to_a_loud_repeat() {
        let sharp = |grit: f32| {
            let p = EchoParams {
                grit,
                ..clean(100.0, 0.0)
            };
            let input: Vec<f32> = (0..9_600).map(|i| tone(i) * 1.8).collect();
            let out = run(SR, &p, &input);
            let echo: Vec<f32> = (0..9_600)
                .map(|i| out[i] - (1.0 - p.mix) * input[i])
                .collect();
            brightness(&echo[6_000..9_600])
        };
        assert!(sharp(1.0) > sharp(0.0) * 1.5);
    }

    #[test]
    fn bass_does_not_build_up_in_the_loop() {
        // A held offset is the lowest note there is. The loop's high-pass
        // must not let it sit and swell through the saturation.
        let p = EchoParams {
            grit: 1.0,
            ..clean(100.0, 0.9)
        };
        let mut e = Echo::new(SR);
        let mut last = 0.0f32;
        for i in 0..(4 * 48_000) {
            let x = if i < 2 * 48_000 { 0.5 } else { 0.0 };
            let (l, _) = e.process(x, x, &p);
            if i > 3 * 48_000 {
                last = last.max((l - x).abs());
            }
        }
        assert!(last < 0.05, "offset still circulating: {last}");
    }

    #[test]
    fn sweeping_every_parameter_does_not_click() {
        // Music-like input: a few partials under a decaying pluck envelope.
        let input = |i: usize| {
            let t = i as f32 / SR;
            let env = (-(t % 0.25) * 8.0).exp();
            let ph = core::f32::consts::TAU * t;
            0.3 * env
                * ((330.0 * ph).sin() + 0.5 * (660.0 * ph).sin() + 0.25 * (1_320.0 * ph).sin())
        };
        let n = 2 * 48_000;
        let mut prev = 0.0f32;
        let mut step_in = 0.0f32;
        for i in 0..n {
            let x = input(i);
            step_in = step_in.max((x - prev).abs());
            prev = x;
        }
        let mut e = Echo::new(SR);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for i in 0..n {
            // Everything glides across its range over the second second.
            let f = ((i as f32 - 48_000.0) / 48_000.0).clamp(0.0, 1.0);
            let lerp = |a: f32, b: f32| a + (b - a) * f;
            let p = EchoParams {
                time_ms: lerp(ECHO_MIN_MS, ECHO_MAX_MS),
                feedback: lerp(0.0, MAX_FEEDBACK),
                tone_hz: lerp(TONE_MIN_HZ, TONE_MAX_HZ),
                wobble: lerp(0.0, 1.0),
                grit: lerp(0.0, 1.0),
                mix: lerp(0.0, 1.0),
            };
            let (l, _) = e.process(input(i), input(i), &p);
            assert!(l.is_finite());
            if i > 0 {
                worst = worst.max((l - prev).abs());
            }
            prev = l;
        }
        // The sweep stretches the pitch of old material, so allow some
        // headroom over the input's own steepest step, but nothing like a jump.
        assert!(
            worst < step_in * 4.0,
            "swept output stepped by {worst}, input's own worst is {step_in}"
        );
    }

    #[test]
    fn a_time_change_glides_rather_than_jumps() {
        let mut e = Echo::new(SR);
        let mut p = clean(1_000.0, 0.0);
        let mut worst = 0.0f32;
        let mut prev = 0.0f32;
        for i in 0..(3 * 48_000) {
            if i == 48_000 {
                p.time_ms = 100.0;
            }
            let (l, _) = e.process(tone(i), tone(i), &p);
            if i > 1 {
                worst = worst.max((l - prev).abs());
            }
            prev = l;
        }
        // The input's own steepest step is about 0.5 * 2 pi 220 / sr = 0.014.
        // Reading faster than real time scales that by the glide's speed.
        assert!(worst < 0.1, "time change stepped by {worst}");
    }

    #[test]
    fn works_at_other_sample_rates() {
        for sr in [22_050.0, 44_100.0, 96_000.0, 192_000.0] {
            for p in [maxed(), EchoParams::default()] {
                let mut e = Echo::new(sr);
                let mut seed = 3u32;
                for _ in 0..(sr as usize * 2) {
                    let x = noise(&mut seed);
                    let (l, r) = e.process(x, -x, &p);
                    assert!(l.is_finite() && r.is_finite());
                    assert!(l.abs() <= 4.0 && r.abs() <= 4.0);
                }
            }
        }
    }

    #[test]
    fn a_mono_source_stays_bounded_and_finite() {
        let mut e = Echo::new(SR);
        let mut seed = 11u32;
        for i in 0..(3 * 48_000) {
            let x = noise(&mut seed);
            let p = if i % 20_000 < 10_000 {
                maxed()
            } else {
                EchoParams::default()
            };
            let (l, r) = e.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite());
            assert!(l.abs() <= 4.0 && r.abs() <= 4.0);
        }
    }

    #[test]
    fn out_of_range_params_are_clamped() {
        let mut e = Echo::new(SR);
        let p = EchoParams {
            time_ms: 1e9,
            feedback: 50.0,
            tone_hz: -3.0,
            wobble: 9.0,
            grit: 9.0,
            mix: 9.0,
        };
        for i in 0..48_000 {
            let (l, r) = e.process(tone(i), tone(i), &p);
            assert!(l.is_finite() && r.is_finite() && l.abs() <= 4.0);
        }
    }

    #[test]
    fn reset_forgets_the_tail() {
        let mut x = Echo::new(48_000.0);
        let p = EchoParams {
            time_ms: ECHO_MAX_MS,
            feedback: MAX_FEEDBACK,
            tone_hz: TONE_MAX_HZ,
            wobble: 1.0,
            grit: 1.0,
            mix: 1.0,
        };
        let mut state = 0x9E37_79B9u32;
        for _ in 0..48_000 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let v = state as f32 / u32::MAX as f32 * 2.0 - 1.0;
            x.process(v, -v, &p);
        }
        x.reset();
        // Silence for longer than the longest line, so any stale audio shows.
        for _ in 0..(48_000 * 3) {
            let (l, r) = x.process(0.0, 0.0, &p);
            assert_eq!((l, r), (0.0, 0.0));
        }
    }

    #[test]
    fn reset_on_a_fresh_node_changes_nothing() {
        let p = EchoParams {
            time_ms: ECHO_MAX_MS,
            feedback: MAX_FEEDBACK,
            tone_hz: TONE_MAX_HZ,
            wobble: 1.0,
            grit: 1.0,
            mix: 1.0,
        };
        let mut a = Echo::new(48_000.0);
        let mut b = Echo::new(48_000.0);
        b.reset();
        for i in 0..4_800 {
            let v = ((i % 211) as f32 / 211.0) - 0.5;
            assert_eq!(a.process(v, v * 0.5, &p), b.process(v, v * 0.5, &p));
        }
    }
}
