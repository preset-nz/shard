//! The ladder: a driven 4-pole low-pass, after the filter.
//!
//! Three parts, in this order.
//!
//! **Drive.** The input is gained from 1x to 10x across the Drive row and
//! squashed by a soft saturator (`soft`), then scaled back down so a hot
//! setting is dirtier rather than louder. The makeup is exact at zero drive
//! and holds a half-scale signal level across the range.
//!
//! **The ladder.** Four zero-delay-feedback one-poles in series, the last
//! fed back to the first. The feedback sum goes through the saturator inside
//! the loop, so as the resonance climbs the peak grows into a growl instead
//! of running away, and past `k = 4` (the loop gain of four poles at their
//! corner, 45 degrees each) the loop sustains itself: resonance at one
//! self-oscillates at the cutoff, at a level the saturator settles. The
//! loop is solved implicitly, in a fixed two Newton steps and a closing
//! saturator evaluation, so it never iterates unboundedly and the
//! saturated signal can never leave `[-1, 1]`.
//!
//! **A little makeup** for the passband the feedback takes away, as ladders
//! do: half of what the linear loop loses. The rest stays lost, which is the
//! sound.
//!
//! Off costs next to nothing: once the 10 ms fade has landed on zero the state is
//! cleared, the smoothers sit where the parameters are and the input comes
//! back as it went in, bit for bit. Switching on fades the wet in from a
//! cleared state, so there is nothing to thump.

use crate::smooth::{OnePole, Ramp};

/// The loop gain at full resonance. Four poles give back a quarter of the
/// signal at the corner, so anything over four sustains.
const K_MAX: f32 = 4.5;
/// How much of the resonance's passband loss comes back.
const MAKEUP: f32 = 0.5;
/// Drive's range in octaves of gain: 1x to 10x.
const DRIVE_OCTAVES: f32 = core::f32::consts::LOG2_10;
/// The level the drive's makeup holds constant.
const DRIVE_REF: f32 = 0.5;
/// How long the switch takes. The same as every other section's.
const FADE_MS: f32 = 10.0;

/// A rational tanh: unit slope at zero, exactly +-1 from three on, monotone
/// and smooth enough. No transcendental, so it is cheap in the loop.
#[inline]
fn soft(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

/// One block's values for the ladder.
#[derive(Debug, Clone, Copy)]
pub struct LadderParams {
    /// The switch as a gate, zero or one.
    pub on: f32,
    /// Dry to wet, 0 to 1.
    pub mix: f32,
    /// 0 to 1, across 1x to 10x of input gain.
    pub drive: f32,
    pub cutoff_hz: f32,
    /// 0 to 1. One self-oscillates.
    pub resonance: f32,
}

impl Default for LadderParams {
    fn default() -> Self {
        Self {
            on: 0.0,
            mix: 1.0,
            drive: 0.3,
            cutoff_hz: 2_000.0,
            resonance: 0.2,
        }
    }
}

/// Where the ladder's rows sit in a table, resolved once at construction.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LadderSlots {
    pub on: usize,
    pub mix: usize,
    pub drive: usize,
    pub cutoff: usize,
    pub resonance: usize,
    pub tracking: usize,
}

impl LadderSlots {
    pub fn resolve(at: impl Fn(&str) -> usize) -> Self {
        Self {
            on: at("ladder.on"),
            mix: at("ladder.mix"),
            drive: at("ladder.drive"),
            cutoff: at("ladder.cutoff"),
            resonance: at("ladder.resonance"),
            tracking: at("ladder.tracking"),
        }
    }

    /// Read once a block. `value` is a continuous row as heard (through the
    /// LFOs); `raw` is the switch, which no LFO moves.
    pub fn read(&self, value: impl Fn(usize) -> f32, raw: impl Fn(usize) -> f32) -> LadderParams {
        LadderParams {
            on: if raw(self.on) >= 0.5 { 1.0 } else { 0.0 },
            mix: value(self.mix),
            drive: value(self.drive),
            cutoff_hz: value(self.cutoff),
            resonance: value(self.resonance),
        }
    }
}

/// The octaves the cutoff moves per octave of the step's pitch, from the
/// stepped tracking row: off, half, full.
pub(crate) fn tracking_from(raw: f32) -> f32 {
    raw.round().clamp(0.0, 2.0) * 0.5
}

#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    s: [f32; 4],
}

