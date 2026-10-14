//! The brief commands (`object_model::generate`) and their agent tools: on a
//! bare document as a script would run them, and through the agent's draft.

use rhizome_pom::{Document, MemoryStore, Outcome};
use serde_json::{json, Value as Json};
use shard_dsp::fx::{self, Kind as Fx};
use shard_dsp::params::index_of;

use crate::agent::shard::{tools, Draft};
use crate::agent::tools::Toolbox;
use crate::music::drums::STYLES;
use crate::music::line::{Figure, Pattern};
use crate::music::{Mode, PROGRESSIONS};
use crate::object_model::commands;
use crate::object_model::generate::{
    LIST_DRUM_STYLES, LIST_PROGRESSIONS, MAKE_ARP, MAKE_BASS, MAKE_DRUMS,
};
use crate::object_model::Shard;
use crate::session_tests::rig;

fn doc() -> Document<Shard> {
    Document::<Shard>::new(MemoryStore::default()).unwrap()
}

fn run(d: &mut Document<Shard>, command: &str, p: Json) -> Result<String, String> {
    match d.run(command, &p) {
        Ok(Outcome::Text(t)) => Ok(t),
        Ok(_) => Ok(String::new()),
        Err(rhizome_pom::Error::Payload(m)) => Err(m),
        Err(e) => Err(e.to_string()),
    }
}

fn song(d: &mut Document<Shard>) -> Json {
    serde_json::from_str(&run(d, commands::READ_SONG, json!({})).unwrap()).unwrap()
}

fn track<'a>(song: &'a Json, name: &str) -> &'a Json {
    song["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {song}"))
}

#[test]
fn the_brief_commands_run_on_a_document_without_a_session() {
    let mut d = doc();
    let out = run(
        &mut d,
        MAKE_DRUMS,
        json!({ "style": "ebm_straight", "bars": 1 }),
    )
    .unwrap();
    assert!(
        out.starts_with("Wrote Drums 2, switched on: 16 steps, kick 4, snare 2"),
        "{out}"
    );
    assert!(out.contains("seed "), "{out}");
    let s = song(&mut d);
    let drums = track(&s, "Drums 2");
    let steps = |drum: &str| -> Vec<i64> {
        drums["hits"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["drum"] == drum)
            .map(|h| h["step"].as_i64().unwrap())
            .collect()
    };
    assert_eq!(steps("kick"), [1, 5, 9, 13]);
    assert_eq!(steps("snare"), [5, 13]);
    assert_eq!(d.tree().undo_label(), Some("Set Drums"));

    let out = run(
        &mut d,
        MAKE_BASS,
        json!({ "key": "A", "progression": "i-bVI-bVII", "figure": "ebm_pulse", "register": "sub" }),
    )
    .unwrap();
    assert!(out.starts_with("Wrote Track 2:"), "{out}");
    assert!(out.contains("on a new FM bass patch of its own"), "{out}");
    assert!(out.contains("in A minor (Am F G)"), "{out}");
    let s = song(&mut d);
    let bass = track(&s, "Track 2");
    assert_eq!(
        bass["length"], 64,
        "four chords' worth, a bar each, in four bars"
    );
    for n in bass["steps"].as_array().unwrap() {
        let note = n["note"].as_i64().unwrap();
        assert!((24..=48).contains(&note), "sub: {n}");
        assert!(Mode::Minor.holds(note as i32 - 9), "{n}");
    }

    let out = run(
        &mut d,
        MAKE_ARP,
        json!({ "key": "E phrygian", "progression": "i-bII", "pattern": "up_down", "rate": 8, "octaves": 2 }),
    )
    .unwrap();
    assert!(out.starts_with("Wrote Track 3:"), "{out}");
    assert!(out.contains("in E phrygian (Em F)"), "{out}");
    let s = song(&mut d);
    let arp = track(&s, "Track 3");
    let notes = arp["steps"].as_array().unwrap();
    assert!(
        notes.iter().all(|n| n["step"].as_i64().unwrap() % 2 == 1),
        "eighths"
    );
    for n in notes {
        assert!(
            Mode::Phrygian.holds(n["note"].as_i64().unwrap() as i32 - 4),
            "{n}"
        );
    }

    for list in [LIST_DRUM_STYLES, LIST_PROGRESSIONS] {
        assert!(run(&mut d, list, json!({}))
            .unwrap()
            .starts_with(['[', '{']));
    }
}

