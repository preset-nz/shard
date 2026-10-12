//! The tracker: the level above the patch.
//!
//! A patch is an instrument, a sound with no clock. The tracker decides when
//! it plays (Georg, 2026-09-14: "so the tracker is one level up from the
//! patch"). It holds what belongs to the arrangement rather than to a sound: the
//! tempo, the swing, and tracks of steps. Loading a different patch leaves
//! the beat alone, and a patch never carries a pattern.
//!
//! **Several tracks, of two kinds** (epic 28). Every one sounds while the
//! transport runs.
//!
//! - **A melody track** plays its own patch: up to four bars of sixteen
//!   steps, each with its own pitch and hold, a fader (`gain`), and the
//!   patch it plays (`track.patch` in the tree; `patch` here is that patch's
//!   runtime id, for the webview's *Plays* pickers, and is never written
//!   back). Monophonic, as in a tracker: a new step cuts the last. Up to
//!   `shard_dsp::VOICES` of them.
//! - **A drums track** (story d) plays a synthesised kit: a kick, snare and
//!   hat grid (`drums`), its own length, fader, switch, tune and decay, and
//!   no patch. Up to `shard_dsp::KITS` of them.
//!
//! **One order, two pools.** The tracks are one list, in the arrangement's
//! order `tracks`, and a drums track can sit anywhere in it. Each kind maps
//! to its own pool by its place among its own kind (`Slot`): the *k*-th
//! melody track plays on voice *k*, the *k*-th drums track on kit *k*. So
//! adding or moving a drums track never moves a melody track's voice.
//!
//! Saved in the same `.shard` file as the patch, in the arrangement node
//! (`object_model::write_tracker`).

use serde::{Deserialize, Serialize};
use shard_dsp::arrangement::{TRACK_GAIN_DEFAULT, TRACK_GAIN_MAX};
use shard_dsp::kit::{
    KitParams, KitPattern, DECAY_MAX, DECAY_MIN, HAT, HIT_MAX, KICK, SNARE, TUNE_RANGE,
};
use shard_dsp::steps::{HOLD_MAX, NUDGE_MAX_MS, PITCH_RANGE, STEPS, VELOCITY_MAX};
use shard_dsp::{StepParams, KITS, VOICES};

pub const TEMPO_MIN: f32 = 40.0;
pub const TEMPO_MAX: f32 = 240.0;
/// Of a sixteenth, the delay of every second one. The UI shows swing the
/// drum-machine way, 50 % straight to 75 % heavy, which is this value times
/// 50 plus 50: the pocket at 56 % is 0.12 (`design/drum-programming.md`).
pub const SWING_MAX: f32 = 0.5;
/// The lengths a track can have, in sixteenths: up to four bars.
pub const LENGTHS: [u32; 5] = [4, 8, 16, 32, 64];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tracker {
    pub tempo: f32,
    /// 0 to `SWING_MAX` of a sixteenth, on the even steps.
    pub swing: f32,
    /// Melody and drums tracks, in the arrangement's order.
    pub tracks: Vec<Track>,
}

/// Where a track plays: the *k*-th of its kind, on voice *k* or kit *k*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Melody(usize),
    Drums(usize),
}

/// Each track's slot, from whether each is a drums track, in order. The one
/// mapping from a track's place to its voice or kit: the session and the
/// meters both go through it.
pub fn slots(drums: impl IntoIterator<Item = bool>) -> Vec<Slot> {
    let (mut melody, mut kits) = (0, 0);
    drums
        .into_iter()
        .map(|is_drums| {
            if is_drums {
                kits += 1;
                Slot::Drums(kits - 1)
            } else {
                melody += 1;
                Slot::Melody(melody - 1)
            }
        })
        .collect()
}

/// A drums track's kit: its kick, snare and hat grid, synthesised
/// (`shard_dsp::kit`), its switch and its sound. Each row is a velocity a
/// step, zero for none, so several drums land on one step. Its length and
/// fader are the track's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kit {
    /// Whether it plays. A new session's drums track starts off, as the
    /// Drums card did; Add Track › Drums arrives on.
    pub on: bool,
    /// Semitones, every drum together, up to `TUNE_RANGE` either way.
    pub tune: f32,
    /// A factor on every drum's decay, `DECAY_MIN` to `DECAY_MAX`.
    pub decay: f32,
    pub kick: Vec<u8>,
    pub snare: Vec<u8>,
    pub hat: Vec<u8>,
}

