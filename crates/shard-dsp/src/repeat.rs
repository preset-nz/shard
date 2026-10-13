//! Repeat.
//!
//! The buffer repeat: grab the last stretch of sound and loop it, the
//! Transporter of the glitch videos (`features/48-repeat`). A ring of the
//! last few seconds is always recording. When Hold rises above half, the most
//! recent Length of it becomes a loop, and the loop plays until Hold falls
//! again. Each rise grabs afresh, so a Square or Random LFO on Hold stutters
//! by itself.
//!
//! **Hold is a gate with hysteresis.** It loops from 0.5 up and lets go only
//! below 0.45, so an LFO or a hand resting near half does not chatter between
//! grabbing and letting go. It is read through the LFOs, once a block, so a
//! grab lands on a block boundary.
//!
//! **While Hold is down the effect only listens.** Its output is the dry,
//! bit for bit, at any Mix: Mix only matters while a loop plays, where it
//! crossfades the dry against the loop like every effect's (at full Mix only
//! the loop is heard).
//!
//! **Length** is how much is grabbed, 5 ms (a buzz) to 2 s (a loop). It is
//! smoothed, and moving it while holding moves the loop's end, never its
//! start: the loop grows into the sound that came after the grab, which is
//! why the ring keeps recording while it holds. It can only grow as far as
//! what has been recorded since the grab.
//!
//! **Speed** is the loop's playback rate, tape-style, ±24 semitones: an
//! octave up plays twice as fast and repeats twice as often. Read between
//! samples through a four-point Hermite curve, as the chorus does, so the
//! top end does not wobble as the read point passes between samples.
//!
//! **Reverse** plays the loop backwards, starting from its newest sample, so
//! a reversed grab starts where the dry was. Switching it while a loop plays
//! turns the read point round where it stands, which is continuous.
//!
//! No clicks: the grab and the release crossfade dry and loop over
//! `FADE_MS`, and each loop join crossfades the end into the start over the
//! same time (or a quarter of the loop, if shorter). A re-grab while a loop
//! still sounds fades the old loop out under the new one. The join reads a
//! little of the audio just before the loop's start (forward) or carries the
//! outgoing read point a little past it (reverse), so the period is exactly
//! Length over the speed ratio and every sample read is recorded audio.
//!
//! The writer stops only if it would overwrite the playing loop, which takes
//! a hold of over two seconds plus Length. Past that the ring is frozen until
//! release; a grab soon after such a hold can contain the seam where
//! recording stopped.

use crate::smooth::{OnePole, Ramp};

/// `repeat.length`'s range, in milliseconds.
pub const LENGTH_MIN_MS: f32 = 5.0;
pub const LENGTH_MAX_MS: f32 = 2_000.0;
/// `repeat.speed`'s reach either way, in semitones. Two octaves: past that a
/// loop is a pitch, not a repeat.
pub const MAX_SPEED_ST: f32 = 24.0;
/// Hold loops from here up...
pub const HOLD_ON: f32 = 0.5;
/// ...and lets go below here. The gap is the hysteresis.
pub const HOLD_OFF: f32 = 0.45;
/// The grab, the release and each loop join crossfade over this.
pub const FADE_MS: f32 = 4.0;
/// How many read points can sound at once: the loop, the one fading out at a
/// join, and the loops of re-grabs fading out under it.
const HEADS: usize = 6;
/// The writer stops this far short of the loop's start, which leaves the
/// pre-roll and the reverse overshoot of a join intact.
const GUARD_MS: f32 = 40.0;
/// Samples between the newest recorded sample and the loop's end at a grab:
/// room for the interpolator's look-ahead.
const LEAD: u64 = 4;

