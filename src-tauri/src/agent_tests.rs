//! The agent player against a session with no sound card and no network:
//! its tools on a draft, a reply applied as one undo step, and the limits.
//! One test, ignored, asks LM Studio for real.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};

use crate::agent::client::{Call, Delta, Failure, Model, Settings, Turn};
use crate::agent::run::{Event, Reason};
use crate::agent::shard::{respond, Draft};
use crate::agent::tools::{Tool, Toolbox};
use crate::object_model::commands::{self, midi_hz, DRONE_ROOT};
use crate::session_tests::rig;
use crate::tracker::Slot;

fn tool(d: &Draft, name: &str) -> Tool {
    d.tools().get(name).expect("the tool").clone()
}

fn call(d: &mut Draft, name: &str, args: Json) -> Result<String, String> {
    let t = tool(d, name);
    d.call(&t, &args)
}

fn turn_calling(name: &str, args: Json) -> Turn {
    Turn {
        text: String::new(),
        calls: vec![Call {
            id: String::new(),
            name: name.into(),
            arguments: args.to_string(),
        }],
        finish: Some("tool_calls".into()),
    }
}

fn done_turn() -> Turn {
    Turn {
        text: "Done.".into(),
        calls: Vec::new(),
        finish: Some("stop".into()),
    }
}

/// A model that answers from a script, keeping every request it was sent.
/// `cancel_on` cancels at that turn, as Esc pressed while it streams.
struct Script {
    turns: Vec<Turn>,
    bodies: Vec<Json>,
    cancel_on: Option<usize>,
    /// Runs out of time at that turn, as a stream still going at the budget.
    late_on: Option<usize>,
}

impl Script {
    fn new(turns: Vec<Turn>) -> Self {
        Self {
            turns,
            bodies: Vec::new(),
            cancel_on: None,
            late_on: None,
        }
    }
}

impl Model for Script {
    fn turn(
        &mut self,
        body: &Json,
        _deadline: Instant,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<Turn, Failure> {
        self.bodies.push(body.clone());
        if self.cancel_on == Some(self.bodies.len() - 1) {
            cancel.store(true, Ordering::Release);
            return Err(Failure::Cancelled);
        }
        if self.late_on == Some(self.bodies.len() - 1) {
            return Err(Failure::Time);
        }
        let t = if self.turns.is_empty() {
            done_turn()
        } else {
            self.turns.remove(0)
        };
        if !t.text.is_empty() {
            on(Delta::Text(t.text.clone()));
        }
        Ok(t)
    }
}

fn reply(r: &crate::session_tests::Rig, model: &mut Script, prompt: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let cancel = AtomicBool::new(false);
    respond(
        &r.session,
        model,
        &Settings::default(),
        prompt,
        false,
        &cancel,
        &mut |e| events.push(e),
    );
    events
}

fn text(r: &crate::session_tests::Rig) -> String {
    r.session.read(|d| d.tree().serialise())
}

#[test]
fn drums_take_general_midi_notes_and_lane_names_and_say_what_they_ignored() {
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    // A new session: track 0 melody, track 1 drums (off, with a beat).
    let out = call(
        &mut d,
        "set_drums",
        json!({ "track": 1, "hits": [
            { "step": 0, "midi_note": 36, "velocity": 120 },
            { "step": 4, "midi_note": 38, "velocity": 110 },
            { "step": 4, "midi_note": 39, "velocity": 90 },
            { "step": 2, "midi_note": 42, "velocity": 70 },
            { "step": 6, "midi_note": 46, "velocity": 80 },
            { "step": 8, "midi_note": 49, "velocity": 60 },
            { "step": 10, "drum": "kick", "velocity": 100 },
            { "step": 12, "midi_note": 41, "velocity": 100 },
            { "step": 14, "midi_note": 60, "velocity": 100 },
        ] }),
    )
    .unwrap();
    assert!(out.contains("kick 2, snare 1, hat 3"), "{out}");
    assert!(out.contains("Ignored notes 41, 60"), "{out}");
    assert!(out.contains("switched on"), "{out}");
    assert!(d.apply(&r.session, "Agent: a beat").unwrap());
    let t = r.session.tracker();
    let kit = t.tracks[1].drums.as_ref().unwrap();
    assert!(kit.on, "a drums track written is switched on");
    assert_eq!((kit.kick[0], kit.kick[10], kit.kick[2]), (120, 100, 0));
    // Two snares on one step: the harder one.
    assert_eq!(kit.snare[4], 110);
    assert_eq!((kit.hat[2], kit.hat[6], kit.hat[8]), (70, 80, 60));
    // The old beat is gone: the grid is replaced whole.
    assert_eq!(kit.kick.iter().filter(|h| **h > 0).count(), 2);
    let (grid, sound, _) = t.kit(0).unwrap();
    assert!(sound.on);
    assert_eq!(grid.hits[0][0], 120);
}

#[test]
fn a_melody_on_an_existing_track_is_pitched_from_its_patchs_root() {
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    // Track 0 plays Patch 1, whose material is the drone: A2, 45.
    let out = call(
        &mut d,
        "set_melody",
        json!({ "track": 0, "steps": [
            { "step": 0, "note": DRONE_ROOT, "velocity": 100, "hold": 2 },
            { "step": 3, "midi_note": DRONE_ROOT + 3, "velocity": 80, "duration": 0.75 },
            { "step": 8, "note": DRONE_ROOT + 12, "velocity": 90, "hold": 1 },
            { "step": 12, "note": DRONE_ROOT + 31, "velocity": 90, "hold": 1 },
        ] }),
    )
    .unwrap();
    assert!(out.contains("root note 45"), "{out}");
    assert!(out.contains("Moved 1 notes by octaves"), "{out}");
    d.apply(&r.session, "Agent: a line").unwrap();
    let steps = &r.session.tracker().tracks[0].steps;
    let on: Vec<(usize, i8, u8, u8)> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.on)
        .map(|(i, s)| (i, s.pitch, s.hold, s.velocity))
        .collect();
    // 31 above the root folds an octave down to 19; a duration of 0.75 is
    // three steps.
    assert_eq!(
        on,
        vec![
            (0, 0, 2, 100),
            (3, 3, 3, 80),
            (8, 12, 1, 90),
            (12, 19, 1, 90)
        ]
    );
}