#[test]
fn a_new_bass_gets_a_driven_fm_patch_and_an_arp_a_brighter_one() {
    let mut d = doc();
    run(
        &mut d,
        MAKE_BASS,
        json!({ "key": "A", "progression": "i", "figure": "gallop" }),
    )
    .unwrap();
    run(
        &mut d,
        MAKE_ARP,
        json!({ "key": "A", "progression": "i", "pattern": "up" }),
    )
    .unwrap();
    let plan = d.projection();
    let patch_of = |i: usize| plan.patch_plan(plan.track_patches[i].unwrap()).unwrap();
    let value = |p: &crate::object_model::PatchPlan, id: &str| p.values[index_of(id).unwrap()];
    let copy0 = fx::POOL
        .iter()
        .position(|&(k, c)| k == Fx::Drive && c == 0)
        .unwrap();
    let (bass, arp) = (patch_of(2), patch_of(3));
    assert_eq!(value(bass, "fm.on"), 1.0);
    // The order names instance n as n + 1, and 0 is an empty slot.
    assert_eq!(value(bass, "fx.order.0"), (copy0 + 1) as f32, "drive leads");
    assert_eq!(value(bass, "fx.order.1"), 0.0, "alone");
    assert_eq!(value(arp, "fx.order.0"), 0.0, "the arp's chain is empty");
    assert!(
        value(bass, &fx::row_id(copy0, "drive.mix")) > 0.0,
        "drive is heard"
    );
    // The bass moves: a driven ladder whose cutoff follows an envelope on
    // every note, and an LFO turning the FM index over four bars.
    assert_eq!(value(bass, "ladder.on"), 1.0, "the ladder is heard");
    assert!(value(bass, "ladder.mix") > 0.0);
    let m = &bass.modulation;
    let cutoff = m.links.get("ladder.cutoff").expect("the cutoff follows");
    assert!(m.envelopes.iter().any(|e| e.id == cutoff.source), "{m:?}");
    assert!(cutoff.hi > cutoff.lo, "the envelope opens the filter");
    let index = m.links.get("fm.index").expect("the index follows");
    let lfo = m
        .lfos
        .iter()
        .find(|l| l.id == index.source)
        .expect("an LFO");
    let four_bars = 120.0 / 60.0 / 16.0;
    assert!((lfo.rate - four_bars).abs() < 1e-3, "{}", lfo.rate);
    assert!(arp.modulation.links.is_empty(), "the arp stays as it was");

    assert!(value(arp, "fm.index") > value(bass, "fm.index"), "brighter");
    assert!(
        value(arp, "env.sustain") < value(bass, "env.sustain"),
        "shorter"
    );
}

#[test]
fn a_line_aimed_at_a_drone_track_goes_to_a_new_track_and_says_so() {
    let mut d = doc();
    let before = song(&mut d);
    let out = run(
        &mut d,
        MAKE_BASS,
        json!({ "track": "Track 1", "key": "E", "progression": "i-bVI", "figure": "sparse" }),
    )
    .unwrap();
    assert!(
        out.starts_with("Track 1 plays the built-in drone, so the bass went on a new track instead. Wrote Track 2:"),
        "{out}"
    );
    assert!(out.contains("on a new FM bass patch of its own"), "{out}");
    let s = song(&mut d);
    assert_eq!(track(&s, "Track 1"), track(&before, "Track 1"), "untouched");
    assert!(!track(&s, "Track 2")["steps"].as_array().unwrap().is_empty());

    // On a track with a sound of its own, the line goes where it was sent.
    let out = run(
        &mut d,
        MAKE_BASS,
        json!({ "track": "Track 2", "key": "E", "progression": "i", "figure": "gallop" }),
    )
    .unwrap();
    assert!(out.starts_with("Wrote Track 2:"), "{out}");
}

