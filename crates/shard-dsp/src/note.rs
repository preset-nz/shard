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

/// What the patch is asked to do on its own, apart from the song (Georg,
/// 2026-10-13: *"play always plays the sequencer, I can't just loop the
/// note"* and *"there is no longer a way to just play the sample start to
/// stop"*). The host chooses it; the engine only follows.
///
/// Under `Loop` and `Once` the host has silenced every other voice and kit
/// and given one voice the patch with its steps off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alone {
    /// The song: every track plays its own patch. A voice with its steps off
    /// does what its Length says.
    #[default]
    Song,
    /// For ever, whatever the Length: the note is held and the sample loops
    /// underneath. Hold, which would let go, holds for ever instead.
    Loop,
    /// One pass from the trim start to the trim end, and then the engine
    /// stops the transport itself. Loop, which has no end, is fitted as a
    /// Sample pass; Hold lasts as long as its note.
    Once,
}

impl Alone {
    pub fn bits(self) -> u8 {
        match self {
            Alone::Song => 0,
            Alone::Loop => 1,
            Alone::Once => 2,
        }
    }

    pub fn from_bits(b: u8) -> Alone {
        match b {
            1 => Alone::Loop,
            2 => Alone::Once,
            _ => Alone::Song,
        }
    }

    /// The Length a voice plays under this: what the patch says, except
    /// where it cannot be honoured.
    pub fn length(self, set: Length) -> Length {
        match (self, set) {
            (Alone::Loop, Length::Hold) => Length::Loop,
            (Alone::Once, Length::Loop) => Length::Sample,
            _ => set,
        }
    }
}
