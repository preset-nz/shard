//! The agent player (roadmap row 24, `projects/shard/design/agent-player.md`):
//! a local model, asked in a panel, writes tracks into the open session
//! through tools, and one undo takes back the whole reply.
//!
//! `client`, `log`, `run` and `tools` know nothing of Shard and live in the
//! family crate `preset-agent-kit` (`packages/agent-kit`, guidance
//! `projects/agent-kit`); they are re-exported here so paths stay
//! `agent::run::...`. `shard` is this app's tools, prompt and draft; the
//! commands the tools run are the object model's (`object_model::commands`).

pub use preset_agent_kit::{client, log, run, tools};

pub mod shard;
