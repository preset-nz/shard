//! The tool loop: ask, run what the model calls, hand back the results, ask
//! again, until it stops calling or a limit stops it. A reasoning model calls
//! one tool a turn and waits for its result, so the loop is the norm, not a
//! fallback.
//!
//! Every limit is reported, never silent (agent-player.md, "Limits"): a token
//! cap per turn, a wall-clock budget per reply checked while streaming, a turn
//! cap, and a cancel flag.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value as Json};

use super::client::{self, Delta, Failure, Model, Sampling};
use super::tools::{self, Toolbox};

/// What one reply may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_tokens: u32,
    pub budget: Duration,
    pub turns: usize,
}

impl Limits {
    /// Reasoning off: a reply in seconds.
    pub fn quick() -> Self {
        Self {
            max_tokens: 8000,
            budget: Duration::from_secs(60),
            turns: 10,
        }
    }

    /// Reasoning on: about a minute a track.
    pub fn considered() -> Self {
        Self {
            budget: Duration::from_secs(240),
            ..Self::quick()
        }
    }
}

/// Which limit stopped a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    Cancelled,
    Time,
    Tokens,
    Turns,
    /// Its own text went round in circles (and, in Quick, again).
    Looping,
}

/// What the webview hears on `agent://event`. Every reply starts with
/// `start` and ends with exactly one `done`; `stopped` or `error` come just
/// before it when the reply didn't finish on its own.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    Start {
        prompt: String,
        /// The model asked, or empty when none could be.
        model: String,
        /// Reasoning on (Considered), not Quick.
        considered: bool,
    },
    Text {
        delta: String,
    },
    Reasoning {
        delta: String,
    },
    Tool {
        name: String,
        args: Json,
        result: String,
        ok: bool,
    },
    Stopped {
        reason: Reason,
    },
    /// A muted line about what the loop did on its own.
    Note {
        text: String,
    },
    Error {
        message: String,
    },
    /// Whether the reply's writes reached the document, as one undo step
    /// named `label`.
    Done {
        applied: bool,
        label: String,
    },
}

/// How a reply ended.
#[derive(Debug, Clone, PartialEq)]
pub enum End {
    /// The model stopped calling tools.
    Finished,
    Stopped(Reason),
    Failed(String),
}

/// One request: the system prompt and the first user message, both the
/// app's.
pub struct Ask<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub user: String,
    pub quick: bool,
    pub limits: Limits,
    pub sampling: Sampling,
}

/// Said when a considered turn is asked again without thinking.
pub const RETRY_NOTE: &str = "Thinking went in circles; asked again without thinking.";