#[test]
fn the_same_brief_writes_the_same_track_and_says_its_seed() {
    let brief = json!({ "style": "glitch_broken", "glitch": 0.7, "bars": 4 });
    let (mut a, mut b) = (doc(), doc());
    let ta = run(&mut a, MAKE_DRUMS, brief.clone()).unwrap();
    let tb = run(&mut b, MAKE_DRUMS, brief.clone()).unwrap();
    assert_eq!(ta, tb);
    assert_eq!(song(&mut a), song(&mut b));
    assert!(ta.contains("none was sent"), "{ta}");
    let mut with_seed = brief.clone();
    with_seed["seed"] = json!(7);
    let mut c = doc();
    let tc = run(&mut c, MAKE_DRUMS, with_seed).unwrap();
    assert!(tc.contains("seed 7;"), "{tc}");
    assert_ne!(song(&mut a), song(&mut c));
}

#[test]
fn a_brief_out_of_the_lists_is_refused_with_them() {
    let mut d = doc();
    let e = run(&mut d, MAKE_DRUMS, json!({ "style": "polka" })).unwrap_err();
    assert!(
        e.contains("ebm_straight") && e.contains("glitch_broken"),
        "{e}"
    );
    let e = run(
        &mut d,
        MAKE_BASS,
        json!({ "key": "A", "progression": "i", "figure": "walking" }),
    )
    .unwrap_err();
    assert!(e.contains("ebm_pulse"), "{e}");
    let e = run(
        &mut d,
        MAKE_BASS,
        json!({ "key": "H", "progression": "i", "figure": "sparse" }),
    )
    .unwrap_err();
    assert!(e.contains("not a key"), "{e}");
    let out = run(
        &mut d,
        MAKE_BASS,
        json!({ "key": "A", "progression": "i-bVI-bIII-bVII-i-bVI-bIII-bVII", "figure": "sparse", "chord_bars": 1, "bars": 4 }),
    )
    .unwrap();
    assert!(out.contains("Only the first 4 of its 8 chords"), "{out}");
}

