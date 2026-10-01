//! Reverb.
//!
//! A sound played in a room reaches you once directly, then again off the
//! nearest walls, then off everything at once, so thick and so quickly that
//! the echoes fuse into a wash that fades away. This builds that wash from
//! eight delay lines that feed one another.
//!
//! Each line's output is darkened a little, turned down a little, and mixed
//! with the other seven through an orthogonal matrix (a Hadamard: every line
//! hears every other, equally, and the mix loses no energy) before going back
//! into the lines. A single pass round the loop therefore spreads one click
//! over eight, the next over sixty-four, and within a few hundred
//! milliseconds the echoes are denser than the ear can separate. The line
//! lengths are different primes, so the echoes never line up into a ring.
//!
//! Decay is exact rather than approximate. Because the matrix loses nothing,
//! all the loss is in each line's gain, and each is set from its own length so
//! that a signal circulating through it falls by 60 dB in `decay_s` seconds
//! whatever the size. The darkening is a one-pole low-pass in every line, at
//! `tone_hz`: highs die faster than lows at every trip round the loop, which
//! is how air and soft walls behave. Wide open it hardly touches the decay;
//! turned down the tail goes dull as it fades.
//!
//! A fixed-length loop rings at its own modes, which sounds metallic. Every
//! line's read point drifts slowly, a fraction of a millisecond either way on
//! its own sine, so the modes never sit still long enough to be heard.
//!
//! Before the loop, the input passes a predelay and four allpass diffusers a
//! side, which smear a click into a short burst so the loop never sees a sharp
//! edge. A few direct taps off the predelayed input make the early
//! reflections, the first echoes off the nearest walls. Left goes into
//! lines 0, 2, 4 and 6, right into 1, 3, 5 and 7; the left output is taken
//! from four other lines than the right's. A mono source is therefore
//! decorrelated into a wide tail.
//!
//! Three voicings are different rooms rather than the same one at three
//! sizes. Room: short lines, light diffusion, strong early reflections
//! close behind the sound, little movement. Hall: long lines, more diffusion,
//! sparse early reflections further out, a slow drift. Plate: no distinct
//! early reflections, the densest diffusion, and a faster, deeper shimmer.
//! `size` scales the line lengths, the diffusers' reach stays as it was.
//!
//! Anything that changes a length would click if it jumped, so none does.
//! Size, the voicing, predelay, tone and decay are smoothed, and every
//! read point glides sample by sample towards its target. Switching voicing
//! is the same thing: each voicing's tables are blended by weights that move
//! from one to the next over about 30 ms, so the lines slide to their new
//! lengths and the early reflections cross-fade. That bends the pitch of
//! whatever is in the loop while it happens, a tape-like swoop, but there is
//! never a step. The cost is that only one set of lines runs, not two.
//!
//! The tail is fed in at a level that falls as the decay lengthens (the
//! square root of the decay), so a held sound reaches about the same level
//! whether the room is small or vast, rather than piling up without limit.
//!
//! At zero mix the node is a bit-exact bypass; the lines keep running, so
//! switching on never plays stale audio.

use crate::smooth::{OnePole, Ramp};

/// The shortest and longest decay, to 60 dB down, in seconds.
pub const MIN_DECAY_S: f32 = 0.2;
pub const MAX_DECAY_S: f32 = 20.0;
pub const DEFAULT_DECAY_S: f32 = 2.5;
/// The damping low-pass's range, in hertz. At the top the tail keeps its air;
/// at the bottom it turns to a dull rumble as it fades.
pub const MIN_TONE_HZ: f32 = 800.0;
pub const MAX_TONE_HZ: f32 = 16_000.0;
pub const DEFAULT_TONE_HZ: f32 = 6_000.0;
/// The longest predelay, in milliseconds.
pub const MAX_PREDELAY_MS: f32 = 200.0;
pub const DEFAULT_PREDELAY_MS: f32 = 12.0;
pub const DEFAULT_SIZE: f32 = 0.5;
pub const DEFAULT_MIX: f32 = 0.3;

