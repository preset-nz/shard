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
//! one undo step, "Agent: a funky beat". A reply stopped by the user or by a
//! limit applies what it wrote so far, the same way; only a reply that
//! failed applies nothing.

use std::sync::atomic::AtomicBool;

use rhizome_pom::{Document, Error as PomError, MemoryStore, Outcome};
use serde_json::{json, Value as Json};

use super::client::{Model, Settings};
use super::run::{self, Ask, End, Event, Limits};
use super::tools::{Access, Tool, Toolbox, Tools};
use crate::music;
use crate::object_model::commands::{self, Target};
use crate::object_model::generate;
use crate::object_model::Shard;
use crate::session::Session;
use crate::tracker::Track;

/// The system prompt: the eval's three-tool prompt (`evals/agent/variants/
/// tools.txt`) with its tool names and units made this app's, tracks and
/// steps named as Shard shows them, steered to the brief tools (`make_*`)
/// for whole tracks; its example is the request of the bass the evals
/// favoured (`fewshot-bass.txt`), answered as a brief. Sent byte for byte
/// the same on every request.
pub const SYSTEM: &str = r#"You are the groove engine inside a step sequencer. You change the song only by calling tools; never answer with JSON in text. Call set_tempo once if a tempo is asked for, then one tool per voice asked for, then stop calling tools and reply in one sentence. For a whole track, choose and let Shard write the notes: make_drums for a beat, make_bass for a bassline, make_arp for an arpeggio. Use set_drums or set_melody only for exact notes the user dictates.

Translate themes and moods into dark electronic textures: EBM (driving sixteenth bass pulse, four-on-the-floor, 120-135 BPM), Glitch (broken, sparse, displaced hits), Trip-Hop (slow, smoky, heavy syncopation, lazy swing), Trap (skittering hi-hats, massive sub-bass, rapid rolls), Tricky/Illbient (offset patterns, industrial clanks, unpredictable rests), Dark Metal D&B (relentless 170+ BPM syncopated breakbeats, distorted driving basslines).

For drums, use General MIDI notes: Kick (36), Snare (38), Closed Hat (42), Open Hat (46), Clap (39). Steps are sixteenths counted from 1, as Shard shows them: on a 16-step track, beats fall on 1, 5, 9 and 13. hold is in steps, so 1 is one sixteenth. Scale velocities (0-127) for ghost notes, accents and groove. Notes are MIDI numbers, middle C is C4 = 60: so E1 = 28, G1 = 31, A1 = 33, G2 = 43, G3 = 55. A bar is 16 steps, and a track holds at most 64 steps (4 bars): for more, write 64 and say they loop. When the user counts in steps, send steps, not bars. Write only the tracks asked for, one track per voice; a track plays one note at a time. If no voice is named, write one track.

Tracks are named as Shard shows them: melody tracks Track 1, Track 2 and on, drums tracks Drums 1, Drums 2 and on. The song as it is now comes with each request. Give a track's name to write over that track; leave track out to write a new one.

Example. User: "A heavy, gritty Trip-Hop bass line that feels like a rainy night in Bristol"
set_tempo {"bpm": 85}
make_bass {"key": "F", "mode": "minor", "feel": "sad", "figure": "sparse", "register": "sub"}"#;

const TRACK: &str = "The track to write over, by its name as Shard shows it (\"Track 1\", \"Drums 2\"), or \"new\". Leave it out to write a new track, or the selected one when it is of this kind.";

const STEP: &str = "counted from 1 to the track's length, as Shard shows them: on a 16-step track, beats fall on 1, 5, 9 and 13";

/// What a track write's answer ends with, so a model that has done what was
/// asked stops rather than checking its work by writing it again.
const FINISHED: &str =
    ". If this answers the request, stop calling tools and reply in one sentence.";

const SEED: &str = "Any whole number. Leave it out the first time; send a different one for another take of the same request.";

const BARS: &str = "1, 2 or 4; a bar is 16 steps, and a track holds at most 4 bars, so a longer request gets 4 that loop";

const LENGTH: &str = "the track's length in steps, 4, 8, 16, 32 or 64, when the user counts in steps (\"8 steps\"); send it instead of bars";