#[test]
fn a_new_melody_track_gets_an_fm_voice_of_its_own_at_its_lowest_note() {
    let r = rig();
    let patches_before = r.session.patches().len();
    let first_patch = r.session.plan().track_patches[0];
    let mut d = Draft::of(&r.session).unwrap();
    let out = call(
        &mut d,
        "set_melody",
        json!({ "track": "new", "steps": [
            { "step": 0, "note": 29, "velocity": 100, "hold": 3 },
            { "step": 4, "note": 29, "velocity": 80, "hold": 2 },
            { "step": 10, "note": 32, "velocity": 95, "hold": 1 },
        ] }),
    )
    .unwrap();
    assert!(out.starts_with("melody track 2 written"), "{out}");
    assert!(out.contains("root note 29"), "{out}");
    d.apply(&r.session, "Agent: a bass").unwrap();
    let t = r.session.tracker();
    assert_eq!(t.slots()[2], Slot::Melody(1));
    let pitches: Vec<i8> = t.tracks[2]
        .steps
        .iter()
        .filter(|s| s.on)
        .map(|s| s.pitch)
        .collect();
    assert_eq!(pitches, vec![0, 0, 3]);
    assert_eq!(r.session.patches().len(), patches_before + 1);
    let plan = r.session.plan();
    // Its own patch: the first track's sound is untouched.
    assert_eq!(plan.track_patches[0], first_patch);
    let mine = plan.track_patches[2].expect("a patch");
    assert_ne!(Some(mine), first_patch);
    let p = plan.patch_plan(mine).unwrap();
    assert_eq!(p.generators, vec!["fm"], "FM alone: the material is out");
    let row = |id: &str| {
        let i = shard_dsp::params::PARAMS
            .iter()
            .position(|d| d.id == id)
            .unwrap();
        p.values[i]
    };
    assert!((row("fm.freq") - midi_hz(29.0)).abs() < 0.01);
    assert_eq!(row("patch.length"), 2.0, "Hold, so a step's hold is heard");
}

