//! Shard's side of the agent: its tools, its prompt, and the draft a reply
//! writes into before anything reaches the session.
//!
//! **A reply never holds a gesture open while the model thinks.** rhizome
//! joins every edit made while a gesture is open into it, and refuses undo
//! until it closes, so a gesture held for a minute would swallow whatever
//! the user did meanwhile. Instead each write runs at once against a draft,
//! a copy of the document made when the reply starts, so the model gets a
//! real answer (and `read_song` reads the draft). The writes queue as the
//! commands and payloads they ran as. When the reply ends they run again on
//! the session's document inside one short gesture (`Session::batch`):
//! one undo step, "Agent: a funky beat". A cancelled reply drops the queue.

use std::sync::atomic::AtomicBool;

use rhizome_pom::{Document, Error as PomError, MemoryStore, Outcome};
use serde_json::{json, Value as Json};

use super::client::{Model, Settings};
use super::run::{self, Ask, End, Event, Limits, Reason};
use super::tools::{Access, Tool, Toolbox, Tools};
use crate::object_model::commands::{self, target};
use crate::object_model::Shard;
use crate::session::Session;
use crate::tracker::Track;

/// The system prompt: the eval's three-tool prompt (`evals/agent/variants/
/// tools.txt`) with its tool names and units made this app's, the song's
/// track numbers explained, and the one bass example the evals favoured
/// (`fewshot-bass.txt`), as calls. Sent byte for byte the same on every
/// request.
pub const SYSTEM: &str = r#"You are the groove engine inside a step sequencer. You change the song only by calling tools; never answer with JSON in text. Call set_tempo once if a tempo is asked for, then set_drums or set_melody once per voice asked for.

Translate themes and moods into dark electronic textures: Trip-Hop (slow, smoky, heavy syncopation, lazy swing), Trap (skittering hi-hats, massive sub-bass, rapid rolls), Tricky/Illbient (offset patterns, industrial clanks, unpredictable rests), Dark Metal D&B (relentless 170+ BPM syncopated breakbeats, distorted driving basslines).

For drums, use General MIDI notes: Kick (36), Snare (38), Closed Hat (42), Open Hat (46), Clap (39). Steps are sixteenths counted from 0. hold is in steps, so 1 is one sixteenth. Scale velocities (0-127) for ghost notes, accents and groove. Write only the tracks asked for, one track per voice; a track plays one note at a time. If no voice is named, write one track.

The song as it is now comes with each request. Give a track number to write over that track; leave track out to write a new one.

Example. User: "A heavy, gritty Trip-Hop bass line that feels like a rainy night in Bristol"
set_tempo {"bpm": 85}
set_melody {"steps": [{"step": 0, "note": 29, "velocity": 100, "hold": 3}, {"step": 4, "note": 29, "velocity": 80, "hold": 2}, {"step": 10, "note": 32, "velocity": 95, "hold": 1}]}"#;

const TRACK: &str = "The track to write over, by its number in the song (from 0), or \"new\". Leave it out to write a new track, or the selected one when it is of this kind.";

