//! Wear.
//!
//! The sound of a tape that has been left on a dashboard, played on a machine
//! nobody looked after. Not hiss: this is damage to the signal's pitch and
//! level, and it is all slow enough to hear as character rather than noise.
//!
//! The pitch comes from a short delay line a side, read at a point that is
//! pushed around. A read point that moves bends pitch for as long as it moves,
//! by minus its own speed, so every source below is a distance in
//! milliseconds and the cents come out of how fast it travels.
//!
//! - Wow is the slow one: a 0.4 Hz sine, six milliseconds either way, plus a
//!   slow random walk of two. At full it leans the pitch about 26 cents each
//!   way from the sine alone, and up to `WOW_CENTS` with the walk. The right
//!   side reads the same wow a twelfth of a turn on, and the same walk, so
//!   the image sways as one rather than splitting.
//! - Flutter is the fast one: around 7.5 Hz, wandering between 6 and 9, its
//!   size wandering too. An eighth of a millisecond at most, so up to
//!   `FLUTTER_CENTS` as measured on a tone. Each side has its own.
//! - Unstable is a chaotic system, not a wobble. Each side steps a Lorenz
//!   attractor a hundred times a second, takes one coordinate, and smooths it
//!   with two 40 ms poles. The coordinate sits on one lobe for a while, spirals
//!   out, and lurches to the other, never on the same schedule twice. Four and a
//!   half milliseconds at full, which is about 40 cents at the steepest lurch and
//!   never beyond `UNSTABLE_CENTS`. The two sides start from different points,
//!   so they lurch at different moments. It is bounded by construction (the
//!   coordinate is clamped, whatever the input does), and it does not listen
//!   to the input.
//!
//! All three sum around a fixed 14 ms delay, which is also the wet signal's
//! latency, and the line holds 40 ms. With every source at full the pitch can
//! leave true by up to `MAX_CENTS` in the worst coincidence, and is usually
//! well inside that.
//!
//! Dropouts are short dips in the level, 5 to 60 ms, with edges smoothed over
//! a few milliseconds so they read as lost oxide and not as clicks. They turn
//! up at random, about six a second per side at full and just over one at the
//! default, and the depth rises with the control. A share of them, about two
//! in five, do not drop the level at all but dull the sound instead, as the
//! head loses contact and the top goes first.
//!
//! Dull is a low-pass gate. Two one-pole low-passes in a row (12 dB an
//! octave) whose cutoff falls from 12 kHz towards 900 Hz as dull rises, and
//! also follows the input's level: an envelope follower, 5 ms to rise and
//! 120 ms to fall, reads the input before the delay. Quiet material sits at
//! the dulled cutoff; a loud hit opens it three quarters of the way back up
//! the range, so notes arrive bright and then close as they die away. At dull
//! zero the cutoff stays at 12 kHz whatever the level.
//!
//! The wet signal is the delayed, dulled, dropped-out copy, and `mix`
//! crossfades it with the dry like every other effect's. The delay is a fixed
//! 14 ms, so around the middle of the range the dry and wet comb a little;
//! that is part of the sound.
//!
//! Every random source comes from `crate::rng`, seeded from the seed given
//! to `with_seed`, so a render is the same every time. At zero mix the node is
//! a bit-exact bypass, and everything keeps running underneath so switching
//! on never plays stale audio.

use crate::rng::Rng;
use crate::smooth::OnePole;
use core::f32::consts::TAU;

