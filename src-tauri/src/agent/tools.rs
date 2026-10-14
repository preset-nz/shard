//! A tool is a command shown to a model (`design/agent-tools.md`): the
//! command's id, plus what a model needs and a menu doesn't, a description
//! and a JSON Schema for its payload, and whether it reads or writes.
//!
//! Nothing here knows Shard. An app hands the loop a `Toolbox`: its tools,
//! and a way to run one. Shard's are in `agent::shard`.

use serde_json::{json, Value as Json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Changes nothing.
    Read,
    /// Changes the document, as part of the reply's one undo step.
    Write,
}

#[derive(Debug, Clone)]
pub struct Tool {
    /// The name the model calls it by.
    pub name: &'static str,
    /// The command it runs.
    pub command: &'static str,
    /// What it does and when to use it, in the model's terms.
    pub description: &'static str,
    /// JSON Schema of its arguments: sent as the tool's `parameters`, and
    /// what a call is checked against before it runs.
    pub parameters: Json,
    pub access: Access,
}

/// The tools an app offers, in the order the model sees them.
#[derive(Debug, Clone)]
pub struct Tools {
    list: Vec<Tool>,
}

impl Tools {
    pub fn new(list: Vec<Tool>) -> Self {
        Self { list }
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.list.iter().find(|t| t.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tool> {
        self.list.iter()
    }

    /// The `tools` array of an OpenAI-compatible request. Built once per
    /// reply and sent unchanged on every turn, so a server's prompt cache
    /// keeps it.
    pub fn openai(&self) -> Json {
        Json::Array(
            self.list
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect(),
        )
    }
}

/// The arguments of a call as JSON, checked against the tool's schema as
/// far as a small model needs: an object, with every required field.
/// Anything finer is the command's to bring into range.
pub fn arguments(tool: &Tool, raw: &str) -> Result<Json, String> {
    let raw = if raw.trim().is_empty() { "{}" } else { raw };
    let args: Json =
        serde_json::from_str(raw).map_err(|e| format!("the arguments are not JSON ({e})"))?;
    if !args.is_object() {
        return Err("the arguments must be a JSON object".into());
    }
    let required = tool
        .parameters
        .get("required")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    let missing: Vec<&str> = required
        .iter()
        .filter_map(Json::as_str)
        .filter(|k| args.get(k).is_none_or(Json::is_null))
        .collect();
    if !missing.is_empty() {
        return Err(format!("missing {}", missing.join(", ")));
    }
    Ok(args)
}

/// What the loop runs tools through: an app's tools and its draft of the
/// document they act on.
pub trait Toolbox {
    fn tools(&self) -> &Tools;

    /// Runs `tool` with checked arguments: the text the model gets back, or
    /// a refusal saying what to send instead.
    fn call(&mut self, tool: &Tool, args: &Json) -> Result<String, String>;
}
