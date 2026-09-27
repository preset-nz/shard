//! How long a note lasts: the patch's Length setting.
//!
//! Georg, 2026-09-27: *"the sample shouldn't set how long the patch is … or
//! should it?"* and then *"how about a setting?"*. A drone runs for ever, a
//! kick plays and ends, and a sample loop is as long as the sample. So the
//! patch says which, as `patch.length`, and the engine keeps one note clock
//! that always runs, reset by the transport and by every step. The setting
//! only chooses what the envelopes read and whether the sample loops.
//!
//! The tracker decides when a note starts; this decides how long it lasts.

/// What sets a note's length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    /// For ever. The envelopes attack once and hold at sustain, and the
    /// sample loops at its own length underneath.
    Loop,
    /// One pass through the sample, as it always was: the envelopes are
    /// fitted to the pass, and an octave up halves both.
    Sample,
    /// Held for `patch.hold` ms of real time, then released. The sample plays
    /// once from the start, and stops at its end or the note's, whichever is
    /// first.
    Hold,
}

impl Length {
    pub const ALL: [Length; 3] = [Length::Loop, Length::Sample, Length::Hold];
    pub const NAMES: [&'static str; 3] = ["Loop", "Sample", "Hold"];

    pub fn from_value(v: f32) -> Length {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}
