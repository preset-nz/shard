//! The agent player (roadmap row 24, `projects/shard/design/agent-player.md`):
//! a local model, asked in a panel, writes tracks into the open session
//! through tools, and one undo takes back the whole reply.
//!
//! `tools`, `client` and `run` know nothing of Shard, so they can lift into
//! a family crate (`agent-kit`, `design/agent-tools.md`) once a second app
//! wants them. `shard` is this app's tools, prompt and draft; the commands
//! the tools run are the object model's (`object_model::commands`).

pub mod client;
pub mod log;
pub mod run;
pub mod shard;
pub mod tools;