/// Runs one reply against `toolbox`, telling `emit` as it goes. Emits
/// neither `start` nor `done`: those are the caller's, who applies the
/// writes.
pub fn reply(
    model: &mut dyn Model,
    toolbox: &mut dyn Toolbox,
    ask: &Ask<'_>,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Event),
) -> End {
    let started = Instant::now();
    let deadline = started + ask.limits.budget;
    // Built once, so every request carries the same bytes before the
    // conversation: the server's prompt cache keeps them.
    let tools = toolbox.tools().openai();
    let mut messages = vec![
        json!({ "role": "system", "content": ask.system }),
        json!({ "role": "user", "content": ask.user }),
    ];
    let mut calls_so_far = 0usize;
    // A considered reply that loops is asked again with reasoning off, once;
    // the rest of the reply stays off, so the request body stays the same
    // from turn to turn.
    let mut quick = ask.quick;
    let mut turns_left = ask.limits.turns;
    while turns_left > 0 {
        turns_left -= 1;
        if cancel.load(Ordering::Acquire) {
            return End::Stopped(Reason::Cancelled);
        }
        let body = client::body(
            ask.model,
            &messages,
            &tools,
            quick,
            ask.limits.max_tokens,
            ask.sampling.for_mode(quick),
        );
        let mut thinking = String::new();
        let mut on = |d: Delta| match d {
            Delta::Text(delta) => emit(Event::Text { delta }),
            Delta::Reasoning(delta) => {
                thinking.push_str(&delta);
                emit(Event::Reasoning { delta })
            }
        };
        let turn = match model.turn(&body, deadline, cancel, &mut on) {
            Ok(t) => t,
            Err(Failure::Cancelled) => return End::Stopped(Reason::Cancelled),
            Err(Failure::Time) => return End::Stopped(Reason::Time),
            Err(Failure::Http(m)) => return End::Failed(m),
            Err(Failure::Looping) if quick => return End::Stopped(Reason::Looping),
            Err(Failure::Looping) => {
                quick = true;
                turns_left += 1;
                emit(Event::Note {
                    text: RETRY_NOTE.into(),
                });
                carry_plan(&mut messages, &thinking);
                continue;
            }
        };
        let cut = turn.finish.as_deref() == Some("length");
        if turn.calls.is_empty() {
            // Cut off with no words and no calls: still thinking.
            if cut && !quick && turn.text.trim().is_empty() {
                quick = true;
                turns_left += 1;
                emit(Event::Note {
                    text: RETRY_NOTE.into(),
                });
                carry_plan(&mut messages, &thinking);
                continue;
            }
            return if cut {
                End::Stopped(Reason::Tokens)
            } else {
                End::Finished
            };
        }
        let ids: Vec<String> = turn
            .calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if c.id.is_empty() {
                    format!("call_{}", calls_so_far + i)
                } else {
                    c.id.clone()
                }
            })
            .collect();
        calls_so_far += turn.calls.len();
        messages.push(json!({
            "role": "assistant",
            "content": if turn.text.is_empty() { Json::Null } else { json!(turn.text) },
            "tool_calls": turn.calls.iter().zip(&ids).map(|(c, id)| json!({
                "id": id,
                "type": "function",
                "function": { "name": c.name, "arguments": c.arguments },
            })).collect::<Vec<_>>(),
        }));
        for (c, id) in turn.calls.iter().zip(&ids) {
            if cancel.load(Ordering::Acquire) {
                return End::Stopped(Reason::Cancelled);
            }
            let (args, outcome) = match toolbox.tools().get(&c.name).cloned() {
                None => (
                    Json::Null,
                    Err(format!(
                        "there is no tool called {}; the tools are {}",
                        c.name,
                        toolbox
                            .tools()
                            .iter()
                            .map(|t| t.name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                ),
                Some(tool) => match tools::arguments(&tool, &c.arguments) {
                    Ok(args) => {
                        let out = toolbox.call(&tool, &args);
                        (args, out)
                    }
                    Err(e) => (json!(c.arguments), Err(e)),
                },
            };
            let ok = outcome.is_ok();
            let result = match outcome {
                Ok(r) => r,
                Err(e) => format!("refused: {e}"),
            };
            emit(Event::Tool {
                name: c.name.clone(),
                args,
                result: result.clone(),
                ok,
            });
            messages.push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
        }
        if cut {
            return End::Stopped(Reason::Tokens);
        }
    }
    End::Stopped(Reason::Turns)
}

/// How much of a cut-off thinking the retry is handed back.
const PLAN_TAIL: usize = 3000;

/// Tells the retried turn what it had worked out: the tail of its thinking,
/// from the start of a line so a sentence isn't cut mid-word. Only messages
/// grow; the system prompt and the tools stay the same bytes.
fn carry_plan(messages: &mut Vec<Json>, thinking: &str) {
    let t = thinking.trim();
    if t.is_empty() {
        return;
    }
    let mut start = t.len().saturating_sub(PLAN_TAIL);
    while !t.is_char_boundary(start) {
        start += 1;
    }
    if start > 0 {
        if let Some(nl) = t[start..].find('\n') {
            start += nl + 1;
        }
    }
    messages.push(json!({
        "role": "user",
        "content": format!(
            "Your thinking was cut off. Your plan so far:\n{}\nMake the tool calls from that plan now, then reply in one sentence.",
            t[start..].trim()
        ),
    }));
}

/// An undo label from a prompt: "Agent: " and its first few words.
pub fn label(prompt: &str) -> String {
    const MOST: usize = 32;
    let words = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.chars().count() <= MOST {
        return format!("Agent: {words}");
    }
    let mut out = String::new();
    for w in words.split(' ') {
        if out.chars().count() + w.chars().count() + 1 > MOST {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.is_empty() {
        out = words.chars().take(MOST).collect();
    }
    format!("Agent: {out}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_the_prompts_first_words() {
        assert_eq!(label("a funky beat"), "Agent: a funky beat");
        assert_eq!(
            label("a dark  bassline in F minor with lots of movement and ghost notes"),
            "Agent: a dark bassline in F minor with…"
        );
    }

    #[test]
    fn events_cross_as_tagged_json() {
        let e = serde_json::to_value(Event::Stopped {
            reason: Reason::Time,
        })
        .unwrap();
        assert_eq!(e, json!({ "kind": "stopped", "reason": "time" }));
        let d = serde_json::to_value(Event::Done {
            applied: true,
            label: "Agent: x".into(),
        })
        .unwrap();
        assert_eq!(
            d,
            json!({ "kind": "done", "applied": true, "label": "Agent: x" })
        );
    }
}