impl Default for Kit {
    /// The kit a new session has: off, with a beat in it, so switching it on
    /// does something.
    fn default() -> Self {
        let p = KitPattern::default();
        let q = KitParams::default();
        Self {
            on: q.on,
            tune: q.tune,
            decay: q.decay,
            kick: p.hits[KICK].to_vec(),
            snare: p.hits[SNARE].to_vec(),
            hat: p.hits[HAT].to_vec(),
        }
    }
}

impl Kit {
    /// What Add Track › Drums adds: on, an empty grid.
    pub fn blank() -> Self {
        Self {
            on: true,
            kick: vec![0; STEPS],
            snare: vec![0; STEPS],
            hat: vec![0; STEPS],
            ..Self::default()
        }
    }

    /// The grid for the engine, `length` steps long.
    pub fn pattern(&self, length: u32) -> KitPattern {
        let mut hits = [[0u8; STEPS]; shard_dsp::kit::VOICES];
        for (v, row) in [(KICK, &self.kick), (SNARE, &self.snare), (HAT, &self.hat)] {
            for (i, hit) in row.iter().take(STEPS).enumerate() {
                hits[v][i] = *hit;
            }
        }
        KitPattern { length, hits }
    }

    /// Its sound for the engine.
    pub fn params(&self) -> KitParams {
        KitParams {
            on: self.on,
            tune: self.tune,
            decay: self.decay,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    /// 4, 8, 16, 32 or 64 sixteenths.
    pub length: u32,
    /// Its fader in the mix, 0 to `TRACK_GAIN_MAX`.
    pub gain: f32,
    /// The runtime id of the patch it plays, as the session compiled it.
    /// Read only: a pick goes through `Session::set_track_patch`.
    pub patch: Option<u64>,
    /// Every step the track can hold, `STEPS` of them, step one first. Steps
    /// past `length` are kept, so shortening a track and lengthening it again
    /// gives them back. Blank on a drums track.
    pub steps: Vec<Step>,
    /// A drums track's kit; none on a melody track.
    pub drums: Option<Kit>,
}

/// One sixteenth of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    /// Whether it triggers the patch.
    pub on: bool,
    /// Semitones from the sample, up to `PITCH_RANGE` either way. Kept for a
    /// step that is off.
    pub pitch: i8,
    /// How long its note is held, in sixteenths; zero is the patch's own Hold
    /// time. Only heard while the patch's Length is Hold.
    pub hold: u8,
    /// How hard it plays, 0 to `VELOCITY_MAX`; full is the default, so a step
    /// never given one sounds as it always did.
    pub velocity: u8,
    /// Its timing in milliseconds, negative early and positive late, up to
    /// `NUDGE_MAX_MS`. The clock holds it to a fifth of a step.
    pub nudge: i8,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            on: false,
            pitch: 0,
            hold: 0,
            velocity: VELOCITY_MAX,
            nudge: 0,
        }
    }
}

impl Default for Tracker {
    fn default() -> Self {
        let p = StepParams::default();
        Self {
            tempo: p.tempo_bpm,
            swing: p.swing,
            tracks: vec![Track::default(), Track::drums()],
        }
    }
}

impl Default for Track {
    /// A new session's first track: the pattern the engine starts with.
    fn default() -> Self {
        let p = StepParams::default();
        Self {
            length: p.length,
            gain: TRACK_GAIN_DEFAULT,
            patch: None,
            steps: (0..STEPS)
                .map(|i| Step {
                    on: (p.pattern >> i) & 1 == 1,
                    pitch: p.pitches[i],
                    hold: p.holds[i],
                    velocity: p.velocities[i],
                    nudge: p.nudges[i],
                })
                .collect(),
            drums: None,
        }
    }
}

impl Track {
    /// What Add Track adds: sixteen empty steps at unity.
    pub fn blank() -> Self {
        Self {
            length: 16,
            gain: TRACK_GAIN_DEFAULT,
            patch: None,
            steps: vec![Step::default(); STEPS],
            drums: None,
        }
    }

    /// A new session's drums track: today's Drums lane, off, with its beat.
    pub fn drums() -> Self {
        Self {
            drums: Some(Kit::default()),
            ..Self::blank()
        }
    }

