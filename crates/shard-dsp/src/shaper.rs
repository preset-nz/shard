//! Transient shaper (row 48), labelled "Transient".
//!
//! It reacts to how fast a sound changes, not how loud it is, so a quiet tick
//! and a loud hit are shaped alike. Attack pushes onsets up or softens them;
//! Sustain lengthens or cuts what rings on after.
//!
//! **Two envelope followers** on one linked detector, `max(|L|, |R|)`: a fast
//! one (attack `FAST_ATTACK_MS`, release Speed) and a slow one (attack Speed,
//! release `SLOW_RELEASE_RATIO` x Speed). Both are compared in the log domain,
//! where a level difference is a number of decibels whatever the level:
//!
//! - at an onset the fast follower is up and the slow one is still climbing,
//!   so `fast - slow` is positive: the **attack signal**;
//! - after the hit the fast follower has fallen and the slow one is still
//!   coming down behind it, so `slow - fast` is positive: the **sustain
//!   signal**. When the sound falls faster than the slow release the gap keeps
//!   growing, so a negative Sustain cuts a tail harder the longer it runs.
//!
//! `gain_dB = Attack * ATTACK_DB_PER_DB * attack_signal
//!          + Sustain * SUSTAIN_DB_PER_DB * sustain_signal`,
//! clamped to `+-GAIN_LIMIT_DB` and smoothed per sample. The same gain goes to
//! both channels, so the image does not move. At Attack = Sustain = 0 the gain
//! is exactly 1 and the wet path is the input, sample for sample.
//!
//! The followers are linear in the signal, and only the log of their ratio is
//! used, so scaling the input moves nothing but the floor (-180 dBFS).
//!
//! `mix` crossfades, `dry * (1 - mix) + wet * mix`, and is a bit-exact bypass
//! at zero. The wet adds no latency.

use crate::smooth::OnePole;

/// The reach of Speed, in milliseconds.
pub const SPEED_MIN_MS: f32 = 1.0;
pub const SPEED_MAX_MS: f32 = 50.0;
/// The largest boost or cut, in decibels either way.
pub const GAIN_LIMIT_DB: f32 = 18.0;
/// The fast follower's attack, in milliseconds.
const FAST_ATTACK_MS: f32 = 1.0;
/// The slow follower lets go this many times slower than Speed.
const SLOW_RELEASE_RATIO: f32 = 8.0;
/// Decibels of gain per decibel of difference between the followers, at
/// full Attack and full Sustain.
const ATTACK_DB_PER_DB: f32 = 1.0;
const SUSTAIN_DB_PER_DB: f32 = 1.0;
/// Added before the log, so silence is a finite, very low level (-180 dBFS).
const FLOOR: f32 = 1e-9;
/// Below this a follower is flushed to zero, clear of the denormals.
const FLUSH: f32 = 1e-20;
/// How long the gain takes to follow its target, in milliseconds.
const GAIN_SMOOTH_MS: f32 = 0.1;

const DB_PER_NEPER: f32 = 8.685_889; // 20 / ln(10)

#[derive(Debug, Clone, Copy)]
pub struct ShaperParams {
    /// -1 to 1: down softens onsets, up sharpens them.
    pub attack: f32,
    /// -1 to 1: down cuts tails, up lets them ring.
    pub sustain: f32,
    /// How long counts as the attack, `SPEED_MIN_MS` to `SPEED_MAX_MS`.
    pub speed_ms: f32,
    /// Dry to shaped, 0 to 1.
    pub mix: f32,
}

impl Default for ShaperParams {
    fn default() -> Self {
        Self {
            attack: 0.0,
            sustain: 0.0,
            speed_ms: 10.0,
            mix: 0.0,
        }
    }
}

/// A one-pole follower with its own attack and release coefficients.
#[derive(Debug, Clone, Copy)]
struct Follower {
    y: f32,
}

impl Follower {
    #[inline]
    fn process(&mut self, x: f32, attack: f32, release: f32) -> f32 {
        let c = if x > self.y { attack } else { release };
        let y = x + c * (self.y - x);
        self.y = if y < FLUSH { 0.0 } else { y };
        self.y
    }
}

