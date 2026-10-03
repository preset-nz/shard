//! The drum kit: kick, snare and hat, synthesised, on the tracker's clock.
//!
//! A stop-gap until roadmap row 25 (a kit sliced from a material) and row 28
//! (several tracks). Georg, 2026-10-03: *"I need a quick way to play a drum
//! beat, base, snare, hihat at least."* The melody track is one monophonic
//! voice playing the patch, so a kick and a hat could not land on the same
//! step. The kit is beside it, not inside it:
//!
//! - **Its own grid.** A row per voice and a byte per step: zero is off,
//!   anything else is the hit's velocity. Several voices fire on one step.
//! - **The tracker's clock.** The kit does not count time itself. It reads
//!   how many sixteenths the steps have run (`StepClock::elapsed`), so it
//!   stays on the melody's grid whatever its own length, and through a tempo
//!   change. The arrangement's swing applies; nudge does not.
//! - **After the patch.** It joins the mix after the patch's fader and before
//!   the arrangement's chain, so the patch's envelope, velocity, Hold and
//!   brake never touch it, and the arrangement's effects and limiter do. In
//!   sound scaping it is silent: that mode hears the patch alone.
//!
//! No samples: each voice is a few lines of synthesis, sized at construction,
//! with nothing allocated per hit.

use crate::rng::Rng;
use crate::steps::STEPS;

/// Kick, snare, hat.
pub const VOICES: usize = 3;
pub const KICK: usize = 0;
pub const SNARE: usize = 1;
pub const HAT: usize = 2;

/// A hit at full velocity.
pub const HIT_MAX: u8 = 127;
/// A ghost note, the second click on a step in the grid.
pub const HIT_GHOST: u8 = 48;

/// The kit's grid. Not parameters: like the melody's steps, it belongs to
/// the tracker and crosses through `StepBank`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KitPattern {
    /// Whether the kit plays. Its clock runs either way, so switching it on
    /// mid-bar lands on the grid.
    pub on: bool,
    /// How many steps the kit's loop runs, 1 to `STEPS`, as a track's does.
    pub length: u32,
    /// Per voice, a byte a step: zero is no hit, else its velocity up to
    /// `HIT_MAX`.
    pub hits: [[u8; STEPS]; VOICES],
}

impl Default for KitPattern {
    /// Off, with a beat in it, so switching on does something: kick on one
    /// and the and of three, snare on two and four, eighth-note hats with the
    /// offbeats softer.
    fn default() -> Self {
        let mut hits = [[0; STEPS]; VOICES];
        for k in [0, 10] {
            hits[KICK][k] = HIT_MAX;
        }
        for k in [4, 12] {
            hits[SNARE][k] = HIT_MAX;
        }
        for k in (0..16).step_by(2) {
            hits[HAT][k] = if k % 4 == 0 { HIT_MAX } else { 80 };
        }
        Self {
            on: false,
            length: 16,
            hits,
        }
    }
}

impl KitPattern {
    /// Voice `v`'s hit on step `k`, 0 to 1.
    #[inline]
    pub fn hit(&self, v: usize, k: u32) -> f32 {
        f32::from(self.hits[v][k as usize % STEPS].min(HIT_MAX)) / f32::from(HIT_MAX)
    }
}

/// Which step of its own loop the kit is in. The clock itself is the
/// tracker's; this only notices when the kit enters a new step.
#[derive(Debug, Clone, Copy, Default)]
pub struct KitClock {
    step: Option<u32>,
}

impl KitClock {
    /// Back to before step one, for when the steps start or stop.
    pub fn reset(&mut self) {
        self.step = None;
    }

    /// The step the kit is in, for the UI. None before the first frame.
    pub fn current(&self) -> Option<u32> {
        self.step
    }

    /// `elapsed` sixteenths since the steps started, from the tracker's clock.
    /// The step, when the kit enters one on this frame. Swing delays the even
    /// steps (the second, the fourth) by that share of a step, as it does the
    /// melody's.
    #[inline]
    pub fn tick(&mut self, elapsed: f64, p: &KitPattern, swing: f32) -> Option<u32> {
        let length = p.length.clamp(1, STEPS as u32);
        let at = elapsed.rem_euclid(f64::from(length));
        let mut k = (at as u32).min(length - 1);
        if k % 2 == 1 && at - f64::from(k) < f64::from(swing.clamp(0.0, 0.5)) {
            k -= 1;
        }
        let entered = self.step != Some(k);
        self.step = Some(k);
        entered.then_some(k)
    }
}

/// The kit's sound, from the arrangement's table once a block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KitParams {
    /// Semitones, every voice together.
    pub tune: f32,
    /// A factor on every voice's decay: 1 is as designed.
    pub decay: f32,
}

impl Default for KitParams {
    fn default() -> Self {
        Self {
            tune: 0.0,
            decay: 1.0,
        }
    }
}

