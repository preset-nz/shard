//! The step clock: a grid that retriggers the patch.
//!
//! The first in-app trigger source (Georg, 2026-09-14: "being able to set
//! the 4/8/16 steps to trigger the patch"). A step that is on plays the
//! material's pass once, which restarts the envelope with it, since the
//! envelope runs on the player's position. Between steps nothing sounds
//! (Georg, the same day, with no step set: "I'd assume it is silent"): the
//! pass stops at the edge of the window, and the cloud stops throwing grains
//! and lets the ones in flight play out. The effects keep their state. What
//! a trigger should render down and what stays live is still open, and this
//! is the place to find out.
//!
//! **The steps belong to the tracker, the level above the patch** (Georg,
//! the same day). Tempo, swing and the pattern are not parameters: they do
//! not live in the table, a patch does not save them, and an LFO cannot
//! reach them. They cross to the audio thread through `StepBank` and into
//! the engine with `Engine::set_steps`, once a block.
//!
//! **A step is always a sixteenth; length shortens the loop.** Four steps
//! at 120 is one beat, half a second, not a bar of quarter notes. That keeps
//! swing meaning one thing, since swing belongs to sixteenths
//! (`design/drum-programming.md`), and leaves room for tracks of different
//! lengths running against each other later.
//!
//! Sample-accurate: the clock is advanced per frame inside the engine's
//! loop, so a trigger lands on the frame it is due, not at a block edge.
//! Swing delays every second step by a share of a step.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepParams {
    /// Whether the steps run at all.
    pub on: bool,
    pub tempo_bpm: f32,
    /// How many steps the track runs, 1 to `STEPS`; the tracker offers 4, 8,
    /// 16, 32 and 64.
    pub length: u32,
    /// 0 to 0.5 of a step, applied to the even-numbered steps (the second,
    /// fourth and so on). Drum machines count the same thing from 50 % to
    /// 75 %; the UI converts.
    pub swing: f32,
    /// One bit per step, bit 0 first.
    pub pattern: u64,
    /// Each step's pitch in semitones, step one first. Varispeed, as the
    /// octave is (Georg, 2026-09-14): up is faster and shorter, and grains
    /// are transposed with it. Read up to `PITCH_RANGE` either way.
    pub pitches: [i8; STEPS],
    /// How long each step's note is held, in sixteenths, step one first. Zero
    /// is the patch's own Hold time (`patch.hold`); anything else replaces it
    /// for that step, up to `HOLD_MAX`. Read only while the patch's Length is
    /// Hold, since Loop and Sample are not gated.
    pub holds: [u8; STEPS],
    /// How hard each step plays, 0 to `VELOCITY_MAX`, step one first. The
    /// whole note is scaled by it; full is unity, so a track that never sets
    /// one sounds as it always did.
    pub velocities: [u8; STEPS],
    /// Each step's microtiming in milliseconds, step one first: negative is
    /// early, positive late (`design/drum-programming.md` section 5). The clock
    /// turns it into samples as it goes, and holds it to a quarter of a step
    /// either way: a fifth of a step, which with the heaviest swing (half a
    /// step) still leaves every step strictly after the one before, so a
    /// nudge can never swap two steps or land them together. Step one cannot
    /// go early, since nothing comes before the loop's start.
    pub nudges: [i8; STEPS],
}

/// The steps a pattern holds: four bars of sixteen.
pub const STEPS: usize = 64;
/// Semitones a step can be pitched, either way.
pub const PITCH_RANGE: i8 = 24;
/// The longest a step can be held, in sixteenths: one bar.
pub const HOLD_MAX: u8 = 16;
/// How far a nudge can reach, as a share of a step. A fifth, because swing can
/// already delay every second step by up to half: half plus a fifth is still
/// before the next step's earliest, a step minus a fifth.
pub const NUDGE_REACH: f32 = 0.2;
/// The most a step can be nudged, in milliseconds, either way.
pub const NUDGE_MAX_MS: i8 = 50;
/// A step at full velocity, as MIDI counts it.
pub const VELOCITY_MAX: u8 = 127;