#[inline]
fn coef(ms: f32, sample_rate: f32) -> f32 {
    (-1.0 / (ms * 0.001 * sample_rate)).exp()
}

#[derive(Debug, Clone)]
pub struct Shaper {
    sample_rate: f32,
    fast: Follower,
    slow: Follower,
    /// The coefficients for the Speed they were made at.
    speed_at: f32,
    fast_release: f32,
    slow_attack: f32,
    slow_release: f32,
    fast_attack: f32,
    attack: OnePole,
    sustain: OnePole,
    speed: OnePole,
    mix: OnePole,
    /// The gain, in decibels, smoothed.
    gain: OnePole,
    /// What the last sample's gain was, in decibels.
    last_db: f32,
    primed: bool,
}

impl Shaper {
    pub fn new(sample_rate: f32) -> Self {
        let smoother = || {
            let mut p = OnePole::new();
            p.set_time(20.0, sample_rate);
            p
        };
        let mut gain = OnePole::new();
        gain.set_time(GAIN_SMOOTH_MS, sample_rate);
        Self {
            sample_rate,
            fast: Follower { y: 0.0 },
            slow: Follower { y: 0.0 },
            speed_at: 0.0,
            fast_release: 0.0,
            slow_attack: 0.0,
            slow_release: 0.0,
            fast_attack: coef(FAST_ATTACK_MS, sample_rate),
            attack: smoother(),
            sustain: smoother(),
            speed: smoother(),
            mix: smoother(),
            gain,
            last_db: 0.0,
            primed: false,
        }
    }

    /// Forgets the sound it holds, without allocating. The smoothers keep
    /// their place: they follow controls, not signal.
    pub fn reset(&mut self) {
        self.fast.y = 0.0;
        self.slow.y = 0.0;
        self.gain.reset(0.0);
        self.last_db = 0.0;
    }

    /// The gain the last sample was shaped by, in decibels.
    pub fn gain_db(&self) -> f32 {
        self.last_db
    }

    fn set_speed(&mut self, ms: f32) {
        // Coefficients move only when Speed has, by more than a tenth of a
        // percent: no exp per sample on a steady control.
        if (ms - self.speed_at).abs() > self.speed_at * 1e-3 {
            self.speed_at = ms;
            self.fast_release = coef(ms, self.sample_rate);
            self.slow_attack = self.fast_release;
            self.slow_release = coef(ms * SLOW_RELEASE_RATIO, self.sample_rate);
        }
    }

    pub fn process(&mut self, l: f32, r: f32, p: &ShaperParams) -> (f32, f32) {
        let attack_target = p.attack.clamp(-1.0, 1.0);
        let sustain_target = p.sustain.clamp(-1.0, 1.0);
        let speed_target = p.speed_ms.clamp(SPEED_MIN_MS, SPEED_MAX_MS);
        if !self.primed {
            self.attack.reset(attack_target);
            self.sustain.reset(sustain_target);
            self.speed.reset(speed_target);
            self.mix.reset(p.mix.clamp(0.0, 1.0));
            self.primed = true;
        }
        let snap = |v: f32, target: f32| {
            if target == 0.0 && v.abs() < 1e-5 {
                0.0
            } else {
                v
            }
        };
        let attack = snap(self.attack.process(attack_target), attack_target);
        if attack == 0.0 {
            self.attack.reset(0.0);
        }
        let sustain = snap(self.sustain.process(sustain_target), sustain_target);
        if sustain == 0.0 {
            self.sustain.reset(0.0);
        }
        let speed = self.speed.process(speed_target);
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };
        self.set_speed(speed);

