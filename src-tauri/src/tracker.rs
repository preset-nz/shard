//! The tracker: the level above the patch.
//!
//! A patch is an instrument, a sound with no clock. The tracker decides when
//! it plays (Georg, 2026-09-14: "so the tracker is one level up from the
//! patch"). It holds what belongs to the arrangement rather than to a sound: the
//! tempo, the swing, and tracks of steps. Loading a different patch leaves
//! the beat alone, and a patch never carries a pattern.
//!
//! **One track today, playing the document's one patch.** Tracks are a list
//! so the file already has the right shape; only the first sounds. A track
//! is up to four bars of sixteen steps, each step with its own pitch and hold. A track
//! names the patch it plays when a document holds more than one (roadmap row
//! 10, node API decision 23). Each track is monophonic, as in a tracker: a
//! new step cuts the last.
//!
//! Saved in the same `.shard` file as the patch, above it. See `patch.rs`.

use serde::{Deserialize, Serialize};
use shard_dsp::steps::{HOLD_MAX, PITCH_RANGE, STEPS};
use shard_dsp::StepParams;

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
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub on: bool,
    /// 4, 8, 16, 32 or 64 sixteenths.
    pub length: u32,
    /// Every step the track can hold, `STEPS` of them, step one first. Steps
    /// past `length` are kept, so shortening a track and lengthening it again
    /// gives them back.
    pub steps: Vec<Step>,
}

/// One sixteenth of a track.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Default for Tracker {
    fn default() -> Self {
        let p = StepParams::default();
        Self {
            tempo: p.tempo_bpm,
            swing: p.swing,
            tracks: vec![Track::default()],
        }
    }
}

impl Default for Track {
    fn default() -> Self {
        let p = StepParams::default();
        Self {
            on: p.on,
            length: p.length,
            steps: (0..STEPS)
                .map(|i| Step {
                    on: (p.pattern >> i) & 1 == 1,
                    pitch: p.pitches[i],
                    hold: p.holds[i],
                })
                .collect(),
        }
    }
}

impl Tracker {
    /// Brought into range: tempo and swing clamped, each length snapped to
    /// the nearest of 4, 8, 16, 32 and 64, each track padded or cut to exactly
    /// `STEPS` steps with pitches and holds in range, and at least one track.
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
        if self.tracks.is_empty() {
            self.tracks.push(Track::default());
        }
        for t in &mut self.tracks {
            t.length = LENGTHS
                .iter()
                .copied()
                .min_by_key(|n| n.abs_diff(t.length))
                .unwrap_or(16);
            t.steps.resize(STEPS, Step::default());
            for step in &mut t.steps {
                step.pitch = step.pitch.clamp(-PITCH_RANGE, PITCH_RANGE);
                step.hold = step.hold.min(HOLD_MAX);
            }
        }
        self
    }

    /// What the engine plays: the first track, at the arrangement's tempo and swing.
    pub fn params(&self) -> StepParams {
        let t = self.tracks.first().cloned().unwrap_or_default();
        let mut p = StepParams {
            on: t.on,
            tempo_bpm: self.tempo,
            length: t.length,
            swing: self.swing,
            pattern: 0,
            pitches: [0; STEPS],
            holds: [0; STEPS],
        };
        for (i, step) in t.steps.iter().take(STEPS).enumerate() {
            p.pattern |= u64::from(step.on) << i;
            p.pitches[i] = step.pitch;
            p.holds[i] = step.hold;
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A track from a pattern (bit 0 first) and nothing else set.
    fn track(length: u32, pattern: u64) -> Track {
        let mut t = Track {
            on: true,
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
        assert_eq!(t.params().pattern, 0b1000_0000_0001);
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
        bank.store(&kept.params());
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
            }
        );
    }

    #[test]
    fn the_engine_plays_the_first_track_at_the_songs_tempo() {
        let mut first = track(8, 0b1001);
        for step in &mut first.steps {
            step.pitch = 7;
            step.hold = 2;
        }
        let t = Tracker {
            tempo: 82.0,
            swing: 0.12,
            tracks: vec![first, track(4, 0b1)],
        };
        assert_eq!(
            t.params(),
            StepParams {
                on: true,
                tempo_bpm: 82.0,
                length: 8,
                swing: 0.12,
                pattern: 0b1001,
                pitches: [7; STEPS],
                holds: [2; STEPS],
            }
        );
    }

    #[test]
    fn the_last_of_sixty_four_steps_reaches_the_engine() {
        let mut t = track(64, 0);
        t.steps[63] = Step {
            on: true,
            pitch: -5,
            hold: 8,
        };
        let p = Tracker {
            tracks: vec![t],
            ..Tracker::default()
        }
        .sanitised()
        .params();
        assert_eq!(p.length, 64);
        assert_eq!(p.pattern, 1 << 63);
        assert_eq!((p.pitches[63], p.holds[63]), (-5, 8));
    }

    #[test]
    fn a_track_survives_a_save_and_missing_fields_read_as_empty_steps() {
        let mut t = Tracker::default();
        t.tracks[0].steps[40] = Step {
            on: true,
            pitch: 7,
            hold: 3,
        };
        let text = serde_json::to_string(&t).unwrap();
        let back: Tracker = serde_json::from_str(&text).unwrap();
        assert_eq!(back, t);
        // A hand-written file with only what it needs.
        let small = r#"{"tracks":[{"length":8,"steps":[{"on":true},{"pitch":5}]}]}"#;
        let t: Tracker = serde_json::from_str(small).unwrap();
        let t = t.sanitised();
        assert!(t.tracks[0].steps[0].on);
        assert_eq!(t.tracks[0].steps[1].pitch, 5);
        assert_eq!(t.tracks[0].steps.len(), STEPS);
    }
}