/// Shard's tools: three coarse writes and a read (the eval's three-tool set,
/// with `set_track` split by kind, since a drums track holds hits on three
/// lanes and a melody track holds notes).
pub fn tools() -> Tools {
    Tools::new(vec![
        Tool {
            name: "set_tempo",
            command: commands::SET_TEMPO,
            description: "Set the song's tempo in beats per minute, and optionally its swing.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "bpm": { "type": "number", "description": "40 to 240" },
                    "swing": { "type": "number", "description": "percent: 50 is straight, 56 a light pocket, 75 heavy" },
                },
                "required": ["bpm"],
            }),
            access: Access::Write,
        },
        Tool {
            name: "set_drums",
            command: commands::SET_DRUMS,
            description: "Write one whole drums track: kick, snare and hat hits on a step grid. Replaces every hit the track had. Several drums may hit on one step.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "track": { "description": TRACK },
                    "length": { "type": "integer", "description": "steps: 16, 32 or 64" },
                    "hits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "step": { "type": "integer" },
                                "midi_note": { "type": "integer", "description": "General MIDI drum note" },
                                "velocity": { "type": "integer" },
                            },
                            "required": ["step", "midi_note", "velocity"],
                        },
                    },
                },
                "required": ["hits"],
            }),
            access: Access::Write,
        },
        Tool {
            name: "set_melody",
            command: commands::SET_MELODY,
            description: "Write one whole melody track: a bass, lead or any pitched voice, one note at a time. Replaces every note the track had. A new track gets a sound of its own.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "track": { "description": TRACK },
                    "length": { "type": "integer", "description": "steps: 16, 32 or 64" },
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "step": { "type": "integer" },
                                "note": { "type": "integer", "description": "MIDI note" },
                                "velocity": { "type": "integer" },
                                "hold": { "type": "integer", "description": "steps the note lasts, 1 to 16" },
                            },
                            "required": ["step", "note", "velocity", "hold"],
                        },
                    },
                },
                "required": ["steps"],
            }),
            access: Access::Write,
        },
        Tool {
            name: "read_song",
            command: commands::READ_SONG,
            description: "Read the song as it is now: tempo, swing and every track, with its number.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            access: Access::Read,
        },
    ])
}

/// A copy of the session's document that a reply writes into, and the
/// writes it queued.
pub struct Draft {
    doc: Document<Shard>,
    tools: Tools,
    /// The selected track when the reply began: what a write that names no
    /// track means, when it is of the right kind.
    selected: Option<usize>,
    /// Each track muted as heard, by place, when the reply began.
    muted: Vec<bool>,
    /// Whether each track was a drums track when the reply began. The
    /// queue names tracks by place, so it applies only to the same tracks.
    kinds: Vec<bool>,
    queue: Vec<(&'static str, Json)>,
}

const DRAFT: &str = "draft.shard";

fn kinds(session: &Session) -> Vec<bool> {
    session
        .tracker()
        .tracks
        .iter()
        .map(Track::is_drums)
        .collect()
}

impl Draft {
    pub fn of(session: &Session) -> Result<Self, String> {
        let text = session.read(|d| d.tree().serialise());
        let store = MemoryStore::default();
        store.put(DRAFT, &text);
        let (doc, _) = Document::<Shard>::open(store, DRAFT).map_err(|e| e.to_string())?;
        Ok(Self {
            doc,
            tools: tools(),
            selected: session.selected_track(),
            muted: session.mix().iter().map(|m| m.heard_mute).collect(),
            kinds: kinds(session),
            queue: Vec::new(),
        })
    }

    /// The song as `read_song` answers: the command's, plus which track is
    /// selected and which are muted, which are the session's.
    pub fn song(&self) -> Json {
        let mut song = commands::song(&self.doc);
        if let Some(tracks) = song.get_mut("tracks").and_then(Json::as_array_mut) {
            for (i, t) in tracks.iter_mut().enumerate() {
                if self.selected == Some(i) {
                    t["selected"] = json!(true);
                }
                if self.muted.get(i).copied().unwrap_or(false) {
                    t["muted"] = json!(true);
                }
            }
        }
        song
    }

    /// The writes queued, as commands and their payloads.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn queue(&self) -> &[(&'static str, Json)] {
        &self.queue
    }

    /// Runs the queue on the session as one undo step named `label`.
    /// Nothing, and false, when nothing was queued. Refused, with nothing
    /// applied, when the session's tracks are no longer the ones the draft
    /// was made from.
    pub fn apply(&self, session: &Session, label: &str) -> Result<bool, String> {
        if self.queue.is_empty() {
            return Ok(false);
        }
        if kinds(session) != self.kinds {
            return Err(
                "the tracks changed while the agent was writing, so nothing was applied; ask again"
                    .into(),
            );
        }
        session.batch(label, |doc| {
            for (command, payload) in &self.queue {
                doc.run(command, payload).map_err(|e| e.to_string())?;
            }
            Ok(())
        })?;
        Ok(true)
    }
}

