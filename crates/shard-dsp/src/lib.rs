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

pub mod arrangement;
pub mod chain;
pub mod chorus;
pub mod crush;
pub mod delay;
pub mod drive;
pub mod duck;
pub mod echo;
pub mod engine;
pub mod envelope;
pub mod filter;
pub mod flanger;
pub mod fm;
pub mod fx;
mod fx_rows;
pub mod granular;
pub mod inspect;
pub mod kit;
pub mod ladder;
pub mod limiter;
pub mod mix;
pub mod modulation;
pub mod note;
pub mod overtone;
pub mod params;
pub mod player;
pub mod preview;
pub mod reverb;
pub mod ringmod;
pub mod rise;
pub mod rng;
pub mod rt;
pub mod scrub;
pub mod shift;
pub mod smooth;
pub mod sources;
pub mod steps;
mod voice;
pub mod wear;

pub use chorus::{Chorus, ChorusParams, ChorusType};
pub use crush::{Crush, CrushParams};
pub use delay::{Delay, DelayParams};
pub use drive::{Drive, DriveParams, DriveType};
pub use duck::{Duck, DuckBank, Ducker, Key};
pub use echo::{Echo, EchoParams};
pub use engine::{Engine, Tap, KITS, TAP_FRAMES, VOICES};
pub use envelope::EnvParams;
pub use filter::{Filter, FilterParams, FilterSlope, FilterType};
pub use flanger::{Flanger, FlangerParams};
pub use fm::{Fm, FmParams, FmType};
pub use granular::{GrainParams, Granular, Window};
pub use inspect::{GrainLog, GrainSpawn};
pub use kit::{Kit, KitBank, KitClock, KitParams, KitPattern};
pub use ladder::{Ladder, LadderParams};
pub use limiter::{Limiter, LimiterParams};
pub use mix::Mix;
pub use modulation::{
    adsr_curve, Curve, Ease, EnvSpec, Globals, LfoSpec, LinkError, ModSet, Shape,
};
pub use note::{Alone, Length};
pub use overtone::{Overtone, OvertoneParams};
pub use params::{ParamBank, ParamDef, Taper, Unit, PARAMS};
pub use player::Player;
pub use reverb::{Reverb, ReverbParams};
pub use ringmod::{RingMod, RingModParams};
pub use rise::{Rise, RiseParams};
pub use smooth::OnePole;
pub use sources::{Generator, Reading, ReadingBank};
pub use steps::{Beat, StepClock, StepParams, Transport};
pub use wear::{Wear, WearParams};
