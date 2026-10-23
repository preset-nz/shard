//! The drum kit: kick, snare and hat, synthesised, on the transport's clock.
//!
//! A stop-gap until roadmap row 25 (a kit sliced from a material). Georg,
//! 2026-10-03: *"I need a quick way to play a drum beat, base, snare, hihat
//! at least."* Since epic 28, story d, **drums are a track type**: each
//! drums track plays one `Kit` of the engine's pool (`engine::KITS`), with
//! its own grid, length, sound, fader, mute and solo.
//!
//! - **A switch like any node's.** `KitParams::on`, on the track's card. Its
//!   clock runs while it is off, so switching it on lands on the grid.
//! - **Its own grid.** A row per lane and a byte per step: zero is off,
//!   anything else is the hit's velocity. Several lanes fire on one step.
//! - **The transport's clock.** The kit does not count time itself. It reads
//!   where the shared transport is (`steps::Beat`), so it stays on every
//!   track's grid whatever its own length, and through a tempo change. The
//!   arrangement's swing applies; nudge does not.
//! - **No patch.** It joins the mix after its track's fader and before the
//!   arrangement's chain, so no patch's envelope, velocity or effects touch
//!   it, and the arrangement's effects and limiter do. It is on the tape, so
//!   a brake slows it with everything else (`process_at`).
//! - **Lanes, each its own voice** (epic 49, story a). A kit holds up to
//!   `MAX_LANES` lanes, each a drum (`LaneKind`) with its own state and its
//!   own row of the grid. A lane is built when it is added, off the audio
//!   thread (`Lane::new`), and swapped into its slot (`Kit::swap_lane`); the
//!   lane it replaces is handed back to be dropped there too. An empty slot
//!   costs nothing and holds no lane. A lane's slot is its identity: it keeps
//!   it however the grid orders the lanes. Every drums track has the three
//!   lanes `STANDARD`, kick, snare and hat, in slots `KICK`, `SNARE` and
//!   `HAT`, until lanes are nodes (story b).
//! - **One noise for the kit.** Every lane reads the same noise sample a
//!   frame, so adding a lane does not change another's hiss.
//!
//! No samples: each drum is a few lines of synthesis, sized when its lane is
//! built, with nothing allocated per hit.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::rng::Rng;
use crate::steps::STEPS;

/// The most lanes a kit holds: the Launchpad's eight rows (roadmap row 62),
/// not anything the engine needs.
pub const MAX_LANES: usize = 8;
/// The slots of the three standard lanes.
pub const KICK: usize = 0;
pub const SNARE: usize = 1;
pub const HAT: usize = 2;

/// What a lane plays: one synthesised drum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneKind {
    Kick,
    Snare,
    Hat,
}

/// The lanes every drums track has, by slot: kick, snare, hat.
pub const STANDARD: [LaneKind; 3] = [LaneKind::Kick, LaneKind::Snare, LaneKind::Hat];

/// A hit at full velocity.
pub const HIT_MAX: u8 = 127;
/// A ghost note, the second click on a step in the grid.
pub const HIT_GHOST: u8 = 48;

/// The kit's grid. Not parameters: like the melody's steps, it belongs to
/// the tracker and crosses through `StepBank`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KitPattern {
    /// How many steps the kit's loop runs, 1 to `STEPS`, as a track's does.
    pub length: u32,
    /// Per lane, by slot, a byte a step: zero is no hit, else its velocity
    /// up to `HIT_MAX`. A slot with no lane plays nothing.
    pub hits: [[u8; STEPS]; MAX_LANES],
}

impl Default for KitPattern {
    /// A beat, so switching the kit on does something: kick on one
    /// and the and of three, snare on two and four, eighth-note hats with the
    /// offbeats softer.
    fn default() -> Self {
        let mut hits = [[0; STEPS]; MAX_LANES];
        for k in [0, 10] {
            hits[KICK][k] = HIT_MAX;
        }
        for k in [4, 12] {
            hits[SNARE][k] = HIT_MAX;
        }
        for k in (0..16).step_by(2) {
            hits[HAT][k] = if k % 4 == 0 { HIT_MAX } else { 80 };
        }
        Self { length: 16, hits }
    }
}

