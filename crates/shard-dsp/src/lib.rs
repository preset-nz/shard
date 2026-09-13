//! Shard's DSP core.
//!
//! No audio device, no file I/O, no UI, no async. Everything here is testable
//! with `cargo test` on any machine, which is the point: the parts that make
//! sound should not need a sound card to verify.
//!
//! The rules that hold across the whole crate, because all of it ends up on
//! the audio thread:
//!
//! - No allocation, no locks, no logging in anything called per sample.
//! - Buffers are sized once, at construction.
//! - Parameters arrive through an atomic bank and are read once per block.
//!
//! The first rule is enforced, not just written down: `rt` holds a guard
//! allocator, and `tests/audio_thread.rs` runs every audio-thread call inside
//! it.

pub mod crush;
pub mod engine;
pub mod envelope;
pub mod granular;
pub mod inspect;
pub mod modulation;
pub mod params;
pub mod player;
pub mod ringmod;
pub mod rng;
pub mod rt;
pub mod smooth;

pub use crush::{Crush, CrushParams};
pub use engine::Engine;
pub use envelope::EnvParams;
pub use granular::{GrainParams, Granular, Window};
pub use inspect::{GrainLog, GrainSpawn};
pub use modulation::{LfoSpec, LinkError, ModSet, Shape};
pub use params::{ParamBank, ParamDef, Taper, Unit, PARAMS};
pub use player::Player;
pub use ringmod::{RingMod, RingModParams};
pub use smooth::OnePole;