/// Where the read point sits, in milliseconds, before any modulation. This is
/// also the wet signal's latency. More than the widest excursion, with a
/// little to spare.
pub const CENTRE_MS: f32 = 14.0;
/// The line's length, in milliseconds. The read point never gets near the end.
pub const LINE_MS: f32 = 40.0;
/// Wow's sine rate, in hertz.
pub const WOW_HZ: f32 = 0.4;
/// Wow's sine reach at full, in milliseconds either way.
pub const WOW_MS: f32 = 6.0;
/// Wow's random walk reach at full, in milliseconds either way.
pub const WALK_MS: f32 = 2.0;
/// How far the right side's wow is behind the left's, in turns.
pub const WOW_PHASE_R: f32 = 1.0 / 12.0;
/// Flutter's centre rate, in hertz. It wanders a fifth either way.
pub const FLUTTER_HZ: f32 = 7.5;
/// Flutter's reach at full, in milliseconds either way.
pub const FLUTTER_MS: f32 = 0.12;
/// Unstable's reach at full, in milliseconds either way.
pub const UNSTABLE_MS: f32 = 4.5;
/// Most the pitch leaves true by, in cents, with unstable at full on its own.
pub const UNSTABLE_CENTS: f32 = 50.0;
/// Most the pitch leaves true by, in cents, with wow at full on its own.
pub const WOW_CENTS: f32 = 40.0;
/// Most the pitch leaves true by, in cents, with flutter at full on its own.
pub const FLUTTER_CENTS: f32 = 20.0;
/// Most the pitch leaves true by, in cents, with every source at full.
pub const MAX_CENTS: f32 = 115.0;
/// The low-pass gate's cutoff when fully open, in hertz.
pub const DULL_TOP_HZ: f32 = 12_000.0;
/// And at its dullest, in hertz.
pub const DULL_BOTTOM_HZ: f32 = 900.0;
/// The envelope follower's rise and fall, in milliseconds.
pub const ENV_ATTACK_MS: f32 = 5.0;
pub const ENV_RELEASE_MS: f32 = 120.0;
/// How much of the dulled range a loud input opens, 0 to 1 in the exponent.
const OPENS: f32 = 0.75;
/// The envelope level at which the gate is fully open.
const LOUD: f32 = 0.3;
/// Dropouts a second per side at full.
pub const DROPOUT_RATE_HZ: f32 = 6.0;
/// The shortest and longest dropout, in milliseconds.
pub const DROPOUT_MIN_MS: f32 = 5.0;
pub const DROPOUT_MAX_MS: f32 = 60.0;
/// Chance a dropout dulls instead of dipping.
const DULL_SHARE: f32 = 0.4;
/// How often the chaotic system is stepped, in hertz.
const CHAOS_HZ: f32 = 100.0;
/// The Lorenz system's x coordinate stays inside about this.
const LORENZ_SCALE: f32 = 20.0;

#[derive(Debug, Clone, Copy)]
pub struct WearParams {
    /// Slow pitch sway, 0 to 1.
    pub wow: f32,
    /// Fast pitch shiver, 0 to 1.
    pub flutter: f32,
    /// Chaotic pitch lurching, 0 to 1. Up to roughly 50 cents.
    pub unstable: f32,
    /// How many dropouts, and how deep, 0 to 1.
    pub dropouts: f32,
    /// How far the low-pass gate closes, 0 to 1.
    pub dull: f32,
    /// Dry to wet, 0 to 1.
    pub mix: f32,
}

impl Default for WearParams {
    fn default() -> Self {
        Self {
            wow: 0.4,
            flutter: 0.3,
            unstable: 0.0,
            dropouts: 0.2,
            dull: 0.4,
            mix: 0.7,
        }
    }
}

/// A slow random path: a new target now and then, through two poles, so it
/// never turns a corner. Bounded to plus or minus one.
#[derive(Debug, Clone, Copy)]
struct Wander {
    target: f32,
    a: OnePole,
    b: OnePole,
    count: u32,
    period: u32,
}

impl Wander {
    /// A new target every `period_ms`, smoothed by `smooth_ms` twice.
    fn new(period_ms: f32, smooth_ms: f32, sample_rate: f32) -> Self {
        let pole = || {
            let mut p = OnePole::new();
            p.set_time(smooth_ms, sample_rate);
            p
        };
        Self {
            target: 0.0,
            a: pole(),
            b: pole(),
            count: 0,
            period: (period_ms * 0.001 * sample_rate).max(1.0) as u32,
        }
    }