impl Channel {
    /// One frame: `x` already driven, `gg` the one-pole's gain `g / (1 + g)`
    /// and `k` the loop gain. Returns the fourth pole's output.
    #[inline]
    fn tick(&mut self, x: f32, gg: f32, k: f32) -> f32 {
        let c = 1.0 - gg;
        let [s1, s2, s3, s4] = self.s;
        // y4 = g4 * u + rest, with u the first pole's input.
        let g2 = gg * gg;
        let g4 = g2 * g2;
        let rest = c * (g2 * gg * s1 + g2 * s2 + gg * s3 + s4);
        // u = soft(a - c4 * u), solved: two Newton steps from the linear
        // answer, then one closing evaluation so it is bounded whatever the
        // steps did.
        let a = x - k * rest;
        let c4 = k * g4;
        let mut u = (a / (1.0 + c4)).clamp(-1.0, 1.0);
        for _ in 0..2 {
            let t = soft(a - c4 * u);
            u -= (u - t) / (1.0 + c4 * (1.0 - t * t));
        }
        let u = soft(a - c4 * u);
        let y1 = gg * u + c * s1;
        let y2 = gg * y1 + c * s2;
        let y3 = gg * y2 + c * s3;
        let y4 = gg * y3 + c * s4;
        self.s = [2.0 * y1 - s1, 2.0 * y2 - s2, 2.0 * y3 - s3, 2.0 * y4 - s4];
        y4
    }
}

pub struct Ladder {
    sample_rate: f32,
    left: Channel,
    right: Channel,
    cutoff: OnePole,
    resonance: OnePole,
    drive: OnePole,
    mix: OnePole,
    on: Ramp,
}

impl Ladder {
    pub fn new(sample_rate: f32) -> Self {
        let d = LadderParams::default();
        let mut cutoff = OnePole::new();
        let mut resonance = OnePole::new();
        let mut drive = OnePole::new();
        let mut mix = OnePole::new();
        for s in [&mut cutoff, &mut resonance, &mut drive, &mut mix] {
            s.set_time(20.0, sample_rate);
        }
        cutoff.reset(d.cutoff_hz);
        resonance.reset(d.resonance);
        drive.reset(d.drive);
        mix.reset(d.mix);
        let mut on = Ramp::new(0.0);
        on.set_time(FADE_MS, sample_rate);
        Self {
            sample_rate,
            left: Channel::default(),
            right: Channel::default(),
            cutoff,
            resonance,
            drive,
            mix,
            on,
        }
    }

    /// Nothing ringing, and the smoothers and the switch where `p` puts
    /// them. For a voice coming back from idle.
    pub fn reset(&mut self, p: &LadderParams) {
        self.left = Channel::default();
        self.right = Channel::default();
        self.on.set(p.on);
        self.settle(p);
    }

