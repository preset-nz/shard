//! Mute and solo (epic 28, story c): what a track's mix switches say, and the
//! gate that carries a change without a click.
//!
//! **The rule is the usual mixer's, and mute wins.** A voice is heard when it
//! is not muted and either it is soloed or nothing is soloed. Soloing a muted
//! track leaves it silent, as in Logic, Pro Tools and Live: a mute is the
//! stronger switch, so soloing never brings back a track you took out.
//! A solo-safe voice (the audition loop, which is no track) ignores other
//! voices' solos but still obeys its own mute.
//!
//! The engine keeps two copies per voice: what the host wants and what is
//! applied. A change waits for the next bar line of the shared transport
//! while it runs, and applies at once while it does not (see
//! `Engine::set_voice_mix`). Silencing finishes on the bar line, fading over
//! the gate's length before it, so the downbeat's note never leaks through
//! the fade; bringing back starts on the bar line.

/// A voice's mix switches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mix {
    pub mute: bool,
    pub solo: bool,
    /// Not silenced by other voices' solos: a voice that is no track.
    pub solo_safe: bool,
}

const MUTE: u8 = 1;
const SOLO: u8 = 2;
const SOLO_SAFE: u8 = 4;

impl Mix {
    /// Whether this voice sounds, given whether any voice is soloed.
    #[inline]
    pub fn audible(self, any_solo: bool) -> bool {
        !self.mute && (self.solo || self.solo_safe || !any_solo)
    }

    /// Packed into a byte, for an atomic hand-off.
    pub fn bits(self) -> u8 {
        let bit = |on: bool, b: u8| if on { b } else { 0 };
        bit(self.mute, MUTE) | bit(self.solo, SOLO) | bit(self.solo_safe, SOLO_SAFE)
    }

    pub fn from_bits(b: u8) -> Self {
        Self {
            mute: b & MUTE != 0,
            solo: b & SOLO != 0,
            solo_safe: b & SOLO_SAFE != 0,
        }
    }
}

/// How long a mute or solo takes to land: short enough to sound on the bar,
/// long enough not to click.
pub const GATE_MS: f32 = 5.0;

/// The gate's length in frames at `sample_rate`: `GATE_MS`, rounded up.
pub fn gate_frames(sample_rate: f32) -> usize {
    // Multiplied first: 0.001 is not exact in binary, and 240.00002 would
    // round up to 241.
    ((GATE_MS * sample_rate / 1000.0).ceil() as usize).max(1)
}

/// A linear ramp between silent and heard, exactly `gate_frames` end to end.
#[derive(Debug, Clone, Copy)]
pub struct Gate {
    value: f32,
    step: f32,
}

impl Gate {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            value: 1.0,
            step: 1.0 / gate_frames(sample_rate) as f32,
        }
    }

    /// One sample nearer `heard`'s end, and the gain to apply. The last step
    /// lands on the end exactly, whatever the float has drifted by.
    #[inline]
    pub fn process(&mut self, heard: bool) -> f32 {
        let target = if heard { 1.0 } else { 0.0 };
        if self.value != target {
            let next = if heard {
                self.value + self.step
            } else {
                self.value - self.step
            };
            self.value = if (next - target).abs() < self.step * 0.5 || (next > target) == heard {
                target
            } else {
                next
            };
        }
        self.value
    }

    /// Straight to `heard`'s end: for a voice waking, or while stopped.
    pub fn reset(&mut self, heard: bool) {
        self.value = if heard { 1.0 } else { 0.0 };
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_wins_over_solo() {
        let m = |mute, solo| Mix {
            mute,
            solo,
            solo_safe: false,
        };
        assert!(m(false, false).audible(false));
        assert!(
            !m(false, false).audible(true),
            "an unsoloed track under a solo"
        );
        assert!(m(false, true).audible(true));
        assert!(
            !m(true, true).audible(true),
            "a muted track stays muted when soloed"
        );
        assert!(!m(true, false).audible(false));
        let safe = Mix {
            solo_safe: true,
            ..Mix::default()
        };
        assert!(safe.audible(true));
    }

    #[test]
    fn bits_round_trip() {
        for b in 0..8 {
            assert_eq!(Mix::from_bits(b).bits(), b);
        }
    }

    #[test]
    fn the_gate_takes_its_time_both_ways() {
        // Exactly its frames end to end, at a rate where they are not a
        // whole number of milliseconds' worth too.
        for sr in [48_000.0, 44_100.0] {
            let mut g = Gate::new(sr);
            let n = gate_frames(sr);
            for _ in 0..n - 1 {
                assert!(g.process(false) > 0.0);
            }
            assert_eq!(g.process(false), 0.0);
            for _ in 0..n - 1 {
                assert!(g.process(true) < 1.0);
            }
            assert_eq!(g.process(true), 1.0);
        }
    }
}