const LINES: usize = 8;
/// Allpass diffusers a side.
const DIFFUSERS: usize = 4;
/// Early-reflection taps a voicing.
const TAPS: usize = 6;
/// Controls are recomputed this often, in frames, and the read points glide
/// in a straight line between recomputations.
const CTRL: usize = 16;
/// The line lengths below are in samples at this rate and scale with it.
const REF_RATE: f32 = 48_000.0;
/// Size 0 to 1 scales every length from `SCALE_MIN` to `SCALE_MIN +
/// SCALE_SPAN`; half is exactly the voicing's own lengths.
const SCALE_MIN: f32 = 0.4;
const SCALE_SPAN: f32 = 1.2;
/// The longest early-reflection tap in any voicing, in milliseconds at scale 1.
const LATEST_TAP_MS: f32 = 61.0;
/// 60 dB as a natural-log amplitude ratio.
const LN_1000: f32 = 6.907_755;
/// How loud the tail of a held noise is against the noise, in amplitude.
/// What it takes to get there is worked out from the decay and the lengths;
/// see `update`.
const TAIL_LEVEL: f32 = 0.3;
/// A line of this many samples at 48 kHz is damped by exactly the tone
/// setting; shorter lines, which are crossed more often a second, are damped
/// less each trip, and longer ones more.
const DAMP_REF: f32 = 1500.0;
/// The tone control's corner is this many times the one-pole's own, so the
/// top of its range leaves the tail bright.
const DAMP_REACH: f32 = 2.0;
/// How much of the four tapped lines reaches an output.
const TAIL_OUT: f32 = 0.5;
/// The orthogonal matrix's scale, one over the square root of eight.
const HADAMARD: f32 = 0.353_553_4;
/// The cross-fade between voicings and the glide of size, as time constants.
const KIND_MS: f32 = 30.0;
const SIZE_MS: f32 = 60.0;
const MIX_MS: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverbType {
    /// A small space: short lines, strong early reflections close behind.
    Room,
    /// A big one: long lines, sparse early reflections, a slow drift.
    Hall,
    /// A sheet of metal: dense from the first instant, bright, shimmering.
    Plate,
}

impl ReverbType {
    pub const ALL: [ReverbType; 3] = [ReverbType::Room, ReverbType::Hall, ReverbType::Plate];
    pub const NAMES: [&'static str; 3] = ["Room", "Hall", "Plate"];

    pub fn from_value(v: f32) -> ReverbType {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReverbParams {
    /// Which voicing.
    pub kind: ReverbType,
    /// Time to fall 60 dB, `MIN_DECAY_S` to `MAX_DECAY_S`.
    pub decay_s: f32,
    /// How big the space is, 0 to 1. Scales every delay.
    pub size: f32,
    /// Where the damping low-pass turns, in hertz.
    pub tone_hz: f32,
    /// Silence before the reverb starts, 0 to `MAX_PREDELAY_MS`.
    pub predelay_ms: f32,
    /// Dry plus this much wet, 0 to 1. The dry stays full.
    pub mix: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            kind: ReverbType::Hall,
            decay_s: DEFAULT_DECAY_S,
            size: DEFAULT_SIZE,
            tone_hz: DEFAULT_TONE_HZ,
            predelay_ms: DEFAULT_PREDELAY_MS,
            mix: DEFAULT_MIX,
        }
    }
}

/// What makes a voicing itself. Everything here is blended by weight while
/// switching.
struct Voicing {
    /// Line lengths in samples at 48 kHz, size one half. All different
    /// primes.
    lines: [f32; LINES],
    /// The diffusers' allpass coefficient: more is smoother and denser.
    diffusion: f32,
    /// How far each line's read point drifts either way, in milliseconds.
    drift_ms: f32,
    /// The drift's base rate in hertz. Each line runs at its own multiple.
    drift_hz: f32,
    /// Early reflections: milliseconds back from the predelayed sound at size
    /// one half, and gain. They alternate left, right.
    taps: [(f32, f32); TAPS],
}

const VOICINGS: [Voicing; 3] = [
    Voicing {
        lines: [331.0, 443.0, 569.0, 691.0, 823.0, 967.0, 1117.0, 1283.0],
        diffusion: 0.45,
        drift_ms: 0.15,
        drift_hz: 0.35,
        taps: [
            (3.1, 0.30),
            (5.9, 0.26),
            (8.3, 0.22),
            (12.7, 0.18),
            (17.9, 0.14),
            (23.3, 0.10),
        ],
    },
    Voicing {
        lines: [
            1051.0, 1327.0, 1621.0, 1931.0, 2347.0, 2753.0, 3181.0, 3571.0,
        ],
        diffusion: 0.6,
        drift_ms: 0.4,
        drift_hz: 0.25,
        taps: [
            (11.0, 0.14),
            (19.0, 0.12),
            (27.0, 0.10),
            (38.0, 0.09),
            (49.0, 0.07),
            (61.0, 0.05),
        ],
    },
    Voicing {
        lines: [557.0, 733.0, 919.0, 1103.0, 1297.0, 1523.0, 1721.0, 1949.0],
        diffusion: 0.75,
        drift_ms: 0.6,
        drift_hz: 0.9,
        taps: [
            (1.7, 0.04),
            (2.9, 0.035),
            (4.3, 0.03),
            (5.9, 0.025),
            (7.7, 0.02),
            (9.7, 0.015),
        ],
    },
];

/// The longest line of any voicing at the biggest size, in samples at 48 kHz.
const LONGEST_LINE: f32 = 3571.0 * (SCALE_MIN + SCALE_SPAN);
/// The drift runs at these multiples of the voicing's rate, so no two lines
/// keep step.
const DRIFT_RATES: [f32; LINES] = [1.0, 1.17, 0.83, 1.31, 0.73, 1.09, 0.91, 1.23];
/// Diffuser allpass lengths in samples at 48 kHz, left then right. Different,
/// so the two sides are decorrelated before they reach the loop.
const DIFFUSER_LENGTHS: [[f32; DIFFUSERS]; 2] =
    [[199.0, 151.0, 541.0, 367.0], [211.0, 163.0, 557.0, 383.0]];
/// Which lines the left and right inputs go into, and with which sign.
const INJECT: [[f32; LINES]; 2] = [
    [1.0, 0.0, 1.0, 0.0, -1.0, 0.0, -1.0, 0.0],
    [0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, -1.0],
];
/// Which lines the left and right outputs are taken from, with sign.
const TAP_OUT: [[f32; LINES]; 2] = [
    [0.0, 1.0, 0.0, -1.0, 1.0, 0.0, -1.0, 0.0],
    [1.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, -1.0],
];

/// Adding and taking away something tiny turns a denormal into zero, and
/// changes nothing that matters.
#[inline]
fn flush(x: f32) -> f32 {
    const TINY: f32 = 1.0e-18;
    (x + TINY) - TINY
}

/// A circular buffer with a fractional, linearly interpolated read. The
/// newest sample is delay zero.
struct Delay {
    buf: Vec<f32>,
    mask: usize,
    pos: usize,
}

impl Delay {
    /// Room for reads up to `max` samples back.
    fn new(max: usize) -> Self {
        let n = (max + 4).next_power_of_two();
        Self {
            buf: vec![0.0; n],
            mask: n - 1,
            pos: 0,
        }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.pos = (self.pos + 1) & self.mask;
        self.buf[self.pos] = x;
    }