/// Shard's tools: the brief tools (whole tracks from a few choices, written
/// by `crate::music`'s generators) and what they choose from, then the
/// eval's exact writes and read (`set_track` split by kind, since a drums
/// track holds hits on three lanes and a melody track holds notes).
pub fn tools() -> Tools {
    let styles = generate::style_names();
    let progressions = generate::progression_names();
    let modes = generate::mode_names();
    let registers = generate::register_names();
    let key = json!({ "type": "string", "description": "the tonic: A, F#, Bb; may name the mode too (\"E phrygian\")" });
    let mode = json!({ "type": "string", "enum": modes, "description": "the scale; left out, the key's or the progression's own" });
    let progression = json!({ "type": "string", "enum": progressions, "description": "only when the user names the chords; otherwise leave it out and send feel" });
    let feel = json!({ "type": "string", "enum": music::feels(), "description": "the mood the chords should have; Shard picks a progression with it" });
    let chord_bars =
        json!({ "type": "number", "enum": [0.5, 1, 2, 4], "description": "bars each chord lasts" });
    Tools::new(vec![
        Tool {
            name: "make_drums",
            command: generate::MAKE_DRUMS,
            description: "Write a whole drums track in a style: Shard places the hits. Replaces every hit the track had.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "track": { "description": TRACK },
                    "style": { "type": "string", "enum": styles, "description": "ebm_straight and ebm_shuffle for EBM, glitch_sparse and glitch_broken for glitch; list_drum_styles describes each" },
                    "bars": { "type": "integer", "description": BARS },
                    "steps": { "type": "integer", "description": LENGTH },
                    "density": { "type": "number", "description": "0 to 1: 0.5 as the style is written, lower thinner, higher adds hats and ghost notes" },
                    "glitch": { "type": "number", "description": "0 to 1: 0 clean; higher drops, displaces and stutters hits" },
                    "seed": { "type": "integer", "description": SEED },
                },
                "required": ["style"],
            }),
            access: Access::Write,
        },
        Tool {
            name: "make_bass",
            command: generate::MAKE_BASS,
            description: "Write a whole bass line that follows a chord progression: Shard places the notes. A new track gets a bass sound of its own.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "track": { "description": TRACK },
                    "key": key,
                    "mode": mode,
                    "feel": feel,
                    "progression": progression,
                    "figure": { "type": "string", "enum": generate::figure_names(), "description": "ebm_pulse: sixteenths on the root with gaps; rolling: three sixteenths after each beat, resting on the kick; octave_pulse: root and octave; acid: a new line each bar with accents, jumps and ties; offbeat: eighths between the kicks; root_fifth; gallop: an eighth and two sixteenths; sparse: long notes and space" },
                    "register": { "type": "string", "enum": registers, "description": "sub, low (the default), mid or high" },
                    "bars": { "type": "integer", "description": BARS },
                    "steps": { "type": "integer", "description": LENGTH },
                    "chord_bars": chord_bars,
                    "seed": { "type": "integer", "description": SEED },
                },
                "required": ["key", "figure"],
            }),
            access: Access::Write,
        },
        Tool {
            name: "make_arp",
            command: generate::MAKE_ARP,
            description: "Write a whole arpeggio track over a chord progression: Shard places the notes. A new track gets an arp sound of its own.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "track": { "description": TRACK },
                    "key": key,
                    "mode": mode,
                    "feel": feel,
                    "progression": progression,
                    "pattern": { "type": "string", "enum": generate::pattern_names() },
                    "rate": { "type": "integer", "enum": generate::rates(), "description": "8 for eighths, 16 for sixteenths" },
                    "octaves": { "type": "integer", "enum": [1, 2] },
                    "register": { "type": "string", "enum": registers, "description": "mid is the default" },
                    "bars": { "type": "integer", "description": BARS },
                    "steps": { "type": "integer", "description": LENGTH },
                    "chord_bars": chord_bars,
                    "seed": { "type": "integer", "description": SEED },
                },
                "required": ["key", "pattern"],
            }),
            access: Access::Write,
        },
        Tool {
            name: "list_drum_styles",
            command: generate::LIST_DRUM_STYLES,
            description: "List the drum styles make_drums plays, each with what it sounds like.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            access: Access::Read,
        },
        Tool {
            name: "list_progressions",
            command: generate::LIST_PROGRESSIONS,
            description: "List chord progressions for make_bass and make_arp by feel and mode, each with its feel.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "feel": { "type": "string", "enum": music::feels() },
                    "mode": { "type": "string", "enum": generate::mode_names() },
                },
                "required": [],
            }),
            access: Access::Read,
        },
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
            description: "Write one whole drums track: kick, snare and hat hits on a step grid, steps counted from 1. Replaces every hit the track had. Several drums may hit on one step.",
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
                                "step": { "type": "integer", "description": STEP },
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
            description: "Write one whole melody track: a bass, lead or any pitched voice, one note at a time, steps counted from 1. Replaces every note the track had. A new track gets a sound of its own.",
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
                                "step": { "type": "integer", "description": STEP },
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
            description: "Read the song as it is now: tempo, swing and every track by its name, steps counted from 1.",
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
    /// queue names tracks as Shard does, by place among their kind, so it
    /// applies only to the same tracks.
    kinds: Vec<bool>,
    /// At most one write per named track: a later write of a track replaces
    /// the earlier, which it would overwrite whole anyway. A write that made
    /// a track (`"new"`) stays, since replaying it makes the track.
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
            if tool.command == commands::READ_SONG {
                return Ok(self.song().to_string());
            }
            // Reads the tables, never the song: nothing to queue.
            return match self.doc.run(tool.command, args) {
                Ok(Outcome::Text(t)) => Ok(t),
                Ok(_) => Ok("done".into()),
                Err(PomError::Payload(m)) => Err(m),
                Err(e) => Err(e.to_string()),
            };
        }
        let mut payload = args.clone();
        let drums = match tool.command {
            commands::SET_DRUMS | generate::MAKE_DRUMS => Some(true),
            commands::SET_MELODY | generate::MAKE_BASS | generate::MAKE_ARP => Some(false),
            _ => None,
        };
        if let Some(drums) = drums {
            let tracker = &self.doc.projection().tracker;
            let named = args.get("track").is_some_and(|t| !t.is_null());
            if !named {
                // No track named: the selected one when it is of this kind.
                let fits = self.selected.filter(|s| {
                    tracker
                        .tracks
                        .get(*s)
                        .is_some_and(|t| t.is_drums() == drums)
                });
                payload["track"] = match fits {
                    Some(s) => json!(commands::track_names(tracker)[s]),
                    None => json!("new"),
                };
            }
            // Refused here rather than by the command, so it costs no edit;
            // and the track as its name, so the same track is always the
            // same payload.
            payload["track"] = match commands::resolve(tracker, &payload, drums)? {
                Target::Place(i) => json!(commands::track_names(tracker)[i]),
                Target::New => json!("new"),
            };
        }
        // A reasoning model checking its work writes the same track again
        // and again until the turn cap; tell it nothing changed.
        let same_target = |(c, p): &&(&'static str, Json)| {
            *c == tool.command && p.get("track") == payload.get("track")
        };
        if self
            .queue
            .iter()
            .rev()
            .find(same_target)
            .is_some_and(|(_, p)| *p == payload)
        {
            return Ok(
                "nothing changed: the song already holds exactly this. If the song is done, stop calling tools and reply in one sentence."
                    .into(),
            );
        }
        match self.doc.run(tool.command, &payload) {
            Ok(out) => {
                let track = payload.get("track").and_then(Json::as_str);
                if drums.is_some() && track != Some("new") {
                    // It overwrites the track whole, so the earlier write
                    // of it would only be undone on replay.
                    self.queue.retain(|(c, p)| {
                        !(*c == tool.command && p.get("track") == payload.get("track"))
                    });
                }
                self.queue.push((tool.command, payload));
                let mut text = match out {
                    Outcome::Text(t) => t,
                    _ => "done".into(),
                };
                if drums.is_some() {
                    text += FINISHED;
                }
                Ok(text)
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
/// `done`. Stopped, by the user or by a limit, what was written so far is
/// applied: each write is a whole track, and one undo takes it all back.
/// Failed, nothing is applied.
pub fn respond(
    session: &Session,
    model: &mut dyn Model,
    settings: &Settings,
    prompt: &str,
    considered: bool,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Event),
) {
    respond_with(
        session, model, settings, SYSTEM, prompt, considered, cancel, emit,
    )
}

/// `respond` with another system prompt: for the live prompt experiments.
#[allow(clippy::too_many_arguments)]
pub fn respond_with(
    session: &Session,
    model: &mut dyn Model,
    settings: &Settings,
    system: &str,
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
        system,
        user: user_message(prompt, &draft.song()),
        quick: !considered,
        sampling: settings.sampling,
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
            true
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