#[test]
fn a_new_melody_track_is_heard() {
    // The first track muted and the drums off, so silence, until the agent
    // adds a track.
    let quiet = rig();
    quiet.session.set_mute(0, true).unwrap();
    let r = rig();
    r.session.set_mute(0, true).unwrap();
    let mut d = Draft::of(&r.session).unwrap();
    call(
        &mut d,
        "set_melody",
        json!({ "steps": [
            { "step": 0, "note": 36, "velocity": 120, "hold": 4 },
            { "step": 8, "note": 39, "velocity": 120, "hold": 4 },
        ] }),
    )
    .unwrap();
    d.apply(&r.session, "Agent: a bass").unwrap();
    let peak = |r: &crate::session_tests::Rig| {
        r.render(48_000.0, 2.0)
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    };
    let (before, after) = (peak(&quiet), peak(&r));
    assert!(before < 0.01, "silent without it: peak {before}");
    assert!(
        after > 0.05,
        "the new track's FM voice is heard: peak {after}"
    );
}

#[test]
fn a_write_naming_no_track_goes_to_the_selected_one_of_its_kind_or_a_new_one() {
    let r = rig();
    r.session.select_track(0).unwrap();
    let mut d = Draft::of(&r.session).unwrap();
    let melody = call(
        &mut d,
        "set_melody",
        json!({ "steps": [{ "step": 0, "note": 45, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap();
    assert!(melody.starts_with("melody track 0 written"), "{melody}");
    // The selected track is a melody track, so drums go on a new one.
    let drums = call(
        &mut d,
        "set_drums",
        json!({ "hits": [{ "step": 0, "midi_note": 36, "velocity": 100 }] }),
    )
    .unwrap();
    assert!(drums.starts_with("drums track 2 written"), "{drums}");
    // The draft reads back what it was told.
    let song = d.song();
    assert_eq!(song["tracks"].as_array().unwrap().len(), 3);
    assert_eq!(song["tracks"][0]["selected"], true);
    assert_eq!(song["tracks"][2]["hits"][0]["drum"], "kick");
    // Nothing reached the session yet.
    assert_eq!(r.session.track_count(), 2);
    assert_eq!(d.queue().len(), 2);
    assert_eq!(d.queue()[1].1["track"], "new");
}

#[test]
fn wrong_kinds_and_missing_tracks_are_refused_with_what_to_do() {
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    let e = call(
        &mut d,
        "set_melody",
        json!({ "track": 1, "steps": [{ "step": 0, "note": 45, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap_err();
    assert!(e.contains("drums track") && e.contains("set_drums"), "{e}");
    let e = call(&mut d, "set_drums", json!({ "track": 7, "hits": [] })).unwrap_err();
    assert!(e.contains("there is no track 7"), "{e}");
    let e = call(
        &mut d,
        "set_drums",
        json!({ "track": "the loud one", "hits": [] }),
    )
    .unwrap_err();
    assert!(e.contains("\"new\""), "{e}");
    let e = call(&mut d, "set_melody", json!({ "track": 0, "steps": "lots" })).unwrap_err();
    assert!(e.contains("\"steps\""), "{e}");
    assert!(d.queue().is_empty(), "a refusal queues nothing");
    assert!(!d.apply(&r.session, "Agent: nothing").unwrap());
}

#[test]
fn a_whole_reply_is_one_undo_step_that_takes_new_tracks_and_patches_with_it() {
    let r = rig();
    let before = text(&r);
    let patches = r.session.patches().len();
    let mut model = Script::new(vec![
        turn_calling("set_tempo", json!({ "bpm": 92, "swing": 56 })),
        turn_calling(
            "set_drums",
            json!({ "hits": [{ "step": 0, "midi_note": 36, "velocity": 110 }] }),
        ),
        turn_calling(
            "set_melody",
            json!({ "track": "new", "steps": [{ "step": 0, "note": 29, "velocity": 100, "hold": 4 }] }),
        ),
        done_turn(),
    ]);
    let events = reply(&r, &mut model, "a  dark bassline and a beat");
    assert_eq!(
        events.first(),
        Some(&Event::Start {
            prompt: "a  dark bassline and a beat".into(),
            model: String::new(),
        })
    );
    assert_eq!(
        events.last(),
        Some(&Event::Done {
            applied: true,
            label: "Agent: a dark bassline and a beat".into()
        })
    );
    let tools: Vec<bool> = events
        .iter()
        .filter_map(|e| match e {
            Event::Tool { ok, .. } => Some(*ok),
            _ => None,
        })
        .collect();
    assert_eq!(tools, vec![true, true, true]);
    let t = r.session.tracker();
    assert_eq!(t.tempo, 92.0);
    assert!((t.swing - 0.12).abs() < 1e-6, "56 % is 0.12 of a sixteenth");
    assert_eq!(t.tracks.len(), 4);
    assert_eq!(r.session.patches().len(), patches + 1);
    let (undo, _) = r.session.history_labels();
    assert_eq!(
        undo.last().map(String::as_str),
        Some("Agent: a dark bassline and a beat")
    );
    assert_eq!(undo.len(), 1, "one step for the whole reply");

    r.session.undo().unwrap();
    assert_eq!(text(&r), before, "one undo takes every write back");
    assert_eq!(r.session.track_count(), 2);
    assert_eq!(r.session.patches().len(), patches);

    // Every request carried the same system prompt and tools, so a prompt
    // cache keeps them; the first user message carried the song.
    let b = &model.bodies;
    assert_eq!(b.len(), 4);
    for body in &b[1..] {
        assert_eq!(body["tools"], b[0]["tools"]);
        assert_eq!(body["messages"][0], b[0]["messages"][0]);
        assert_eq!(body["messages"][1], b[0]["messages"][1]);
    }
    assert!(b[0]["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("\"tracks\""));
    assert_eq!(b[0]["reasoning_effort"], "none");
    assert_eq!(b[3]["messages"].as_array().unwrap().len(), 2 + 3 * 2);
}

#[test]
fn a_cancelled_reply_applies_nothing() {
    let r = rig();
    let before = text(&r);
    let mut model = Script::new(vec![turn_calling(
        "set_drums",
        json!({ "hits": [{ "step": 0, "midi_note": 36, "velocity": 110 }] }),
    )]);
    model.cancel_on = Some(1);
    let events = reply(&r, &mut model, "a beat");
    assert!(events.contains(&Event::Stopped {
        reason: Reason::Cancelled
    }));
    assert_eq!(
        events.last(),
        Some(&Event::Done {
            applied: false,
            label: "Agent: a beat".into()
        })
    );
    assert_eq!(text(&r), before);
    assert!(r.session.history_labels().0.is_empty(), "no undo step");
}

#[test]
fn a_reply_stopped_by_its_turn_cap_says_so_and_keeps_what_it_wrote() {
    let r = rig();
    let calls: Vec<Turn> = (0..12)
        .map(|_| turn_calling("set_tempo", json!({ "bpm": 100 })))
        .collect();
    let mut model = Script::new(calls);
    let events = reply(&r, &mut model, "keep going");
    assert_eq!(model.bodies.len(), 10, "the turn cap");
    assert!(events.contains(&Event::Stopped {
        reason: Reason::Turns
    }));
    assert!(matches!(
        events.last(),
        Some(Event::Done { applied: true, .. })
    ));
    assert_eq!(r.session.tracker().tempo, 100.0);
}

#[test]
fn a_reply_out_of_time_says_so_and_keeps_what_it_wrote() {
    let r = rig();
    let mut model = Script::new(vec![turn_calling(
        "set_drums",
        json!({ "track": 1, "hits": [{ "step": 0, "midi_note": 36, "velocity": 110 }] }),
    )]);
    model.late_on = Some(1);
    let events = reply(&r, &mut model, "a beat");
    let n = events.len();
    assert_eq!(
        events[n - 2],
        Event::Stopped {
            reason: Reason::Time
        }
    );
    assert!(matches!(events[n - 1], Event::Done { applied: true, .. }));
    assert_eq!(
        r.session.tracker().tracks[1].drums.as_ref().unwrap().kick[0],
        110
    );
}

#[test]
fn a_call_cut_off_by_the_token_cap_is_refused_and_the_reply_stops() {
    let r = rig();
    let before = text(&r);
    let mut model = Script::new(vec![Turn {
        text: String::new(),
        calls: vec![Call {
            id: "a".into(),
            name: "set_drums".into(),
            arguments: "{\"hits\": [{\"step\": 0, \"midi_no".into(),
        }],
        finish: Some("length".into()),
    }]);
    let events = reply(&r, &mut model, "a beat");
    assert_eq!(model.bodies.len(), 1, "no turn after the cap");
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Tool { ok: false, .. })));
    assert!(events.contains(&Event::Stopped {
        reason: Reason::Tokens
    }));
    assert!(matches!(
        events.last(),
        Some(Event::Done { applied: false, .. })
    ));
    assert_eq!(text(&r), before);
}

#[test]
fn the_same_write_again_changes_nothing_and_says_so() {
    let r = rig();
    let args = json!({ "track": 1, "hits": [{ "step": 0, "midi_note": 36, "velocity": 110 }] });
    let mut d = Draft::of(&r.session).unwrap();
    call(&mut d, "set_drums", args.clone()).unwrap();
    let again = call(&mut d, "set_drums", args).unwrap();
    assert!(again.starts_with("nothing changed"), "{again}");
    assert_eq!(d.queue().len(), 1);
}

#[test]
fn a_model_that_cant_be_chosen_stops_the_reply_before_it_starts() {
    struct Unloaded;
    impl Model for Unloaded {
        fn resolve(&mut self, _: &str) -> Result<String, String> {
            Err("No model loaded in LM Studio. Load one there, then ask again.".into())
        }
        fn turn(
            &mut self,
            _: &Json,
            _: Instant,
            _: &AtomicBool,
            _: &mut dyn FnMut(Delta),
        ) -> Result<Turn, Failure> {
            panic!("never asked");
        }
    }
    let r = rig();
    let mut events = Vec::new();
    respond(
        &r.session,
        &mut Unloaded,
        &Settings::default(),
        "a beat",
        false,
        &AtomicBool::new(false),
        &mut |e| events.push(e),
    );
    assert_eq!(
        events,
        vec![
            Event::Start {
                prompt: "a beat".into(),
                model: String::new()
            },
            Event::Error {
                message: "No model loaded in LM Studio. Load one there, then ask again.".into()
            },
            Event::Done {
                applied: false,
                label: "Agent: a beat".into()
            },
        ]
    );
}

#[test]
fn bad_arguments_and_unknown_tools_go_back_to_the_model_as_refusals() {
    let r = rig();
    let mut model = Script::new(vec![
        Turn {
            text: String::new(),
            calls: vec![
                Call {
                    id: "a".into(),
                    name: "set_drums".into(),
                    arguments: "{\"hits\": [".into(),
                },
                Call {
                    id: "b".into(),
                    name: "set_track".into(),
                    arguments: "{}".into(),
                },
                Call {
                    id: "c".into(),
                    name: "set_tempo".into(),
                    arguments: "{}".into(),
                },
            ],
            finish: Some("tool_calls".into()),
        },
        done_turn(),
    ]);
    let events = reply(&r, &mut model, "a beat");
    let results: Vec<(String, bool)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Tool { result, ok, .. } => Some((result.clone(), *ok)),
            _ => None,
        })
        .collect();
    assert!(results[0]
        .0
        .starts_with("refused: the arguments are not JSON"));
    assert!(results[1].0.contains("there is no tool called set_track"));
    assert_eq!(results[2].0, "refused: missing bpm");
    assert!(results.iter().all(|(_, ok)| !ok));
    // What the model was told is what the panel showed.
    let sent = &model.bodies[1]["messages"];
    assert_eq!(sent[3]["role"], "tool");
    assert_eq!(sent[3]["tool_call_id"], "a");
    assert_eq!(sent[3]["content"].as_str().unwrap(), results[0].0);
    assert_eq!(
        events.last(),
        Some(&Event::Done {
            applied: false,
            label: "Agent: a beat".into()
        })
    );
}

#[test]
fn a_reply_whose_tracks_changed_meanwhile_applies_nothing() {
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    call(
        &mut d,
        "set_drums",
        json!({ "track": 1, "hits": [{ "step": 0, "midi_note": 36, "velocity": 110 }] }),
    )
    .unwrap();
    // While the model thought, a track was added by hand.
    r.session.add_track(true).unwrap();
    let before = text(&r);
    let e = d.apply(&r.session, "Agent: a beat").unwrap_err();
    assert!(e.contains("tracks changed"), "{e}");
    assert_eq!(text(&r), before);
}

#[test]
fn the_commands_run_on_a_document_without_a_session() {
    // What a CLI or a Python module would do: open, run by id, read.
    use rhizome_pom::{Document, MemoryStore, Outcome};
    let mut doc = Document::<crate::object_model::Shard>::new(MemoryStore::default()).unwrap();
    let out = doc
        .run(
            commands::SET_MELODY,
            &json!({ "steps": [{ "step": 0, "note": 40, "velocity": 100, "hold": 1 }] }),
        )
        .unwrap();
    assert!(matches!(out, Outcome::Text(t) if t.starts_with("melody track 2 written")));
    let Outcome::Text(song) = doc.run(commands::READ_SONG, &json!({})).unwrap() else {
        panic!("read_song answers text");
    };
    let song: Json = serde_json::from_str(&song).unwrap();
    assert_eq!(song["tracks"][2]["root"], 40);
    assert_eq!(song["tracks"][2]["steps"][0]["note"], 40);
    assert_eq!(doc.tree().undo_label(), Some("Set Melody"));
}

/// A model that prints when each turn ends and what it called.
struct Logged<M>(M, Instant);

impl<M: Model> Model for Logged<M> {
    fn resolve(&mut self, configured: &str) -> Result<String, String> {
        self.0.resolve(configured)
    }

    fn turn(
        &mut self,
        body: &Json,
        deadline: Instant,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<Turn, Failure> {
        let (mut text, mut reasoning) = (0, 0);
        let out = self.0.turn(body, deadline, cancel, &mut |d| {
            match &d {
                Delta::Text(t) => text += t.len(),
                Delta::Reasoning(r) => reasoning += r.len(),
            }
            on(d)
        });
        let calls = match &out {
            Ok(t) => t
                .calls
                .iter()
                .map(|c| format!("{} ({} chars)", c.name, c.arguments.len()))
                .collect::<Vec<_>>()
                .join(", "),
            Err(e) => format!("{e:?}"),
        };
        println!(
            "turn ended at {:.1} s: {text} chars of text, {reasoning} of reasoning; {calls}",
            self.1.elapsed().as_secs_f32()
        );
        out
    }
}

/// Asks LM Studio for real: `cargo test -p shard live_lm_studio -- --ignored
/// --nocapture`, with the model loaded; `SHARD_AGENT_CONSIDERED=1` for
/// reasoning on.
#[test]
#[ignore = "needs LM Studio running with a model loaded"]
fn live_lm_studio_writes_a_four_on_the_floor_beat() {
    let r = rig();
    // SHARD_AGENT_CONSIDERED=1 asks with reasoning on.
    let considered = std::env::var("SHARD_AGENT_CONSIDERED").is_ok_and(|v| v == "1");
    let settings = Settings::default();
    let mut model = Logged(crate::agent::client::Http::new(&settings), Instant::now());
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    let started = Instant::now();
    respond(
        &r.session,
        &mut model,
        &settings,
        "a four-on-the-floor beat",
        considered,
        &cancel,
        &mut |e| {
            match &e {
                Event::Text { .. } | Event::Reasoning { .. } => {}
                other => println!("{}", serde_json::to_string(other).unwrap()),
            }
            events.push(e);
        },
    );
    let took = started.elapsed();
    println!("took {:.1} s", took.as_secs_f32());
    let budget = if considered { 241 } else { 61 };
    assert!(took < Duration::from_secs(budget));
    assert!(matches!(
        events.last(),
        Some(Event::Done { applied: true, .. })
    ));
    let t = r.session.tracker();
    let on: Vec<&crate::tracker::Kit> = t
        .drums()
        .filter_map(|d| d.drums.as_ref())
        .filter(|k| k.on)
        .collect();
    let count = |row: &Vec<u8>| row.iter().filter(|h| **h > 0).count();
    let hits: usize = on
        .iter()
        .map(|k| count(&k.kick) + count(&k.snare) + count(&k.hat))
        .sum();
    let kicks: usize = on.iter().map(|k| count(&k.kick)).sum();
    println!("song: {}", r.session.read(commands::song));
    println!("{hits} hits, {kicks} of them kicks");
    assert!(hits > 0, "a drums track got hits");
}
