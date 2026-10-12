//! Sidechain ducking (roadmap row 41): a track's volume dips on another
//! track's steps.
//!
//! **The key's steps drive the dip, not its loudness.** Each step of the key
//! track that begins fires a dip, and the step's velocity scales its depth,
//! so the pump sits on the grid and is the same every bar. A drums track keys
//! from its kick lane only. Nothing listens to audio.
//!
//! **The dip** (`Ducker`) falls linearly over `ATTACK_MS` from where it is to
//! `1 - depth × velocity`, starting on the very sample the key's step sounds,
//! then comes back exponentially over the track's Release. A new hit during
//! the release dips again from where it is; a hit shallower than where the
//! dip already is changes nothing, so the gain never jumps up. At rest the
//! gain is exactly one, so a track with no key, or a depth of zero, is
//! bit-exact with no ducking at all.
//!
//! **Who pumps** (`Engine`): a key pumps while it is heard. A muted key does
//! not, nor one silenced by another track's solo, as an audio sidechain
//! would behave. A *Key only* key is silent but still pumps, whatever is
//! soloed, until it is muted. A drums track switched off fires no hits, so it
//! does not pump either.
//!
//! **Key only** silences the channel's output (over the mix gate's length,
//! `mix::GATE_MS`) while its steps keep keying: a ghost kick.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// How far the volume dips at full velocity, as a fraction of it: 0.75 is
/// about −12 dB.
pub const DEPTH_DEFAULT: f32 = 0.75;
/// How long the dip takes to come back, in ms: after this long it has come
/// back all but 5 % of the way.
pub const RELEASE_DEFAULT_MS: f32 = 200.0;
pub const RELEASE_MIN_MS: f32 = 10.0;
pub const RELEASE_MAX_MS: f32 = 1_000.0;
/// How long the dip takes to fall its full depth, in ms: short enough to sit
/// on the step, long enough not to click.
pub const ATTACK_MS: f32 = 2.0;

/// The track a duck follows, by where it plays: a voice or a kit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Voice(usize),
    Kit(usize),
}

/// One track's ducking, as the host hands it to the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Duck {
    /// The track whose steps dip this one, or none.
    pub key: Option<Key>,
    /// How far a full-velocity step dips the volume, 0 to 1 (silence).
    pub depth: f32,
    /// How long the dip takes to come back, `RELEASE_MIN_MS` to
    /// `RELEASE_MAX_MS`.
    pub release_ms: f32,
    /// Silent, its steps still keying whatever it ducks.
    pub key_only: bool,
}

impl Default for Duck {
    fn default() -> Self {
        Self {
            key: None,
            depth: DEPTH_DEFAULT,
            release_ms: RELEASE_DEFAULT_MS,
            key_only: false,
        }
    }
}

impl Duck {
    /// Brought into range; a value that is no number is its default.
    pub fn sanitised(self) -> Self {
        let or = |v: f32, d: f32| if v.is_finite() { v } else { d };
        Self {
            depth: or(self.depth, DEPTH_DEFAULT).clamp(0.0, 1.0),
            release_ms: or(self.release_ms, RELEASE_DEFAULT_MS)
                .clamp(RELEASE_MIN_MS, RELEASE_MAX_MS),
            ..self
        }
    }
}

const NONE: u32 = 0;
const VOICE: u32 = 1 << 16;
const KIT: u32 = 2 << 16;
const INDEX: u32 = 0xFFFF;

/// A track's ducking, shared between the session and the audio thread, read
/// once a block. A read between two stores may mix them for one block, which
/// is harmless: every field is in range on its own.
pub struct DuckBank {
    key: AtomicU32,
    depth: AtomicU32,
    release: AtomicU32,
    key_only: AtomicBool,
}

impl Default for DuckBank {
    fn default() -> Self {
        let d = Duck::default();
        Self {
            key: AtomicU32::new(NONE),
            depth: AtomicU32::new(d.depth.to_bits()),
            release: AtomicU32::new(d.release_ms.to_bits()),
            key_only: AtomicBool::new(d.key_only),
        }
    }
}

impl DuckBank {
    pub fn store(&self, d: &Duck) {
        let key = match d.key {
            None => NONE,
            Some(Key::Voice(i)) => VOICE | (i as u32 & INDEX),
            Some(Key::Kit(k)) => KIT | (k as u32 & INDEX),
        };
        self.key.store(key, Ordering::Relaxed);
        self.depth.store(d.depth.to_bits(), Ordering::Relaxed);
        self.release
            .store(d.release_ms.to_bits(), Ordering::Relaxed);
        self.key_only.store(d.key_only, Ordering::Relaxed);
    }

    #[inline]
    pub fn load(&self) -> Duck {
        let key = self.key.load(Ordering::Relaxed);
        let index = (key & INDEX) as usize;
        Duck {
            key: match key & !INDEX {
                VOICE => Some(Key::Voice(index)),
                KIT => Some(Key::Kit(index)),
                _ => None,
            },
            depth: f32::from_bits(self.depth.load(Ordering::Relaxed)),
            release_ms: f32::from_bits(self.release.load(Ordering::Relaxed)),
            key_only: self.key_only.load(Ordering::Relaxed),
        }
    }
}

/// Below this much dip the release lands on unity exactly.
const REST: f32 = 1e-6;