    #[inline]
    fn read(&self, d: f32) -> f32 {
        let d = d.clamp(0.0, (self.mask - 2) as f32);
        let i = d as usize;
        let f = d - i as f32;
        let a = self.buf[self.pos.wrapping_sub(i) & self.mask];
        let b = self.buf[self.pos.wrapping_sub(i + 1) & self.mask];
        a + (b - a) * f
    }
}

/// A value moving in a straight line to its target over a fixed number of
/// steps.
#[derive(Debug, Clone, Copy, Default)]
struct Glide {
    cur: f32,
    step: f32,
}

impl Glide {
    fn jump(&mut self, v: f32) {
        self.cur = v;
        self.step = 0.0;
    }

    fn aim(&mut self, target: f32, steps: usize) {
        self.step = (target - self.cur) / steps as f32;
    }

    #[inline]
    fn advance(&mut self) -> f32 {
        self.cur += self.step;
        self.cur
    }
}

/// A Schroeder allpass: flat in level, smears in time.
struct Allpass {
    delay: Delay,
    length: f32,
}

impl Allpass {
    fn new(length: usize) -> Self {
        Self {
            delay: Delay::new(length + 2),
            length: length as f32,
        }
    }

    #[inline]
    fn process(&mut self, x: f32, g: f32) -> f32 {
        let w = self.delay.read(self.length - 1.0);
        let v = x - g * w;
        self.delay.push(flush(v));
        w + g * v
    }
}

struct Line {
    delay: Delay,
    glide: Glide,
    /// The damping low-pass's state.
    lp: f32,
    /// Loss a trip round the loop.
    gain: f32,
    /// How far the damping low-pass moves towards its input each sample.
    damp: f32,
    /// The drift's phase, in turns.
    phase: f32,
}

pub struct Reverb {
    sample_rate: f32,
    lines: Vec<Line>,
    diffusers: [Vec<Allpass>; 2],
    /// The input, left and right, before the predelay.
    pre: [Delay; 2],
    /// The predelay in samples.
    pre_glide: Glide,
    /// Early-reflection taps for every voicing, alternating left, right.
    taps: Vec<Glide>,
    tap_gain: [f32; TAPS * 3],
    /// How much of each voicing is heard.
    blend: [OnePole; 3],
    size: OnePole,
    decay: OnePole,
    tone: OnePole,
    pre_ms: OnePole,
    mix: Ramp,
    diffusion: f32,
    drift_depth: f32,
    drift_inc: f32,
    feed: f32,
    /// A gentle high-pass on the wet signal so nothing sits at DC.
    dc: [(f32, f32); 2],
    dc_r: f32,
    countdown: usize,
    primed: bool,
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let srf = sample_rate / REF_RATE;
        let ctrl_rate = sample_rate / CTRL as f32;
        let drift_max = 0.7 * 0.001 * sample_rate;
        let lines = (0..LINES)
            .map(|i| Line {
                delay: Delay::new((LONGEST_LINE * srf + drift_max) as usize + 4),
                glide: Glide::default(),
                lp: 0.0,
                gain: 0.0,
                damp: 1.0,
                phase: i as f32 / LINES as f32,
            })
            .collect();
        let diffusers = [0, 1].map(|s| {
            DIFFUSER_LENGTHS[s]
                .iter()
                .map(|&n| Allpass::new((n * srf).round() as usize))
                .collect()
        });
        let pre_len = ((MAX_PREDELAY_MS + LATEST_TAP_MS * (SCALE_MIN + SCALE_SPAN))
            * 0.001
            * sample_rate) as usize
            + 8;
        let mut blend = [OnePole::new(); 3];
        let mut size = OnePole::new();
        let mut decay = OnePole::new();
        let mut tone = OnePole::new();
        let mut pre_ms = OnePole::new();
        for b in &mut blend {
            b.set_time(KIND_MS, ctrl_rate);
        }
        size.set_time(SIZE_MS, ctrl_rate);
        decay.set_time(60.0, ctrl_rate);
        tone.set_time(40.0, ctrl_rate);
        pre_ms.set_time(60.0, ctrl_rate);
        let mut mix = Ramp::new(0.0);
        mix.set_time(MIX_MS, sample_rate);
        Self {
            sample_rate,
            lines,
            diffusers,
            pre: [Delay::new(pre_len), Delay::new(pre_len)],
            pre_glide: Glide::default(),
            taps: vec![Glide::default(); TAPS * 3],
            tap_gain: [0.0; TAPS * 3],
            blend,
            size,
            decay,
            tone,
            pre_ms,
            mix,
            diffusion: 0.5,
            drift_depth: 0.0,
            drift_inc: 0.0,
            feed: 0.0,
            dc: [(0.0, 0.0); 2],
            dc_r: 1.0 - core::f32::consts::TAU * 10.0 / sample_rate,
            countdown: 0,
            primed: false,
        }
    }