impl Default for StepParams {
    /// Off, and four on the floor, so switching on does something.
    fn default() -> Self {
        Self {
            on: false,
            tempo_bpm: 120.0,
            length: 16,
            swing: 0.0,
            pattern: 0b0001_0001_0001_0001,
            pitches: [0; STEPS],
            holds: [0; STEPS],
            velocities: [VELOCITY_MAX; STEPS],
            nudges: [0; STEPS],
        }
    }
}

impl StepParams {
    /// Step `k`'s hold in sixteenths, in range. Zero means the patch's own.
    #[inline]
    pub fn hold_of(&self, k: u32) -> u32 {
        u32::from(self.holds[k as usize % STEPS].min(HOLD_MAX))
    }

    /// Step `k`'s nudge in samples at `sample_rate`, held to a fifth of a step
    /// (`step_len` samples) either way, and to zero if early on step one.
    #[inline]
    pub fn nudge_samples(&self, k: u32, sample_rate: f32, step_len: f32) -> f32 {
        let ms = f32::from(self.nudges[k as usize % STEPS].clamp(-NUDGE_MAX_MS, NUDGE_MAX_MS));
        let reach = NUDGE_REACH * step_len;
        let samples = (ms * 0.001 * sample_rate).clamp(-reach, reach);
        if k == 0 {
            samples.max(0.0)
        } else {
            samples
        }
    }

    /// Step `k`'s velocity as a gain, 0 to 1.
    #[inline]
    pub fn velocity_of(&self, k: u32) -> f32 {
        f32::from(self.velocities[k as usize % STEPS].min(VELOCITY_MAX)) / f32::from(VELOCITY_MAX)
    }

    /// Step `k`'s pitch as a read ratio and in semitones, in range.
    #[inline]
    pub fn pitch_of(&self, k: u32) -> (f32, f32) {
        let semis = self.pitches[k as usize % STEPS].clamp(-PITCH_RANGE, PITCH_RANGE) as f32;
        ((semis / 12.0).exp2(), semis)
    }
}

/// The steps, shared between whatever edits the tracker and the audio thread.
///
/// The same idea as `ParamBank`, for numbers that are not parameters.
/// Stored whole, read whole once a block. A read that lands between two
/// fields of a store plays one block with half the old tracker, which nobody
/// can hear and the next block corrects.
pub struct StepBank {
    on: AtomicBool,
    tempo_bpm: AtomicU32,
    length: AtomicU32,
    swing: AtomicU32,
    pattern: AtomicU64,
    /// The pitches, a byte a step: steps one to eight, then nine to sixteen.
    pitches: [AtomicU64; WORDS],
    /// The holds, a byte a step, packed the same way.
    holds: [AtomicU64; WORDS],
    /// The velocities, a byte a step, packed the same way.
    velocities: [AtomicU64; WORDS],
    /// The nudges, a byte a step, packed like the pitches.
    nudges: [AtomicU64; WORDS],
}

/// Words of eight steps a byte each.
const WORDS: usize = STEPS / 8;

/// Eight pitches into a word, step one in the low byte.
fn pack(pitches: &[i8]) -> u64 {
    pitches
        .iter()
        .enumerate()
        .fold(0, |w, (i, p)| w | (u64::from(*p as u8) << (i * 8)))
}

/// Eight bytes into a word, step one in the low byte.
fn pack_u8(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .enumerate()
        .fold(0, |w, (i, b)| w | (u64::from(*b) << (i * 8)))
}

impl StepBank {
    pub fn new(p: StepParams) -> Self {
        Self {
            on: AtomicBool::new(p.on),
            tempo_bpm: AtomicU32::new(p.tempo_bpm.to_bits()),
            length: AtomicU32::new(p.length),
            swing: AtomicU32::new(p.swing.to_bits()),
            pattern: AtomicU64::new(p.pattern),
            pitches: std::array::from_fn(|w| AtomicU64::new(pack(&p.pitches[w * 8..w * 8 + 8]))),
            holds: std::array::from_fn(|w| AtomicU64::new(pack_u8(&p.holds[w * 8..w * 8 + 8]))),
            velocities: std::array::from_fn(|w| {
                AtomicU64::new(pack_u8(&p.velocities[w * 8..w * 8 + 8]))
            }),
            nudges: std::array::from_fn(|w| AtomicU64::new(pack(&p.nudges[w * 8..w * 8 + 8]))),
        }
    }

