//! The agent player against a session with no sound card and no network:
//! its tools on a draft, a reply applied as one undo step, and the limits.
//! One test, ignored, asks LM Studio for real.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};

use crate::agent::client::{Call, Chosen, Delta, Failure, Model, Settings, Turn};
use crate::agent::run::{Event, Reason};
use crate::agent::shard::{respond, Draft};
use crate::agent::tools::{Tool, Toolbox};
use crate::object_model::commands::{self, midi_hz, DRONE_ROOT};
use crate::session_tests::rig;
use crate::tracker::{Kit, KitLane, Slot};

/// The lane of `kit` labelled `label`.
fn lane<'a>(kit: &'a Kit, label: &str) -> &'a KitLane {
    kit.lanes
        .iter()
        .find(|l| l.label == label)
        .unwrap_or_else(|| panic!("no lane {label}"))
}

fn hits_of(kit: &Kit, label: &str) -> Vec<u8> {
    lane(kit, label).hits.clone()
}

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
    /// Loops (in its reasoning) at these turns, if the body asks for thinking.
    loop_on: Vec<usize>,
}

impl Script {
    fn new(turns: Vec<Turn>) -> Self {
        Self {
            turns,
            bodies: Vec::new(),
            cancel_on: None,
            late_on: None,
            loop_on: Vec::new(),
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
        if self.loop_on.contains(&(self.bodies.len() - 1)) {
            on(Delta::Reasoning(
                "Plan: set_tempo {\"bpm\": 175}\nOne check: round and round.".into(),
            ));
            return Err(Failure::Looping);
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
    // A new session: Track 1, then Drums 1 (off, with a beat). Steps count
    // from 1, as the step strip shows them; the document holds them from 0.
    let out = call(
        &mut d,
        "set_drums",
        json!({ "track": "Drums 1", "hits": [
            { "step": 1, "midi_note": 36, "velocity": 120 },
            { "step": 5, "midi_note": 38, "velocity": 110 },
            { "step": 5, "midi_note": 39, "velocity": 90 },
            { "step": 3, "midi_note": 42, "velocity": 70 },
            { "step": 7, "midi_note": 46, "velocity": 80 },
            { "step": 9, "midi_note": 49, "velocity": 60 },
            { "step": 11, "drum": "kick", "velocity": 100 },
            { "step": 13, "midi_note": 41, "velocity": 100 },
            { "step": 15, "midi_note": 60, "velocity": 100 },
        ] }),
    )
    .unwrap();
    assert!(out.starts_with("Wrote Drums 1, switched on"), "{out}");
    assert!(out.contains("kick 2, snare 1, hat 3"), "{out}");
    assert!(out.contains("Ignored notes 41, 60"), "{out}");
    assert!(out.contains("switched on"), "{out}");
    assert!(d.apply(&r.session, "Agent: a beat").unwrap());
    let t = r.session.tracker();
    let kit = t.tracks[1].drums.as_ref().unwrap();
    assert!(kit.on, "a drums track written is switched on");
    assert_eq!(
        (
            lane(kit, "Kick").hits[0],
            lane(kit, "Kick").hits[10],
            lane(kit, "Kick").hits[2]
        ),
        (120, 100, 0)
    );
    // Two snares on one step: the harder one.
    assert_eq!(lane(kit, "Snare").hits[4], 110);
    assert_eq!(
        (
            lane(kit, "Hat closed").hits[2],
            lane(kit, "Hat closed").hits[6],
            lane(kit, "Hat closed").hits[8]
        ),
        (70, 80, 60)
    );
    // The old beat is gone: the grid is replaced whole.
    assert_eq!(lane(kit, "Kick").hits.iter().filter(|h| **h > 0).count(), 2);
    let (grid, sound, _) = t.kit(0, &[0, 1, 2, 3]).unwrap();
    assert!(sound.on);
    assert_eq!(grid.hits[0][0], 120);
}

fn drums_of(r: &crate::session_tests::Rig) -> Kit {
    r.session.tracker().tracks[1].drums.clone().unwrap()
}

fn beat() -> Json {
    json!({ "track": "Drums 1", "hits": [
        { "step": 1, "midi_note": 36, "velocity": 120 },
        { "step": 5, "midi_note": 38, "velocity": 110 },
        { "step": 3, "midi_note": 42, "velocity": 70 },
    ] })
}

/// Sets lane `at` of Drums 1 to `hits` by step (from 0), through the tracker.
fn put_hits(r: &crate::session_tests::Rig, at: usize, hits: &[(usize, u8)]) {
    let mut t = r.session.tracker();
    for (step, v) in hits {
        t.tracks[1].drums.as_mut().unwrap().lanes[at].hits[*step] = *v;
    }
    r.session.set_tracker(t).unwrap();
}

#[test]
fn a_beat_leaves_a_material_lane_alone() {
    let r = rig();
    let id = r
        .session
        .pool()
        .materials
        .iter()
        .find(|m| m.name == crate::materials::DRONE_NAME)
        .unwrap()
        .id;
    let at = r.session.add_lane(1).unwrap();
    r.session
        .set_lane_plays(1, at, "material", Some(id))
        .unwrap();
    put_hits(&r, at, &[(2, 99), (9, 88)]);
    let before = lane(&drums_of(&r), "Clap").clone();
    let mut d = Draft::of(&r.session).unwrap();
    call(&mut d, "set_drums", beat()).unwrap();
    assert!(d.apply(&r.session, "Agent: a beat").unwrap());
    let kit = drums_of(&r);
    assert_eq!(lane(&kit, "Clap"), &before);
    assert_eq!(before.material, Some(id));
    assert_eq!(lane(&kit, "Kick").hits[0], 120);
}

#[test]
fn a_beat_silences_the_open_hat() {
    let r = rig();
    put_hits(&r, 3, &[(7, 90), (15, 90)]);
    let mut d = Draft::of(&r.session).unwrap();
    call(&mut d, "set_drums", beat()).unwrap();
    d.apply(&r.session, "Agent: a beat").unwrap();
    let kit = drums_of(&r);
    assert!(lane(&kit, "Hat open").hits.iter().all(|h| *h == 0));
    assert_eq!(lane(&kit, "Hat closed").hits[2], 70);
}

#[test]
fn a_beat_adds_the_lane_a_kind_lacks_and_one_undo_restores_it() {
    let r = rig();
    // Drums 1 is track 1; its snare lane is the second.
    r.session.remove_lane(1, 1).unwrap();
    let before = drums_of(&r);
    assert!(before.lanes.iter().all(|l| l.kind != "snare"));
    let mut d = Draft::of(&r.session).unwrap();
    call(&mut d, "set_drums", beat()).unwrap();
    d.apply(&r.session, "Agent: a beat").unwrap();
    let kit = drums_of(&r);
    assert_eq!(kit.lanes.len(), before.lanes.len() + 1);
    assert_eq!(lane(&kit, "Snare").kind, "snare");
    assert_eq!(lane(&kit, "Snare").hits[4], 110);
    assert_eq!(lane(&kit, "Kick").hits[0], 120);
    r.session.undo().unwrap();
    assert_eq!(drums_of(&r), before);
}

#[test]
fn a_full_track_without_a_snare_refuses_a_snare() {
    let r = rig();
    r.session.remove_lane(1, 1).unwrap();
    while drums_of(&r).lanes.len() < shard_dsp::kit::MAX_LANES {
        r.session.add_lane(1).unwrap();
    }
    let before = drums_of(&r);
    let mut d = Draft::of(&r.session).unwrap();
    let e = call(&mut d, "set_drums", beat()).unwrap_err();
    assert!(e.contains("full") && e.contains("snare"), "{e}");
    d.apply(&r.session, "Agent: a beat").ok();
    assert_eq!(drums_of(&r), before);
}

#[test]
fn a_melody_on_an_existing_track_is_pitched_from_its_patchs_root() {
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    // Track 1 plays Patch 1, whose material is the drone: A2, 45.
    let out = call(
        &mut d,
        "set_melody",
        json!({ "track": "Track 1", "steps": [
            { "step": 1, "note": DRONE_ROOT, "velocity": 100, "hold": 2 },
            { "step": 4, "midi_note": DRONE_ROOT + 3, "velocity": 80, "duration": 0.75 },
            { "step": 9, "note": DRONE_ROOT + 12, "velocity": 90, "hold": 1 },
            { "step": 13, "note": DRONE_ROOT + 31, "velocity": 90, "hold": 1 },
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
            { "step": 1, "note": 29, "velocity": 100, "hold": 3 },
            { "step": 5, "note": 29, "velocity": 80, "hold": 2 },
            { "step": 11, "note": 32, "velocity": 95, "hold": 1 },
        ] }),
    )
    .unwrap();
    assert!(out.starts_with("Wrote Track 2:"), "{out}");
    assert!(out.contains("To change it, write over Track 2"), "{out}");
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
            { "step": 1, "note": 36, "velocity": 120, "hold": 4 },
            { "step": 9, "note": 39, "velocity": 120, "hold": 4 },
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
        json!({ "steps": [{ "step": 1, "note": 45, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap();
    assert!(melody.starts_with("Wrote Track 1:"), "{melody}");
    // The selected track is a melody track, so drums go on a new one.
    let drums = call(
        &mut d,
        "set_drums",
        json!({ "hits": [{ "step": 1, "midi_note": 36, "velocity": 100 }] }),
    )
    .unwrap();
    assert!(drums.starts_with("Wrote Drums 2,"), "{drums}");
    // The draft reads back what it was told, in the UI's names and steps.
    let song = d.song();
    let names: Vec<&str> = song["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Track 1", "Drums 1", "Drums 2"]);
    assert!(song["tracks"][0].get("track").is_none(), "no bare place");
    assert_eq!(song["tracks"][0]["selected"], true);
    assert_eq!(song["tracks"][0]["steps"][0]["step"], 1);
    assert_eq!(song["tracks"][2]["hits"][0]["drum"], "kick");
    assert_eq!(song["tracks"][2]["hits"][0]["step"], 1);
    // Nothing reached the session yet.
    assert_eq!(r.session.track_count(), 2);
    assert_eq!(d.queue().len(), 2);
    assert_eq!(d.queue()[0].1["track"], "Track 1");
    assert_eq!(d.queue()[1].1["track"], "new");
}

#[test]
fn wrong_kinds_and_missing_tracks_are_refused_with_what_to_do() {
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    let note = json!([{ "step": 1, "note": 45, "velocity": 100, "hold": 1 }]);
    let e = call(
        &mut d,
        "set_melody",
        json!({ "track": "Drums 1", "steps": note }),
    )
    .unwrap_err();
    assert!(
        e.contains("Drums 1 is a drums track") && e.contains("set_drums"),
        "{e}"
    );
    let e = call(&mut d, "set_drums", json!({ "track": 7, "hits": [] })).unwrap_err();
    assert_eq!(
        e,
        "there is no Drums 7; the song has Track 1, Drums 1. Leave track out to write a new drums track"
    );
    let e = call(
        &mut d,
        "set_melody",
        json!({ "track": "Track 3", "steps": note }),
    )
    .unwrap_err();
    assert!(
        e.starts_with("there is no Track 3; the song has Track 1, Drums 1"),
        "{e}"
    );
    let e = call(&mut d, "set_melody", json!({ "track": 0, "steps": note })).unwrap_err();
    assert!(e.contains("tracks count from 1"), "{e}");
    let e = call(
        &mut d,
        "set_drums",
        json!({ "track": "the loud one", "hits": [] }),
    )
    .unwrap_err();
    assert!(
        e.contains("\"new\"") && e.contains("Track 1, Drums 1"),
        "{e}"
    );
    let e = call(
        &mut d,
        "set_drums",
        json!({ "track": "Drums 1", "hits": [
            { "step": 0, "midi_note": 36, "velocity": 100 },
            { "step": 4, "midi_note": 38, "velocity": 100 },
        ] }),
    )
    .unwrap_err();
    assert!(e.contains("steps count from 1"), "{e}");
    let e = call(
        &mut d,
        "set_melody",
        json!({ "track": "Track 1", "steps": "lots" }),
    )
    .unwrap_err();
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
            json!({ "hits": [{ "step": 1, "midi_note": 36, "velocity": 110 }] }),
        ),
        turn_calling(
            "set_melody",
            json!({ "track": "new", "steps": [{ "step": 1, "note": 29, "velocity": 100, "hold": 4 }] }),
        ),
        done_turn(),
    ]);
    let events = reply(&r, &mut model, "a  dark bassline and a beat");
    assert_eq!(
        events.first(),
        Some(&Event::Start {
            prompt: "a  dark bassline and a beat".into(),
            model: String::new(),
            considered: false,
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
fn a_stop_keeps_what_was_written() {
    let r = rig();
    let before = text(&r);
    let mut model = Script::new(vec![turn_calling(
        "set_drums",
        json!({ "hits": [{ "step": 1, "midi_note": 36, "velocity": 110 }] }),
    )]);
    // Stopped while the model thinks about its second turn.
    model.cancel_on = Some(1);
    let events = reply(&r, &mut model, "a beat");
    assert!(events.contains(&Event::Stopped {
        reason: Reason::Cancelled
    }));
    assert_eq!(
        events.last(),
        Some(&Event::Done {
            applied: true,
            label: "Agent: a beat".into()
        })
    );
    let t = r.session.tracker();
    assert_eq!(t.tracks.len(), 3);
    assert_eq!(hits_of(t.tracks[2].drums.as_ref().unwrap(), "Kick")[0], 110);
    let (undo, _) = r.session.history_labels();
    assert_eq!(
        undo,
        ["Agent: a beat"],
        "one undo step, as a finished reply"
    );
    r.session.undo().unwrap();
    assert_eq!(text(&r), before);
}

#[test]
fn a_stop_before_any_write_changes_nothing() {
    let r = rig();
    let before = text(&r);
    let mut model = Script::new(Vec::new());
    model.cancel_on = Some(0);
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
        json!({ "track": "Drums 1", "hits": [{ "step": 1, "midi_note": 36, "velocity": 110 }] }),
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
        hits_of(
            r.session.tracker().tracks[1].drums.as_ref().unwrap(),
            "Kick"
        )[0],
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
    let hits = json!([{ "step": 1, "midi_note": 36, "velocity": 110 }]);
    let mut d = Draft::of(&r.session).unwrap();
    let first = call(&mut d, "set_drums", json!({ "track": 1, "hits": hits })).unwrap();
    assert!(
        first.ends_with(
            ". If this answers the request, stop calling tools and reply in one sentence."
        ),
        "a track write ends by saying when to stop: {first}"
    );
    // The same track by its name: the same write.
    let again = call(
        &mut d,
        "set_drums",
        json!({ "track": "drums 1", "hits": hits }),
    )
    .unwrap();
    assert!(again.starts_with("nothing changed"), "{again}");
    assert_eq!(d.queue().len(), 1);
}

#[test]
fn a_later_write_of_a_track_replaces_the_earlier_in_the_queue() {
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    let line = |step: u32| json!({ "track": 1, "steps": [{ "step": step, "note": 45, "velocity": 100, "hold": 1 }] });
    call(&mut d, "set_tempo", json!({ "bpm": 90, "swing": 56 })).unwrap();
    for step in 1..=6 {
        call(&mut d, "set_melody", line(step)).unwrap();
    }
    let made = call(
        &mut d,
        "set_melody",
        json!({ "steps": [{ "step": 1, "note": 40, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap();
    assert!(made.starts_with("Wrote Track 2:"), "{made}");
    call(&mut d, "set_melody", json!({ "track": "Track 2", "steps": [{ "step": 3, "note": 40, "velocity": 100, "hold": 1 }] })).unwrap();
    call(&mut d, "set_melody", line(9)).unwrap();
    // The tempo, the made track, its rewrite, and Track 1's last line.
    let queued: Vec<&Json> = d.queue().iter().map(|(_, p)| &p["track"]).collect();
    assert_eq!(
        queued,
        [
            &Json::Null,
            &json!("new"),
            &json!("Track 2"),
            &json!("Track 1")
        ]
    );
    d.apply(&r.session, "Agent: a line").unwrap();
    let t = r.session.tracker();
    let on = |i: usize| -> Vec<usize> {
        t.tracks[i]
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.on)
            .map(|(i, _)| i)
            .collect()
    };
    assert_eq!(on(0), [8], "the last write of Track 1, its step 9");
    assert_eq!(on(2), [2], "Track 2 made, then rewritten");
    assert!((t.swing - 0.12).abs() < 1e-6);
}

#[test]
fn track_1_in_set_melody_is_the_first_melody_track() {
    // The live failure: "a bassline on track 1" sent track 1, which by
    // place was the drums track.
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    let out = call(
        &mut d,
        "set_melody",
        json!({ "track": 1, "steps": [{ "step": 1, "note": 45, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap();
    assert!(out.starts_with("Wrote Track 1:"), "{out}");
    let out = call(
        &mut d,
        "set_melody",
        json!({ "track": "track 1", "steps": [{ "step": 16, "note": 45, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap();
    assert!(out.starts_with("Wrote Track 1:"), "{out}");
    d.apply(&r.session, "Agent: a line").unwrap();
    let steps = &r.session.tracker().tracks[0].steps;
    assert!(steps[15].on && !steps[0].on, "step 16 is the last of 16");
}

#[test]
fn a_model_that_cant_be_chosen_stops_the_reply_before_it_starts() {
    struct Unloaded;
    impl Model for Unloaded {
        fn resolve(&mut self, _: &str) -> Result<Chosen, String> {
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
                model: String::new(),
                considered: false,
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
        json!({ "track": "Drums 1", "hits": [{ "step": 1, "midi_note": 36, "velocity": 110 }] }),
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
            &json!({ "steps": [{ "step": 1, "note": 40, "velocity": 100, "hold": 1 }] }),
        )
        .unwrap();
    assert!(matches!(out, Outcome::Text(t) if t.starts_with("Wrote Track 2:")));
    let Outcome::Text(song) = doc.run(commands::READ_SONG, &json!({})).unwrap() else {
        panic!("read_song answers text");
    };
    let song: Json = serde_json::from_str(&song).unwrap();
    assert_eq!(song["tracks"][2]["root"], 40);
    assert_eq!(song["tracks"][2]["name"], "Track 2");
    assert_eq!(song["tracks"][2]["steps"][0]["note"], 40);
    assert_eq!(song["tracks"][2]["steps"][0]["step"], 1);
    assert_eq!(doc.tree().undo_label(), Some("Set Melody"));
}

#[test]
fn a_considered_turn_that_loops_is_asked_again_without_thinking() {
    let r = rig();
    let mut m = Script::new(vec![
        turn_calling("set_tempo", json!({ "bpm": 175 })),
        done_turn(),
    ]);
    m.loop_on = vec![0];
    let mut events = Vec::new();
    let cancel = AtomicBool::new(false);
    respond(
        &r.session,
        &mut m,
        &Settings::default(),
        "fast",
        true,
        &cancel,
        &mut |e| events.push(e),
    );
    assert_eq!(m.bodies.len(), 3);
    assert!(m.bodies[0].get("reasoning_effort").is_none());
    assert_eq!(m.bodies[1]["reasoning_effort"], "none");
    let (first, second) = (
        m.bodies[0]["messages"].as_array().unwrap(),
        m.bodies[1]["messages"].as_array().unwrap(),
    );
    assert_eq!(&second[..first.len()], &first[..]);
    assert_eq!(second.len(), first.len() + 1);
    let carried = second[first.len()]["content"].as_str().unwrap();
    assert!(carried.starts_with("Your thinking was cut off. Your plan so far:\n"));
    assert!(carried.contains("set_tempo {\"bpm\": 175}"));
    assert_eq!(m.bodies[1]["tools"], m.bodies[0]["tools"]);
    assert_eq!(m.bodies[0]["presence_penalty"], 1.0);
    assert_eq!(m.bodies[1]["presence_penalty"], 0.0);
    assert!(
        events.iter().any(|e| matches!(e,
        Event::Note { text } if text == "Thinking went in circles; asked again without thinking."))
    );
    assert!(!events.iter().any(|e| matches!(e, Event::Stopped { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Tool { name, ok: true, .. } if name == "set_tempo")));
}

#[test]
fn a_cut_off_turn_still_thinking_is_asked_again_once_and_a_second_loop_stops() {
    let r = rig();
    let cut = Turn {
        text: String::new(),
        calls: Vec::new(),
        finish: Some("length".into()),
    };
    let mut m = Script::new(vec![cut, done_turn()]);
    let events = reply_with(&r, &mut m, "x", true);
    assert_eq!(m.bodies.len(), 2);
    assert_eq!(m.bodies[1]["reasoning_effort"], "none");
    assert!(events.iter().any(|e| matches!(e, Event::Note { .. })));

    let mut m = Script::new(vec![]);
    m.loop_on = vec![0, 1];
    let events = reply_with(&r, &mut m, "y", true);
    assert_eq!(m.bodies.len(), 2);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Stopped {
            reason: Reason::Looping
        }
    )));
    assert_eq!(
        serde_json::to_value(Event::Stopped {
            reason: Reason::Looping
        })
        .unwrap(),
        json!({ "kind": "stopped", "reason": "looping" })
    );
}

fn reply_with(
    r: &crate::session_tests::Rig,
    model: &mut Script,
    prompt: &str,
    considered: bool,
) -> Vec<Event> {
    let mut events = Vec::new();
    let cancel = AtomicBool::new(false);
    respond(
        &r.session,
        model,
        &Settings::default(),
        prompt,
        considered,
        &cancel,
        &mut |e| events.push(e),
    );
    events
}

/// A model that prints when each turn ends and what it called.
struct Logged<M>(M, Instant);

impl<M: Model> Model for Logged<M> {
    fn resolve(&mut self, configured: &str) -> Result<Chosen, String> {
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
                Delta::Calling => {}
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
        .flat_map(|k| &k.lanes)
        .map(|l| count(&l.hits))
        .sum();
    let kicks: usize = on.iter().map(|k| count(&lane(k, "Kick").hits)).sum();
    println!("song: {}", r.session.read(commands::song));
    println!("{hits} hits, {kicks} of them kicks");
    assert!(hits > 0, "a drums track got hits");
}

/// Asks LM Studio for a bassline on Track 1, by name: `cargo test -p shard
/// live_lm_studio_writes -- --ignored --nocapture`, with the model loaded;
/// `SHARD_AGENT_CONSIDERED=1` for reasoning on. It should write Track 1 and
/// stop on its own, before any limit.
#[test]
#[ignore = "needs LM Studio running with a model loaded"]
fn live_lm_studio_writes_a_glitchy_death_metal_bass_on_track_1() {
    let r = rig();
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
        "track 1: glitchy, speedy, death metal baseline in G.",
        considered,
        &cancel,
        &mut |e| {
            match &e {
                Event::Reasoning { delta } => {
                    if let Ok(path) = std::env::var("SHARD_AGENT_DUMP") {
                        use std::io::Write;
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                        {
                            let _ = f.write_all(delta.as_bytes());
                        }
                    }
                }
                Event::Text { .. } => {}
                other => println!(
                    "{:.1} s {}",
                    started.elapsed().as_secs_f32(),
                    serde_json::to_string(other).unwrap()
                ),
            }
            events.push(e);
        },
    );
    println!("took {:.1} s", started.elapsed().as_secs_f32());
    println!("song: {}", r.session.read(commands::song));
    assert!(matches!(
        events.last(),
        Some(Event::Done { applied: true, .. })
    ));
    assert!(r.session.tracker().tracks[0].steps.iter().any(|s| s.on));
}

#[test]
#[ignore = "needs LM Studio running with a model loaded"]
fn live_lm_studio_writes_a_bassline_on_track_1_and_stops() {
    let r = rig();
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
        "only a bassline on Track 1: a death metal verse in G, heavy and dark",
        considered,
        &cancel,
        &mut |e| {
            match &e {
                Event::Text { .. } | Event::Reasoning { .. } => {}
                other => println!(
                    "{:.1} s {}",
                    started.elapsed().as_secs_f32(),
                    serde_json::to_string(other).unwrap()
                ),
            }
            events.push(e);
        },
    );
    println!("took {:.1} s", started.elapsed().as_secs_f32());
    assert!(
        !events.iter().any(|e| matches!(e, Event::Stopped { .. })),
        "it stopped on its own"
    );
    assert!(matches!(
        events.last(),
        Some(Event::Done { applied: true, .. })
    ));
    let t = r.session.tracker();
    println!("song: {}", r.session.read(commands::song));
    assert_eq!(t.tracks.len(), 2, "no new track");
    assert!(t.tracks[0].steps.iter().any(|s| s.on), "Track 1 has notes");
}

/// Strips the sampling fields from a body: the server's own defaults.
struct NoSampling<M>(M);

impl<M: Model> Model for NoSampling<M> {
    fn resolve(&mut self, configured: &str) -> Result<Chosen, String> {
        self.0.resolve(configured)
    }

    fn turn(
        &mut self,
        body: &Json,
        deadline: Instant,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<Turn, Failure> {
        let mut b = body.clone();
        for k in ["temperature", "top_p", "top_k", "min_p", "presence_penalty"] {
            b.as_object_mut().unwrap().remove(k);
        }
        self.0.turn(&b, deadline, cancel, on)
    }
}

/// How a bassline fills its track: notes, rests (steps nothing sounds on,
/// holds counted), distinct pitches, velocity spread.
fn fill(r: &crate::session_tests::Rig) -> (usize, usize, usize, u8) {
    let t = r.session.tracker();
    let Some(track) = t.melody().last() else {
        return (0, 16, 0, 0);
    };
    let n = track.length as usize;
    let mut covered = vec![false; n];
    let (mut notes, mut pitches, mut vels) = (0, std::collections::BTreeSet::new(), Vec::new());
    for (i, s) in track.steps.iter().take(n).enumerate().filter(|(_, s)| s.on) {
        notes += 1;
        pitches.insert(s.pitch);
        vels.push(s.velocity);
        for c in covered.iter_mut().skip(i).take(s.hold.max(1) as usize) {
            *c = true;
        }
    }
    let rests = covered.iter().filter(|c| !**c).count();
    let spread = vels.iter().max().unwrap_or(&0) - vels.iter().min().unwrap_or(&0);
    (notes, rests, pitches.len(), spread)
}

/// Basslines under prompt and sampling variants, in Quick, 3 reps each:
/// `SHARD_VARIANTS=a,b,c,d cargo test -p shard live_fill -- --ignored
/// --nocapture`. Prints one row per answer and a mean per variant.
#[test]
#[ignore = "needs LM Studio running with a model loaded"]
fn live_fill_by_variant() {
    let wanted = std::env::var("SHARD_VARIANTS").unwrap_or_else(|_| "a".into());
    let reps: usize = std::env::var("SHARD_REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let beats = " on a 16-step track, beats fall on 1, 5, 9 and 13";
    let notes_line = "Notes are MIDI numbers, middle C is C4 = 60: so E1 = 28, G1 = 31, A1 = 33, G2 = 43, G3 = 55. ";
    let prompts = [
        "track 1: glitchy, speedy, death metal baseline in G",
        "a sparse dub bassline in E minor, lots of space",
        "a funky syncopated bassline in A",
        "a slow claustrophobic bass, 87 bpm",
    ];
    for v in wanted.split(',') {
        let system = match v {
            "a" | "b" => crate::agent::shard::SYSTEM.to_string(),
            "c" => crate::agent::shard::SYSTEM
                .replace(&format!(":{beats}"), "")
                .replace(notes_line, ""),
            "d" => crate::agent::shard::SYSTEM.replace(
                "Write only the tracks asked for",
                "Leave rests: a bassline breathes; most steps are empty or held. Write only the tracks asked for",
            ),
            "e" => crate::agent::shard::SYSTEM.replace(notes_line, ""),
            other => panic!("no variant {other}"),
        };
        assert_ne!(system, "", "{v}");
        if v == "c" {
            assert!(
                !system.contains("beats fall"),
                "c still has the beats sentence"
            );
        }
        let settings = Settings::default();
        let mut sums = [0.0f64; 5];
        let mut count = 0.0;
        for prompt in prompts {
            for _ in 0..reps {
                let r = rig();
                let http = crate::agent::client::Http::new(&settings);
                let cancel = AtomicBool::new(false);
                let started = Instant::now();
                let mut stopped = false;
                let mut emit = |e: Event| stopped |= matches!(e, Event::Stopped { .. });
                if v == "b" {
                    let mut m = NoSampling(http);
                    crate::agent::shard::respond_with(
                        &r.session, &mut m, &settings, &system, prompt, false, &cancel, &mut emit,
                    );
                } else {
                    let mut m = http;
                    crate::agent::shard::respond_with(
                        &r.session, &mut m, &settings, &system, prompt, false, &cancel, &mut emit,
                    );
                }
                let secs = started.elapsed().as_secs_f64();
                let (n, rests, pitches, spread) = fill(&r);
                println!("{v} | {prompt:.40} | notes {n} rests {rests} pitches {pitches} vel-spread {spread} | {secs:.1} s{}", if stopped { " STOPPED" } else { "" });
                for (s, x) in sums.iter_mut().zip([
                    n as f64,
                    rests as f64,
                    pitches as f64,
                    spread as f64,
                    secs,
                ]) {
                    *s += x;
                }
                count += 1.0;
            }
        }
        println!(
            "MEAN {v}: notes {:.1} rests {:.1} pitches {:.1} vel-spread {:.1} time {:.1} s",
            sums[0] / count,
            sums[1] / count,
            sums[2] / count,
            sums[3] / count,
            sums[4] / count
        );
    }
}

#[test]
fn make_bass_with_a_name_labels_the_new_track_and_its_patch_in_one_undo_step() {
    let r = rig();
    let before = text(&r);
    let mut model = Script::new(vec![
        turn_calling(
            "make_bass",
            json!({ "key": "F", "figure": "sparse", "name": "  Witch Bass " }),
        ),
        turn_calling(
            "make_drums",
            json!({ "style": "ebm_straight", "name": "Witch hats" }),
        ),
        done_turn(),
    ]);
    let events = reply(&r, &mut model, "a witchy bass and hats");
    let said: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Tool {
                ok: true, result, ..
            } => Some(result.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        said[0].contains("Named the track and its patch \"Witch Bass\""),
        "{}",
        said[0]
    );
    assert!(
        said[1].contains("Named the track \"Witch hats\""),
        "{}",
        said[1]
    );

    let t = r.session.tracker();
    assert_eq!(
        t.names(),
        ["Track 1", "Drums 1", "Witch Bass", "Witch hats"]
    );
    let patches = r.session.patches();
    assert_eq!(patches.last().unwrap().name, "Witch Bass");
    assert!(patches.last().unwrap().played, "the new track plays it");
    let (undo, _) = r.session.history_labels();
    assert_eq!(undo.len(), 1, "one step for the whole reply");

    r.session.undo().unwrap();
    assert_eq!(text(&r), before, "the names go with the tracks");
    assert_eq!(r.session.tracker().names(), ["Track 1", "Drums 1"]);
}

#[test]
fn a_track_is_written_over_by_its_name_and_the_queue_keeps_its_place() {
    let r = rig();
    r.session.add_track(false).unwrap();
    r.session.rename_track(2, "Sub bass").unwrap();
    let mut d = Draft::of(&r.session).unwrap();
    let out = call(
        &mut d,
        "set_melody",
        json!({ "track": "sub BASS", "steps": [{ "step": 3, "note": 40, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap();
    assert!(out.starts_with("Wrote Track 2:"), "{out}");
    assert_eq!(
        d.queue()[0].1["track"],
        "Track 2",
        "a label is resolved to its place"
    );
    // A rename in one write survives a later write that leaves the name out.
    call(
        &mut d,
        "set_melody",
        json!({ "track": "Track 2", "name": "Low end", "steps": [{ "step": 5, "note": 40, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap();
    call(
        &mut d,
        "set_melody",
        json!({ "track": "Low end", "steps": [{ "step": 7, "note": 40, "velocity": 100, "hold": 1 }] }),
    )
    .unwrap();
    assert_eq!(d.queue().len(), 1);
    assert_eq!(d.queue()[0].1["name"], "Low end");
    d.apply(&r.session, "Agent: a line").unwrap();
    let t = r.session.tracker();
    assert_eq!(t.names(), ["Track 1", "Drums 1", "Low end"]);
    assert!(t.tracks[2].steps[6].on && !t.tracks[2].steps[4].on);
}

#[test]
fn a_name_that_is_another_tracks_is_refused_and_a_bad_track_lists_the_tracks() {
    let r = rig();
    r.session.add_track(false).unwrap();
    r.session.rename_track(0, "Sub bass").unwrap();
    let mut d = Draft::of(&r.session).unwrap();
    let steps = json!([{ "step": 1, "note": 40, "velocity": 100, "hold": 1 }]);

    for taken in ["sub bass", "Track 1", "Drums 1", "new"] {
        let e = call(
            &mut d,
            "set_melody",
            json!({ "track": "Track 2", "name": taken, "steps": steps }),
        )
        .unwrap_err();
        assert!(
            e.contains("name this track something else") || e.contains("can't be a track's name"),
            "{taken}: {e}"
        );
    }
    // A new track can't take one either.
    let e = call(
        &mut d,
        "set_melody",
        json!({ "name": "Sub bass", "steps": steps }),
    )
    .unwrap_err();
    assert!(e.contains("Sub bass (Track 1)"), "{e}");
    assert!(d.queue().is_empty(), "a refused write is not queued");

    let e = call(
        &mut d,
        "set_melody",
        json!({ "track": "Nope", "steps": steps }),
    )
    .unwrap_err();
    assert!(e.contains("Sub bass (Track 1), Drums 1, Track 2"), "{e}");
}

#[test]
fn read_song_gives_the_place_and_the_label_and_every_write_takes_a_name() {
    let r = rig();
    r.session.rename_track(0, "Sub bass").unwrap();
    let d = Draft::of(&r.session).unwrap();
    let song = d.song();
    assert_eq!(song["tracks"][0]["name"], "Track 1");
    assert_eq!(song["tracks"][0]["label"], "Sub bass");
    assert!(song["tracks"][1].get("label").is_none());
    for which in [
        "make_drums",
        "make_bass",
        "make_arp",
        "set_drums",
        "set_melody",
    ] {
        let t = tool(&d, which);
        assert_eq!(
            t.parameters["properties"]["name"]["type"], "string",
            "{which} takes a name"
        );
    }
}

#[test]
fn a_track_the_model_leaves_nameless_gets_a_name_of_shards_and_the_answer_says_so() {
    let r = rig();
    let bass = json!({ "key": "D", "figure": "ebm_pulse", "track": "new" });
    let mut model = Script::new(vec![
        turn_calling("make_bass", bass.clone()),
        turn_calling("make_bass", bass),
        turn_calling(
            "make_arp",
            json!({ "key": "D", "pattern": "up", "track": "new" }),
        ),
        turn_calling(
            "make_drums",
            json!({ "style": "ebm_straight", "track": "Drums 1" }),
        ),
        done_turn(),
    ]);
    let events = reply(&r, &mut model, "industrial bass, arp and beat");
    let said: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Tool {
                ok: true, result, ..
            } => Some(result.as_str()),
            _ => None,
        })
        .collect();
    assert!(said[0].contains("\"Bass\""), "{}", said[0]);
    assert!(said[0].contains("Shard chose the name"), "{}", said[0]);
    assert!(
        said[1].starts_with("nothing changed"),
        "the same write again is still the same write: {}",
        said[1]
    );
    assert_eq!(
        r.session.tracker().names(),
        ["Track 1", "EBM beat", "Bass", "Arp"]
    );
    assert_eq!(r.session.patches().last().unwrap().name, "Arp");
}

#[test]
fn the_api_key_never_reaches_a_saved_agent_session() {
    let settings = Settings {
        api_key: "sk-secret-123".into(),
        ..Settings::default()
    };
    let (text, json) = crate::dump_headers(&settings, "0.0.0");
    assert!(!text.contains("sk-secret-123"), "agent.log header");
    assert!(
        !json.to_string().contains("sk-secret-123"),
        "agent.jsonl header"
    );
    assert!(
        text.contains(&settings.base_url),
        "the header still names the server"
    );
}

#[test]
fn the_api_key_is_kept_in_agent_json_but_not_in_events() {
    let settings = Settings {
        api_key: "sk-secret-123".into(),
        ..Settings::default()
    };
    let saved = serde_json::to_string(&settings).unwrap();
    assert!(saved.contains("\"apiKey\":\"sk-secret-123\""));
    let back: Settings = serde_json::from_str(&saved).unwrap();
    assert_eq!(back.api_key, "sk-secret-123");
}

fn state_at(name: &str) -> (crate::AgentState, std::path::PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("shard-agent-switch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("agent.json");
    (crate::AgentState::load(path.clone()), path)
}

#[test]
fn the_agent_is_off_when_agent_json_has_no_enabled_key() {
    let (missing_file, path) = state_at("missing");
    assert!(!missing_file
        .enabled
        .load(std::sync::atomic::Ordering::Acquire));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"baseUrl":"http://x/v1","model":"m"}"#).unwrap();
    let old = crate::AgentState::load(path);
    assert!(!old.enabled.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(old.settings.lock().unwrap().base_url, "http://x/v1");
    assert!(old.require_enabled().is_err());
}

#[test]
fn the_switch_round_trips_through_agent_json_beside_the_settings() {
    use std::sync::atomic::Ordering;
    let (state, path) = state_at("round");
    let settings = Settings {
        base_url: "http://localhost:8000/v1".into(),
        model: "qwen".into(),
        ..Settings::default()
    };
    state.save(&settings, true).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"enabled\": true"), "{text}");
    let back = crate::AgentState::load(path);
    assert!(back.enabled.load(Ordering::Acquire));
    assert!(back.require_enabled().is_ok());
    assert_eq!(*back.settings.lock().unwrap(), settings);
}

#[test]
fn every_agent_command_asks_the_switch() {
    let (state, _) = state_at("refuse");
    let err = state.require_enabled().unwrap_err();
    assert!(err.contains("Enable Shard agent"), "{err}");
    state
        .enabled
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(state.require_enabled().is_ok());
}

#[test]
fn the_server_list_is_the_kits() {
    let got = crate::known_servers();
    assert_eq!(got.len(), crate::agent::client::KNOWN_SERVERS.len());
    for (g, (name, url)) in got.iter().zip(crate::agent::client::KNOWN_SERVERS) {
        assert_eq!((g.name.as_str(), g.base_url.as_str()), (*name, *url));
        assert_eq!(g.address, crate::agent::client::address(url));
    }
    assert_eq!(got[0].base_url, Settings::default().base_url);
}