/// Below this a voice is done and plays an exact zero: −80 dB.
const SILENT: f32 = 1e-4;
/// A voice cut by its own next hit fades out over this, so the cut does not
/// click. The new hit starts at once.
const CUT_MS: f32 = 3.0;

/// One drum sounding. Everything per hit is worked out at the hit, so a
/// sample costs a few multiplies.
#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    sounding: bool,
    /// The level, which falls by `fall` a sample from the hit's velocity.
    amp: f32,
    fall: f32,
    /// The tonal part's level and its fall, for the snare's body.
    tone: f32,
    tone_fall: f32,
    /// The pitch above the resting one, falling by `bend_fall`, in Hz.
    bend: f32,
    bend_fall: f32,
    /// Resting pitch, in cycles a sample.
    rest: f32,
    /// Cycles, 0 to 1.
    phase: f32,
    /// One-pole lowpass states, for the noise's highpass.
    lp1: f32,
    lp2: f32,
}

/// A voice's next sample, given a noise sample.
type Render = fn(&mut Voice, noise: f32, hp: f32) -> f32;

fn kick(v: &mut Voice, _noise: f32, _hp: f32) -> f32 {
    let out = (v.phase * core::f32::consts::TAU).sin() * v.amp;
    v.phase += v.rest + v.bend;
    v.phase -= v.phase.floor();
    v.bend *= v.bend_fall;
    out
}

fn snare(v: &mut Voice, noise: f32, hp: f32) -> f32 {
    let body = (v.phase * core::f32::consts::TAU).sin() * v.tone;
    v.phase += v.rest;
    v.phase -= v.phase.floor();
    v.tone *= v.tone_fall;
    v.lp1 += hp * (noise - v.lp1);
    (body + (noise - v.lp1) * 0.8) * v.amp
}

fn hat(v: &mut Voice, noise: f32, hp: f32) -> f32 {
    // Two highpasses, so only the fizz is left.
    v.lp1 += hp * (noise - v.lp1);
    let once = noise - v.lp1;
    v.lp2 += hp * (once - v.lp2);
    (once - v.lp2) * v.amp
}

const RENDER: [Render; VOICES] = [kick, snare, hat];

pub struct Kit {
    sample_rate: f32,
    voices: [Voice; VOICES],
    /// What each voice was when its next hit cut it, fading out.
    tails: [Voice; VOICES],
    tail_left: [f32; VOICES],
    noise: Rng,
    /// The one-pole coefficients the snare's and the hat's noise are
    /// highpassed with.
    snare_hp: f32,
    hat_hp: f32,
}

/// A one-pole lowpass coefficient for `hz`.
fn one_pole(hz: f32, sample_rate: f32) -> f32 {
    1.0 - (-core::f32::consts::TAU * hz / sample_rate).exp()
}

/// The factor a level falls by each sample to lose 60 dB in `seconds`.
fn fall_over(seconds: f32, sample_rate: f32) -> f32 {
    (-6.9 / (seconds.max(1e-3) * sample_rate)).exp()
}

