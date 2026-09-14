//! The tracker: the level above the patch.
//!
//! A patch is an instrument, a sound with no clock. The tracker decides when
//! it plays (Georg, 2026-09-14: "so the tracker is one level up from the
//! patch"). It holds what belongs to the song rather than to a sound: the
//! tempo, the swing, and tracks of steps. Loading a different patch leaves
//! the beat alone, and a patch never carries a pattern.
//!
//! **One track today, playing the document's one patch.** Tracks are a list
//! so the file already has the right shape; only the first sounds. A track
//! names the patch it plays when a document holds more than one (roadmap row
//! 10, node API decision 23). Each track is monophonic, as in a tracker: a
//! new step cuts the last.
//!
//! Saved in the same `.shard` file as the patch, above it. See `patch.rs`.

use serde::{Deserialize, Serialize};
use shard_dsp::StepParams;

pub const TEMPO_MIN: f32 = 40.0;
pub const TEMPO_MAX: f32 = 240.0;
/// Of a sixteenth. Past about 62 % it stops being a pocket and becomes a
/// shuffle (`design/drum-programming.md`); the range allows going there.
pub const SWING_MAX: f32 = 0.75;
/// The lengths a track can have, in sixteenths.
pub const LENGTHS: [u32; 3] = [4, 8, 16];
/// Steps a pattern can hold. Bits past a track's length are kept, so
/// shortening a track and lengthening it again gives the steps back.
const PATTERN_MASK: u32 = 0xFFFF;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tracker {
    pub tempo: f32,
    /// 0 to `SWING_MAX` of a sixteenth, on the even steps.
    pub swing: f32,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub on: bool,
    /// 4, 8 or 16 sixteenths.
    pub length: u32,
    /// One bit per step, step one first. A whole number in the file, never a
    /// float, so it reads back exactly.
    pub pattern: u32,
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
            pattern: p.pattern,
        }
    }
}

impl Tracker {
    /// Brought into range: tempo and swing clamped, each length snapped to
    /// the nearest of 4, 8 and 16, patterns cut to sixteen steps, and at
    /// least one track. Everything the UI or a hand-edited file sends passes
    /// through here before the engine hears it.
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
            t.pattern &= PATTERN_MASK;
        }
        self
    }

    /// What the engine plays: the first track, at the song's tempo and swing.
    pub fn params(&self) -> StepParams {
        let t = self.tracks.first().copied().unwrap_or_default();
        StepParams {
            on: t.on,
            tempo_bpm: self.tempo,
            length: t.length,
            swing: self.swing,
            pattern: t.pattern,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonsense_is_brought_into_range() {
        let t = Tracker {
            tempo: f32::NAN,
            swing: 9.0,
            tracks: vec![Track {
                on: true,
                length: 11,
                pattern: 0x1_2345,
            }],
        }
        .sanitised();
        assert_eq!(t.tempo, Tracker::default().tempo);
        assert_eq!(t.swing, SWING_MAX);
        assert_eq!(t.tracks[0].length, 8, "11 is nearest to 8");
        assert_eq!(t.tracks[0].pattern, 0x2345, "sixteen steps at most");
        let fast = Tracker {
            tempo: 500.0,
            ..Tracker::default()
        };
        assert_eq!(fast.sanitised().tempo, TEMPO_MAX);
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
            tracks: vec![Track {
                on: true,
                length: 4,
                pattern: 0b1000_0000_0001,
            }],
            ..Tracker::default()
        }
        .sanitised();
        assert_eq!(t.tracks[0].pattern, 0b1000_0000_0001);
    }

    #[test]
    fn the_engine_plays_the_first_track_at_the_songs_tempo() {
        let t = Tracker {
            tempo: 82.0,
            swing: 0.56,
            tracks: vec![
                Track {
                    on: true,
                    length: 8,
                    pattern: 0b1001,
                },
                Track {
                    on: false,
                    length: 4,
                    pattern: 0b1,
                },
            ],
        };
        assert_eq!(
            t.params(),
            StepParams {
                on: true,
                tempo_bpm: 82.0,
                length: 8,
                swing: 0.56,
                pattern: 0b1001,
            }
        );
    }
}