    fn settle(&mut self, p: &LadderParams) {
        self.cutoff.reset(p.cutoff_hz);
        self.resonance.reset(p.resonance.clamp(0.0, 1.0));
        self.drive.reset(p.drive.clamp(0.0, 1.0));
        self.mix.reset(p.mix.clamp(0.0, 1.0));
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &LadderParams) -> (f32, f32) {
        let on = self.on.process(p.on);
        if on == 0.0 {
            // Off, and the fade has landed: the input comes back untouched,
            // and nothing runs. The next switch-on starts from silence.
            self.left = Channel::default();
            self.right = Channel::default();
            self.settle(p);
            return (l, r);
        }
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0)) * on;
        let max_hz = self.sample_rate * 0.45;
        let hz = self.cutoff.process(p.cutoff_hz.clamp(5.0, max_hz));
        let res = self.resonance.process(p.resonance.clamp(0.0, 1.0));
        let drive = self.drive.process(p.drive.clamp(0.0, 1.0));

        let g = (core::f32::consts::PI * hz / self.sample_rate).tan();
        let gg = g / (1.0 + g);
        let k = K_MAX * res;
        let gain = (drive * DRIVE_OCTAVES).exp2();
        let makeup = soft(DRIVE_REF) / soft(DRIVE_REF * gain);
        let comp = 1.0 + MAKEUP * k;

        let wl = self.left.tick(soft(l * gain) * makeup, gg, k) * comp;
        let wr = self.right.tick(soft(r * gain) * makeup, gg, k) * comp;
        // Resonance with a hot input can still be pushed by a wild sweep;
        // keep the state finite whatever comes in.
        if !(wl.is_finite() && wr.is_finite()) {
            self.left = Channel::default();
            self.right = Channel::default();
            return (l, r);
        }
        (l + (wl - l) * mix, r + (wr - r) * mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn params(cutoff_hz: f32, resonance: f32, drive: f32) -> LadderParams {
        LadderParams {
            on: 1.0,
            mix: 1.0,
            drive,
            cutoff_hz,
            resonance,
        }
    }

    fn sine(hz: f32, amp: f32, i: usize) -> f32 {
        (core::f32::consts::TAU * hz * i as f32 / SR).sin() * amp
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    /// A settled sine through the ladder, the left channel's second half.
    fn through(p: &LadderParams, hz: f32, amp: f32) -> Vec<f32> {
        let mut f = Ladder::new(SR);
        let out: Vec<f32> = (0..48_000)
            .map(|i| {
                let x = sine(hz, amp, i);
                f.process(x, x, p).0
            })
            .collect();
        out[24_000..].to_vec()
    }

    /// The amplitude of `hz` in `v` (Goertzel).
    fn bin(v: &[f32], hz: f32) -> f32 {
        let w = core::f32::consts::TAU * hz / SR;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, x) in v.iter().enumerate() {
            re += x * (w * i as f32).cos();
            im += x * (w * i as f32).sin();
        }
        2.0 * (re * re + im * im).sqrt() / v.len() as f32
    }

    #[test]
    fn the_soft_saturator_is_odd_bounded_and_unit_slope() {
        assert_eq!(soft(0.0), 0.0);
        assert_eq!(soft(5.0), 1.0);
        assert_eq!(soft(-5.0), -1.0);
        assert!((soft(0.001) - 0.001).abs() < 1e-6);
        let mut prev = -1.0;
        for i in -400..=400 {
            let y = soft(i as f32 * 0.01);
            assert!(
                y >= prev - 1e-6 && y.abs() <= 1.0 + 1e-6,
                "{y} after {prev}"
            );
            prev = y;
        }
    }

    #[test]
    fn it_falls_twenty_four_db_an_octave_above_the_cutoff() {
        // Low resonance, low drive, a quiet signal so the saturators are
        // linear: tones at 4x and 8x the cutoff.
        let p = params(1_000.0, 0.0, 0.0);
        let a = db(rms(&through(&p, 4_000.0, 0.02)));
        let b = db(rms(&through(&p, 8_000.0, 0.02)));
        println!("ladder slope {:.2} dB/oct", a - b);
        assert!((a - b - 24.0).abs() < 2.0, "{} dB/oct", a - b);
    }

    #[test]
    fn full_resonance_keeps_ringing_at_the_cutoff() {
        for cutoff in [220.0, 1_000.0, 4_000.0] {
            let p = params(cutoff, 1.0, 0.0);
            let mut f = Ladder::new(SR);
            let mut out = Vec::new();
            for i in 0..SR as usize * 2 {
                let x = if i == 0 { 0.5 } else { 0.0 };
                out.push(f.process(x, x, &p).0);
            }
            let tail = &out[SR as usize..];
            let peak = tail.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let crossings = tail
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count();
            let hz = crossings as f32 / (tail.len() as f32 / SR);
            println!("self-oscillation at cutoff {cutoff}: {hz:.1} Hz, peak {peak:.3}");
            assert!(peak > 0.05, "{cutoff}: died, peak {peak}");
            assert!(peak < 4.0, "{cutoff}: ran away, peak {peak}");
            assert!((hz / cutoff - 1.0).abs() < 0.03, "{cutoff}: rang at {hz}");
        }
    }

    #[test]
    fn below_full_resonance_it_rings_down() {
        let p = params(1_000.0, 0.6, 0.0);
        let mut f = Ladder::new(SR);
        let mut out = Vec::new();
        for i in 0..SR as usize {
            let x = if i == 0 { 0.5 } else { 0.0 };
            out.push(f.process(x, x, &p).0);
        }
        assert!(rms(&out[SR as usize - 4_800..]) < 1e-4);
    }

    #[test]
    fn a_fast_sweep_of_everything_stays_finite_and_bounded() {
        let mut f = Ladder::new(SR);
        let mut peak = 0.0f32;
        for i in 0..SR as usize * 4 {
            let t = i as f32 / SR;
            let p = LadderParams {
                on: if (i / 9_000) % 5 == 4 { 0.0 } else { 1.0 },
                mix: 1.0,
                drive: (t * 13.0).sin() * 0.5 + 0.5,
                cutoff_hz: 20.0 * 1_000.0f32.powf((t * 7.0).sin() * 0.5 + 0.5),
                resonance: if (i / 5_000) % 2 == 0 { 1.0 } else { 0.0 },
            };
            let x = (i as f32 * 0.37).sin() * 0.7 + (i as f32 * 0.011).sin() * 0.3;
            let (l, r) = f.process(x, -x, &p);
            assert!(l.is_finite() && r.is_finite(), "non-finite at {i}");
            peak = peak.max(l.abs()).max(r.abs());
        }
        println!("sweep peak {peak:.2}");
        assert!(peak < 8.0, "ran away: {peak}");
    }

    #[test]
    fn a_non_finite_input_does_not_lock_it_up() {
        let p = params(1_000.0, 0.9, 0.5);
        let mut f = Ladder::new(SR);
        f.process(f32::NAN, f32::NAN, &p);
        for i in 0..4_800 {
            let x = sine(220.0, 0.5, i);
            let (l, r) = f.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite(), "stuck at {i}");
        }
    }

    #[test]
    fn drive_adds_harmonics_without_the_level_jumping() {
        // A sine well under the cutoff: the third harmonic grows with drive
        // and the overall level stays within a small factor.
        let mut thirds = Vec::new();
        let mut levels = Vec::new();
        for drive in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let out = through(&params(8_000.0, 0.0, drive), 400.0, 0.5);
            thirds.push(bin(&out, 1_200.0) / bin(&out, 400.0));
            levels.push(rms(&out));
        }
        println!("third/fundamental {thirds:?}, rms {levels:?}");
        assert!(thirds.windows(2).all(|w| w[1] > w[0]), "{thirds:?}");
        assert!(thirds[4] > thirds[0] * 5.0, "{thirds:?}");
        let (lo, hi) = levels
            .iter()
            .fold((f32::MAX, 0.0f32), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(hi / lo < 1.6, "level moved {lo}..{hi}");
    }

    #[test]
    fn off_is_bit_exact_dry() {
        let on = params(700.0, 0.7, 0.6);
        let off = LadderParams { on: 0.0, ..on };
        let mut f = Ladder::new(SR);
        for i in 0..96_000 {
            let x = sine(110.0, 0.6, i) + sine(3_100.0, 0.2, i);
            let p = if (20_000..50_000).contains(&i) {
                &on
            } else {
                &off
            };
            let y = f.process(x, -x, p);
            // Fresh off: dry, to the bit. The fade out has landed by 50_500.
            if !(20_000..50_500).contains(&i) {
                assert_eq!(y, (x, -x), "sample {i}");
            }
        }
        // And a ladder that was never on says the same.
        let mut g = Ladder::new(SR);
        assert_eq!(g.process(0.3, -0.2, &off), (0.3, -0.2));
    }

    #[test]
    fn switching_does_not_click() {
        let worst = |toggle: bool| {
            let mut f = Ladder::new(SR);
            let mut prev = 0.0f32;
            let mut worst = 0.0f32;
            for i in 0..96_000 {
                let p = LadderParams {
                    on: if toggle && (i / 4_000) % 2 == 1 {
                        0.0
                    } else {
                        1.0
                    },
                    ..params(900.0, 0.4, 0.3)
                };
                let x = sine(220.0, 0.6, i) + sine(3_000.0, 0.1, i);
                let (l, _) = f.process(x, x, &p);
                if i > 4_800 {
                    worst = worst.max((l - prev).abs());
                }
                prev = l;
            }
            worst
        };
        // The steepest the signal is on its own, dry: a click would step
        // well past it.
        let dry = (0..96_000usize)
            .skip(1)
            .map(|i| {
                let x = |i| sine(220.0, 0.6, i) + sine(3_000.0, 0.1, i);
                (x(i) - x(i - 1)).abs()
            })
            .fold(0.0f32, f32::max);
        let steady = worst(false).max(dry);
        let switched = worst(true);
        println!("ladder toggle jump {switched}, steady {steady}");
        assert!(
            switched < steady * 1.2,
            "toggling stepped {switched} vs {steady}"
        );
    }

    #[test]
    fn tracking_reads_off_half_full() {
        assert_eq!(tracking_from(0.0), 0.0);
        assert_eq!(tracking_from(1.0), 0.5);
        assert_eq!(tracking_from(2.0), 1.0);
        assert_eq!(tracking_from(9.0), 1.0);
        assert_eq!(tracking_from(-1.0), 0.0);
    }
}