        // The detector and the followers run whatever the mix, so turning the
        // mix up on a sound already playing shapes it from the first sample.
        let det = l.abs().max(r.abs());
        let fast = self.fast.process(det, self.fast_attack, self.fast_release);
        let slow = self.slow.process(det, self.slow_attack, self.slow_release);
        let diff = (fast + FLOOR).ln() - (slow + FLOOR).ln();
        let diff_db = diff * DB_PER_NEPER;
        let target_db = (attack * ATTACK_DB_PER_DB * diff_db.max(0.0)
            + sustain * SUSTAIN_DB_PER_DB * (-diff_db).max(0.0))
        .clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB);
        let mut db = self.gain.process(target_db);
        if target_db == 0.0 && db.abs() < 1e-6 {
            self.gain.reset(0.0);
            db = 0.0;
        }
        self.last_db = db;
        let g = if db == 0.0 {
            1.0
        } else {
            (db / DB_PER_NEPER).exp()
        };

        if mix == 0.0 {
            return (l, r);
        }
        (l * (1.0 - mix) + l * g * mix, r * (1.0 - mix) + r * g * mix)
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

    fn wet(attack: f32, sustain: f32) -> ShaperParams {
        ShaperParams {
            attack,
            sustain,
            speed_ms: 10.0,
            mix: 1.0,
        }
    }

    /// A drum-like hit: noise under a 30 ms exponential fall, after
    /// `lead` samples of silence, `total` samples long.
    fn hit(level: f32, lead: usize, total: usize) -> Vec<f32> {
        let mut seed = 11u32;
        (0..total)
            .map(|n| {
                if n < lead {
                    0.0
                } else {
                    let t = (n - lead) as f32 / SR;
                    level * noise(&mut seed) * (-t / 0.030).exp()
                }
            })
            .collect()
    }

    fn run(x: &[f32], p: &ShaperParams) -> (Vec<f32>, Vec<f32>) {
        let mut s = Shaper::new(SR);
        let (mut out, mut gain) = (Vec::new(), Vec::new());
        for v in x {
            let (a, b) = s.process(*v, *v, p);
            assert_eq!(a, b);
            out.push(a);
            gain.push(s.gain_db());
        }
        (out, gain)
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    fn energy(x: &[f32]) -> f64 {
        x.iter().map(|v| (*v as f64).powi(2)).sum()
    }

    fn db(a: f64, b: f64) -> f64 {
        10.0 * (a / b).log10()
    }

    const LEAD: usize = 4_800;

    #[test]
    fn zero_attack_and_sustain_at_full_mix_is_the_input_exactly() {
        let mut s = Shaper::new(SR);
        let p = wet(0.0, 0.0);
        let mut seed = 3u32;
        for n in 0..48_000 {
            // Bursts and gaps, so the followers move.
            let k = if (n / 6_000) % 2 == 0 { 0.8 } else { 0.01 };
            let (l, r) = (k * noise(&mut seed), k * noise(&mut seed));
            assert_eq!(s.process(l, r, &p), (l, r));
        }
    }

    #[test]
    fn zero_mix_is_a_bit_exact_bypass() {
        let mut s = Shaper::new(SR);
        let p = ShaperParams {
            attack: 1.0,
            sustain: -1.0,
            speed_ms: 3.0,
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
        let mut s = Shaper::new(SR);
        let on = wet(1.0, -1.0);
        let off = ShaperParams { mix: 0.0, ..on };
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
    fn controls_returned_to_zero_end_exactly_transparent() {
        let mut s = Shaper::new(SR);
        let mut seed = 5u32;
        for _ in 0..20_000 {
            s.process(noise(&mut seed), noise(&mut seed), &wet(1.0, -1.0));
        }
        for _ in 0..48_000 {
            s.process(noise(&mut seed), noise(&mut seed), &wet(0.0, 0.0));
        }
        for _ in 0..100 {
            let (l, r) = (noise(&mut seed), noise(&mut seed));
            assert_eq!(s.process(l, r, &wet(0.0, 0.0)), (l, r));
        }
    }

    #[test]
    fn mix_is_a_crossfade() {
        let full = wet(1.0, 0.0);
        let half = ShaperParams { mix: 0.5, ..full };
        let mut a = Shaper::new(SR);
        let mut b = Shaper::new(SR);
        let x = hit(0.5, LEAD, 24_000);
        for v in &x {
            let (w, _) = a.process(*v, *v, &full);
            let (h, _) = b.process(*v, *v, &half);
            assert!((h - (0.5 * v + 0.5 * w)).abs() < 1e-5);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut s = Shaper::new(SR);
        let p = wet(1.0, 1.0);
        for _ in 0..10_000 {
            assert_eq!(s.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn the_gain_curve_does_not_depend_on_level() {
        // The same hit at -30 and at -6 dBFS: the gain in decibels follows
        // the same curve, within a decibel, sample for sample.
        let quiet = 10f32.powf(-30.0 / 20.0);
        let loud = 10f32.powf(-6.0 / 20.0);
        for &(a, s) in &[(1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-1.0, 1.0)] {
            let (_, gq) = run(&hit(quiet, LEAD, 24_000), &wet(a, s));
            let (_, gl) = run(&hit(loud, LEAD, 24_000), &wet(a, s));
            let worst = gq
                .iter()
                .zip(&gl)
                .skip(LEAD)
                .take(14_400)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 1.0, "attack {a} sustain {s}: {worst} dB apart");
        }
    }

    #[test]
    fn attack_up_raises_the_onset_against_the_tail_and_down_lowers_it() {
        let x = hit(0.3, LEAD, 24_000);
        let ratio = |a: f32| {
            let (y, _) = run(&x, &wet(a, 0.0));
            let onset = peak(&y[LEAD..LEAD + 480]);
            let tail = peak(&y[LEAD + 4_800..LEAD + 9_600]);
            20.0 * (onset / tail).log10()
        };
        let (flat, up, down) = (ratio(0.0), ratio(1.0), ratio(-1.0));
        assert!(up > flat + 3.0, "up {up} flat {flat}");
        assert!(down < flat - 3.0, "down {down} flat {flat}");
    }

    #[test]
    fn sustain_down_shortens_the_tail_and_up_lengthens_it() {
        let x = hit(0.3, LEAD, 36_000);
        let from = LEAD + 2_400; // onset + 50 ms
        let tail = |s: f32| {
            let (y, _) = run(&x, &wet(0.0, s));
            energy(&y[from..])
        };
        let (flat, cut, long) = (tail(0.0), tail(-1.0), tail(1.0));
        assert!(db(cut, flat) < -6.0, "cut {} dB", db(cut, flat));
        assert!(db(long, flat) > 3.0, "long {} dB", db(long, flat));
    }

    #[test]
    fn the_gain_never_leaves_eighteen_decibels() {
        let mut seed = 21u32;
        let mut x = Vec::new();
        // Clicks out of silence, bursts, and decays at several speeds.
        for k in 0..20 {
            x.extend(std::iter::repeat_n(0.0, 3_000 + k * 400));
            x.extend((0..(100 + k * 200)).map(|_| noise(&mut seed)));
        }
        for &speed in &[1.0f32, 10.0, 50.0] {
            for &(a, s) in &[(1.0, 1.0), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0)] {
                let p = ShaperParams {
                    speed_ms: speed,
                    ..wet(a, s)
                };
                let (_, gain) = run(&x, &p);
                for g in gain {
                    assert!(g.abs() <= GAIN_LIMIT_DB + 1e-4, "{g}");
                }
            }
        }
        // And the limit is reached on a click out of silence.
        let (_, g) = run(&hit(0.5, LEAD, 9_600), &wet(1.0, 0.0));
        assert!(g.iter().fold(0.0f32, |m, v| m.max(*v)) > 10.0);
    }

    #[test]
    fn the_stereo_image_does_not_move() {
        let mut s = Shaper::new(SR);
        let p = wet(1.0, -1.0);
        let x = hit(0.4, LEAD, 24_000);
        for v in &x {
            let (l, r) = (*v, *v * 0.25 + 0.01 * v.signum());
            let (a, b) = s.process(l, r, &p);
            if l.abs() > 1e-4 {
                assert!((a / l - b / r).abs() < 1e-4 * (a / l).abs().max(1.0));
            }
        }
    }

    #[test]
    fn a_sustained_tone_is_left_alone() {
        // After the start, a steady sine has nothing to shape.
        let x: Vec<f32> = (0..48_000)
            .map(|n| 0.5 * (std::f32::consts::TAU * 1_000.0 * n as f32 / SR).sin())
            .collect();
        let (_, g) = run(&x, &wet(1.0, -1.0));
        let late = g[24_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(late < 0.5, "{late} dB");
    }

    #[test]
    fn sweeping_the_controls_does_not_click() {
        // A steady sine, every control swept hard: no step beyond what the
        // sine makes itself.
        for which in 0..3 {
            let mut s = Shaper::new(SR);
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            for n in 0..(SR as usize * 2) {
                let t = n as f32 / SR;
                let lfo = (std::f32::consts::TAU * 3.0 * t).sin();
                let mut p = wet(0.0, 0.0);
                match which {
                    0 => p.attack = lfo,
                    1 => p.sustain = lfo,
                    _ => p.speed_ms = 10.0 * 5f32.powf(lfo),
                }
                let x = 0.5 * (std::f32::consts::TAU * 220.0 * n as f32 / SR).sin();
                let (a, _) = s.process(x, x, &p);
                if n > 4_800 {
                    worst = worst.max((a - prev).abs());
                }
                prev = a;
            }
            // 0.5 * 2 pi * 220 / 48000 = 0.0144 for the sine alone.
            assert!(worst < 0.04, "control {which}: {worst}");
        }
    }

    #[test]
    fn a_hit_shaped_hard_does_not_click() {
        // The gain moves by at most the smoother allows. The one big step is
        // the onset out of silence, which the sound itself makes; after the
        // first millisecond the gain glides.
        let x = hit(0.5, LEAD, 24_000);
        let (_, gain) = run(&x, &wet(1.0, -1.0));
        let step = gain[LEAD + 48..]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(step < 3.0, "{step} dB in one sample");
    }

    #[test]
    fn reset_forgets_the_sound() {
        let mut s = Shaper::new(SR);
        let p = wet(1.0, 1.0);
        let mut seed = 9u32;
        for _ in 0..20_000 {
            s.process(noise(&mut seed), noise(&mut seed), &p);
        }
        s.reset();
        assert_eq!((s.fast.y, s.slow.y, s.gain_db()), (0.0, 0.0, 0.0));
        for _ in 0..20_000 {
            assert_eq!(s.process(0.0, 0.0, &p), (0.0, 0.0));
        }
        // A tick after the reset is shaped as on a fresh one.
        let mut fresh = Shaper::new(SR);
        let mut a = Vec::new();
        let mut b = Vec::new();
        for n in 0..2_000 {
            let x = if n < 50 { 0.5 } else { 0.0 };
            a.push(s.process(x, x, &p).0);
            b.push(fresh.process(x, x, &p).0);
        }
        assert_eq!(a, b);
    }

    #[test]
    fn works_at_each_rate() {
        for &sr in &[22_050.0f32, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut s = Shaper::new(sr);
            let mut seed = 2u32;
            let n = sr as usize;
            let mut gmax = 0.0f32;
            for i in 0..n * 2 {
                let env = (-((i % n) as f32 / sr) / 0.03).exp();
                let x = if i < n / 4 {
                    0.0
                } else {
                    noise(&mut seed) * env
                };
                for &speed in &[1.0f32, 50.0] {
                    let p = ShaperParams {
                        speed_ms: speed,
                        ..wet(1.0, -1.0)
                    };
                    let (a, b) = s.process(x, x, &p);
                    assert!(a.is_finite() && b.is_finite());
                    gmax = gmax.max(s.gain_db().abs());
                }
            }
            assert!(gmax <= GAIN_LIMIT_DB + 1e-4 && gmax > 1.0, "{sr}: {gmax}");
        }
    }

    #[test]
    fn the_curve_keeps_its_shape_across_rates() {
        // The same hit at 44.1 and 192 kHz: the onset peak of the gain is
        // within 2 dB.
        let at = |sr: f32| {
            let mut s = Shaper::new(sr);
            let mut seed = 11u32;
            let mut top = 0.0f32;
            for n in 0..(sr as usize / 2) {
                let lead = (0.1 * sr) as usize;
                let x = if n < lead {
                    0.0
                } else {
                    noise(&mut seed) * (-((n - lead) as f32 / sr) / 0.03).exp()
                };
                s.process(x, x, &wet(1.0, 0.0));
                top = top.max(s.gain_db());
            }
            top
        };
        assert!((at(44_100.0) - at(192_000.0)).abs() < 2.0);
    }
}