    /// `gain` makes up for the smoothing taking the swing out of it.
    #[inline]
    fn process(&mut self, rng: &mut Rng, gain: f32) -> f32 {
        if self.count == 0 {
            self.target = rng.next_bipolar();
            self.count = self.period;
        }
        self.count -= 1;
        (self.b.process(self.a.process(self.target)) * gain).clamp(-1.0, 1.0)
    }
}

/// Everything a side keeps to itself: flutter, the chaotic system, the
/// dropouts and the low-pass gate.
struct Side {
    rng: Rng,
    flutter_phase: f32,
    flutter_rate: Wander,
    flutter_size: Wander,
    lorenz: [f32; 3],
    chaos_count: u32,
    chaos_a: OnePole,
    chaos_b: OnePole,
    chaos_now: f32,
    /// Samples left in the dropout under way, or zero.
    drop_left: u32,
    drop_gain_target: f32,
    drop_dull_target: f32,
    drop_gain: OnePole,
    drop_dull: OnePole,
    env: f32,
    lp: [f32; 2],
}

impl Side {
    fn new(seed: u32, sample_rate: f32) -> Self {
        let mut rng = Rng::new(seed);
        let smoother = |ms: f32, v: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        // A different starting point for each side, near the attractor.
        let mut lorenz = [
            1.0 + rng.next_bipolar(),
            1.0 + rng.next_bipolar(),
            20.0 + rng.next_bipolar(),
        ];
        for _ in 0..400 {
            lorenz_step(&mut lorenz);
        }
        let flutter_phase = rng.next_f32();
        Self {
            rng,
            flutter_phase,
            flutter_rate: Wander::new(40.0, 60.0, sample_rate),
            flutter_size: Wander::new(60.0, 80.0, sample_rate),
            lorenz,
            chaos_count: 0,
            chaos_a: smoother(40.0, 0.0),
            chaos_b: smoother(40.0, 0.0),
            chaos_now: 0.0,
            drop_left: 0,
            drop_gain_target: 1.0,
            drop_dull_target: 0.0,
            drop_gain: smoother(3.0, 1.0),
            drop_dull: smoother(8.0, 0.0),
            env: 0.0,
            lp: [0.0; 2],
        }
    }

    /// This sample's flutter and chaos, each in plus or minus one.
    #[inline]
    fn modulate(&mut self, sample_rate: f32, chaos_period: u32) -> (f32, f32) {
        let jitter = self.flutter_rate.process(&mut self.rng, 3.0);
        let size = self.flutter_size.process(&mut self.rng, 3.0);
        self.flutter_phase += FLUTTER_HZ * (1.0 + 0.2 * jitter) / sample_rate;
        self.flutter_phase -= self.flutter_phase.floor();
        let flutter = (TAU * self.flutter_phase).sin() * (0.75 + 0.25 * size);

        if self.chaos_count == 0 {
            lorenz_step(&mut self.lorenz);
            let x = self.lorenz[0];
            if !x.is_finite() || x.abs() > 100.0 {
                // Cannot happen with these constants; a guard, not a path.
                self.lorenz = [1.0, 1.0, 20.0];
            }
            self.chaos_now = (self.lorenz[0] / LORENZ_SCALE).clamp(-1.0, 1.0);
            self.chaos_count = chaos_period;
        }
        self.chaos_count -= 1;
        let chaos = self.chaos_b.process(self.chaos_a.process(self.chaos_now));
        (flutter, chaos)
    }

    /// Run the dropout scheduler one sample and give back the level to
    /// multiply by (1 is untouched) and how far to close the low-pass gate
    /// (0 is not at all).
    #[inline]
    fn dropout(&mut self, amount: f32, sample_rate: f32) -> (f32, f32) {
        if self.drop_left > 0 {
            self.drop_left -= 1;
            if self.drop_left == 0 {
                self.drop_gain_target = 1.0;
                self.drop_dull_target = 0.0;
            }
        } else if amount > 0.0 && self.rng.next_f32() < amount * DROPOUT_RATE_HZ / sample_rate {
            let ms = DROPOUT_MIN_MS + self.rng.next_f32() * (DROPOUT_MAX_MS - DROPOUT_MIN_MS);
            self.drop_left = (ms * 0.001 * sample_rate) as u32;
            let depth = (0.3 + 0.7 * amount) * (0.4 + 0.6 * self.rng.next_f32());
            if self.rng.next_f32() < DULL_SHARE {
                self.drop_dull_target = (0.5 + 0.5 * amount).min(1.0);
            } else {
                self.drop_gain_target = 1.0 - depth.min(1.0);
            }
        }
        (
            self.drop_gain.process(self.drop_gain_target),
            self.drop_dull.process(self.drop_dull_target),
        )
    }
}