impl KitPattern {
    /// Slot `v`'s hit on step `k`, 0 to 1.
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

/// How far Tune reaches, in semitones either way.
pub const TUNE_RANGE: f32 = 12.0;
/// Decay's range, a factor on every voice's own.
pub const DECAY_MIN: f32 = 0.25;
pub const DECAY_MAX: f32 = 4.0;

/// A drums track's sound, handed to its kit once a block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KitParams {
    /// Whether it plays: off, no new hit starts and its fader closes.
    pub on: bool,
    /// Semitones, every voice together, up to `TUNE_RANGE` either way.
    pub tune: f32,
    /// A factor on every voice's decay: 1 is as designed.
    pub decay: f32,
}

impl Default for KitParams {
    fn default() -> Self {
        Self {
            on: false,
            tune: 0.0,
            decay: 1.0,
        }
    }
}

/// Words of eight steps a byte each.
const WORDS: usize = STEPS / 8;

/// A drums track's grid and sound, shared between whatever edits it and the
/// audio thread, the way `StepBank` shares a track's steps. Stored whole,
/// read whole once a block.
pub struct KitBank {
    length: AtomicU32,
    /// The hits, a slot at a time, a byte a step, step one in the low byte.
    hits: [[AtomicU64; WORDS]; MAX_LANES],
    on: AtomicBool,
    tune: AtomicU32,
    decay: AtomicU32,
}

impl KitBank {
    pub fn new(p: &KitPattern, q: &KitParams) -> Self {
        let bank = Self {
            length: AtomicU32::new(0),
            hits: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            on: AtomicBool::new(false),
            tune: AtomicU32::new(0),
            decay: AtomicU32::new(0),
        };
        bank.store(p, q);
        bank
    }

