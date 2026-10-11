//! Several patches in a session (epic 10, story a): the current patch is the
//! session's, every patch-scoped command acts on it, and the engine follows
//! it without a click.

use shard_dsp::{Engine, Generator};

use crate::object_model::{self as om, ShardEdit};
use crate::session::PatchInfo;
use crate::session_tests::{rig, Rig};

fn get(r: &Rig, id: &str) -> f32 {
    r.bank.get_by_id(id).unwrap()
}

fn default_of(id: &str) -> f32 {
    shard_dsp::params::PARAMS
        .iter()
        .find(|p| p.id == id)
        .unwrap()
        .default
}

fn listed(r: &Rig) -> Vec<PatchInfo> {
    r.session.patches()
}

fn names(r: &Rig) -> Vec<String> {
    listed(r).into_iter().map(|p| p.name).collect()
}

fn current(r: &Rig) -> PatchInfo {
    listed(r)
        .into_iter()
        .find(|p| p.current)
        .expect("a current patch")
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shard-patches-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("saved.shard")
}

#[test]
fn a_new_session_has_one_patch_current_and_played() {
    let r = rig();
    let all = listed(&r);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "Patch 1");
    assert!(all[0].current && all[0].played);
    assert_eq!(r.session.current_patch(), Some(all[0].id));
}

#[test]
fn new_patch_is_one_undo_step_and_undo_falls_back_to_the_tracks_patch() {
    let r = rig();
    r.session.set_param("grain.size", 80.0).unwrap();
    let first = current(&r).id;

    let second = r.session.new_patch().unwrap();
    assert_eq!(names(&r), ["Patch 1", "Patch 2"]);
    assert_eq!(current(&r).id, second, "the new patch is current");
    assert!(!current(&r).played, "the track still plays the first");
    assert_eq!(get(&r, "grain.size"), default_of("grain.size"));
    assert_eq!(r.session.history().0.as_deref(), Some("New Patch"));

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("New Patch"));
    assert_eq!(names(&r), ["Patch 1"]);
    assert_eq!(current(&r).id, first, "the current patch falls back");
    assert_eq!(get(&r, "grain.size"), 80.0, "and the engine plays it");
    assert_eq!(
        r.session.undo().unwrap().as_deref(),
        Some("Set Size"),
        "New Patch was a single step"
    );
    r.session.redo().unwrap();

    assert_eq!(r.session.redo().unwrap().as_deref(), Some("New Patch"));
    assert_eq!(names(&r), ["Patch 1", "Patch 2"], "redo brings it back");
}

