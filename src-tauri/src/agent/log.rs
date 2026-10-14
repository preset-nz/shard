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

    /// The log as text. Streamed deltas of one kind run together into one
    /// block, so a reply reads as what was said.
    pub fn render(&self) -> String {
        let mut out = String::new();
        // Which streamed block is open, so the next delta of that kind joins it.
        let mut open: Option<&str> = None;
        for (at, event) in &self.lines {
            let (kind, delta) = match event {
                Event::Text { delta } => ("text", Some(delta)),
                Event::Reasoning { delta } => ("reasoning", Some(delta)),
                _ => ("", None),
            };
            if let Some(delta) = delta {
                if open != Some(kind) {
                    if open.is_some() {
                        out.push('\n');
                    }
                    out.push_str(&format!("[{at:8.2}s] {kind}:\n"));
                    open = Some(kind);
                }
                out.push_str(delta);
                continue;
            }
            if open.take().is_some() {
                out.push('\n');
            }
            out.push_str(&format!("[{at:8.2}s] "));
            match event {
                Event::Start { prompt, model } => {
                    out.push_str(&format!("start, model {model:?}\nprompt: {prompt}\n"))
                }
                Event::Tool {
                    name,
                    args,
                    result,
                    ok,
                } => out.push_str(&format!(
                    "tool {name} {}\nargs: {args}\nresult: {result}\n",
                    if *ok { "ok" } else { "refused" }
                )),
                Event::Stopped { reason } => out.push_str(&format!("stopped: {reason:?}\n")),
                Event::Note { text } => out.push_str(&format!("note: {text}\n")),
                Event::Error { message } => out.push_str(&format!("error: {message}\n")),
                Event::Done { applied, label } => {
                    out.push_str(&format!("done, applied {applied}, undo step {label:?}\n\n"))
                }
                Event::Text { .. } | Event::Reasoning { .. } => {}
            }
        }
        if open.is_some() {
            out.push('\n');
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
    }
}
