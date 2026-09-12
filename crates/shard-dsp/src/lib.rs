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

pub mod engine;
pub mod granular;
pub mod params;
pub mod ringmod;
pub mod rng;
pub mod smooth;

pub use engine::Engine;
pub use granular::{GrainParams, Granular, Window};
pub use params::{ParamBank, ParamDef, Taper, Unit, PARAMS};
pub use ringmod::{RingMod, RingModParams};
pub use smooth::OnePole;