    pub fn store(&self, p: &KitPattern, q: &KitParams) {
        for (v, row) in p.hits.iter().enumerate() {
            for w in 0..WORDS {
                let word = row[w * 8..w * 8 + 8]
                    .iter()
                    .enumerate()
                    .fold(0u64, |acc, (i, b)| acc | (u64::from(*b) << (i * 8)));
                self.hits[v][w].store(word, Ordering::Relaxed);
            }
        }
        self.length.store(p.length, Ordering::Relaxed);
        self.on.store(q.on, Ordering::Relaxed);
        self.tune.store(q.tune.to_bits(), Ordering::Relaxed);
        self.decay.store(q.decay.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn load(&self) -> (KitPattern, KitParams) {
        let mut hits = [[0u8; STEPS]; MAX_LANES];
        for (v, row) in hits.iter_mut().enumerate() {
            for (i, hit) in row.iter_mut().enumerate() {
                *hit = (self.hits[v][i / 8].load(Ordering::Relaxed) >> ((i % 8) * 8)) as u8;
            }
        }
        (
            KitPattern {
                length: self.length.load(Ordering::Relaxed),
                hits,
            },
            KitParams {
                on: self.on.load(Ordering::Relaxed),
                tune: f32::from_bits(self.tune.load(Ordering::Relaxed)),
                decay: f32::from_bits(self.decay.load(Ordering::Relaxed)),
            },
        )
    }
}

impl Default for KitBank {
    fn default() -> Self {
        Self::new(&KitPattern::default(), &KitParams::default())
    }
}

/// How hard each drum drives its saturation. A drum never reaches one.
const GLUE: f32 = 1.8;

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
    /// The second part's level and its fall: the snare's body, the kick's
    /// click.
    tone: f32,
    tone_fall: f32,
    /// The pitch above the resting one, falling by `bend_fall`, in Hz.
    bend: f32,
    bend_fall: f32,
    /// Resting pitch, in cycles a sample.
    rest: f32,
    /// Cycles, 0 to 1.
    phase: f32,
    /// A one-pole lowpass state, for the noise's highpass.
    lp1: f32,
}

/// A voice's next sample, given a noise sample.
type Render = fn(&mut Voice, noise: f32, hp: f32) -> f32;

/// A sine falling from body to sub (`design/drum-programming.md` section 3:
/// the body is what small speakers hear), with a few milliseconds of noise
/// on top as the click.
fn kick(v: &mut Voice, noise: f32, _hp: f32) -> f32 {
    let out = ((v.phase * core::f32::consts::TAU).sin() + noise * v.tone) * v.amp;
    v.phase += v.rest + v.bend;
    v.phase -= v.phase.floor();
    v.bend *= v.bend_fall;
    v.tone *= v.tone_fall;
    out
}

fn snare(v: &mut Voice, noise: f32, hp: f32) -> f32 {
    let body = (v.phase * core::f32::consts::TAU).sin() * v.tone;
    v.phase += v.rest;
    v.phase -= v.phase.floor();
    v.tone *= v.tone_fall;
    v.lp1 += hp * (noise - v.lp1);
    (body + (noise - v.lp1)) * v.amp
}

fn hat(v: &mut Voice, noise: f32, hp: f32) -> f32 {
    // One highpass: the fizz, with enough of it left to be heard. Two threw
    // away all but a fifth of the noise.
    v.lp1 += hp * (noise - v.lp1);
    (noise - v.lp1) * v.amp
}

impl LaneKind {
    fn render(self) -> Render {
        match self {
            Self::Kick => kick,
            Self::Snare => snare,
            Self::Hat => hat,
        }
    }
}

/// One lane's voice: its drum sounding, and what its next hit cut, fading
/// out. Built off the audio thread, boxed, and never moved once it plays.
#[derive(Debug)]
pub struct Lane {
    kind: LaneKind,
    voice: Voice,
    /// What the voice was when its next hit cut it.
    tail: Voice,
    tail_left: f32,
    /// The one-pole coefficient its noise is highpassed with: the snare's
    /// and the hat's. The kick's noise is not.
    hp: f32,
}

impl Lane {
    /// A lane playing `kind`, silent. Allocates: never on the audio thread.
    pub fn new(kind: LaneKind, sample_rate: f32) -> Box<Self> {
        let hp = match kind {
            LaneKind::Kick => 0.0,
            LaneKind::Snare => one_pole(1_200.0, sample_rate),
            LaneKind::Hat => one_pole(5_000.0, sample_rate),
        };
        Box::new(Self {
            kind,
            voice: Voice::default(),
            tail: Voice::default(),
            tail_left: 0.0,
            hp,
        })
    }

    pub fn kind(&self) -> LaneKind {
        self.kind
    }

    fn sounding(&self) -> bool {
        self.voice.sounding || self.tail_left > 0.0
    }
}

pub struct Kit {
    sample_rate: f32,
    /// The lanes, by slot. Only the pointers are the kit's.
    lanes: [Option<Box<Lane>>; MAX_LANES],
    noise: Rng,
    /// Tape time: how far between the last two samples the kit made the
    /// output is, and those two samples. See `process_at`.
    tape_at: f32,
    tape_last: f32,
    tape_next: f32,
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
        Self::seeded(sample_rate, 0)
    }

    /// A kit whose noise is salted by `salt` (`rng::salt` of its place in
    /// the pool), so two drums tracks playing one grid are not the same
    /// hiss. Salt 0 is the seed the kit always had.
    pub fn seeded(sample_rate: f32, salt: u32) -> Self {
        Self {
            sample_rate,
            lanes: Default::default(),
            noise: Rng::new(0x005E_EDD2 ^ salt),
            tape_at: 0.0,
            tape_last: 0.0,
            tape_next: 0.0,
        }
    }