    pub fn store(&self, p: &StepParams) {
        self.on.store(p.on, Ordering::Relaxed);
        self.tempo_bpm
            .store(p.tempo_bpm.to_bits(), Ordering::Relaxed);
        self.length.store(p.length, Ordering::Relaxed);
        self.swing.store(p.swing.to_bits(), Ordering::Relaxed);
        self.pattern.store(p.pattern, Ordering::Relaxed);
        for w in 0..WORDS {
            self.pitches[w].store(pack(&p.pitches[w * 8..w * 8 + 8]), Ordering::Relaxed);
            self.holds[w].store(pack_u8(&p.holds[w * 8..w * 8 + 8]), Ordering::Relaxed);
            self.nudges[w].store(pack(&p.nudges[w * 8..w * 8 + 8]), Ordering::Relaxed);
            self.velocities[w].store(pack_u8(&p.velocities[w * 8..w * 8 + 8]), Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn load(&self) -> StepParams {
        let mut pitches = [0i8; STEPS];
        let mut holds = [0u8; STEPS];
        let mut velocities = [0u8; STEPS];
        let mut nudges = [0i8; STEPS];
        for i in 0..STEPS {
            pitches[i] = (self.pitches[i / 8].load(Ordering::Relaxed) >> ((i % 8) * 8)) as u8 as i8;
            holds[i] = (self.holds[i / 8].load(Ordering::Relaxed) >> ((i % 8) * 8)) as u8;
            nudges[i] = (self.nudges[i / 8].load(Ordering::Relaxed) >> ((i % 8) * 8)) as u8 as i8;
            velocities[i] = (self.velocities[i / 8].load(Ordering::Relaxed) >> ((i % 8) * 8)) as u8;
        }
        StepParams {
            on: self.on.load(Ordering::Relaxed),
            tempo_bpm: f32::from_bits(self.tempo_bpm.load(Ordering::Relaxed)),
            length: self.length.load(Ordering::Relaxed),
            swing: f32::from_bits(self.swing.load(Ordering::Relaxed)),
            pattern: self.pattern.load(Ordering::Relaxed),
            pitches,
            holds,
            velocities,
            nudges,
        }
    }
}

impl Default for StepBank {
    fn default() -> Self {
        Self::new(StepParams::default())
    }
}

pub struct StepClock {
    sample_rate: f32,
    /// Samples into the pattern.
    pos: f32,
    /// The step the last frame was in, or none before the first.
    step: Option<u32>,
    running: bool,
}

impl StepClock {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            pos: 0.0,
            step: None,
            running: false,
        }
    }

    /// Start from the top, or stop. Starting always begins at step one.
    pub fn set_running(&mut self, running: bool) {
        if running && !self.running {
            self.pos = 0.0;
            self.step = None;
        }
        self.running = running;
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// The step the clock is in, for the UI. None while stopped.
    pub fn current(&self) -> Option<u32> {
        if self.running {
            self.step
        } else {
            None
        }
    }

    /// A sixteenth note, in samples.
    pub fn step_len(&self, p: &StepParams) -> f32 {
        self.sample_rate * 60.0 / p.tempo_bpm.clamp(20.0, 300.0) / 4.0
    }

    /// Where step `k` begins, with swing and its own nudge.
    fn onset(&self, k: u32, step_len: f32, p: &StepParams) -> f32 {
        let swing = p.swing.clamp(0.0, 0.5);
        let late = if k % 2 == 1 { swing * step_len } else { 0.0 };
        k as f32 * step_len + late + p.nudge_samples(k, self.sample_rate, step_len)
    }

    /// One frame on. The step, when a step that is on begins on this frame.
    #[inline]
    pub fn tick(&mut self, p: &StepParams) -> Option<u32> {
        if !self.running {
            return None;
        }
        let step_len = self.step_len(p);
        let length = p.length.clamp(1, STEPS as u32);
        let total = length as f32 * step_len;
        // Which step contains `pos`: the last whose onset is at or before it.
        // A step nudged early has begun before the grid says, and one nudged
        // late has not, so look a step either way.
        let mut k = ((self.pos / step_len) as u32).min(length - 1);
        while k > 0 && self.onset(k, step_len, p) > self.pos {
            k -= 1;
        }
        while k + 1 < length && self.onset(k + 1, step_len, p) <= self.pos {
            k += 1;
        }
        // Before step one's own onset (a late nudge on it) the clock is still
        // in the last step of the loop before; on the very first pass that
        // means no step at all yet, and nothing triggers.
        let before_first = k == 0 && self.onset(0, step_len, p) > self.pos;
        let k = if before_first { length - 1 } else { k };
        let entered = self.step != Some(k) && !(before_first && self.step.is_none());
        self.step = Some(k);
        self.pos += 1.0;
        if self.pos >= total {
            self.pos -= total;
        }
        (entered && (p.pattern >> k) & 1 == 1).then_some(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn triggers(p: &StepParams, frames: usize) -> Vec<usize> {
        let mut c = StepClock::new(SR);
        c.set_running(true);
        (0..frames).filter(|_| c.tick(p).is_some()).collect()
    }

    #[test]
    fn four_on_the_floor_at_120_is_every_half_second() {
        let p = StepParams {
            on: true,
            tempo_bpm: 120.0,
            length: 16,
            swing: 0.0,
            pattern: 0b0001_0001_0001_0001,
            pitches: [0; STEPS],
            holds: [0; STEPS],
            velocities: [VELOCITY_MAX; STEPS],
            nudges: [0; STEPS],
        };
        let t = triggers(&p, SR as usize * 4);
        // Steps 0, 4, 8, 12 of a 16-step bar at 120: every 0.5 s, 8 in 4 s.
        assert_eq!(t.len(), 8, "{t:?}");
        for (i, at) in t.iter().enumerate() {
            assert_eq!(*at, i * 24_000, "{t:?}");
        }
    }

    #[test]
    fn four_steps_loop_every_beat_not_every_bar() {
        let p = StepParams {
            on: true,
            tempo_bpm: 120.0,
            length: 4,
            swing: 0.0,
            pattern: 0b0001,
            pitches: [0; STEPS],
            holds: [0; STEPS],
            velocities: [VELOCITY_MAX; STEPS],
            nudges: [0; STEPS],
        };
        let t = triggers(&p, SR as usize);
        // Four sixteenths at 120 is half a second; step one fires twice.
        assert_eq!(t, vec![0, 24_000]);
    }

    #[test]
    fn swing_delays_the_even_steps_only() {
        let straight = StepParams {
            on: true,
            tempo_bpm: 120.0,
            length: 4,
            swing: 0.0,
            pattern: 0b1111,
            pitches: [0; STEPS],
            holds: [0; STEPS],
            velocities: [VELOCITY_MAX; STEPS],
            nudges: [0; STEPS],
        };
        let swung = StepParams {
            swing: 0.5,
            ..straight
        };
        // One beat: a sixteenth is 6,000 samples, and half of one is 3,000.
        let a = triggers(&straight, SR as usize / 2);
        let b = triggers(&swung, SR as usize / 2);
        assert_eq!(a, vec![0, 6_000, 12_000, 18_000]);
        assert_eq!(b, vec![0, 9_000, 12_000, 21_000]);
    }

    #[test]
    fn starting_begins_at_step_one_and_stopping_reports_none() {
        let p = StepParams {
            pattern: 0b1,
            length: 4,
            ..Default::default()
        };
        let mut c = StepClock::new(SR);
        assert_eq!(c.tick(&p), None);
        assert_eq!(c.current(), None);
        c.set_running(true);
        for _ in 0..1_000 {
            c.tick(&p);
        }
        assert_eq!(c.current(), Some(0));
        c.set_running(false);
        assert_eq!(c.current(), None);
        c.set_running(true);
        assert_eq!(c.tick(&p), Some(0), "restarting fires step one again");
    }

    #[test]
    fn a_trigger_says_which_step_fired() {
        let p = StepParams {
            on: true,
            length: 4,
            pattern: 0b1010,
            ..Default::default()
        };
        let mut c = StepClock::new(SR);
        c.set_running(true);
        let fired: Vec<u32> = (0..24_000).filter_map(|_| c.tick(&p)).collect();
        assert_eq!(fired, vec![1, 3]);
    }

    #[test]
    fn a_pitch_out_of_range_is_read_at_the_end_of_it() {
        let mut p = StepParams::default();
        p.pitches[0] = 12;
        p.pitches[1] = i8::MIN;
        assert_eq!(p.pitch_of(0), (2.0, 12.0));
        assert_eq!(p.pitch_of(1).1, -f32::from(PITCH_RANGE));
    }

    #[test]
    fn the_bank_hands_back_exactly_what_it_was_given() {
        let mut pitches = [0i8; STEPS];
        for (i, p) in pitches.iter_mut().enumerate() {
            // Across the whole range, signed bytes and all, all sixty-four.
            *p = ((i as i32 * 5) % 97 - 48) as i8;
        }
        pitches[63] = -24;
        let p = StepParams {
            on: true,
            tempo_bpm: 87.5,
            length: 8,
            swing: 0.12,
            pattern: 0xA5A5_5A5A_F0F0_0F0F,
            pitches,
            holds: {
                let mut h = [0u8; STEPS];
                for (i, v) in h.iter_mut().enumerate() {
                    *v = (i as u8).wrapping_mul(5) % 20;
                }
                h
            },
            velocities: {
                let mut v = [0u8; STEPS];
                for (i, x) in v.iter_mut().enumerate() {
                    *x = (i as u8).wrapping_mul(3) % 128;
                }
                v
            },
            nudges: {
                let mut n = [0i8; STEPS];
                for (i, x) in n.iter_mut().enumerate() {
                    *x = ((i as i32 * 7) % 101 - 50) as i8;
                }
                n
            },
        };
        let bank = StepBank::default();
        assert_eq!(bank.load(), StepParams::default());
        bank.store(&p);
        assert_eq!(bank.load(), p);
    }

    #[test]
    fn a_hold_is_brought_into_range_and_zero_means_the_patchs() {
        let mut p = StepParams::default();
        p.holds[0] = 0;
        p.holds[1] = 4;
        p.holds[2] = 200;
        assert_eq!(p.hold_of(0), 0);
        assert_eq!(p.hold_of(1), 4);
        assert_eq!(p.hold_of(2), u32::from(HOLD_MAX));
    }

    #[test]
    fn a_sixty_four_step_track_reaches_its_last_step_and_wraps() {
        // Steps one and sixty-four of a four-bar track at 120 bpm: a sixteenth
        // is 125 ms, so the last begins at 7.875 s and the bar comes round at 8.
        let p = StepParams {
            on: true,
            length: 64,
            pattern: 1 | (1 << 63),
            ..Default::default()
        };
        let t = triggers(&p, SR as usize * 17);
        let at = |n: f32| (n * SR) as usize;
        assert_eq!(t.len(), 5, "{t:?}");
        for (got, want) in t.iter().zip([0.0, 7.875, 8.0, 15.875, 16.0]) {
            assert!(
                (*got as i64 - at(want) as i64).abs() <= 1,
                "{got} vs {want}"
            );
        }
    }

    #[test]
    fn a_step_past_the_tracks_length_never_plays() {
        let p = StepParams {
            on: true,
            length: 16,
            pattern: 1 << 20,
            ..Default::default()
        };
        assert!(triggers(&p, SR as usize * 6).is_empty());
    }

    #[test]
    fn velocity_is_a_gain_from_silence_to_unity_and_defaults_to_full() {
        let mut p = StepParams::default();
        assert_eq!(p.velocity_of(0), 1.0, "a step never given one is at full");
        p.velocities[1] = 0;
        p.velocities[2] = 64;
        p.velocities[3] = 255;
        assert_eq!(p.velocity_of(1), 0.0);
        assert!((p.velocity_of(2) - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(p.velocity_of(3), 1.0, "out of range is full, not louder");
    }

    /// The frame step `k` first triggers on, over a few bars at 120 bpm
    /// (a sixteenth is 6 000 samples at 48 kHz).
    fn first_trigger(p: &StepParams, k: usize) -> usize {
        let mut c = StepClock::new(SR);
        c.set_running(true);
        for frame in 0..SR as usize * 2 {
            if c.tick(p) == Some(k as u32) {
                return frame;
            }
        }
        panic!("step {k} never triggered");
    }

    fn only(k: usize, nudge: i8) -> StepParams {
        let mut p = StepParams {
            on: true,
            length: 16,
            pattern: 1 << k,
            ..Default::default()
        };
        p.nudges[k] = nudge;
        p
    }

    #[test]
    fn a_nudge_moves_a_step_by_that_many_milliseconds() {
        let on_the_grid = first_trigger(&only(4, 0), 4);
        assert_eq!(on_the_grid, 24_000, "step five at 120 bpm is 0.5 s in");
        // 15 ms late is 720 samples; 10 ms early is 480.
        let late = first_trigger(&only(4, 15), 4);
        let early = first_trigger(&only(4, -10), 4);
        assert!((late as i64 - 24_720).abs() <= 1, "{late}");
        assert!((early as i64 - 23_520).abs() <= 1, "{early}");
    }

    #[test]
    fn a_nudge_is_held_to_a_fifth_of_a_step_so_steps_never_swap() {
        // 50 ms is more than a fifth of a 125 ms step: held to 25 ms, which
        // is 1 200 samples.
        let late = first_trigger(&only(4, 50), 4);
        assert!((late as i64 - (24_000 + 1_200)).abs() <= 1, "{late}");
        let early = first_trigger(&only(4, -50), 4);
        assert!((early as i64 - (24_000 - 1_200)).abs() <= 1, "{early}");
    }

    #[test]
    fn the_first_step_can_be_late_but_not_early() {
        assert_eq!(
            first_trigger(&only(0, -20), 0),
            0,
            "nothing comes before the start"
        );
        assert!((first_trigger(&only(0, 10), 0) as i64 - 480).abs() <= 1);
    }

    #[test]
    fn nudged_steps_keep_their_order_and_each_fires_once_a_bar() {
        // Every step on, alternately pushed and pulled as far as allowed, with
        // swing on: the triggers must still come in step order, once each.
        let mut p = StepParams {
            on: true,
            length: 16,
            pattern: 0xFFFF,
            swing: 0.5,
            ..Default::default()
        };
        for (i, n) in p.nudges.iter_mut().enumerate() {
            *n = if i % 2 == 0 { -50 } else { 50 };
        }
        let mut c = StepClock::new(SR);
        c.set_running(true);
        let mut seen = Vec::new();
        for _ in 0..SR as usize * 4 {
            if let Some(k) = c.tick(&p) {
                seen.push(k);
            }
        }
        // Two bars of sixteen in four seconds at 120 bpm.
        assert_eq!(seen.len(), 32, "{seen:?}");
        for bar in seen.chunks(16) {
            assert_eq!(bar, (0..16).collect::<Vec<u32>>().as_slice());
        }
    }

    #[test]
    fn no_nudge_leaves_the_grid_exactly_as_it_was() {
        let p = StepParams {
            on: true,
            pattern: 0xFFFF,
            ..Default::default()
        };
        let t = triggers(&p, SR as usize * 2);
        for (k, at) in t.iter().enumerate() {
            assert_eq!(*at, k * 6_000, "step {k}");
        }
    }
}