    /// What Add Track › Drums adds: on, an empty grid, sixteen steps at unity.
    pub fn blank_drums() -> Self {
        Self {
            drums: Some(Kit::blank()),
            ..Self::blank()
        }
    }

    pub fn is_drums(&self) -> bool {
        self.drums.is_some()
    }
}

impl Tracker {
    /// Brought into range: tempo and swing clamped, each length snapped to
    /// the nearest of 4, 8, 16, 32 and 64, each track padded or cut to exactly
    /// `STEPS` steps with pitches and holds in range, each fader in range, a
    /// kit's rows and sound in range, between one melody track and `VOICES`
    /// of them, and at most `KITS` drums tracks.
    /// Everything the UI or a hand-edited file sends passes through here
    /// before the engine hears it.
    pub fn sanitised(mut self) -> Self {
        let fallback = Self::default();
        self.tempo = if self.tempo.is_finite() {
            self.tempo.clamp(TEMPO_MIN, TEMPO_MAX)
        } else {
            fallback.tempo
        };
        self.swing = if self.swing.is_finite() {
            self.swing.clamp(0.0, SWING_MAX)
        } else {
            fallback.swing
        };
        if !self.tracks.iter().any(|t| !t.is_drums()) {
            self.tracks.insert(0, Track::default());
        }
        let (mut melody, mut kits) = (0, 0);
        self.tracks.retain(|t| {
            let count = if t.is_drums() { &mut kits } else { &mut melody };
            *count += 1;
            *count <= if t.is_drums() { KITS } else { VOICES }
        });
        let snap = |length: u32| {
            LENGTHS
                .iter()
                .copied()
                .min_by_key(|n| n.abs_diff(length))
                .unwrap_or(16)
        };
        for t in &mut self.tracks {
            if let Some(kit) = &mut t.drums {
                t.patch = None;
                let fallback = Kit::default();
                kit.tune = if kit.tune.is_finite() {
                    kit.tune.clamp(-TUNE_RANGE, TUNE_RANGE)
                } else {
                    fallback.tune
                };
                kit.decay = if kit.decay.is_finite() {
                    kit.decay.clamp(DECAY_MIN, DECAY_MAX)
                } else {
                    fallback.decay
                };
                for row in [&mut kit.kick, &mut kit.snare, &mut kit.hat] {
                    row.resize(STEPS, 0);
                    for hit in row.iter_mut() {
                        *hit = (*hit).min(HIT_MAX);
                    }
                }
            }
            t.length = snap(t.length);
            t.gain = if t.gain.is_finite() {
                t.gain.clamp(0.0, TRACK_GAIN_MAX)
            } else {
                TRACK_GAIN_DEFAULT
            };
            t.steps.resize(STEPS, Step::default());
            for step in &mut t.steps {
                step.pitch = step.pitch.clamp(-PITCH_RANGE, PITCH_RANGE);
                step.hold = step.hold.min(HOLD_MAX);
                step.velocity = step.velocity.min(VELOCITY_MAX);
                step.nudge = step.nudge.clamp(-NUDGE_MAX_MS, NUDGE_MAX_MS);
            }
        }
        self
    }

    /// Each track's slot, in order: which voice or kit plays it.
    pub fn slots(&self) -> Vec<Slot> {
        slots(self.tracks.iter().map(Track::is_drums))
    }

    /// The melody tracks, in order: the *k*-th plays on voice *k*.
    pub fn melody(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| !t.is_drums())
    }

    /// The drums tracks, in order: the *k*-th plays on kit *k*.
    pub fn drums(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.is_drums())
    }

    /// The transport: the steps running, at the arrangement's tempo and
    /// swing (`Engine::set_transport`).
    pub fn transport(&self) -> StepParams {
        StepParams {
            on: true,
            tempo_bpm: self.tempo,
            swing: self.swing,
            ..StepParams::default()
        }
    }

    /// What the `k`-th drums track's kit plays (`Engine::set_kit`), and its
    /// fader; none when there is no such track.
    pub fn kit(&self, k: usize) -> Option<(KitPattern, KitParams, f32)> {
        let t = self.drums().nth(k)?;
        let kit = t.drums.as_ref()?;
        Some((kit.pattern(t.length), kit.params(), t.gain))
    }

    /// What voice `k` plays (`Engine::set_voice_steps`): the `k`-th melody
    /// track's steps following the transport, or a blank track when there
    /// is no such track. The transport's half is filled in too.
    pub fn params(&self, k: usize) -> StepParams {
        let blank = Track::blank();
        let t = self.melody().nth(k).unwrap_or(&blank);
        let mut p = StepParams {
            on: true,
            tempo_bpm: self.tempo,
            length: t.length,
            swing: self.swing,
            pattern: 0,
            pitches: [0; STEPS],
            holds: [0; STEPS],
            velocities: [VELOCITY_MAX; STEPS],
            nudges: [0; STEPS],
        };
        for (i, step) in t.steps.iter().take(STEPS).enumerate() {
            p.pattern |= u64::from(step.on) << i;
            p.pitches[i] = step.pitch;
            p.holds[i] = step.hold;
            p.velocities[i] = step.velocity;
            p.nudges[i] = step.nudge;
        }
        p
    }
}