    /// Put `lane` in slot `v`, or empty it with none, and hand back what was
    /// there, to be dropped off the audio thread. Lands at once: a lane taken
    /// out mid-hit stops where it is.
    pub fn swap_lane(&mut self, v: usize, lane: Option<Box<Lane>>) -> Option<Box<Lane>> {
        core::mem::replace(&mut self.lanes[v], lane)
    }

    /// The `STANDARD` lanes built into their slots, replacing what was
    /// there. Allocates: for a kit not yet playing, as a test or a render
    /// with no session has.
    pub fn standard_lanes(&mut self) {
        for (v, kind) in STANDARD.into_iter().enumerate() {
            self.lanes[v] = Some(Lane::new(kind, self.sample_rate));
        }
    }

    /// Nothing sounding, as if no hit had ever happened.
    pub fn reset(&mut self) {
        for lane in self.lanes.iter_mut().flatten() {
            lane.voice = Voice::default();
            lane.tail = Voice::default();
            lane.tail_left = 0.0;
        }
        self.tape_at = 0.0;
        self.tape_last = 0.0;
        self.tape_next = 0.0;
    }

    /// One frame on a tape running at `rate` (0 to 1): the kit slows with
    /// everything else, like a finger on the reel. At full speed this is
    /// `process`, sample for sample. Slower, the kit makes its samples at
    /// that rate and the output reads between them, so a hit drops in pitch
    /// and rings longer as the tape slows.
    #[inline]
    pub fn process_at(&mut self, rate: f32) -> f32 {
        if rate >= 1.0 {
            self.tape_at = 0.0;
            self.tape_last = self.tape_next;
            self.tape_next = self.process();
            return self.tape_next;
        }
        self.tape_at += rate.max(0.0);
        while self.tape_at >= 1.0 {
            self.tape_at -= 1.0;
            self.tape_last = self.tape_next;
            self.tape_next = self.process();
        }
        self.tape_last + (self.tape_next - self.tape_last) * self.tape_at
    }

    /// Whether anything is still sounding.
    pub fn sounding(&self) -> bool {
        self.lanes.iter().flatten().any(|l| l.sounding())
    }

    /// Strike slot `v`'s lane at `velocity`, 0 to 1; a slot with no lane
    /// ignores it. A voice still sounding hands itself to its tail, which
    /// fades out under the new hit.
    pub fn hit(&mut self, v: usize, velocity: f32, p: &KitParams) {
        if velocity <= 0.0 {
            return;
        }
        let sr = self.sample_rate;
        let Some(lane) = self.lanes[v].as_deref_mut() else {
            return;
        };
        if lane.voice.sounding {
            lane.tail = lane.voice;
            lane.tail_left = CUT_MS * 0.001 * sr;
        }
        let tune = (p.tune / 12.0).exp2();
        let decay = p.decay.clamp(0.1, 10.0);
        let voice = &mut lane.voice;
        // Squared, as velocity usually maps to level, so the glue after the
        // sum does not flatten a ghost note into a normal one.
        *voice = Voice {
            sounding: true,
            amp: velocity * velocity,
            ..Voice::default()
        };
        match lane.kind {
            LaneKind::Kick => {
                voice.rest = 50.0 * tune / sr;
                voice.bend = 130.0 * tune / sr;
                voice.bend_fall = fall_over(0.15, sr);
                voice.tone = 0.5;
                voice.tone_fall = fall_over(0.008, sr);
                voice.fall = fall_over(0.6 * decay, sr);
                voice.amp *= 0.9;
            }
            LaneKind::Snare => {
                voice.rest = 185.0 * tune / sr;
                voice.tone = 0.5;
                voice.tone_fall = fall_over(0.12 * decay, sr);
                voice.fall = fall_over(0.35 * decay, sr);
                voice.amp *= 0.8;
            }
            LaneKind::Hat => {
                voice.fall = fall_over(0.12 * decay, sr);
                voice.amp *= 1.1;
            }
        }
    }

