//! What the agent did this run of the app, kept so it can be written out
//! beside the session for later investigation. It holds every event of every
//! reply, reasoning included when the model sends it: the file is Georg's,
//! on this machine, and loops only show in the thinking.

use std::time::Instant;

use super::run::Event;

/// Every event since the app started, each with the seconds it arrived at.
pub struct Log {
    started: Instant,
    lines: Vec<(f64, Event)>,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            lines: Vec::new(),
        }
    }
}

impl Log {
    pub fn push(&mut self, event: &Event) {
        let at = self.started.elapsed().as_secs_f64();
        self.lines.push((at, event.clone()));
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The events with streamed deltas of one kind run together into one,
    /// so a reply reads as what was said. Each keeps the seconds its first
    /// delta arrived at.
    fn blocks(&self) -> Vec<(f64, Event)> {
        let mut out: Vec<(f64, Event)> = Vec::new();
        for (at, event) in &self.lines {
            if let Some((_, last)) = out.last_mut() {
                match (last, event) {
                    (Event::Text { delta: d }, Event::Text { delta })
                    | (Event::Reasoning { delta: d }, Event::Reasoning { delta }) => {
                        d.push_str(delta);
                        continue;
                    }
                    _ => {}
                }
            }
            out.push((*at, event.clone()));
        }
        out
    }

    /// The log as JSON lines, for `evals/agent/agent_logs.py`: one event a
    /// line, tagged as it crossed to the webview, with `at` in seconds.
    pub fn jsonl(&self) -> String {
        let mut out = String::new();
        for (at, event) in self.blocks() {
            let mut line = serde_json::to_value(&event).unwrap_or_default();
            line["at"] = serde_json::json!((at * 100.0).round() / 100.0);
            out.push_str(&line.to_string());
            out.push('\n');
        }
        out
    }

    /// The log as text.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (at, event) in self.blocks() {
            out.push_str(&format!("[{at:8.2}s] "));
            match event {
                Event::Text { delta } => out.push_str(&format!("text:\n{delta}\n")),
                Event::Reasoning { delta } => out.push_str(&format!("reasoning:\n{delta}\n")),
                Event::Start {
                    prompt,
                    model,
                    considered,
                } => out.push_str(&format!(
                    "start, {}, model {model:?}\nprompt: {prompt}\n",
                    if considered { "considered" } else { "quick" }
                )),
                Event::Tool {
                    name,
                    args,
                    result,
                    ok,
                } => out.push_str(&format!(
                    "tool {name} {}\nargs: {args}\nresult: {result}\n",
                    if ok { "ok" } else { "refused" }
                )),
                Event::Stopped { reason } => out.push_str(&format!("stopped: {reason:?}\n")),
                Event::Note { text } => out.push_str(&format!("note: {text}\n")),
                Event::Error { message } => out.push_str(&format!("error: {message}\n")),
                Event::Done { applied, label } => {
                    out.push_str(&format!("done, applied {applied}, undo step {label:?}\n\n"))
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deltas_run_together_and_other_events_break_the_block() {
        let mut log = Log::default();
        assert!(log.is_empty());
        for e in [
            Event::Start {
                prompt: "a beat".into(),
                model: "m".into(),
                considered: true,
            },
            Event::Reasoning {
                delta: "Four on ".into(),
            },
            Event::Reasoning {
                delta: "the floor.".into(),
            },
            Event::Tool {
                name: "set_tempo".into(),
                args: json!({ "bpm": 120 }),
                result: "ok".into(),
                ok: true,
            },
            Event::Text {
                delta: "Done.".into(),
            },
            Event::Done {
                applied: true,
                label: "Agent: a beat".into(),
            },
        ] {
            log.push(&e);
        }
        let text = log.render();
        assert!(text.contains("reasoning:\nFour on the floor.\n"));
        assert!(text.contains("tool set_tempo ok"));
        assert!(text.contains("text:\nDone.\n"));
        assert!(text.contains("prompt: a beat"));
        assert!(text.contains("start, considered"));

        let lines: Vec<serde_json::Value> = log
            .jsonl()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let kinds: Vec<&str> = lines.iter().map(|l| l["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["start", "reasoning", "tool", "text", "done"]);
        assert_eq!(lines[1]["delta"], "Four on the floor.");
        assert_eq!(lines[0]["considered"], true);
        assert!(lines.iter().all(|l| l["at"].is_number()));
    }
}
