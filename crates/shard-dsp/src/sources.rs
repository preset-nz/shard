//! How each generator reads its material.
//!
//! Georg, 2026-09-15: *"I also want to be able to wire the material into the
//! player or the granular node."* Each generator reads its own material, and
//! each material keeps its own octave and trim, which every generator wired to
//! it reads through. Those belong to the material rather than to the patch, so
//! they are not parameters: they arrive once a block, as the tracker's steps
//! do.

use std::sync::atomic::{AtomicU32, Ordering};

/// How far the octave reaches either way.
pub const OCTAVE_RANGE: f32 = 2.0;

/// The generators that read a material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generator {
    /// Plain playback: the Sample node, `material.*` in the table.
    Player,
    /// The cloud: the Granular node, `grain.*`.
    Grain,
}

impl Generator {
    pub const ALL: [Generator; 2] = [Generator::Player, Generator::Grain];

    /// Its place in anything held per generator.
    pub fn index(self) -> usize {
        match self {
            Generator::Player => 0,
            Generator::Grain => 1,
        }
    }
}

/// Which part of its material a generator reads, and at what octave.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// Whole octaves, varispeed: one up reads twice as fast.
    pub octave: f32,
    /// The window's ends, 0 to 1 of the file, in either order.
    pub trim_start: f32,
    pub trim_end: f32,
}

impl Default for Reading {
    /// The whole file at its own pitch.
    fn default() -> Self {
        Self {
            octave: 0.0,
            trim_start: 0.0,
            trim_end: 1.0,
        }
    }
}

impl Reading {
    /// The octave as a whole number inside the range. A value between octaves
    /// lands on the nearest, and a non-number on none.
    pub fn octaves(&self) -> f32 {
        if self.octave.is_finite() {
            self.octave.round().clamp(-OCTAVE_RANGE, OCTAVE_RANGE)
        } else {
            0.0
        }
    }

    /// The window as indices into a material `len` samples long. Always
    /// ordered and at least two samples wide, however the ends are set, so
    /// crossed or collapsed ends keep playing rather than panic.
    pub fn window(&self, len: usize) -> (usize, usize) {
        if len < 2 {
            return (0, len);
        }
        let end = |v: f32, fallback: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                fallback
            }
        };
        let a = end(self.trim_start, 0.0);
        let b = end(self.trim_end, 1.0);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let lo = ((a * len as f32) as usize).min(len - 2);
        let hi = ((b * len as f32) as usize).clamp(lo + 2, len);
        (lo, hi)
    }
}

/// The readings, shared between whatever edits materials and the audio
/// thread. The same idea as `StepBank`: stored whole, read whole once a block.
/// A read that lands between two fields plays one block with half the old
/// reading, which the next block corrects.
pub struct ReadingBank {
    /// Octave, start and end: the player's, then the cloud's.
    values: [AtomicU32; 6],
}

impl Default for ReadingBank {
    fn default() -> Self {
        let bank = Self {
            values: std::array::from_fn(|_| AtomicU32::new(0)),
        };
        bank.store(Reading::default(), Reading::default());
        bank
    }
}

impl ReadingBank {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn store(&self, player: Reading, grain: Reading) {
        for (i, r) in [player, grain].into_iter().enumerate() {
            let at = i * 3;
            self.values[at].store(r.octave.to_bits(), Ordering::Relaxed);
            self.values[at + 1].store(r.trim_start.to_bits(), Ordering::Relaxed);
            self.values[at + 2].store(r.trim_end.to_bits(), Ordering::Relaxed);
        }
    }

    /// The player's reading, then the cloud's.
    #[inline]
    pub fn load(&self) -> (Reading, Reading) {
        let get = |i: usize| f32::from_bits(self.values[i].load(Ordering::Relaxed));
        let at = |i: usize| Reading {
            octave: get(i),
            trim_start: get(i + 1),
            trim_end: get(i + 2),
        };
        (at(0), at(3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trimmed(a: f32, b: f32) -> Reading {
        Reading {
            octave: 0.0,
            trim_start: a,
            trim_end: b,
        }
    }

    #[test]
    fn a_window_survives_crossed_or_collapsed_ends() {
        // The two ends are independent, so a hand will cross them. It has to
        // keep playing rather than panic on an empty slice.
        for (a, b) in [
            (0.9, 0.1),
            (0.5, 0.5),
            (1.0, 1.0),
            (0.0, 0.0),
            (0.3, 0.3001),
            (f32::NAN, 2.0),
        ] {
            let (lo, hi) = trimmed(a, b).window(48_000);
            assert!(lo + 2 <= hi && hi <= 48_000, "{a}..{b} gave {lo}..{hi}");
        }
        assert_eq!(Reading::default().window(1), (0, 1));
        assert_eq!(Reading::default().window(0), (0, 0));
    }

    #[test]
    fn crossed_ends_read_the_same_window() {
        assert_eq!(
            trimmed(0.2, 0.6).window(10_000),
            trimmed(0.6, 0.2).window(10_000)
        );
    }

    #[test]
    fn octaves_are_whole_and_in_range() {
        for (asked, got) in [(1.4, 1.0), (-7.0, -2.0), (2.6, 2.0), (f32::NAN, 0.0)] {
            let r = Reading {
                octave: asked,
                ..Reading::default()
            };
            assert_eq!(r.octaves(), got, "octave {asked}");
        }
    }

    #[test]
    fn the_bank_hands_back_what_it_was_given() {
        let bank = ReadingBank::new();
        assert_eq!(bank.load(), (Reading::default(), Reading::default()));
        let player = Reading {
            octave: 1.0,
            trim_start: 0.25,
            trim_end: 0.5,
        };
        let grain = Reading {
            octave: -2.0,
            trim_start: 0.9,
            trim_end: 0.1,
        };
        bank.store(player, grain);
        assert_eq!(bank.load(), (player, grain));
    }
}