    /// One frame, mono. Each drum is glued by a soft saturation on its own,
    /// as if on its own channel, then the lanes are summed. The glue lifts the
    /// body of each hit against its peak, so the kit holds its own beside a
    /// sustained patch rather than being only transients the limiter takes
    /// away. On its own channel, a loud kick cannot squash the hat on top of
    /// it, as one shared saturation did. Each drum stays under one; the sum
    /// can pass it, and the arrangement's limiter holds it. A voice that has
    /// fallen below `SILENT` stops and plays an exact zero.
    #[inline]
    pub fn process(&mut self) -> f32 {
        if !self.sounding() {
            return 0.0;
        }
        let noise = self.noise.next_bipolar();
        let cut = 1.0 / (CUT_MS * 0.001 * self.sample_rate);
        let mut out = 0.0;
        for lane in self.lanes.iter_mut().flatten() {
            // This drum's own channel: its tail fading out and its hit.
            let render = lane.kind.render();
            let mut drum = 0.0;
            if lane.tail_left > 0.0 {
                let gain = lane.tail_left * cut;
                drum += render(&mut lane.tail, noise, lane.hp) * gain;
                lane.tail_left = (lane.tail_left - 1.0).max(0.0);
            }
            let voice = &mut lane.voice;
            if voice.sounding {
                drum += render(voice, noise, lane.hp);
                voice.amp *= voice.fall;
                if voice.amp < SILENT {
                    voice.sounding = false;
                }
            }
            if drum != 0.0 {
                out += (drum * GLUE).tanh();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A kit with the standard lanes, salted by `salt`.
    fn standard(salt: u32) -> Kit {
        let mut kit = Kit::seeded(SR, salt);
        kit.standard_lanes();
        kit
    }

    fn render(kit: &mut Kit, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| kit.process()).collect()
    }

    #[test]
    fn every_voice_sounds_then_falls_to_an_exact_zero() {
        for v in 0..STANDARD.len() {
            let mut kit = standard(0);
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
    fn a_kit_bank_hands_over_what_it_was_given() {
        let p = KitPattern {
            length: 32,
            hits: std::array::from_fn(|v| std::array::from_fn(|i| ((i * 11 + v * 29) % 128) as u8)),
        };
        let q = KitParams {
            on: true,
            tune: -3.0,
            decay: 1.3,
        };
        let bank = KitBank::default();
        assert_eq!(bank.load(), (KitPattern::default(), KitParams::default()));
        bank.store(&p, &q);
        assert_eq!(bank.load(), (p, q));
    }

    #[test]
    fn two_salts_are_two_hisses() {
        let p = KitParams::default();
        let mut a = standard(0);
        let mut b = standard(crate::rng::salt(1));
        a.hit(HAT, 1.0, &p);
        b.hit(HAT, 1.0, &p);
        assert_ne!(render(&mut a, 480), render(&mut b, 480));
    }

    #[test]
    fn nothing_hit_is_silence() {
        let mut kit = standard(0);
        assert!(render(&mut kit, 4_800).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn velocity_scales_a_hit() {
        let peak = |vel: f32| {
            let mut kit = standard(0);
            kit.hit(KICK, vel, &KitParams::default());
            render(&mut kit, 4_800)
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()))
        };
        // Half velocity is about 7 dB down, through the curve and the glue.
        let (soft, full) = (peak(0.5), peak(1.0));
        assert!(soft < full * 0.55, "{soft} against {full}");
        assert!(soft > full * 0.3, "{soft} against {full}");
    }

    #[test]
    fn a_voice_hit_again_mid_decay_does_not_click() {
        // The kick is a sine, so a cut with no fade jumps by however far the
        // old one was from zero. Compare the biggest frame-to-frame step at
        // the second hit with the biggest within a single hit.
        let p = KitParams::default();
        let mut kit = standard(0);
        kit.hit(KICK, 1.0, &p);
        let first = render(&mut kit, 2_000);
        let steady = first
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        let mut kit = standard(0);
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