    /// Bytes of delay memory held, for a test to keep it bounded.
    pub fn memory_bytes(&self) -> usize {
        let lines: usize = self.lines.iter().map(|l| l.delay.buf.len()).sum();
        let ap: usize = self
            .diffusers
            .iter()
            .flatten()
            .map(|a| a.delay.buf.len())
            .sum();
        (lines + ap + self.pre[0].buf.len() + self.pre[1].buf.len()) * 4
    }

    /// Recompute everything that depends on the controls, and aim the glides
    /// at where they should be `CTRL` frames on.
    fn update(&mut self, p: &ReverbParams) {
        let sr = self.sample_rate;
        let srf = sr / REF_RATE;
        let kind = p.kind as usize;
        let decay_target = p.decay_s.clamp(MIN_DECAY_S, MAX_DECAY_S);
        let (size_t, tone_t, pre_t) = (
            p.size.clamp(0.0, 1.0),
            p.tone_hz.clamp(MIN_TONE_HZ, MAX_TONE_HZ),
            p.predelay_ms.clamp(0.0, MAX_PREDELAY_MS),
        );
        if !self.primed {
            for (k, b) in self.blend.iter_mut().enumerate() {
                b.reset(if k == kind { 1.0 } else { 0.0 });
            }
            self.size.reset(size_t);
            self.decay.reset(decay_target);
            self.tone.reset(tone_t);
            self.pre_ms.reset(pre_t);
        }
        let w: [f32; 3] =
            core::array::from_fn(|k| self.blend[k].process(if k == kind { 1.0 } else { 0.0 }));
        let scale = SCALE_MIN + SCALE_SPAN * self.size.process(size_t);
        let decay = self.decay.process(decay_target);
        let tone = self.tone.process(tone_t);
        let pre = self.pre_ms.process(pre_t) * 0.001 * sr;

        let (mut diffusion, mut drift_ms, mut drift_hz) = (0.0, 0.0, 0.0);
        for (v, wk) in VOICINGS.iter().zip(w) {
            diffusion += wk * v.diffusion;
            drift_ms += wk * v.drift_ms;
            drift_hz += wk * v.drift_hz;
        }
        self.diffusion = diffusion;
        self.drift_depth = drift_ms * 0.001 * sr;
        self.drift_inc = drift_hz / sr;

        let mut len_sum = 0.0;
        for (i, line) in self.lines.iter_mut().enumerate() {
            let len: f32 = VOICINGS.iter().zip(w).map(|(v, wk)| wk * v.lines[i]).sum();
            let len = (len * scale * srf).max(4.0);
            if self.primed {
                line.glide.aim(len, CTRL);
            } else {
                line.glide.jump(len);
            }
            line.gain = (-LN_1000 * len / (sr * decay)).exp();
            let corner = tone * DAMP_REACH * (DAMP_REF * srf / len).sqrt();
            line.damp = 1.0 - (-core::f32::consts::TAU * corner / sr).exp();
            len_sum += len;
        }
        // A packet of energy circulates for about `decay * sr / 13.8` samples
        // and is heard once a trip, so what comes out of a held noise goes as
        // decay over mean length. Feeding with the inverse keeps it level.
        self.feed = TAIL_LEVEL * (55.2 * (len_sum / LINES as f32) / (decay * sr)).sqrt();
        self.pre_glide.aim(pre, CTRL);
        if !self.primed {
            self.pre_glide.jump(pre);
        }
        for (k, v) in VOICINGS.iter().enumerate() {
            for (j, &(ms, gain)) in v.taps.iter().enumerate() {
                let n = k * TAPS + j;
                let t = pre + ms * 0.001 * sr * scale;
                if self.primed {
                    self.taps[n].aim(t, CTRL);
                } else {
                    self.taps[n].jump(t);
                }
                self.tap_gain[n] = gain * w[k];
            }
        }
        self.primed = true;
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &ReverbParams) -> (f32, f32) {
        if self.countdown == 0 {
            self.update(p);
            self.countdown = CTRL;
        }
        self.countdown -= 1;

        self.pre[0].push(l);
        self.pre[1].push(r);
        let pd = self.pre_glide.advance();
        let mut in_lr = [self.pre[0].read(pd), self.pre[1].read(pd)];
        for (side, x) in in_lr.iter_mut().enumerate() {
            for a in self.diffusers[side].iter_mut() {
                *x = a.process(*x, self.diffusion);
            }
        }

        // Early reflections, straight off the predelayed input.
        let mut wet = [0.0f32; 2];
        for n in 0..TAPS * 3 {
            let d = self.taps[n].advance();
            let g = self.tap_gain[n];
            if g > 1.0e-5 {
                let side = n % 2;
                wet[side] += g * self.pre[side].read(d);
            }
        }

        // The loop.
        let mut y = [0.0f32; LINES];
        for (i, line) in self.lines.iter_mut().enumerate() {
            let len = line.glide.advance();
            line.phase += self.drift_inc * DRIFT_RATES[i];
            line.phase -= line.phase.floor();
            let d = len + self.drift_depth * (line.phase * core::f32::consts::TAU).sin();
            let x = line.delay.read(d - 1.0);
            line.lp = flush(line.lp + line.damp * (x - line.lp));
            y[i] = line.lp * line.gain;
        }
        for (side, w) in wet.iter_mut().enumerate() {
            let mut t = 0.0;
            for i in 0..LINES {
                t += TAP_OUT[side][i] * y[i];
            }
            *w += TAIL_OUT * t;
        }
        hadamard(&mut y);
        let inject = self.feed * 0.5;
        for (i, line) in self.lines.iter_mut().enumerate() {
            let feed = inject * (INJECT[0][i] * in_lr[0] + INJECT[1][i] * in_lr[1]);
            line.delay.push(flush(y[i] * HADAMARD + feed));
        }

        for (w, (x1, y1)) in wet.iter_mut().zip(self.dc.iter_mut()) {
            let out = flush(*w - *x1 + self.dc_r * *y1);
            *x1 = *w;
            *y1 = out;
            *w = out;
        }

        let m = self.mix.process(p.mix.clamp(0.0, 1.0));
        if m == 0.0 {
            (l, r)
        } else {
            (l + wet[0] * m, r + wet[1] * m)
        }
    }
}