impl Toolbox for Draft {
    fn tools(&self) -> &Tools {
        &self.tools
    }

    fn call(&mut self, tool: &Tool, args: &Json) -> Result<String, String> {
        if tool.access == Access::Read {
            return Ok(self.song().to_string());
        }
        let mut payload = args.clone();
        let drums = match tool.command {
            commands::SET_DRUMS => Some(true),
            commands::SET_MELODY => Some(false),
            _ => None,
        };
        let named = args.get("track").is_some_and(|t| !t.is_null());
        if let (Some(drums), false) = (drums, named) {
            // No track named: the selected one when it is of this kind.
            let tracks = &self.doc.projection().tracker.tracks;
            let fits = self
                .selected
                .filter(|s| tracks.get(*s).is_some_and(|t| t.is_drums() == drums));
            payload["track"] = match fits {
                Some(s) => json!(s),
                None => json!("new"),
            };
        }
        if drums.is_some() {
            // Refused here rather than by the command, so it costs no edit.
            target(&payload)?;
        }
        // A reasoning model checking its work writes the same track again
        // and again until the turn cap; tell it nothing changed.
        if self
            .queue
            .last()
            .is_some_and(|(c, p)| *c == tool.command && *p == payload)
        {
            return Ok(
                "nothing changed: the song already holds exactly this. If the song is done, stop calling tools."
                    .into(),
            );
        }
        match self.doc.run(tool.command, &payload) {
            Ok(out) => {
                self.queue.push((tool.command, payload));
                Ok(match out {
                    Outcome::Text(t) => t,
                    _ => "done".into(),
                })
            }
            Err(PomError::Payload(m)) => Err(m),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The first user message: the request, and the song as `read_song` would
/// answer, so a quick reply needn't spend a turn reading it.
pub fn user_message(prompt: &str, song: &Json) -> String {
    format!("{prompt}\n\nThe song now:\n{song}")
}

/// One whole reply: `start`, the loop, the writes applied as one undo step,
/// `done`. Cancelled or failed, nothing is applied. Stopped by a limit, what
/// was written so far is applied: each write is a whole track, and one undo
/// takes it all back.
pub fn respond(
    session: &Session,
    model: &mut dyn Model,
    settings: &Settings,
    prompt: &str,
    considered: bool,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Event),
) {
    let label = run::label(prompt);
    let chosen = model.resolve(&settings.model);
    emit(Event::Start {
        prompt: prompt.to_string(),
        model: chosen.clone().unwrap_or_default(),
    });
    let name = match chosen {
        Ok(name) => name,
        Err(message) => {
            emit(Event::Error { message });
            emit(Event::Done {
                applied: false,
                label,
            });
            return;
        }
    };
    let mut draft = match Draft::of(session) {
        Ok(d) => d,
        Err(message) => {
            emit(Event::Error { message });
            emit(Event::Done {
                applied: false,
                label,
            });
            return;
        }
    };
    let ask = Ask {
        model: &name,
        system: SYSTEM,
        user: user_message(prompt, &draft.song()),
        quick: !considered,
        limits: if considered {
            Limits::considered()
        } else {
            Limits::quick()
        },
    };
    let end = run::reply(model, &mut draft, &ask, cancel, emit);
    let keep = match end {
        End::Finished => true,
        End::Stopped(reason) => {
            emit(Event::Stopped { reason });
            reason != Reason::Cancelled
        }
        End::Failed(message) => {
            emit(Event::Error { message });
            false
        }
    };
    let applied = if keep {
        match draft.apply(session, &label) {
            Ok(a) => a,
            Err(message) => {
                emit(Event::Error { message });
                false
            }
        }
    } else {
        false
    };
    emit(Event::Done { applied, label });
}