/// A whole-pattern edit, for entering music faster than one step at a time
/// (roadmap row 23 d). Each acts on one track, the one the tracker shows. Each
/// is brought into range afterwards by `sanitised`, so a bad number cannot
/// put the tracker out of range.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Edit {
    /// Empty bar `bar` (sixteen steps): no triggers, no pitch, no hold.
    ClearBar { bar: usize },
    /// Put these steps into bar `bar`, as copied from another.
    PasteBar { bar: usize, steps: Vec<Step> },
    /// Copy bar `from` over every other bar within the track's length.
    RepeatBar { from: usize },
    /// Play the pattern twice: copy the steps within the track's length after
    /// themselves and double the length. A track already at `STEPS` is left
    /// alone.
    Duplicate,
    /// Slide the steps within the track's length by `by` places, wrapping
    /// round: positive is later.
    Rotate { by: i32 },
    /// Move every pitch by `semis`, in the whole track or just one bar.
    Transpose { semis: i32, bar: Option<usize> },
}

/// Steps in a bar.
pub const BAR: usize = 16;

impl Tracker {
    /// What the edit is called in Undo.
    pub fn label_of(edit: &Edit) -> &'static str {
        match edit {
            Edit::ClearBar { .. } => "Clear a bar",
            Edit::PasteBar { .. } => "Paste a bar",
            Edit::RepeatBar { .. } => "Repeat a bar",
            Edit::Duplicate => "Duplicate the pattern",
            Edit::Rotate { .. } => "Rotate the steps",
            Edit::Transpose { .. } => "Transpose the steps",
        }
    }

    /// Apply a whole-pattern edit to track `track`. Out-of-range bars, a
    /// track that does not exist and a drums track (whose grid has no
    /// pitches to move yet) are ignored rather than guessed at.
    pub fn apply(mut self, track: usize, edit: &Edit) -> Self {
        self = self.sanitised();
        let Some(t) = self.tracks.get_mut(track).filter(|t| !t.is_drums()) else {
            return self;
        };
        let bars = (t.length as usize / BAR).max(1);
        let span = |bar: usize| bar * BAR..(bar + 1) * BAR;
        match edit {
            Edit::ClearBar { bar } if *bar < STEPS / BAR => {
                for step in &mut t.steps[span(*bar)] {
                    *step = Step::default();
                }
            }
            Edit::PasteBar { bar, steps } if *bar < STEPS / BAR => {
                for (dst, src) in t.steps[span(*bar)].iter_mut().zip(steps) {
                    *dst = *src;
                }
            }
            Edit::RepeatBar { from } if *from < STEPS / BAR => {
                let source: Vec<Step> = t.steps[span(*from)].to_vec();
                for bar in (0..bars).filter(|bar| bar != from) {
                    t.steps[span(bar)].copy_from_slice(&source);
                }
            }
            Edit::Duplicate if (t.length as usize) * 2 <= STEPS => {
                let n = t.length as usize;
                t.steps.copy_within(..n, n);
                t.length *= 2;
            }
            Edit::Rotate { by } => {
                let n = (t.length as usize).min(STEPS);
                let by = by.rem_euclid(n as i32) as usize;
                t.steps[..n].rotate_right(by);
            }
            Edit::Transpose { semis, bar } => {
                let range = match bar {
                    Some(b) if *b < STEPS / BAR => span(*b),
                    Some(_) => 0..0,
                    None => 0..STEPS,
                };
                for step in &mut t.steps[range] {
                    step.pitch = (i32::from(step.pitch) + semis)
                        .clamp(-i32::from(PITCH_RANGE), i32::from(PITCH_RANGE))
                        as i8;
                }
            }
            _ => {}
        }
        self.sanitised()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A track from a pattern (bit 0 first) and nothing else set.
    fn track(length: u32, pattern: u64) -> Track {
        let mut t = Track {
            length,
            ..Track::default()
        };
        for (i, step) in t.steps.iter_mut().enumerate() {
            step.on = (pattern >> i) & 1 == 1;
        }
        t
    }

    #[test]
    fn nonsense_is_brought_into_range() {
        let mut t = Tracker {
            tempo: f32::NAN,
            swing: 9.0,
            tracks: vec![track(11, 0)],
        };
        t.tracks[0].steps = vec![
            Step {
                on: true,
                pitch: 99,
                hold: 99,
                velocity: 200,
                nudge: -99,
            };
            3
        ];
        let t = t.sanitised();
        assert_eq!(t.tempo, Tracker::default().tempo);
        assert_eq!(t.swing, SWING_MAX);
        assert_eq!(t.tracks[0].length, 8, "11 is nearest to 8");
        assert_eq!(t.tracks[0].steps.len(), STEPS, "padded to every step");
        assert_eq!(
            t.tracks[0].steps[0].pitch, PITCH_RANGE,
            "two octaves up at most"
        );
        assert_eq!(t.tracks[0].steps[0].hold, HOLD_MAX, "a bar at most");
        assert_eq!(t.tracks[0].steps[0].velocity, VELOCITY_MAX, "full at most");
        assert_eq!(
            t.tracks[0].steps[0].nudge, -NUDGE_MAX_MS,
            "fifty ms at most"
        );
        assert!(!t.tracks[0].steps[3].on, "the padding is empty steps");
        let fast = Tracker {
            tempo: 500.0,
            ..Tracker::default()
        };
        assert_eq!(fast.sanitised().tempo, TEMPO_MAX);
    }

    #[test]
    fn lengths_snap_to_the_nearest_of_five() {
        for (asked, want) in [
            (1, 4),
            (6, 4),
            (7, 8),
            (20, 16),
            (25, 32),
            (50, 64),
            (999, 64),
        ] {
            let t = Tracker {
                tracks: vec![track(asked, 0)],
                ..Tracker::default()
            }
            .sanitised();
            assert_eq!(t.tracks[0].length, want, "{asked}");
        }
    }

    #[test]
    fn a_tracker_with_no_tracks_gets_one() {
        let t = Tracker {
            tracks: vec![],
            ..Tracker::default()
        }
        .sanitised();
        assert_eq!(t.tracks, vec![Track::default()]);
    }

    #[test]
    fn shortening_a_track_keeps_the_steps_past_its_end() {
        let t = Tracker {
            tracks: vec![track(4, 0b1000_0000_0001)],
            ..Tracker::default()
        }
        .sanitised();
        assert!(t.tracks[0].steps[11].on);
        assert_eq!(t.params(0).pattern, 0b1000_0000_0001);
    }

    #[test]
    fn what_reaches_the_audio_thread_is_the_tracker_in_range() {
        // The path `apply_tracker` takes: bring into range, then store for
        // the callback to load once a block.
        use shard_dsp::steps::StepBank;
        let mut t = track(3, 0);
        t.steps = vec![
            Step {
                on: true,
                pitch: -100,
                hold: 3,
                velocity: 5,
                nudge: 7,
            };
            100
        ];
        let sent = Tracker {
            tempo: 500.0,
            swing: -1.0,
            tracks: vec![t],
        };
        let kept = sent.sanitised();
        let bank = StepBank::default();
        bank.store(&kept.params(0));
        assert_eq!(
            bank.load(),
            StepParams {
                on: true,
                tempo_bpm: TEMPO_MAX,
                length: 4,
                swing: 0.0,
                pattern: u64::MAX,
                pitches: [-PITCH_RANGE; STEPS],
                holds: [3; STEPS],
                velocities: [5; STEPS],
                nudges: [7; STEPS],
            }
        );
    }

    #[test]
    fn a_drums_tracks_kit_reaches_the_engine_in_range() {
        // A kit too long, too loud, too short in its rows, tuned and decayed
        // past its ends, and a patch it cannot play.
        let mut drums = Track::drums();
        drums.length = 30;
        drums.patch = Some(4);
        drums.drums = Some(Kit {
            on: true,
            tune: 40.0,
            decay: f32::NAN,
            kick: vec![200; 3],
            snare: vec![9; 100],
            hat: Vec::new(),
        });
        let kept = Tracker {
            tracks: vec![track(16, 0), drums],
            ..Tracker::default()
        }
        .sanitised();
        assert_eq!(kept.tracks[1].patch, None);
        let (pattern, params, gain) = kept.kit(0).expect("one drums track");
        assert_eq!(pattern.length, 32);
        let mut hits = [[0; STEPS]; shard_dsp::kit::VOICES];
        hits[KICK][..3].fill(HIT_MAX);
        hits[SNARE] = [9; STEPS];
        assert_eq!(pattern.hits, hits);
        assert_eq!(
            params,
            KitParams {
                on: true,
                tune: TUNE_RANGE,
                decay: 1.0
            }
        );
        assert_eq!(gain, TRACK_GAIN_DEFAULT);
        assert_eq!(kept.kit(1), None);
    }

    #[test]
    fn each_kind_plays_on_its_own_pool_by_its_place_among_its_kind() {
        // Melody, drums, melody, drums, drums: a drums track anywhere in the
        // order never moves a melody track's voice.
        let t = Tracker {
            tracks: vec![
                track(16, 0b1),
                Track::drums(),
                track(8, 0b11),
                Track::blank_drums(),
                Track::drums(),
            ],
            ..Tracker::default()
        };
        assert_eq!(
            t.slots(),
            vec![
                Slot::Melody(0),
                Slot::Drums(0),
                Slot::Melody(1),
                Slot::Drums(1),
                Slot::Drums(2)
            ]
        );
        assert_eq!((t.params(1).length, t.params(1).pattern), (8, 0b11));
        assert!(t.kit(1).unwrap().1.on, "Add Track › Drums arrives on");
        assert_eq!(t.kit(0).unwrap().1.on, Kit::default().on);
    }

    #[test]
    fn there_are_never_more_drums_tracks_than_kits_nor_none_to_play_a_patch() {
        let t = Tracker {
            tracks: vec![Track::blank_drums(); KITS + 2],
            ..Tracker::default()
        }
        .sanitised();
        assert_eq!(t.drums().count(), KITS);
        assert_eq!(t.melody().count(), 1, "a melody track is put first");
        assert!(!t.tracks[0].is_drums());
    }

    #[test]
    fn a_pattern_edit_leaves_a_drums_track_alone() {
        let t = Tracker::default();
        assert_eq!(
            t.clone().apply(1, &Edit::ClearBar { bar: 0 }),
            t.sanitised()
        );
    }

    #[test]
    fn a_new_session_has_a_melody_track_and_a_drums_track_switched_off() {
        let t = Tracker::default();
        assert_eq!(t.slots(), vec![Slot::Melody(0), Slot::Drums(0)]);
        let (pattern, params, _) = t.kit(0).unwrap();
        assert_eq!(pattern, KitPattern::default());
        assert!(!params.on);
    }

    #[test]
    fn each_track_plays_its_own_steps_at_the_songs_tempo() {
        let mut first = track(8, 0b1001);
        for step in &mut first.steps {
            step.pitch = 7;
            step.hold = 2;
            step.velocity = 64;
            step.nudge = -3;
        }
        let t = Tracker {
            tempo: 82.0,
            swing: 0.12,
            tracks: vec![first, track(4, 0b1)],
        };
        assert_eq!(
            t.params(0),
            StepParams {
                on: true,
                tempo_bpm: 82.0,
                length: 8,
                swing: 0.12,
                pattern: 0b1001,
                pitches: [7; STEPS],
                holds: [2; STEPS],
                velocities: [64; STEPS],
                nudges: [-3; STEPS],
            }
        );
        let second = t.params(1);
        assert_eq!((second.length, second.pattern), (4, 0b1));
        // A track that is not there plays nothing.
        assert_eq!(t.params(5).pattern, 0);
        let transport = t.transport();
        assert!(transport.on);
        assert_eq!((transport.tempo_bpm, transport.swing), (82.0, 0.12));
    }

    #[test]
    fn a_new_track_is_sixteen_empty_steps_at_unity() {
        let t = Track::blank();
        assert_eq!(t.length, 16);
        assert_eq!(t.gain, TRACK_GAIN_DEFAULT);
        assert!(t.steps.iter().all(|s| *s == Step::default()));
        assert_eq!(t.steps.len(), STEPS);
    }

    #[test]
    fn there_are_never_more_tracks_than_voices_and_faders_stay_in_range() {
        let mut t = Tracker {
            tracks: vec![Track::blank(); VOICES + 3],
            ..Tracker::default()
        };
        t.tracks[0].gain = 9.0;
        t.tracks[1].gain = f32::NAN;
        let t = t.sanitised();
        assert_eq!(t.tracks.len(), VOICES);
        assert_eq!(t.tracks[0].gain, TRACK_GAIN_MAX);
        assert_eq!(t.tracks[1].gain, TRACK_GAIN_DEFAULT);
    }

    #[test]
    fn a_pattern_edit_acts_on_its_own_track_only() {
        let mut t = counted(16);
        t.tracks.push(t.tracks[0].clone());
        let edited = t.clone().apply(1, &Edit::ClearBar { bar: 0 });
        assert_eq!(edited.tracks[0], t.clone().sanitised().tracks[0]);
        assert!(edited.tracks[1].steps[..16].iter().all(|s| !s.on));
        // A track that does not exist changes nothing.
        assert_eq!(t.clone().apply(4, &Edit::Duplicate), t.sanitised());
    }

    #[test]
    fn the_last_of_sixty_four_steps_reaches_the_engine() {
        let mut t = track(64, 0);
        t.steps[63] = Step {
            on: true,
            pitch: -5,
            hold: 8,
            velocity: 40,
            nudge: -12,
        };
        let p = Tracker {
            tracks: vec![t],
            ..Tracker::default()
        }
        .sanitised()
        .params(0);
        assert_eq!(p.length, 64);
        assert_eq!(p.pattern, 1 << 63);
        assert_eq!(
            (p.pitches[63], p.holds[63], p.velocities[63], p.nudges[63]),
            (-5, 8, 40, -12)
        );
    }

    #[test]
    fn a_track_survives_a_save_and_missing_fields_read_as_empty_steps() {
        let mut t = Tracker::default();
        t.tracks[0].steps[40] = Step {
            on: true,
            pitch: 7,
            hold: 3,
            velocity: 90,
            nudge: 15,
        };
        t.tracks[1].drums.as_mut().unwrap().snare[9] = 33;
        let text = serde_json::to_string(&t).unwrap();
        let back: Tracker = serde_json::from_str(&text).unwrap();
        assert_eq!(back, t);
        // A hand-written file with only what it needs.
        let small = r#"{"tracks":[{"length":8,"steps":[{"on":true},{"pitch":5}]}]}"#;
        let t: Tracker = serde_json::from_str(small).unwrap();
        let t = t.sanitised();
        assert!(t.tracks[0].steps[0].on);
        assert_eq!(t.tracks[0].steps[1].pitch, 5);
        assert_eq!(
            t.tracks[0].steps[1].velocity, VELOCITY_MAX,
            "a missing velocity is full"
        );
        assert_eq!(t.tracks[0].steps.len(), STEPS);
    }

    /// A track of `length` steps whose step `i` has pitch `i` and is on when
    /// `i` is even, so every position is told apart.
    fn counted(length: u32) -> Tracker {
        let mut t = track(length, 0);
        for (i, step) in t.steps.iter_mut().enumerate() {
            step.on = i % 2 == 0;
            step.pitch = (i as i8) % 20;
        }
        Tracker {
            tracks: vec![t],
            ..Tracker::default()
        }
    }

    #[test]
    fn clearing_a_bar_empties_only_that_bar() {
        let t = counted(32).apply(0, &Edit::ClearBar { bar: 1 });
        let steps = &t.tracks[0].steps;
        assert!(steps[16..32].iter().all(|s| *s == Step::default()));
        assert_eq!(steps[3].pitch, 3, "the first bar is untouched");
        assert!(steps[2].on);
    }

    #[test]
    fn pasting_a_bar_puts_the_copied_steps_there() {
        let source = counted(16).tracks[0].steps[..BAR].to_vec();
        let t = counted(32).apply(0, &Edit::ClearBar { bar: 1 }).apply(
            0,
            &Edit::PasteBar {
                bar: 1,
                steps: source.clone(),
            },
        );
        assert_eq!(&t.tracks[0].steps[16..32], &source[..]);
    }

    #[test]
    fn duplicating_plays_the_pattern_twice_and_stops_at_the_longest_track() {
        let one_bar = counted(16);
        let t = one_bar.clone().apply(0, &Edit::Duplicate);
        assert_eq!(t.tracks[0].length, 32);
        assert_eq!(&t.tracks[0].steps[16..32], &one_bar.tracks[0].steps[..16]);
        assert_eq!(&t.tracks[0].steps[..16], &one_bar.tracks[0].steps[..16]);
        let short = counted(4).apply(0, &Edit::Duplicate);
        assert_eq!(short.tracks[0].length, 8);
        assert_eq!(&short.tracks[0].steps[4..8], &short.tracks[0].steps[..4]);
        let full = counted(64);
        assert_eq!(full.clone().apply(0, &Edit::Duplicate), full);
    }

    #[test]
    fn repeating_a_bar_fills_the_rest_of_the_track_and_no_further() {
        let t = counted(32).apply(0, &Edit::RepeatBar { from: 0 });
        let steps = &t.tracks[0].steps;
        assert_eq!(&steps[16..32], &steps[..16]);
        // Bars past the track's length are not touched.
        assert_eq!(steps[32].pitch, 12);
        let four = counted(64).apply(0, &Edit::RepeatBar { from: 1 });
        assert_eq!(&four.tracks[0].steps[..16], &four.tracks[0].steps[16..32]);
        assert_eq!(&four.tracks[0].steps[48..], &four.tracks[0].steps[16..32]);
    }

    #[test]
    fn rotating_slides_within_the_length_and_wraps() {
        let t = counted(8).apply(0, &Edit::Rotate { by: 1 });
        let steps = &t.tracks[0].steps;
        assert_eq!(steps[0].pitch, 7, "the last step came round to the front");
        assert_eq!(steps[1].pitch, 0);
        assert_eq!(steps[8].pitch, 8, "past the length is not moved");
        let back = counted(8).apply(0, &Edit::Rotate { by: -1 });
        assert_eq!(back.tracks[0].steps[7].pitch, 0);
        let full = counted(8).apply(0, &Edit::Rotate { by: 8 });
        assert_eq!(full, counted(8).sanitised());
    }

    #[test]
    fn transposing_moves_every_pitch_and_stops_at_the_range() {
        let t = counted(16).apply(
            0,
            &Edit::Transpose {
                semis: 5,
                bar: None,
            },
        );
        assert_eq!(t.tracks[0].steps[3].pitch, 8);
        let high = counted(16).apply(
            0,
            &Edit::Transpose {
                semis: 100,
                bar: None,
            },
        );
        assert_eq!(high.tracks[0].steps[3].pitch, PITCH_RANGE);
        let one = counted(32).apply(
            0,
            &Edit::Transpose {
                semis: 1,
                bar: Some(1),
            },
        );
        assert_eq!(one.tracks[0].steps[3].pitch, 3);
        assert_eq!(one.tracks[0].steps[17].pitch, 18);
    }

    #[test]
    fn a_bar_that_does_not_exist_changes_nothing() {
        let before = counted(16).sanitised();
        assert_eq!(counted(16).apply(0, &Edit::ClearBar { bar: 9 }), before);
        assert_eq!(counted(16).apply(0, &Edit::RepeatBar { from: 9 }), before);
        assert_eq!(
            counted(16).apply(
                0,
                &Edit::Transpose {
                    semis: 3,
                    bar: Some(7)
                }
            ),
            before
        );
    }

    #[test]
    fn an_edit_arrives_as_json_tagged_by_its_op() {
        let e: Edit = serde_json::from_str(r#"{"op":"transpose","semis":-12,"bar":null}"#).unwrap();
        assert_eq!(
            e,
            Edit::Transpose {
                semis: -12,
                bar: None
            }
        );
        let e: Edit = serde_json::from_str(r#"{"op":"clear_bar","bar":2}"#).unwrap();
        assert_eq!(e, Edit::ClearBar { bar: 2 });
    }
}