#[test]
fn selecting_a_patch_adds_no_undo_step_no_unsaved_mark_and_no_tree_change() {
    let r = rig();
    let first = current(&r).id;
    r.session.new_patch().unwrap();
    let path = temp("select");
    r.session.save(&path).unwrap();
    let history = r.session.history_labels();
    let tree = r.session.read(|d| d.tree().serialise());

    r.session.select_patch(first).unwrap();
    assert_eq!(current(&r).id, first);
    assert!(!r.session.is_unsaved());
    assert_eq!(r.session.history_labels(), history);
    assert_eq!(r.session.read(|d| d.tree().serialise()), tree);
    assert!(r.session.select_patch(u64::MAX).is_err());
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn every_patch_command_acts_on_the_current_patch_only() {
    let r = rig();
    let first = current(&r).id;
    r.session.set_param("grain.size", 80.0).unwrap();
    let second = r.session.new_patch().unwrap();
    r.session.set_param("grain.size", 120.0).unwrap();
    r.session.fx_add("patch", "chorus").unwrap();
    r.session.add_lfo().unwrap();
    assert_eq!(get(&r, "grain.size"), 120.0);

    r.session.select_patch(first).unwrap();
    assert_eq!(get(&r, "grain.size"), 80.0);
    assert!(
        shard_dsp::fx::order_of(&r.bank).is_empty(),
        "no chorus here"
    );
    assert!(r.session.modulation().0.lfos.is_empty(), "nor the LFO");

    r.session.select_patch(second).unwrap();
    assert_eq!(get(&r, "grain.size"), 120.0);
    assert_eq!(shard_dsp::fx::order_of(&r.bank).len(), 1);
    assert_eq!(r.session.modulation().0.lfos.len(), 1);
}

#[test]
fn several_patches_save_and_open_with_their_order_and_the_tracks_patch() {
    let path = temp("round-trip");
    let a = rig();
    a.session.set_param("grain.size", 80.0).unwrap();
    a.session.new_patch().unwrap();
    a.session.set_param("grain.size", 120.0).unwrap();
    a.session.new_patch().unwrap();
    a.session.save(&path).unwrap();

    let b = rig();
    let report = b.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert_eq!(names(&b), ["Patch 1", "Patch 2", "Patch 3"]);
    let all = listed(&b);
    assert!(
        all[0].played && all[0].current,
        "opens on the track's patch"
    );
    assert_eq!(get(&b, "grain.size"), 80.0);
    b.session.select_patch(all[1].id).unwrap();
    assert_eq!(get(&b, "grain.size"), 120.0);
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn a_file_whose_track_names_no_patch_plays_the_first() {
    let path = temp("no-reference");
    let a = rig();
    a.session.set_param("grain.size", 80.0).unwrap();
    a.session.new_patch().unwrap();
    a.session
        .edit("Plays nothing", None, |tx| {
            let track = tx
                .at("/arrangements/arrangement")
                .and_then(|arr| arr.order(om::TRACKS).into_iter().next())
                .map(|t| t.id())
                .expect("a track");
            tx.clear_ref(track, om::TRACK_PATCH)
        })
        .unwrap();
    a.session.save(&path).unwrap();

    let b = rig();
    b.session.open(&path).unwrap();
    let all = listed(&b);
    assert!(all[0].played && all[0].current);
    assert_eq!(get(&b, "grain.size"), 80.0);
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn a_patch_with_no_label_shows_its_place() {
    let r = rig();
    r.session.new_patch().unwrap();
    r.session
        .edit("Unname", None, |tx| {
            let p = tx.at("/patches/patch").map(|p| p.id()).unwrap();
            tx.put(p, om::PATCH_LABEL, rhizome_core::Value::Text(String::new()))
        })
        .unwrap();
    assert_eq!(names(&r), ["Patch 1", "Patch 2"]);
    r.session.new_patch().unwrap();
    assert_eq!(names(&r)[2], "Patch 3");
}

/// The audio callback's hand-offs, as `lib.rs` makes them, then one block.
fn block(r: &Rig, e: &mut Engine, out: &mut [f32], retired: &mut Vec<shard_dsp::ModSet>) {
    if let Some(set) = r.mod_swap.lock().unwrap().take() {
        retired.push(e.set_modulation(set));
    }
    {
        let mut slots = r.swap.lock().unwrap();
        for g in Generator::ALL {
            if let Some(buf) = slots[g.index()].take() {
                e.swap_source(g, buf);
            }
        }
    }
    let steps = r.steps.load();
    e.set_steps(steps);
    e.set_arrangement(&r.arrangement, !steps.on);
    let (player, grain) = r.readings.load();
    e.set_readings(player, grain);
    e.process_block(out, &r.bank);
}

/// The largest sample-to-sample jump on the left channel over `blocks`,
/// switching to `switch_to` (cycling) every `every` blocks, after a settle.
fn worst_jump(r: &Rig, switch_to: &[u64], every: usize, blocks: usize) -> f32 {
    let mut e = Engine::new(48_000.0, 128);
    e.set_playing(true);
    let mut out = vec![0.0f32; 256];
    let mut retired = Vec::new();
    let (mut prev, mut worst) = (0.0f32, 0.0f32);
    let mut next = 0;
    for n in 0..blocks {
        if !switch_to.is_empty() && n > 0 && n % every == 0 {
            r.session
                .select_patch(switch_to[next % switch_to.len()])
                .unwrap();
            next += 1;
        }
        block(r, &mut e, &mut out, &mut retired);
        for s in out.iter().step_by(2) {
            assert!(s.is_finite());
            if n > 40 {
                worst = worst.max((s - prev).abs());
            }
            prev = *s;
        }
    }
    worst
}

#[test]
fn switching_the_current_patch_while_playing_does_not_click() {
    // Two patches as different as a switch can make them, each smooth on
    // its own so a cut would stand out: the plain drone, against a low FM
    // sine with the material off, a darker filter, another output level and
    // drive in its chain.
    let r = rig();
    let first = current(&r).id;
    let second = r.session.new_patch().unwrap();
    for (id, v) in [
        ("material.on", 0.0),
        ("fm.on", 1.0),
        ("fm.freq", 60.0),
        ("fm.index", 0.0),
        ("fm.feedback", 0.0),
        ("fm.drift", 0.0),
        ("fm.gain", 1.5),
        ("filter.on", 1.0),
        ("filter.cutoff", 900.0),
        ("amp.gain", 0.5),
    ] {
        r.session.set_param(id, v).unwrap();
    }
    let n = r.session.fx_add("patch", "drive").unwrap();
    r.session
        .set_param(&shard_dsp::fx::row_id(n, "drive.mix"), 0.6)
        .unwrap();

    // Each patch alone, steady: the yardstick is the larger of the two.
    r.session.select_patch(first).unwrap();
    let steady_first = worst_jump(&r, &[], 0, 400);
    r.session.select_patch(second).unwrap();
    let steady_second = worst_jump(&r, &[], 0, 400);
    let steady = steady_first.max(steady_second);
    assert!(
        steady > 1e-4,
        "both patches sound ({steady_first}, {steady_second})"
    );

    r.session.select_patch(first).unwrap();
    let switched = worst_jump(&r, &[second, first], 47, 600);
    assert!(
        switched < steady * 2.0,
        "switching jumped by {switched} against a steady {steady} \
         ({steady_first} and {steady_second})"
    );
}

#[test]
fn switching_rebuilds_modulation_only_when_it_differs() {
    // A ModSet is built on the command thread, and only for a patch whose
    // modulation is not the one the engine has.
    let r = rig();
    let first = current(&r).id;
    let second = r.session.new_patch().unwrap();
    r.mod_swap.lock().unwrap().take();
    r.session.select_patch(first).unwrap();
    assert!(
        r.mod_swap.lock().unwrap().take().is_none(),
        "the same empty modulation is not handed over again"
    );
    r.session.select_patch(second).unwrap();
    r.session.add_lfo().unwrap();
    r.mod_swap.lock().unwrap().take();
    r.session.select_patch(first).unwrap();
    assert!(r.mod_swap.lock().unwrap().take().is_some());
}

#[test]
fn a_new_patch_in_a_session_without_the_drone_is_silent() {
    let r = rig();
    let drone = r.session.pool().materials[0].id;
    r.session.remove_material(drone).unwrap();
    r.session.new_patch().unwrap();
    let wires = r.session.pool().wires;
    assert_eq!((wires.material, wires.grain), (None, None));
}

fn played(r: &Rig) -> PatchInfo {
    listed(r)
        .into_iter()
        .find(|p| p.played)
        .expect("a played patch")
}

#[test]
fn set_plays_is_one_undo_step_and_undo_restores_the_previous_patch() {
    let r = rig();
    let first = played(&r).id;
    let second = r.session.new_patch().unwrap();
    let before = r.session.history_labels().0.len();

    assert!(!r.session.set_track_patch(second).unwrap(), "sound scaping");
    assert_eq!(played(&r).id, second);
    assert_eq!(r.session.history().0.as_deref(), Some("Set Plays"));
    assert_eq!(r.session.history_labels().0.len(), before + 1);
    assert!(r.session.is_unsaved());

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Plays"));
    assert_eq!(played(&r).id, first, "undo restores the reference");
    assert_eq!(r.session.redo().unwrap().as_deref(), Some("Set Plays"));
    assert_eq!(played(&r).id, second);
}

#[test]
fn an_unchanged_or_refused_pick_adds_no_step() {
    let r = rig();
    let first = played(&r).id;
    let second = r.session.new_patch().unwrap();
    let history = r.session.history_labels();
    let tree = r.session.read(|d| d.tree().serialise());

    assert!(!r.session.set_track_patch(first).unwrap());
    assert_eq!(r.session.history_labels(), history, "already played");

    assert!(r.session.set_track_patch(u64::MAX).is_err());
    // A material is a node with a runtime id, and not a patch.
    let material = r.session.pool().materials[0].id;
    assert!(r.session.set_track_patch(material).is_err());
    assert_eq!(r.session.history_labels(), history);
    assert_eq!(r.session.read(|d| d.tree().serialise()), tree);
    assert_eq!(played(&r).id, first);
    assert_eq!(current(&r).id, second, "and the current patch stays");
}

#[test]
fn in_tracker_mode_the_engine_follows_what_the_track_plays() {
    let r = rig();
    r.session.set_param("grain.size", 80.0).unwrap();
    let first = played(&r).id;
    let second = r.session.new_patch().unwrap();
    r.session.set_param("grain.size", 120.0).unwrap();
    r.session.select_patch(first).unwrap();
    r.session.set_mode(true);
    assert_eq!(get(&r, "grain.size"), 80.0);

    assert!(
        r.session.set_track_patch(second).unwrap(),
        "the current patch moved"
    );
    assert_eq!(current(&r).id, second);
    assert_eq!(get(&r, "grain.size"), 120.0, "the engine plays the pick");
    assert_eq!(
        r.session
            .plan()
            .current
            .and_then(|n| r.session.plan().id_of(n)),
        Some(second)
    );

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Plays"));
    assert_eq!(current(&r).id, first);
    assert_eq!(get(&r, "grain.size"), 80.0, "undo returns the engine too");
    r.session.redo().unwrap();
    assert_eq!(current(&r).id, second);
    assert_eq!(get(&r, "grain.size"), 120.0);
}

#[test]
fn outside_tracker_mode_set_plays_leaves_what_is_heard() {
    let r = rig();
    r.session.set_param("grain.size", 80.0).unwrap();
    let first = played(&r).id;
    let second = r.session.new_patch().unwrap();
    r.session.select_patch(first).unwrap();

    r.session.set_track_patch(second).unwrap();
    assert_eq!(current(&r).id, first, "the patch being shaped stays");
    assert_eq!(get(&r, "grain.size"), 80.0);
    r.session.undo().unwrap();
    assert_eq!(current(&r).id, first);
}