/// Every name in every table is offered in a tool's schema, and the list
/// tools answer every one.
#[test]
fn every_table_entry_is_reachable_through_the_tools() {
    let t = tools();
    let schema_enum = |tool: &str, field: &str| -> Vec<String> {
        t.get(tool).unwrap().parameters["properties"][field]["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("{tool}.{field} has an enum"))
            .iter()
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
            .collect()
    };
    let styles: Vec<String> = STYLES.iter().map(|s| s.name.to_string()).collect();
    assert_eq!(schema_enum("make_drums", "style"), styles);
    let progs: Vec<String> = PROGRESSIONS.iter().map(|p| p.name.to_string()).collect();
    assert_eq!(schema_enum("make_bass", "progression"), progs);
    assert_eq!(schema_enum("make_arp", "progression"), progs);
    let figures: Vec<String> = Figure::ALL.iter().map(|f| f.name().to_string()).collect();
    assert_eq!(schema_enum("make_bass", "figure"), figures);
    let patterns: Vec<String> = Pattern::ALL.iter().map(|p| p.name().to_string()).collect();
    assert_eq!(schema_enum("make_arp", "pattern"), patterns);
    let modes: Vec<String> = Mode::ALL.iter().map(|m| m.name().to_string()).collect();
    assert_eq!(schema_enum("make_bass", "mode"), modes);
    assert_eq!(schema_enum("list_progressions", "mode"), modes);

    let mut d = doc();
    let listed: Json =
        serde_json::from_str(&run(&mut d, LIST_DRUM_STYLES, json!({})).unwrap()).unwrap();
    let listed: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(listed, STYLES.iter().map(|s| s.name).collect::<Vec<_>>());
    let all: Json =
        serde_json::from_str(&run(&mut d, LIST_PROGRESSIONS, json!({})).unwrap()).unwrap();
    assert_eq!(
        all["progressions"].as_array().unwrap().len(),
        PROGRESSIONS.len()
    );
    // Every feel and every mode finds something, and nothing finds nothing.
    for feel in schema_enum("list_progressions", "feel") {
        for mode in &modes {
            let got: Json = serde_json::from_str(
                &run(
                    &mut d,
                    LIST_PROGRESSIONS,
                    json!({ "feel": feel, "mode": mode }),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(
                !got["progressions"].as_array().unwrap().is_empty(),
                "{feel} {mode}"
            );
        }
    }
    // Every progression and figure writes.
    for p in PROGRESSIONS {
        run(&mut d, MAKE_BASS, json!({ "track": "Track 1", "key": "D", "progression": p.name, "figure": "root_fifth" }))
            .unwrap_or_else(|e| panic!("{}: {e}", p.name));
    }
    for s in STYLES {
        run(
            &mut d,
            MAKE_DRUMS,
            json!({ "track": "Drums 1", "style": s.name }),
        )
        .unwrap();
    }
}

#[test]
fn the_draft_writes_briefs_as_tracks_and_reads_lists_without_queueing() {
    let r = rig();
    let mut d = Draft::of(&r.session).unwrap();
    let list = d.tools().get("list_progressions").unwrap().clone();
    let text = d
        .call(&list, &json!({ "feel": "dark", "mode": "phrygian" }))
        .unwrap();
    assert!(text.contains("i-bII"), "{text}");
    assert!(d.queue().is_empty(), "a read queues nothing");

    let bass = d.tools().get("make_bass").unwrap().clone();
    let args = json!({ "track": "new", "key": "A", "progression": "i-bVI-bVII", "figure": "octave_pulse" });
    let out = d.call(&bass, &args).unwrap();
    assert!(
        out.contains("stop calling tools"),
        "ends with the finish cue: {out}"
    );
    assert_eq!(d.queue().len(), 1);
    let again = d.call(&bass, &args).unwrap();
    assert!(again.starts_with("nothing changed"), "{again}");
    d.apply(&r.session, "Agent: bass").unwrap();
    let t = r.session.tracker();
    assert!(t
        .tracks
        .iter()
        .any(|t| !t.is_drums() && t.steps.iter().filter(|s| s.on).count() == 64));
}

/// The three first-slice prompts against LM Studio, in Quick: `cargo test -p
/// shard live_brief -- --ignored --nocapture`, with the model loaded. Prints
/// each tool call, the time, and what each track holds.
#[test]
#[ignore = "needs LM Studio running with a model loaded"]
fn live_brief_prompts() {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use crate::agent::client::{Http, Settings};
    use crate::agent::run::Event;
    use crate::agent::shard::respond;

    let prompts = [
        "an EBM beat with a pulsing bass in A minor, dark",
        "glitchy broken drums, sparse",
        "a phrygian arp over a dark progression in E",
    ];
    for prompt in prompts {
        let r = rig();
        let settings = Settings::default();
        let mut model = Http::new(&settings);
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        println!("\n== {prompt}");
        respond(
            &r.session,
            &mut model,
            &settings,
            prompt,
            false,
            &cancel,
            &mut |e| match &e {
                Event::Tool {
                    name,
                    args,
                    result,
                    ok,
                } => {
                    println!(
                        "  {name} {args} -> {}{result}",
                        if *ok { "" } else { "REFUSED: " }
                    )
                }
                Event::Text { .. } | Event::Reasoning { .. } => {}
                other => println!("  {}", serde_json::to_string(other).unwrap()),
            },
        );
        println!("  took {:.1} s", started.elapsed().as_secs_f32());
        let song = r.session.read(commands::song);
        println!("  tempo {} swing {}", song["tempo"], song["swing"]);
        for t in song["tracks"].as_array().unwrap() {
            if t["kind"] == "drums" {
                let hits = t["hits"].as_array().unwrap();
                let lane = |l: &str| -> Vec<i64> {
                    hits.iter()
                        .filter(|h| h["drum"] == l)
                        .map(|h| h["step"].as_i64().unwrap())
                        .collect()
                };
                println!(
                    "  {} ({} steps, on {}): kick {:?}\n      snare {:?}\n      hat {:?}",
                    t["name"],
                    t["length"],
                    t["on"],
                    lane("kick"),
                    lane("snare"),
                    lane("hat")
                );
            } else {
                let steps = t["steps"].as_array().unwrap();
                let mut pitches: Vec<String> = steps
                    .iter()
                    .map(|n| crate::music::note_name(n["note"].as_i64().unwrap() as i32))
                    .collect();
                let first: Vec<String> = pitches.iter().take(16).cloned().collect();
                pitches.sort();
                pitches.dedup();
                let held: i64 = steps.iter().map(|n| n["hold"].as_i64().unwrap()).sum();
                println!(
                    "  {} ({} steps): {} notes, {} held steps, rests {}, pitches {:?}\n      first: {}",
                    t["name"],
                    t["length"],
                    steps.len(),
                    held,
                    t["length"].as_i64().unwrap() - held.min(t["length"].as_i64().unwrap()),
                    pitches,
                    first.join(" ")
                );
            }
        }
    }
}