/// One step of the Lorenz system, in two Euler halves. Time runs 0.008
/// units a step, so with 100 steps a second one loop of its spiral takes
/// about a second.
#[inline]
fn lorenz_step(s: &mut [f32; 3]) {
    const DT: f32 = 0.004;
    for _ in 0..2 {
        let [x, y, z] = *s;
        s[0] = x + DT * 10.0 * (y - x);
        s[1] = y + DT * (x * (28.0 - z) - y);
        s[2] = z + DT * (x * y - 8.0 / 3.0 * z);
    }
}

pub struct Wear {
    /// Interleaved stereo frames, sized once. `write` is the newest.
    line: Vec<f32>,
    write: usize,
    frames: usize,
    sample_rate: f32,
    chaos_period: u32,
    wow_phase: f32,
    walk: Wander,
    walk_rng: Rng,
    sides: [Side; 2],
    wow: OnePole,
    flutter: OnePole,
    unstable: OnePole,
    mix: OnePole,
    attack: f32,
    release: f32,
}

impl Wear {
    pub fn new(sample_rate: f32) -> Self {
        Self::with_seed(sample_rate, 0x5EED_0001)
    }

    /// Every random source is derived from `seed`, so equal seeds render
    /// equal sound.
    pub fn with_seed(sample_rate: f32, seed: u32) -> Self {
        let frames = (LINE_MS * 0.001 * sample_rate).ceil() as usize + 4;
        let d = WearParams::default();
        // Depths move a read point by milliseconds, so a step in one is a
        // glide in pitch. Slow, to keep that glide to a few tens of cents.
        let smoother = |ms: f32, v: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        let coef = |ms: f32| 1.0 - (-1.0 / (ms * 0.001 * sample_rate)).exp();
        Self {
            line: vec![0.0; frames * 2],
            write: 0,
            frames,
            sample_rate,
            chaos_period: (sample_rate / CHAOS_HZ).max(1.0) as u32,
            wow_phase: 0.0,
            walk: Wander::new(50.0, 400.0, sample_rate),
            walk_rng: Rng::new(seed ^ 0xA5A5_1234),
            sides: [
                Side::new(seed.wrapping_mul(0x9E37_79B1) ^ 0x1111_1111, sample_rate),
                Side::new(seed.wrapping_mul(0x85EB_CA6B) ^ 0x2222_2222, sample_rate),
            ],
            wow: smoother(200.0, d.wow),
            flutter: smoother(200.0, d.flutter),
            unstable: smoother(200.0, d.unstable),
            mix: smoother(20.0, 0.0),
            attack: coef(ENV_ATTACK_MS),
            release: coef(ENV_RELEASE_MS),
        }
    }