/// `smoother` towards `target`, landing on it exactly once within `near`.
/// In f32 a one-pole stalls short of its target by up to half an ulp over
/// one minus its coefficient, which for a 2 s Length is several frames: the
/// loop would never settle on the length asked for, and a steady loop's
/// period would drift. The last step is a fraction of a millisecond of loop
/// end or a thousandth of a semitone, neither of which moves a read point.
#[inline]
fn land(smoother: &mut OnePole, target: f32, near: f32) -> f32 {
    let v = smoother.process(target);
    if (v - target).abs() <= near {
        smoother.reset(target);
        target
    } else {
        v
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RepeatParams {
    /// 0 to 1. Above `HOLD_ON` it loops; it lets go below `HOLD_OFF`.
    pub hold: f32,
    /// How much is grabbed and looped, `LENGTH_MIN_MS` to `LENGTH_MAX_MS`.
    pub length_ms: f32,
    /// Playback rate in semitones, ±`MAX_SPEED_ST`.
    pub speed_st: f32,
    /// Plays the loop backwards.
    pub reverse: bool,
    /// Dry to the loop, 0 to 1. Only heard while holding.
    pub mix: f32,
}

impl Default for RepeatParams {
    fn default() -> Self {
        Self {
            hold: 0.0,
            length_ms: 125.0,
            speed_st: 0.0,
            reverse: false,
            mix: 0.0,
        }
    }
}

/// One read point in the ring.
#[derive(Debug, Clone, Copy)]
struct Head {
    /// The loop's start, as an absolute frame count.
    start: u64,
    /// Where it reads, in frames from `start`.
    x: f64,
    /// 1 forwards, -1 backwards.
    dir: f64,
    gain: f32,
    target: f32,
    /// Gain moved per frame.
    step: f32,
}

impl Head {
    const IDLE: Head = Head {
        start: 0,
        x: 0.0,
        dir: 1.0,
        gain: 0.0,
        target: 0.0,
        step: 1.0,
    };

    fn sounding(&self) -> bool {
        self.gain > 0.0 || self.target > 0.0
    }
}

pub struct Repeat {
    /// Interleaved stereo frames, sized once.
    ring: Vec<f32>,
    frames: u64,
    /// Frames written since construction, starting high enough that a loop
    /// start a few seconds back never goes below zero.
    now: u64,
    sample_rate: f32,
    heads: [Head; HEADS],
    /// The playing loop's start, which the writer must not reach.
    protect: Option<u64>,
    /// The gate, after hysteresis.
    holding: bool,
    /// Whether a loop plays (a grab happened and Hold has not let go).
    looping: bool,
    /// Dry to loop, ramped over `FADE_MS`.
    hold_fade: Ramp,
    fade_frames: f32,
    guard: u64,
    length: OnePole,
    speed: OnePole,
    mix: OnePole,
}

impl Repeat {
    pub fn new(sample_rate: f32) -> Self {
        let guard = (GUARD_MS * 0.001 * sample_rate).ceil() as u64;
        // Two seconds a loop can reach past its start, two seconds of history
        // before it, and the guard.
        let frames = (2.0 * LENGTH_MAX_MS * 0.001 * sample_rate).ceil() as u64 + guard + 16;
        let d = RepeatParams::default();
        let smoother = |ms: f32, v: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        let mut hold_fade = Ramp::new(0.0);
        hold_fade.set_time(FADE_MS, sample_rate);
        Self {
            ring: vec![0.0; frames as usize * 2],
            frames,
            now: frames * 2,
            sample_rate,
            heads: [Head::IDLE; HEADS],
            protect: None,
            holding: false,
            looping: false,
            hold_fade,
            fade_frames: FADE_MS * 0.001 * sample_rate,
            guard,
            // Length moves the loop's end; slower than the house 20 ms so a
            // turn of the knob sweeps the loop rather than jumping it.
            length: smoother(50.0, d.length_ms),
            speed: smoother(20.0, d.speed_st),
            mix: smoother(20.0, d.mix),
        }
    }

    /// Forgets all recorded audio and any loop. The smoothers stay where
    /// they are, and so does the gate: with Hold already up, the next loop
    /// waits for a fresh rise rather than grabbing the silence.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.heads = [Head::IDLE; HEADS];
        self.protect = None;
        self.looping = false;
        self.hold_fade.set(0.0);
    }

    /// Both channels at `pos` frames (absolute, fractional), Hermite.
    #[inline]
    fn read(&self, pos: f64) -> (f32, f32) {
        let whole = pos.floor();
        let t = (pos - whole) as f32;
        let i = whole as u64;
        let n = self.frames;
        let at = |k: u64, ch: usize| self.ring[((k % n) as usize) * 2 + ch];
        let mut out = [0.0; 2];
        for (ch, o) in out.iter_mut().enumerate() {
            let (xm1, x0, x1, x2) = (at(i - 1, ch), at(i, ch), at(i + 1, ch), at(i + 2, ch));
            let c1 = 0.5 * (x1 - xm1);
            let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
            let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
            *o = ((c3 * t + c2) * t + c1) * t + x0;
        }
        (out[0], out[1])
    }

    /// A free head, or the quietest if every one sounds.
    fn free_head(&mut self) -> usize {
        if let Some(i) = self.heads.iter().position(|h| !h.sounding()) {
            return i;
        }
        let mut quietest = 0;
        for (i, h) in self.heads.iter().enumerate() {
            if h.gain < self.heads[quietest].gain {
                quietest = i;
            }
        }
        quietest
    }

    /// The last `len` frames become the loop. Anything still sounding fades
    /// out under it.
    fn grab(&mut self, len: f32, dir: f64) {
        let start = self.now - len.ceil() as u64 - LEAD;
        let mut sounding = false;
        let step = 1.0 / self.fade_frames.max(1.0);
        for h in self.heads.iter_mut().filter(|h| h.sounding()) {
            sounding = true;
            h.target = 0.0;
            h.step = step;
        }
        let i = self.free_head();
        self.heads[i] = Head {
            start,
            x: if dir > 0.0 { 0.0 } else { len as f64 },
            dir,
            // Fading in from the dry needs no fade of its own; under an old
            // loop it crossfades.
            gain: if sounding { 0.0 } else { 1.0 },
            target: 1.0,
            step,
        };
        self.protect = Some(start);
        self.looping = true;
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &RepeatParams) -> (f32, f32) {
        let len_ms = land(
            &mut self.length,
            p.length_ms.clamp(LENGTH_MIN_MS, LENGTH_MAX_MS),
            1e-4 * LENGTH_MAX_MS,
        );
        let speed_st = land(
            &mut self.speed,
            p.speed_st.clamp(-MAX_SPEED_ST, MAX_SPEED_ST),
            1e-3,
        );
        // Lands on its target, zero above all: off has to be bit-exact. One
        // too, so a full mix while holding leaves nothing of the dry.
        let mix = land(&mut self.mix, p.mix.clamp(0.0, 1.0), 1e-4);

        // Record, unless that would reach the playing loop.
        let may_write = match self.protect {
            Some(start) => self.now - start < self.frames - self.guard,
            None => true,
        };
        if may_write {
            let at = (self.now % self.frames) as usize * 2;
            self.ring[at] = l;
            self.ring[at + 1] = r;
        }
        self.now += 1;

        // Divided last, so a whole number of milliseconds at a whole rate
        // is a whole number of frames.
        let len = len_ms * self.sample_rate / 1000.0;
        let dir = if p.reverse { -1.0 } else { 1.0 };
        if !self.holding && p.hold >= HOLD_ON {
            self.holding = true;
            self.grab(len, dir);
        } else if self.holding && p.hold < HOLD_OFF {
            self.holding = false;
            self.looping = false;
        }

        let h = self.hold_fade.process(if self.looping { 1.0 } else { 0.0 });
        if h == 0.0 && !self.looping {
            if self.protect.is_some() {
                self.heads = [Head::IDLE; HEADS];
                self.protect = None;
            }
            return (l, r);
        }

        let ratio = (speed_st / 12.0).exp2() as f64;
        // What has been recorded past a loop's start bounds how far it can
        // reach, less the interpolator's look-ahead. The writer stops
        // `frames - guard` past the playing loop's start.
        let (now, last) = (self.now, self.frames - self.guard);
        let reach = |start: u64| (now.min(start + last) - start) as f64 - 3.0;
        let lowest = -((self.guard / 2) as f64);
        let mut loop_out = (0.0f32, 0.0f32);
        let mut join: Option<(u64, f64, f32)> = None;
        for i in 0..HEADS {
            let head = self.heads[i];
            if !head.sounding() {
                continue;
            }
            let reach = reach(head.start);
            let span = (len as f64).min(reach).max(2.0);
            let x = head.x.clamp(lowest, reach);
            let (a, b) = self.read(head.start as f64 + x);
            loop_out.0 += head.gain * a;
            loop_out.1 += head.gain * b;

            let head = &mut self.heads[i];
            head.gain = if (head.target - head.gain).abs() <= head.step {
                head.target
            } else {
                head.gain + head.step.copysign(head.target - head.gain)
            };
            if head.target == 0.0 {
                // Fading out: carries on as it was going.
                head.x += head.dir * ratio;
                continue;
            }
            head.dir = dir;
            head.x += dir * ratio;
            // The join fades over `FADE_MS`, or a quarter of the loop.
            let fade = (self.fade_frames as f64).min(0.25 * span / ratio).max(1.0);
            let lead = fade * ratio;
            let wrap = if dir > 0.0 {
                // Forwards: the next pass starts `lead` before the start, so
                // it reaches the start exactly as this one reaches the end.
                (head.x >= span - lead).then(|| {
                    let k = ((head.x - (span - lead)) / span).floor() + 1.0;
                    head.x - k * span
                })
            } else if head.x <= 0.0 {
                // Backwards: this pass runs on past the start while the next
                // comes in from the end.
                Some(head.x + span)
            } else if head.x > span + 2.0 {
                // The loop shrank under a backward read point.
                let k = ((head.x - span) / span).ceil();
                Some(head.x - k * span)
            } else {
                None
            };
            if let Some(x) = wrap {
                head.target = 0.0;
                head.step = 1.0 / fade as f32;
                join = Some((head.start, x, head.step));
            }
        }
        if let Some((start, x, step)) = join {
            let i = self.free_head();
            self.heads[i] = Head {
                start,
                x,
                dir,
                gain: 0.0,
                target: 1.0,
                step,
            };
        }

        let wet_l = l * (1.0 - h) + loop_out.0 * h;
        let wet_r = r * (1.0 - h) + loop_out.1 * h;
        if mix == 0.0 {
            return (l, r);
        }
        (l * (1.0 - mix) + wet_l * mix, r * (1.0 - mix) + wet_r * mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn tone(i: usize, hz: f32, sr: f32) -> f32 {
        (core::f32::consts::TAU * hz * i as f32 / sr).sin() * 0.5
    }

    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn held(length_ms: f32, speed_st: f32, reverse: bool) -> RepeatParams {
        RepeatParams {
            hold: 1.0,
            length_ms,
            speed_st,
            reverse,
            mix: 1.0,
        }
    }

    fn open(p: RepeatParams) -> RepeatParams {
        RepeatParams { hold: 0.0, ..p }
    }

    /// Settles every smoother on `p` with Hold down, feeding `input`.
    fn settle(x: &mut Repeat, p: &RepeatParams, frames: usize, input: impl Fn(usize) -> f32) {
        for i in 0..frames {
            let v = input(i);
            x.process(v, v, &open(*p));
        }
    }

    #[test]
    fn mix_at_zero_is_transparent_even_while_holding() {
        let mut x = Repeat::new(SR);
        let p = RepeatParams {
            mix: 0.0,
            ..held(50.0, 7.0, true)
        };
        for i in 0..48_000 {
            let v = tone(i, 220.0, SR);
            assert_eq!(x.process(v, -v, &p), (v, -v));
        }
    }

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        let mut x = Repeat::new(SR);
        let on = held(80.0, 0.0, false);
        let off = RepeatParams { mix: 0.0, ..on };
        for i in 0..4_800 {
            let v = tone(i, 220.0, SR);
            x.process(v, v, &on);
        }
        for i in 0..48_000 {
            let v = tone(i, 220.0, SR);
            x.process(v, v, &off);
        }
        for _ in 0..1_000 {
            assert_eq!(x.process(0.5, -0.25, &off), (0.5, -0.25));
        }
    }

    #[test]
    fn hold_down_passes_the_dry_bit_for_bit_at_full_mix() {
        // The choice: while not holding, the effect only listens, so Mix
        // changes nothing until a loop plays.
        let mut x = Repeat::new(SR);
        let p = open(held(125.0, 0.0, false));
        for i in 0..48_000 {
            let v = tone(i, 220.0, SR);
            assert_eq!(x.process(v, 0.3 * v, &p), (v, 0.3 * v));
        }
    }

    #[test]
    fn silence_in_silence_out() {
        for reverse in [false, true] {
            let mut x = Repeat::new(SR);
            let p = held(5.0, MAX_SPEED_ST, reverse);
            for i in 0..20_000 {
                // Hold toggled, so grabs, joins and releases all run.
                let q = if (i / 1_000) % 2 == 0 { p } else { open(p) };
                assert_eq!(x.process(0.0, 0.0, &q), (0.0, 0.0));
            }
        }
    }

    #[test]
    fn holding_repeats_the_grab_every_length_over_the_speed() {
        // 100 ms is 4,800 frames; an octave up, 2,400.
        for (speed, period) in [(0.0, 4_800), (12.0, 2_400), (-12.0, 9_600)] {
            for reverse in [false, true] {
                let mut x = Repeat::new(SR);
                let p = held(100.0, speed, reverse);
                let mut seed = 0x9E37_79B9u32;
                let input: Vec<f32> = (0..48_000).map(|_| noise(&mut seed) * 0.5).collect();
                // Long enough for Length's smoother to land exactly.
                settle(&mut x, &p, 96_000, |i| input[i % 48_000]);
                let out: Vec<f32> = (0..period * 4)
                    .map(|i| x.process(input[i % 48_000], 0.0, &p).0)
                    .collect();
                // After the grab's fade, each pass is the last.
                let mut worst = 0.0f32;
                for i in 400..(out.len() - period) {
                    worst = worst.max((out[i] - out[i + period]).abs());
                }
                assert!(
                    worst < 1e-3,
                    "speed {speed} reverse {reverse}: off by {worst} a period on"
                );
                // And it is the grab, not the input carrying on.
                let fresh: f32 = (0..period)
                    .map(|i| (out[period + i] - input[period + i]).abs())
                    .sum();
                assert!(fresh > 1.0, "speed {speed}: the loop is just the input");
            }
        }
    }

    #[test]
    fn at_unit_speed_the_loop_is_the_last_length_of_input() {
        let mut x = Repeat::new(SR);
        let p = held(100.0, 0.0, false);
        let mut seed = 0x1234_5678u32;
        let input: Vec<f32> = (0..110_000).map(|_| noise(&mut seed) * 0.5).collect();
        settle(&mut x, &p, 100_000, |i| input[i]);
        let out: Vec<f32> = (100_000..110_000)
            .map(|i| x.process(input[i], 0.0, &p).0)
            .collect();
        // The grab happens on frame 100,000, which was just recorded; the
        // loop starts LEAD + 4,800 frames back. Compare past the first join,
        // away from any fade.
        let start = 100_001 - 4_800 - LEAD as usize;
        for k in 400..4_000 {
            let want = input[start + k];
            assert!(
                (out[4_800 + k] - want).abs() < 1e-5,
                "frame {k}: {} against {want}",
                out[4_800 + k]
            );
        }
    }

    #[test]
    fn reverse_plays_the_grab_backwards() {
        let mut x = Repeat::new(SR);
        let p = held(100.0, 0.0, true);
        // A rising ramp: forwards it climbs, backwards it falls.
        let ramp = |i: usize| (i % 48_000) as f32 / 48_000.0 - 0.5;
        settle(&mut x, &p, 30_000, ramp);
        let out: Vec<f32> = (30_000..40_000)
            .map(|i| x.process(ramp(i), 0.0, &p).0)
            .collect();
        let falling = out
            .windows(2)
            .skip(400)
            .take(4_000)
            .filter(|w| w[1] < w[0])
            .count();
        assert!(falling > 3_900, "only {falling} of 4,000 steps fell");
        // And it starts from the newest audio, which the dry was at.
        assert!((out[0] - ramp(30_000)).abs() < 0.01, "{}", out[0]);
    }

    #[test]
    fn a_fresh_rise_grabs_new_audio() {
        let mut x = Repeat::new(SR);
        let p = held(50.0, 0.0, false);
        let level = |i: usize| if i < 20_000 { 0.4 } else { -0.3 };
        let mut out = Vec::new();
        for i in 0..40_000 {
            // Held 10,000..15,000 and again from 30,000.
            let q = if (10_000..15_000).contains(&i) || i >= 30_000 {
                p
            } else {
                open(p)
            };
            out.push(x.process(level(i), 0.0, &q).0);
        }
        for (i, &v) in out.iter().enumerate().skip(10_500).take(4_000) {
            assert!((v - 0.4).abs() < 1e-4, "first loop at {i}: {v}");
        }
        // The input has changed under the first loop's memory...
        for (i, &v) in out.iter().enumerate().skip(30_500).take(9_000) {
            assert!((v + 0.3).abs() < 1e-4, "second loop at {i}: {v}");
        }
    }

    #[test]
    fn hold_has_hysteresis() {
        let mut x = Repeat::new(SR);
        let p = held(50.0, 0.0, false);
        let level = |i: usize| if i < 10_000 { 0.4 } else { -0.3 };
        for i in 0..20_000 {
            // Up to 0.6 at 5,000, then wandering just under half: it holds.
            let hold = if i < 5_000 {
                0.0
            } else if i < 6_000 {
                0.6
            } else {
                0.47 + 0.02 * ((i / 64) % 2) as f32
            };
            let out = x.process(level(i), 0.0, &RepeatParams { hold, ..p }).0;
            if i > 6_000 {
                assert!((out - 0.4).abs() < 1e-4, "let go at {i}: {out}");
            }
        }
    }

    #[test]
    fn the_loop_is_as_loud_as_the_input() {
        for (length, speed) in [(5.0, 0.0), (37.0, 5.0), (125.0, 0.0), (900.0, -7.0)] {
            let mut x = Repeat::new(SR);
            let p = held(length, speed, false);
            let mut seed = 0xDEAD_BEEFu32;
            let mut tilt = 0.0f32;
            let mut sig = |_: usize| {
                tilt += 0.6 * (noise(&mut seed) * 0.5 - tilt);
                tilt
            };
            let input: Vec<f32> = (0..200_000).map(&mut sig).collect();
            settle(&mut x, &p, 100_000, |i| input[i]);
            let (mut dry, mut wet) = (0.0f64, 0.0f64);
            for &v in &input[100_000..] {
                let o = x.process(v, v, &p).0;
                dry += (v * v) as f64;
                wet += (o * o) as f64;
            }
            let db = 10.0 * (wet / dry).log10();
            assert!(db.abs() < 1.5, "{length} ms at {speed}: {db:.2} dB");
        }
    }

    /// The largest sample-to-sample step of a 55 Hz tone through `script`.
    fn worst_step(frames: usize, script: impl Fn(usize) -> RepeatParams) -> f32 {
        let mut x = Repeat::new(SR);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for i in 0..frames {
            let v = tone(i, 55.0, SR);
            let (l, _) = x.process(v, v, &script(i));
            if i > 0 {
                worst = worst.max((l - prev).abs());
            }
            prev = l;
        }
        worst
    }

    #[test]
    fn grabbing_releasing_and_joining_do_not_click() {
        // A 55 Hz tone moves at most 0.0036 a frame, and at four times speed
        // 0.0144. A 4 ms fade between two unrelated points of it adds at most
        // 1/192 a frame; a hard cut would step by up to 1.
        let dry = worst_step(10_000, |_| open(held(100.0, 0.0, false)));
        // Lengths that are no whole number of the tone's periods, so the
        // joins are discontinuous unless they are faded.
        for length in [5.0, 13.0, 77.0, 333.0] {
            for speed in [-24.0, 0.0, 24.0] {
                for reverse in [false, true] {
                    let worst = worst_step(96_000, |i| {
                        // Hold toggled at block-sized irregular times.
                        let on = (i / 128) % 37 < 23;
                        let p = held(length, speed, reverse);
                        if on {
                            p
                        } else {
                            open(p)
                        }
                    });
                    // The tone sped up, plus the largest jump a fade can
                    // spread (the tone's full swing) over the shortest fade:
                    // a quarter of the loop's period, or `FADE_MS`.
                    let ratio = (speed / 12.0f32).exp2();
                    let period = length * 0.001 * SR / ratio;
                    let fade = (FADE_MS * 0.001 * SR).min(0.25 * period).max(1.0);
                    let bound = dry * ratio.max(1.0) + 1.0 / fade + 0.006;
                    assert!(
                        worst < bound,
                        "{length} ms, {speed} st, reverse {reverse}: stepped {worst} (bound {bound})"
                    );
                }
            }
        }
    }

    #[test]
    fn hold_flipped_every_block_does_not_click() {
        // A fast Square or Random LFO on Hold: grabbed, let go and grabbed
        // again inside a single fade, with old loops fading under new ones.
        for block in [64, 128] {
            for length in [5.0, 13.0, 333.0] {
                for speed in [0.0, 24.0] {
                    let worst = worst_step(48_000, |i| {
                        let p = held(length, speed, false);
                        if (i / block) % 2 == 0 {
                            p
                        } else {
                            open(p)
                        }
                    });
                    let dry = 0.0036f32;
                    let ratio = (speed / 12.0f32).exp2();
                    let period = length * 0.001 * SR / ratio;
                    let fade = (FADE_MS * 0.001 * SR).min(0.25 * period).max(1.0);
                    let bound = dry * ratio.max(1.0) + 1.0 / fade + 0.006;
                    assert!(
                        worst < bound,
                        "every {block}: {length} ms, {speed} st: stepped {worst} (bound {bound})"
                    );
                }
            }
        }
    }

    #[test]
    fn sweeping_every_parameter_while_holding_does_not_click() {
        let worst = worst_step(192_000, |i| {
            let t = i as f32 / 192_000.0;
            RepeatParams {
                hold: if (i / 128) % 50 < 45 { 1.0 } else { 0.0 },
                // Length up and down its whole range, Speed across its own,
                // Reverse flipped now and then, Mix swept.
                length_ms: LENGTH_MIN_MS
                    * (LENGTH_MAX_MS / LENGTH_MIN_MS).powf(0.5 - 0.5 * (t * 9.0).cos()),
                speed_st: MAX_SPEED_ST * (t * 7.0).sin(),
                reverse: (i / 9_000) % 2 == 1,
                mix: 0.5 + 0.5 * (t * 5.0).sin(),
            }
        });
        // Four times speed at most: 0.0144 a frame from the tone itself, the
        // rest from fades, the shortest of which (a 5 ms loop at four times
        // speed) is 15 frames. A cut would step by up to 1.
        assert!(worst < 0.1, "stepped {worst}");
    }

    #[test]
    fn works_and_sizes_itself_at_each_rate() {
        for sr in [22_050.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut x = Repeat::new(sr);
            let wanted = (2.0 * LENGTH_MAX_MS * 0.001 * sr) as u64;
            assert!(x.frames >= wanted, "{sr}: {} frames", x.frames);
            // A full-length loop held for longer than the ring, at speed.
            let p = held(LENGTH_MAX_MS, 12.0, false);
            let n = (sr * 6.0) as usize;
            for i in 0..n {
                let v = tone(i, 220.0, sr);
                let q = if i < (sr * 2.5) as usize { open(p) } else { p };
                let (l, r) = x.process(v, v, &q);
                assert!(
                    l.is_finite() && r.is_finite() && l.abs() <= 1.0,
                    "{sr}: {l}"
                );
            }
            // And a short one repeats on time.
            let mut x = Repeat::new(sr);
            let p = held(10.0, 0.0, false);
            let period = (0.01 * sr) as usize;
            let mut seed = 7u32;
            let input: Vec<f32> = (0..(sr as usize)).map(|_| noise(&mut seed)).collect();
            settle(&mut x, &p, (sr * 1.5) as usize, |i| input[i % input.len()]);
            let out: Vec<f32> = (0..period * 6)
                .map(|i| x.process(input[i], 0.0, &p).0)
                .collect();
            if (0.01 * sr).fract() == 0.0 {
                for i in period..period * 4 {
                    assert!((out[i] - out[i + period]).abs() < 1e-3, "{sr} at {i}");
                }
            }
        }
    }

    #[test]
    fn a_long_hold_never_reads_past_what_was_recorded() {
        // Length grown from 5 ms to 2 s straight after the grab: the loop can
        // only reach audio recorded since, so nothing older turns up. The
        // ring is filled with 0.1, then 0.25 just before the grab and -0.25
        // after it; anything stale would read 0.1 for a long stretch.
        let mut x = Repeat::new(SR);
        let mut p = held(LENGTH_MIN_MS, 0.0, false);
        settle(&mut x, &p, SR as usize * 5, |i| {
            if i < SR as usize * 4 {
                0.1
            } else {
                0.25
            }
        });
        p.length_ms = LENGTH_MAX_MS;
        let mut between = 0;
        // Past the point where the writer stops, so the frozen ring is read.
        for i in 0..(SR as usize * 7) {
            let (l, _) = x.process(-0.25, 0.0, &p);
            assert!(l.abs() < 0.3, "frame {i}: {l}");
            if l.abs() < 0.2 {
                between += 1;
            }
        }
        // Only the grab's fade and a few joins' pass between the two levels.
        assert!(between < 2_000, "{between} frames between the levels");
    }

    #[test]
    fn reset_forgets_the_audio() {
        for reverse in [false, true] {
            let mut x = Repeat::new(SR);
            let p = held(500.0, 5.0, reverse);
            let mut seed = 0x1234_5678u32;
            for _ in 0..48_000 {
                x.process(noise(&mut seed), noise(&mut seed), &open(p));
            }
            for _ in 0..10_000 {
                x.process(noise(&mut seed), noise(&mut seed), &p);
            }
            x.reset();
            // Hold stays up, then is let go and taken again: neither the old
            // loop nor anything recorded before the reset comes back.
            for i in 0..(x.frames as usize * 2) {
                let q = if (i / 20_000) % 2 == 0 { p } else { open(p) };
                assert_eq!(x.process(0.0, 0.0, &q), (0.0, 0.0), "frame {i}");
            }
        }
    }

    #[test]
    fn reset_on_a_fresh_repeat_changes_nothing() {
        let p = held(70.0, -3.0, false);
        let mut a = Repeat::new(SR);
        let mut b = Repeat::new(SR);
        b.reset();
        for i in 0..20_000 {
            let v = tone(i, 220.0, SR);
            let q = if i < 5_000 { open(p) } else { p };
            assert_eq!(a.process(v, v, &q), b.process(v, v, &q));
        }
    }

    #[test]
    fn full_mix_while_holding_leaves_none_of_the_dry() {
        let mut x = Repeat::new(SR);
        let p = held(50.0, 0.0, false);
        settle(&mut x, &p, 48_000, |_| 0.0);
        // Silence grabbed, then a loud input: once the grab's fade is over,
        // only the silent loop is heard.
        for i in 0..10_000 {
            let (l, r) = x.process(tone(i, 220.0, SR), 0.7, &p);
            if i >= 200 {
                assert_eq!((l, r), (0.0, 0.0), "frame {i}");
            }
        }
    }

    #[test]
    fn the_default_is_inside_every_range() {
        let d = RepeatParams::default();
        assert!((LENGTH_MIN_MS..=LENGTH_MAX_MS).contains(&d.length_ms));
        assert!(d.speed_st.abs() <= MAX_SPEED_ST);
        assert!(d.hold < HOLD_OFF);
        assert_eq!(d.mix, 0.0);
    }
}