/// The unscaled eight-point Hadamard transform, in place.
#[inline]
fn hadamard(y: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        let mut i = 0;
        while i < LINES {
            for j in i..i + h {
                let (a, b) = (y[j], y[j + h]);
                y[j] = a + b;
                y[j + h] = a - b;
            }
            i += h * 2;
        }
        h *= 2;
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

    fn wet(kind: ReverbType, decay_s: f32) -> ReverbParams {
        ReverbParams {
            kind,
            decay_s,
            size: 0.5,
            tone_hz: MAX_TONE_HZ,
            predelay_ms: 0.0,
            mix: 1.0,
        }
    }

    /// The wet part only, left and right, of an impulse in the left.
    fn impulse_response(sr: f32, p: &ReverbParams, seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let mut rv = Reverb::new(sr);
        let n = (seconds * sr) as usize;
        let (mut wl, mut wr) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..n {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = rv.process(x, 0.0, p);
            wl.push(l - x);
            wr.push(r);
        }
        (wl, wr)
    }

    /// Time to fall 60 dB by Schroeder backward integration, fitted between
    /// -5 and -35 dB.
    fn rt60(sr: f32, ir: &[(f32, f32)]) -> f32 {
        let mut edc = vec![0.0f64; ir.len()];
        let mut acc = 0.0f64;
        for i in (0..ir.len()).rev() {
            acc += (ir[i].0 as f64).powi(2) + (ir[i].1 as f64).powi(2);
            edc[i] = acc;
        }
        let total = edc[0];
        let (mut sx, mut sy, mut sxx, mut sxy, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, e) in edc.iter().enumerate() {
            let db = 10.0 * (e / total).log10();
            if (-35.0..=-5.0).contains(&db) {
                let t = i as f64 / sr as f64;
                sx += t;
                sy += db;
                sxx += t * t;
                sxy += t * db;
                n += 1.0;
            }
        }
        let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
        (-60.0 / slope) as f32
    }

    fn ir_pairs(sr: f32, p: &ReverbParams, seconds: f32) -> Vec<(f32, f32)> {
        let (l, r) = impulse_response(sr, p, seconds);
        l.into_iter().zip(r).collect()
    }

    #[test]
    fn kind_from_value_clamps() {
        assert_eq!(ReverbType::from_value(-1.0), ReverbType::Room);
        assert_eq!(ReverbType::from_value(1.0), ReverbType::Hall);
        assert_eq!(ReverbType::from_value(2.4), ReverbType::Plate);
        assert_eq!(ReverbType::from_value(9.0), ReverbType::Plate);
        assert_eq!(ReverbType::NAMES.len(), ReverbType::ALL.len());
    }

    #[test]
    fn zero_mix_is_a_bit_exact_bypass() {
        let mut rv = Reverb::new(SR);
        let mut s = 7u32;
        let p = ReverbParams {
            mix: 0.0,
            ..Default::default()
        };
        for _ in 0..20_000 {
            let (l, r) = (noise(&mut s), noise(&mut s));
            assert_eq!(rv.process(l, r, &p), (l, r));
        }
        // Wet for a while, then off: exact again once the ramp has landed,
        // and the lines kept running meanwhile.
        let on = ReverbParams {
            mix: 0.8,
            ..Default::default()
        };
        for _ in 0..30_000 {
            rv.process(noise(&mut s), noise(&mut s), &on);
        }
        for _ in 0..(0.05 * SR) as usize {
            rv.process(noise(&mut s), noise(&mut s), &p);
        }
        for _ in 0..10_000 {
            let (l, r) = (noise(&mut s), noise(&mut s));
            assert_eq!(rv.process(l, r, &p), (l, r));
        }
    }

    #[test]
    fn silence_in_silence_out() {
        for kind in ReverbType::ALL {
            let mut rv = Reverb::new(SR);
            let p = wet(kind, 5.0);
            for _ in 0..50_000 {
                assert_eq!(rv.process(0.0, 0.0, &p), (0.0, 0.0));
            }
        }
    }

    #[test]
    fn measured_decay_matches_the_setting() {
        for kind in ReverbType::ALL {
            for &t in &[0.5f32, 3.0] {
                let ir = ir_pairs(SR, &wet(kind, t), t * 2.5 + 0.5);
                let got = rt60(SR, &ir);
                assert!(
                    (got - t).abs() <= 0.25 * t,
                    "{kind:?} asked {t} s, measured {got} s"
                );
            }
        }
    }

    #[test]
    fn decay_holds_at_other_sample_rates_and_sizes() {
        for &sr in &[44_100.0f32, 96_000.0] {
            for &size in &[0.0f32, 1.0] {
                let p = ReverbParams {
                    size,
                    ..wet(ReverbType::Hall, 1.5)
                };
                let got = rt60(sr, &ir_pairs(sr, &p, 4.5));
                assert!(
                    (got - 1.5).abs() <= 0.375,
                    "{sr} Hz size {size}: measured {got} s"
                );
            }
        }
    }

    #[test]
    fn impulse_response_has_no_dc_and_fades_steadily() {
        for kind in ReverbType::ALL {
            let ir = ir_pairs(SR, &wet(kind, 2.0), 6.0);
            for side in 0..2 {
                let pick = |f: &(f32, f32)| if side == 0 { f.0 } else { f.1 };
                let sum: f64 = ir.iter().map(|f| pick(f) as f64).sum();
                let energy: f64 = ir.iter().map(|f| (pick(f) as f64).powi(2)).sum();
                let n = ir.len() as f64;
                // A noise-like response of this energy would wander by about
                // sqrt(n * energy / n) = sqrt(energy) in its sum.
                assert!(
                    sum.abs() < 3.0 * energy.sqrt(),
                    "{kind:?} side {side}: sum {sum}, energy {energy}, n {n}"
                );
            }
            // Energy in 50 ms windows, once the loop is dense, only falls.
            let w = (0.05 * SR) as usize;
            let energies: Vec<f64> = ir
                .chunks(w)
                .map(|c| {
                    c.iter()
                        .map(|f| (f.0 as f64).powi(2) + (f.1 as f64).powi(2))
                        .sum()
                })
                .collect();
            let start = 4;
            for i in start..energies.len() - 2 {
                assert!(
                    energies[i + 1] <= energies[i] * 1.15,
                    "{kind:?}: window {} rose, {} to {}",
                    i + 1,
                    energies[i],
                    energies[i + 1]
                );
            }
        }
    }

    #[test]
    fn full_scale_noise_at_longest_decay_does_not_run_away() {
        for kind in ReverbType::ALL {
            let mut rv = Reverb::new(SR);
            let p = ReverbParams {
                kind,
                decay_s: MAX_DECAY_S,
                size: 1.0,
                tone_hz: MAX_TONE_HZ,
                predelay_ms: 0.0,
                mix: 1.0,
            };
            let mut s = 99u32;
            let mut peak = 0.0f32;
            for _ in 0..(6.0 * SR) as usize {
                let (l, r) = rv.process(noise(&mut s), noise(&mut s), &p);
                assert!(l.is_finite() && r.is_finite());
                peak = peak.max(l.abs()).max(r.abs());
            }
            assert!(peak <= 4.0, "{kind:?} peaked at {peak} with noise in");
            let mut last = 0.0f32;
            for k in 0..5 {
                let mut e = 0.0f32;
                for _ in 0..(1.0 * SR) as usize {
                    let (l, r) = rv.process(0.0, 0.0, &p);
                    assert!(l.is_finite() && r.is_finite() && l.abs() <= 4.0);
                    e += l * l + r * r;
                }
                if k > 0 {
                    assert!(e <= last, "{kind:?}: tail energy rose in second {k}");
                }
                last = e;
            }
        }
    }

    #[test]
    fn held_noise_is_about_as_loud_whatever_the_decay() {
        let level = |t: f32| {
            let mut rv = Reverb::new(SR);
            let p = wet(ReverbType::Hall, t);
            let mut s = 5u32;
            let mut e = 0.0f64;
            let n = (3.0 * t.max(1.0) * SR) as usize;
            for i in 0..n {
                let x = noise(&mut s) * 0.5;
                let (l, _) = rv.process(x, x, &p);
                if i > n / 2 {
                    e += ((l - x) as f64).powi(2);
                }
            }
            (e / (n / 2) as f64).sqrt()
        };
        let (short, long) = (level(0.3), level(8.0));
        assert!(
            short / long < 3.0 && long / short < 3.0,
            "levels {short} and {long}"
        );
    }

    /// Two slow sines, so there is a signal to click against.
    fn music(i: usize, sr: f32) -> f32 {
        let t = i as f32 / sr;
        0.4 * (core::f32::consts::TAU * 220.0 * t).sin()
            + 0.3 * (core::f32::consts::TAU * 330.0 * t).sin()
    }

    /// Largest step between neighbouring samples of the wet signal.
    fn max_wet_step(sweep: impl Fn(f32) -> ReverbParams, sr: f32) -> (f32, f32) {
        let mut rv = Reverb::new(sr);
        let n = sr as usize;
        let mut last = (0.0f32, 0.0f32);
        let mut worst = 0.0f32;
        let mut input_worst = 0.0f32;
        for i in 0..n * 2 {
            let x = music(i, sr);
            // Two seconds: settle with the sweep at its start, then sweep.
            let t = ((i as f32 - n as f32) / n as f32).clamp(0.0, 1.0);
            let (l, r) = rv.process(x, x * 0.8, &sweep(t));
            if i > n / 2 {
                worst = worst.max((l - last.0).abs()).max((r - last.1).abs());
                input_worst = input_worst.max((x - music(i - 1, sr)).abs());
            }
            last = (l, r);
        }
        (worst, input_worst)
    }

    #[test]
    fn sweeping_every_control_does_not_click() {
        let base = |kind| ReverbParams {
            kind,
            decay_s: 3.0,
            size: 0.5,
            tone_hz: 6_000.0,
            predelay_ms: 12.0,
            mix: 0.6,
        };
        type Sweep = Box<dyn Fn(f32) -> ReverbParams>;
        let sweeps: Vec<(&str, Sweep)> = vec![
            (
                "size",
                Box::new(move |t| ReverbParams {
                    size: t,
                    ..base(ReverbType::Hall)
                }),
            ),
            (
                "size room",
                Box::new(move |t| ReverbParams {
                    size: t,
                    ..base(ReverbType::Room)
                }),
            ),
            (
                "decay",
                Box::new(move |t| ReverbParams {
                    decay_s: MIN_DECAY_S * (MAX_DECAY_S / MIN_DECAY_S).powf(t),
                    ..base(ReverbType::Plate)
                }),
            ),
            (
                "tone",
                Box::new(move |t| ReverbParams {
                    tone_hz: MIN_TONE_HZ * (MAX_TONE_HZ / MIN_TONE_HZ).powf(t),
                    ..base(ReverbType::Hall)
                }),
            ),
            (
                "predelay",
                Box::new(move |t| ReverbParams {
                    predelay_ms: t * MAX_PREDELAY_MS,
                    ..base(ReverbType::Room)
                }),
            ),
            (
                "mix",
                Box::new(move |t| ReverbParams {
                    mix: t,
                    ..base(ReverbType::Hall)
                }),
            ),
        ];
        for sr in [44_100.0f32, 48_000.0, 96_000.0] {
            for (name, sweep) in &sweeps {
                let (worst, input) = max_wet_step(sweep, sr);
                assert!(
                    worst <= 4.0 * input,
                    "{name} at {sr}: step {worst} against input's {input}"
                );
            }
        }
    }

    #[test]
    fn switching_voicing_does_not_click() {
        for from in ReverbType::ALL {
            for to in ReverbType::ALL {
                if from == to {
                    continue;
                }
                let mut rv = Reverb::new(SR);
                let n = SR as usize;
                let mut last = (0.0f32, 0.0f32);
                let mut worst = 0.0f32;
                let mut input = 0.0f32;
                for i in 0..n * 2 {
                    let x = music(i, SR);
                    let p = ReverbParams {
                        kind: if i < n { from } else { to },
                        mix: 0.6,
                        ..Default::default()
                    };
                    let (l, r) = rv.process(x, x, &p);
                    if i > n / 2 {
                        worst = worst.max((l - last.0).abs()).max((r - last.1).abs());
                        input = input.max((x - music(i - 1, SR)).abs());
                    }
                    last = (l, r);
                }
                assert!(
                    worst <= 4.0 * input,
                    "{from:?} to {to:?}: step {worst} against {input}"
                );
            }
        }
    }

    #[test]
    fn voicings_sound_different() {
        let early = |kind| -> f64 {
            let ir = ir_pairs(SR, &wet(kind, 2.0), 0.3);
            let e = |a: usize, b: usize| -> f64 {
                ir[a..b]
                    .iter()
                    .map(|f| (f.0 as f64).powi(2) + (f.1 as f64).powi(2))
                    .sum()
            };
            e(0, (0.03 * SR) as usize) / e((0.1 * SR) as usize, (0.3 * SR) as usize)
        };
        let (room, hall, plate) = (
            early(ReverbType::Room),
            early(ReverbType::Hall),
            early(ReverbType::Plate),
        );
        assert!(room > hall && hall > plate * 0.5, "{room} {hall} {plate}");
        assert!((room - plate).abs() > 0.1 * room);
    }

    #[test]
    fn predelay_holds_the_wet_back() {
        for ms in [10.0f32, 50.0] {
            let p = ReverbParams {
                predelay_ms: ms,
                ..wet(ReverbType::Hall, 2.0)
            };
            let (l, r) = impulse_response(SR, &p, 0.3);
            let first = l
                .iter()
                .zip(&r)
                .position(|(a, b)| a.abs() > 1.0e-4 || b.abs() > 1.0e-4)
                .unwrap();
            let want = ms * 0.001 * SR;
            assert!(
                (first as f32) >= want - 2.0 && (first as f32) < want + 0.03 * SR,
                "predelay {ms} ms: first wet at {first}"
            );
        }
    }

    #[test]
    fn mono_source_is_wide_and_bounded() {
        let mut rv = Reverb::new(SR);
        let p = ReverbParams {
            mix: 1.0,
            ..Default::default()
        };
        let mut s = 3u32;
        let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..(3.0 * SR) as usize {
            let x = noise(&mut s) * 0.5;
            let (l, r) = rv.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite() && l.abs() < 4.0 && r.abs() < 4.0);
            if i > SR as usize {
                let (wl, wr) = ((l - x) as f64, (r - x) as f64);
                ll += wl * wl;
                rr += wr * wr;
                lr += wl * wr;
            }
        }
        let corr = lr / (ll * rr).sqrt();
        assert!(corr.abs() < 0.5, "channels correlated at {corr}");
    }

    #[test]
    fn memory_stays_small() {
        assert!(Reverb::new(48_000.0).memory_bytes() < 4_000_000);
        assert!(Reverb::new(96_000.0).memory_bytes() < 8_000_000);
    }
}