    /// Four-point interpolated read, `delay` samples back.
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
    pub fn process(&mut self, l: f32, r: f32, p: &WearParams) -> (f32, f32) {
        let wow = self.wow.process(p.wow.clamp(0.0, 1.0));
        let flutter = self.flutter.process(p.flutter.clamp(0.0, 1.0));
        let unstable = self.unstable.process(p.unstable.clamp(0.0, 1.0));
        let dull = p.dull.clamp(0.0, 1.0);
        let dropouts = p.dropouts.clamp(0.0, 1.0);
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Snap the last of a fade to a true zero, as the chorus does.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        self.write = (self.write + 1) % self.frames;
        self.line[self.write * 2] = l;
        self.line[self.write * 2 + 1] = r;

        self.wow_phase += WOW_HZ / self.sample_rate;
        self.wow_phase -= self.wow_phase.floor();
        let walk = self.walk.process(&mut self.walk_rng, 2.5);

        let ms_to_samples = 0.001 * self.sample_rate;
        let inputs = [l, r];
        let mut out = [0.0; 2];
        for ch in 0..2 {
            let phase = self.wow_phase - WOW_PHASE_R * ch as f32;
            let wow_ms = wow * (WOW_MS * (TAU * phase).sin() + WALK_MS * walk);
            let (fl, chaos) = self.sides[ch].modulate(self.sample_rate, self.chaos_period);
            let ms =
                CENTRE_MS + wow_ms + flutter * FLUTTER_MS * fl + unstable * UNSTABLE_MS * chaos;
            let delayed = self.tap(ch, ms * ms_to_samples);

            let side = &mut self.sides[ch];
            let (gain, dulling) = side.dropout(dropouts, self.sample_rate);

            // The follower reads the input, before the delay, so the gate
            // opens as the note arrives rather than 14 ms after.
            let x = inputs[ch].abs();
            let k = if x > side.env {
                self.attack
            } else {
                self.release
            };
            side.env += k * (x - side.env);
            if side.env < 1e-12 {
                side.env = 0.0;
            }
            let open = (side.env / LOUD).min(1.0);
            let closed = dull * (1.0 - OPENS * open);
            let mut cutoff = DULL_TOP_HZ * (DULL_BOTTOM_HZ / DULL_TOP_HZ).powf(closed);
            cutoff *= 1.0 - 0.75 * dulling;
            let g = 1.0 - (-TAU * cutoff / self.sample_rate).exp();
            for s in side.lp.iter_mut() {
                // Flush the tail before it reaches denormals.
                if s.abs() < 1e-20 {
                    *s = 0.0;
                }
            }
            side.lp[0] += g * (delayed - side.lp[0]);
            side.lp[1] += g * (side.lp[0] - side.lp[1]);
            let wet = side.lp[1] * gain;

            out[ch] = inputs[ch] * (1.0 - mix) + wet * mix;
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

    fn off() -> WearParams {
        WearParams {
            wow: 0.0,
            flutter: 0.0,
            unstable: 0.0,
            dropouts: 0.0,
            dull: 0.0,
            mix: 1.0,
        }
    }

    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    /// A tone a little like music: two partials and a slow swell.
    fn music(i: usize, sr: f32) -> f32 {
        let t = i as f32 / sr;
        let swell = 0.6 + 0.4 * (TAU * 2.0 * t).sin();
        swell * (0.35 * (TAU * 220.0 * t).sin() + 0.2 * (TAU * 660.0 * t).sin())
    }

    /// Run a 1 kHz sine through and give each cycle's pitch error in cents,
    /// left side, after the first two seconds while the depth smoothers
    /// settle.
    fn cents(p: &WearParams, sr: f32, seconds: f32, seed: u32) -> Vec<f32> {
        let mut w = Wear::with_seed(sr, seed);
        let n = (seconds * sr) as usize;
        let skip = (2.0 * sr) as usize;
        let mut prev = 0.0f32;
        let mut last_cross: Option<f32> = None;
        let mut out = Vec::new();
        for i in 0..n {
            let x = (TAU * 1000.0 * i as f32 / sr).sin() * 0.5;
            let (y, _) = w.process(x, x, p);
            if i > 0 && prev < 0.0 && y >= 0.0 {
                let at = i as f32 - 1.0 + (-prev) / (y - prev);
                if let Some(l) = last_cross {
                    if i > skip {
                        out.push(1200.0 * (sr / (at - l) / 1000.0).log2());
                    }
                }
                last_cross = Some(at);
            }
            prev = y;
        }
        out
    }

    fn worst(c: &[f32]) -> f32 {
        c.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn pitch_stays_inside_the_documented_cents_for_each_source() {
        let wow = WearParams { wow: 1.0, ..off() };
        let flutter = WearParams {
            flutter: 1.0,
            ..off()
        };
        let unstable = WearParams {
            unstable: 1.0,
            ..off()
        };
        let all = WearParams {
            wow: 1.0,
            flutter: 1.0,
            unstable: 1.0,
            ..off()
        };
        for (name, p, limit) in [
            ("wow", wow, WOW_CENTS),
            ("flutter", flutter, FLUTTER_CENTS),
            ("unstable", unstable, UNSTABLE_CENTS),
            ("all", all, MAX_CENTS),
        ] {
            let c = cents(&p, SR, 40.0, 7);
            let w = worst(&c);
            println!("{name}: worst {w:.1} cents");
            assert!(w <= limit, "{name} reached {w} cents, documented {limit}");
            assert!(w > limit * 0.3, "{name} barely moved: {w}");
        }
    }

    #[test]
    fn with_everything_off_the_pitch_is_true() {
        let c = cents(&off(), SR, 5.0, 7);
        assert!(worst(&c) < 1.0, "drifted {} cents", worst(&c));
    }

    #[test]
    fn unstable_lurches_rather_than_wobbles() {
        // A wobble spends its time at the extremes of a sine; the chaotic
        // path should visit both signs and not repeat.
        let p = WearParams {
            unstable: 1.0,
            ..off()
        };
        let c = cents(&p, SR, 40.0, 3);
        assert!(c.iter().any(|v| *v > 15.0) && c.iter().any(|v| *v < -15.0));
        let half = c.len() / 2;
        let diff: f32 = c[..half]
            .iter()
            .zip(&c[half..])
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / half as f32;
        assert!(diff > 3.0, "the second half repeats the first: {diff}");
    }

    #[test]
    fn unstable_stays_bounded_for_a_minute() {
        let mut w = Wear::new(SR);
        let p = WearParams {
            wow: 1.0,
            flutter: 1.0,
            unstable: 1.0,
            dropouts: 1.0,
            dull: 1.0,
            mix: 1.0,
        };
        let mut rng = 99u32;
        for i in 0..(60 * SR as usize) {
            // Loud, and then not: the chaos must not care.
            let amp = if (i / 48_000) % 2 == 0 { 1.0 } else { 0.0 };
            let x = noise(&mut rng) * amp;
            let (a, b) = w.process(x, -x, &p);
            assert!(a.is_finite() && b.is_finite() && a.abs() <= 1.5 && b.abs() <= 1.5);
        }
        for s in &w.sides {
            assert!(s.lorenz.iter().all(|v| v.is_finite() && v.abs() < 100.0));
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_sound_and_another_does_not() {
        let p = WearParams {
            unstable: 0.8,
            dropouts: 0.8,
            ..WearParams::default()
        };
        let run = |seed| {
            let mut w = Wear::with_seed(SR, seed);
            (0..48_000)
                .map(|i| w.process(music(i, SR), music(i, SR), &p))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(5), run(5));
        assert_ne!(run(5), run(6));
    }

    /// RMS in 10 ms windows of a steady tone through dropouts.
    fn windows(dropouts: f32, seconds: usize) -> Vec<f32> {
        let mut w = Wear::new(SR);
        let p = WearParams { dropouts, ..off() };
        let win = 480;
        let mut out = Vec::new();
        let mut acc = 0.0;
        for i in 0..seconds * SR as usize {
            let x = (TAU * 1000.0 * i as f32 / SR).sin() * 0.5;
            let (y, _) = w.process(x, x, &p);
            acc += y * y;
            if (i + 1) % win == 0 {
                out.push((acc / win as f32).sqrt());
                acc = 0.0;
            }
        }
        out
    }

    #[test]
    fn dropouts_dip_the_level_only_now_and_then() {
        let none = windows(0.0, 10);
        let (lo, hi) = none[10..]
            .iter()
            .fold((f32::MAX, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        assert!(hi / lo < 1.01, "level moved with dropouts off: {lo} {hi}");

        let w = windows(1.0, 20);
        let mut sorted = w[10..].to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let typical = sorted[sorted.len() / 2];
        let dips = w[10..].iter().filter(|v| **v < typical * 0.8).count();
        let share = dips as f32 / (w.len() - 10) as f32;
        assert!(w[10..].iter().any(|v| *v < typical * 0.5), "no deep dip");
        assert!(
            share > 0.03 && share < 0.6,
            "dips should be intermittent, got {share}"
        );
        // More dropouts spend more time down than fewer.
        let few = windows(0.2, 20);
        let few_dips = few[10..].iter().filter(|v| **v < typical * 0.8).count();
        assert!(few_dips < dips);
    }

    /// High-band over low-band output level for a two-tone input at `amp`.
    fn hf_over_lf(amp: f32, dull: f32) -> f32 {
        let mut w = Wear::new(SR);
        let p = WearParams { dull, ..off() };
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        // Narrow detectors: correlate with each tone.
        let (mut lc, mut ls, mut hc, mut hs) = (0.0, 0.0, 0.0, 0.0);
        for i in 0..48_000 {
            let t = i as f32 / SR;
            let x = amp * ((TAU * 200.0 * t).sin() + (TAU * 6000.0 * t).sin());
            let (y, _) = w.process(x, x, &p);
            if i > 24_000 {
                lc += y * (TAU * 200.0 * t).cos();
                ls += y * (TAU * 200.0 * t).sin();
                hc += y * (TAU * 6000.0 * t).cos();
                hs += y * (TAU * 6000.0 * t).sin();
            }
        }
        lo += lc * lc + ls * ls;
        hi += hc * hc + hs * hs;
        (hi / lo).sqrt()
    }

    #[test]
    fn the_gate_dulls_quiet_input_more_than_loud() {
        let quiet = hf_over_lf(0.01, 0.8);
        let loud = hf_over_lf(0.5, 0.8);
        let open = hf_over_lf(0.01, 0.0);
        println!("quiet {quiet:.3} loud {loud:.3} open {open:.3}");
        assert!(quiet < loud * 0.6, "quiet {quiet} vs loud {loud}");
        assert!(open > quiet * 2.0, "dull 0 should stay bright: {open}");
    }

    #[test]
    fn mix_zero_is_a_bit_exact_bypass_and_state_keeps_running() {
        let mut w = Wear::new(SR);
        let p = WearParams {
            unstable: 1.0,
            dropouts: 1.0,
            mix: 0.0,
            ..WearParams::default()
        };
        let mut rng = 5u32;
        // The mix fades down from its default first, then snaps to zero.
        for _ in 0..48_000 {
            w.process(noise(&mut rng), noise(&mut rng), &p);
        }
        for _ in 0..48_000 {
            let (x, y) = (noise(&mut rng), noise(&mut rng));
            let (a, b) = w.process(x, y, &p);
            assert_eq!((a, b), (x, y));
        }
        // Switched on, the line holds recent audio and not stale silence.
        let on = WearParams { mix: 1.0, ..p };
        let mut energy = 0.0;
        for _ in 0..480 {
            let x = noise(&mut rng);
            let (a, _) = w.process(x, x, &on);
            energy += a * a;
        }
        assert!(energy > 1.0, "the line was not kept filled");
    }

    #[test]
    fn silence_in_silence_out() {
        let mut w = Wear::new(SR);
        let p = WearParams {
            wow: 1.0,
            flutter: 1.0,
            unstable: 1.0,
            dropouts: 1.0,
            dull: 1.0,
            mix: 1.0,
        };
        for _ in 0..48_000 {
            assert_eq!(w.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn nothing_runs_away_at_full_with_loud_noise_and_then_silence() {
        let mut w = Wear::new(SR);
        let p = WearParams {
            wow: 1.0,
            flutter: 1.0,
            unstable: 1.0,
            dropouts: 1.0,
            dull: 1.0,
            mix: 1.0,
        };
        let mut rng = 11u32;
        for _ in 0..(5 * SR as usize) {
            let (x, y) = (noise(&mut rng), noise(&mut rng));
            let (a, b) = w.process(x, y, &p);
            assert!(a.abs() <= 4.0 && b.abs() <= 4.0 && a.is_finite() && b.is_finite());
        }
        let mut last = (1.0, 1.0);
        for _ in 0..(2 * SR as usize) {
            last = w.process(0.0, 0.0, &p);
            assert!(last.0.abs() <= 4.0 && last.1.abs() <= 4.0);
        }
        assert!(
            last.0.abs() < 1e-6 && last.1.abs() < 1e-6,
            "tail never ended"
        );
    }

    #[test]
    fn a_parameter_sweep_does_not_click() {
        let mut w = Wear::new(SR);
        let n = SR as usize;
        let mut input_step = 0.0f32;
        let mut output_step = 0.0f32;
        let (mut px, mut py) = (0.0, 0.0);
        // Settle first so the sweep starts from the sound, not the silence.
        for i in 0..n {
            w.process(music(i, SR), music(i, SR), &WearParams::default());
        }
        for i in 0..n {
            let u = i as f32 / n as f32;
            let p = WearParams {
                wow: u,
                flutter: u,
                unstable: u,
                dropouts: u,
                dull: u,
                mix: 0.3 + 0.7 * u,
            };
            let x = music(n + i, SR);
            let (y, _) = w.process(x, x, &p);
            if i > 0 {
                input_step = input_step.max((x - px).abs());
                output_step = output_step.max((y - py).abs());
            }
            px = x;
            py = y;
        }
        assert!(
            output_step < input_step * 3.0,
            "output stepped {output_step}, input {input_step}"
        );
    }

    #[test]
    fn works_at_every_sample_rate() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let w = Wear::new(sr);
            assert!(w.frames as f32 >= LINE_MS * 0.001 * sr);
            let p = WearParams {
                wow: 1.0,
                flutter: 1.0,
                unstable: 1.0,
                ..off()
            };
            let c = cents(&p, sr, 20.0, 2);
            assert!(worst(&c) <= MAX_CENTS, "{sr}: {}", worst(&c));
        }
    }

    #[test]
    fn a_mono_source_stays_bounded_and_the_sides_are_not_identical() {
        let mut w = Wear::new(SR);
        let p = WearParams {
            unstable: 1.0,
            dropouts: 0.8,
            ..WearParams::default()
        };
        let mut differ = 0.0f32;
        for i in 0..(10 * SR as usize) {
            let x = music(i, SR);
            let (a, b) = w.process(x, x, &p);
            assert!(a.is_finite() && b.is_finite() && a.abs() < 1.5 && b.abs() < 1.5);
            differ = differ.max((a - b).abs());
        }
        assert!(differ > 0.01, "a mono source stayed mono");
    }

    #[test]
    fn wow_sways_both_sides_together() {
        // With only wow on, the sides run the same path a twelfth of a turn
        // apart: their pitch errors are strongly correlated.
        let p = WearParams {
            wow: 1.0,
            mix: 1.0,
            ..off()
        };
        let mut w = Wear::new(SR);
        let (mut prev, mut last) = ([0.0f32; 2], [None::<f32>; 2]);
        let mut tracks: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
        for i in 0..(30 * SR as usize) {
            let x = (TAU * 1000.0 * i as f32 / SR).sin() * 0.5;
            let (a, b) = w.process(x, x, &p);
            for (ch, y) in [a, b].into_iter().enumerate() {
                if i > 96_000 && prev[ch] < 0.0 && y >= 0.0 {
                    let at = i as f32 - 1.0 + (-prev[ch]) / (y - prev[ch]);
                    if let Some(l) = last[ch] {
                        tracks[ch].push(1200.0 * (SR / (at - l) / 1000.0).log2());
                    }
                    last[ch] = Some(at);
                }
                prev[ch] = y;
            }
        }
        let n = tracks[0].len().min(tracks[1].len());
        let (a, b) = (&tracks[0][..n], &tracks[1][..n]);
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(dot / (na * nb) > 0.8, "correlation {}", dot / (na * nb));
    }
}