/// One channel's dip: a gain, one at rest. Held as the dip itself, so the
/// release keeps its precision all the way down to rest.
#[derive(Debug, Clone, Copy)]
pub struct Ducker {
    /// How far the gain is below one.
    dip: f32,
    /// Where the attack is headed.
    target: f32,
    attacking: bool,
    /// The fall a sample, of a full-depth dip.
    attack_step: f32,
    /// What is left of the dip after a sample of release.
    release_coef: f32,
    release_ms: f32,
    sample_rate: f32,
}

impl Ducker {
    pub fn new(sample_rate: f32) -> Self {
        let mut d = Self {
            dip: 0.0,
            target: 0.0,
            attacking: false,
            attack_step: 1.0 / (ATTACK_MS * sample_rate / 1000.0).ceil().max(1.0),
            release_coef: 0.0,
            release_ms: 0.0,
            sample_rate,
        };
        d.set_release(RELEASE_DEFAULT_MS);
        d
    }

    /// The release time, in ms. Works out its coefficient only when it
    /// changes, so once a block costs nothing.
    pub fn set_release(&mut self, ms: f32) {
        let ms = if ms.is_finite() {
            ms.clamp(RELEASE_MIN_MS, RELEASE_MAX_MS)
        } else {
            RELEASE_DEFAULT_MS
        };
        if ms != self.release_ms {
            self.release_ms = ms;
            let frames = ms * 0.001 * self.sample_rate;
            // All but 5 % of the way back after `ms`.
            self.release_coef = (-(20.0f32.ln()) / frames).exp();
        }
    }

    /// A step began: dip by `amount` (depth times velocity, 0 to 1) from
    /// where the gain is now. A shallower dip than the one under way is
    /// ignored, so a hit never lifts the gain.
    #[inline]
    pub fn hit(&mut self, amount: f32) {
        let amount = amount.clamp(0.0, 1.0);
        if amount > self.dip {
            // Mid-attack, the deeper of the two.
            self.target = if self.attacking {
                amount.max(self.target)
            } else {
                amount
            };
            self.attacking = true;
        }
    }

    /// One sample on, and the gain to apply.
    #[inline]
    pub fn process(&mut self) -> f32 {
        if self.attacking {
            self.dip += self.attack_step;
            if self.dip >= self.target {
                self.dip = self.target;
                self.attacking = false;
            }
        } else if self.dip > 0.0 {
            self.dip *= self.release_coef;
            if self.dip < REST {
                self.dip = 0.0;
            }
        }
        1.0 - self.dip
    }

    /// Back to rest at once: for a channel waking, or the transport stopping.
    pub fn reset(&mut self) {
        self.dip = 0.0;
        self.target = 0.0;
        self.attacking = false;
    }

    pub fn gain(&self) -> f32 {
        1.0 - self.dip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn at_rest_the_gain_is_exactly_one() {
        let mut d = Ducker::new(SR);
        for _ in 0..1_000 {
            assert_eq!(d.process(), 1.0);
        }
        d.hit(0.0);
        assert_eq!(d.process(), 1.0, "a dip of nothing is no dip");
    }

    #[test]
    fn a_hit_dips_from_its_own_sample_and_comes_back_to_exactly_one() {
        let mut d = Ducker::new(SR);
        d.hit(0.75);
        assert!(d.process() < 1.0, "the dip starts on the hit's sample");
        let attack = (ATTACK_MS * 0.001 * SR) as usize;
        let mut lowest = 1.0f32;
        for _ in 0..attack {
            lowest = lowest.min(d.process());
        }
        assert!((lowest - 0.25).abs() < 1e-6, "full depth reached: {lowest}");
        // After the release time it is all but 5 % of the way back.
        let release = (RELEASE_DEFAULT_MS * 0.001 * SR) as usize;
        let mut g = 0.0;
        for _ in 0..release {
            g = d.process();
        }
        assert!(
            (1.0 - g - 0.75 * 0.05).abs() < 0.002,
            "after the release: {g}"
        );
        for _ in 0..SR as usize * 2 {
            g = d.process();
        }
        assert_eq!(g, 1.0, "back at rest exactly");
    }

    #[test]
    fn a_hit_during_the_release_dips_again_from_where_it_is() {
        let mut d = Ducker::new(SR);
        d.hit(0.5);
        for _ in 0..4_800 {
            d.process();
        }
        let before = d.gain();
        assert!(before > 0.5 && before < 1.0);
        d.hit(0.5);
        let next = d.process();
        assert!(
            next < before && before - next < 0.02,
            "{before} then {next}"
        );
        // A shallower one changes nothing.
        let mut e = Ducker::new(SR);
        e.hit(0.9);
        for _ in 0..200 {
            e.process();
        }
        let deep = e.gain();
        e.hit(0.2);
        assert!(e.process() >= deep, "a soft hit never lifts the dip");
    }

    #[test]
    fn the_bank_hands_a_duck_across_whole() {
        let bank = DuckBank::default();
        assert_eq!(bank.load(), Duck::default());
        for key in [None, Some(Key::Voice(7)), Some(Key::Kit(3))] {
            let d = Duck {
                key,
                depth: 0.4,
                release_ms: 333.0,
                key_only: true,
            };
            bank.store(&d);
            assert_eq!(bank.load(), d);
        }
    }

    #[test]
    fn a_duck_is_brought_into_range() {
        let d = Duck {
            key: None,
            depth: 3.0,
            release_ms: f32::NAN,
            key_only: false,
        }
        .sanitised();
        assert_eq!((d.depth, d.release_ms), (1.0, RELEASE_DEFAULT_MS));
    }
}