impl Kit {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            voices: [Voice::default(); VOICES],
            tails: [Voice::default(); VOICES],
            tail_left: [0.0; VOICES],
            noise: Rng::new(0x005E_EDD2),
            snare_hp: one_pole(1_200.0, sample_rate),
            hat_hp: one_pole(7_000.0, sample_rate),
        }
    }

    /// Nothing sounding, as if no hit had ever happened.
    pub fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
        self.tails = [Voice::default(); VOICES];
        self.tail_left = [0.0; VOICES];
    }

    /// Whether anything is still sounding.
    pub fn sounding(&self) -> bool {
        self.voices.iter().any(|v| v.sounding) || self.tail_left.iter().any(|&t| t > 0.0)
    }

    /// Strike voice `v` at `velocity`, 0 to 1. A voice still sounding hands
    /// itself to its tail, which fades out under the new hit.
    pub fn hit(&mut self, v: usize, velocity: f32, p: &KitParams) {
        if velocity <= 0.0 {
            return;
        }
        if self.voices[v].sounding {
            self.tails[v] = self.voices[v];
            self.tail_left[v] = CUT_MS * 0.001 * self.sample_rate;
        }
        let sr = self.sample_rate;
        let tune = (p.tune / 12.0).exp2();
        let decay = p.decay.clamp(0.1, 10.0);
        let voice = &mut self.voices[v];
        *voice = Voice {
            sounding: true,
            amp: velocity,
            ..Voice::default()
        };
        match v {
            KICK => {
                voice.rest = 48.0 * tune / sr;
                voice.bend = 110.0 * tune / sr;
                voice.bend_fall = fall_over(0.12, sr);
                voice.fall = fall_over(0.45 * decay, sr);
                voice.amp *= 0.9;
            }
            SNARE => {
                voice.rest = 185.0 * tune / sr;
                voice.tone = 0.6;
                voice.tone_fall = fall_over(0.12 * decay, sr);
                voice.fall = fall_over(0.25 * decay, sr);
                voice.amp *= 0.6;
            }
            _ => {
                voice.fall = fall_over(0.09 * decay, sr);
                voice.amp *= 0.35;
            }
        }
    }

    /// One frame, mono. Voices are summed; a voice that has fallen below
    /// `SILENT` stops and plays an exact zero.
    #[inline]
    pub fn process(&mut self) -> f32 {
        if !self.sounding() {
            return 0.0;
        }
        let noise = self.noise.next_bipolar();
        let hp = [0.0, self.snare_hp, self.hat_hp];
        let cut = 1.0 / (CUT_MS * 0.001 * self.sample_rate);
        let mut out = 0.0;
        for v in 0..VOICES {
            if self.tail_left[v] > 0.0 {
                let gain = self.tail_left[v] * cut;
                out += RENDER[v](&mut self.tails[v], noise, hp[v]) * gain;
                self.tail_left[v] = (self.tail_left[v] - 1.0).max(0.0);
            }
            let voice = &mut self.voices[v];
            if !voice.sounding {
                continue;
            }
            out += RENDER[v](voice, noise, hp[v]);
            voice.amp *= voice.fall;
            if voice.amp < SILENT {
                voice.sounding = false;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn render(kit: &mut Kit, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| kit.process()).collect()
    }

    #[test]
    fn every_voice_sounds_then_falls_to_an_exact_zero() {
        for v in 0..VOICES {
            let mut kit = Kit::new(SR);
            kit.hit(v, 1.0, &KitParams::default());
            let out = render(&mut kit, SR as usize * 3);
            let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.1, "voice {v} peaks at {peak}");
            assert!(peak <= 1.0, "voice {v} peaks at {peak}");
            assert!(!kit.sounding(), "voice {v} still sounding after 3 s");
            assert_eq!(*out.last().unwrap(), 0.0, "voice {v}");
        }
    }

    #[test]
    fn nothing_hit_is_silence() {
        let mut kit = Kit::new(SR);
        assert!(render(&mut kit, 4_800).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn velocity_scales_a_hit() {
        let peak = |vel: f32| {
            let mut kit = Kit::new(SR);
            kit.hit(KICK, vel, &KitParams::default());
            render(&mut kit, 4_800)
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let (soft, full) = (peak(0.5), peak(1.0));
        assert!((soft / full - 0.5).abs() < 0.01, "{soft} against {full}");
    }

    #[test]
    fn a_voice_hit_again_mid_decay_does_not_click() {
        // The kick is a sine, so a cut with no fade jumps by however far the
        // old one was from zero. Compare the biggest frame-to-frame step at
        // the second hit with the biggest within a single hit.
        let p = KitParams::default();
        let mut kit = Kit::new(SR);
        kit.hit(KICK, 1.0, &p);
        let first = render(&mut kit, 2_000);
        let steady = first
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        let mut kit = Kit::new(SR);
        kit.hit(KICK, 1.0, &p);
        let mut out = render(&mut kit, 1_013);
        kit.hit(KICK, 1.0, &p);
        out.extend(render(&mut kit, 1_000));
        let seam = out[1_008..1_030]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(seam <= steady * 1.5, "seam {seam} against {steady}");
    }

    #[test]
    fn the_kit_clock_enters_each_step_once_and_follows_the_tracker() {
        let p = KitPattern::default();
        let mut clock = KitClock::default();
        // Sixteenths elapsed, a tenth of a step at a time, over two loops.
        let entered: Vec<u32> = (0..320)
            .filter_map(|i| clock.tick(f64::from(i) * 0.1, &p, 0.0))
            .collect();
        let expected: Vec<u32> = (0..16).chain(0..16).collect();
        assert_eq!(entered, expected);
    }

    #[test]
    fn a_short_kit_wraps_inside_a_longer_track() {
        let p = KitPattern {
            length: 4,
            ..KitPattern::default()
        };
        let mut clock = KitClock::default();
        let entered: Vec<u32> = (0..16)
            .filter_map(|i| clock.tick(f64::from(i), &p, 0.0))
            .collect();
        assert_eq!(entered, [0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3]);
    }

    #[test]
    fn swing_delays_the_kits_even_steps() {
        let p = KitPattern::default();
        let mut clock = KitClock::default();
        let onsets: Vec<f64> = (0..400)
            .filter_map(|i| {
                let at = f64::from(i) * 0.01;
                clock.tick(at, &p, 0.25).map(|_| at)
            })
            .collect();
        assert_eq!(onsets.len(), 4);
        for (k, at) in onsets.iter().enumerate() {
            let want = k as f64 + if k % 2 == 1 { 0.25 } else { 0.0 };
            assert!((at - want).abs() < 0.011, "step {k} at {at}, want {want}");
        }
    }
}
