//! Several patches in a session (epic 10, story a): the current patch is the
//! session's and every patch-scoped command acts on it. Since epic 28 it is
//! never what the tracks play: a selected patch no track plays is auditioned,
//! and switching that audition doesn't click.

use shard_dsp::Engine;

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

/// The largest sample-to-sample jump on the left channel over `blocks`,
/// through the audio callback's own hand-offs, running `act` with the count
/// every `every` blocks, after a settle.
pub(crate) fn worst_jump(r: &Rig, every: usize, blocks: usize, mut act: impl FnMut(usize)) -> f32 {
    let mut e = Engine::new(48_000.0, 128);
    e.set_voice_active(0, false);
    e.set_playing(true);
    let mut out = vec![0.0f32; 256];
    let mut held = crate::Held::new();
    let (mut prev, mut worst) = (0.0f32, 0.0f32);
    let mut next = 0;
    for n in 0..blocks {
        if every > 0 && n > 0 && n % every == 0 {
            act(next);
            next += 1;
        }
        r.block(&mut e, &mut out, &mut held);
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

/// The first track's steps off its patch: its voice falls silent, so only
/// an audition is heard.
fn quiet_track(r: &Rig) {
    let mut t = r.session.tracker();
    t.tracks[0].steps.iter_mut().for_each(|s| s.on = false);
    r.session.set_tracker(t).unwrap();
}

#[test]
fn switching_the_auditioned_patch_while_playing_does_not_click() {
    // Two patches no track plays, as different as a switch can make them,
    // each smooth on its own so a cut would stand out: the plain drone,
    // against a low FM sine with the material off, a darker filter, another
    // output level and drive in its chain. The track plays a third, silent.
    let r = rig();
    quiet_track(&r);
    let first = r.session.new_patch().unwrap();
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
    let steady_first = worst_jump(&r, 0, 400, |_| {});
    r.session.select_patch(second).unwrap();
    let steady_second = worst_jump(&r, 0, 400, |_| {});
    let steady = steady_first.max(steady_second);
    assert!(
        steady > 1e-4,
        "both patches sound ({steady_first}, {steady_second})"
    );

    r.session.select_patch(first).unwrap();
    let switched = worst_jump(&r, 47, 600, |n| {
        r.session.select_patch([second, first][n % 2]).unwrap();
    });
    assert!(
        switched < steady * 2.0,
        "switching jumped by {switched} against a steady {steady} \
         ({steady_first} and {steady_second})"
    );
}

#[test]
fn switching_rebuilds_modulation_only_when_it_differs() {
    // A ModSet is built on the command thread, and only for a voice whose
    // modulation is not the one it has: here the audition voice.
    let audition = crate::session::AUDITION;
    let r = rig();
    let first = r.session.new_patch().unwrap();
    let second = r.session.new_patch().unwrap();
    r.voice(0).mod_swap.lock().unwrap().take();
    r.voice(audition).mod_swap.lock().unwrap().take();
    r.session.select_patch(first).unwrap();
    assert!(
        r.voice(audition).mod_swap.lock().unwrap().take().is_none(),
        "the same empty modulation is not handed over again"
    );
    r.session.select_patch(second).unwrap();
    r.session.add_lfo().unwrap();
    r.voice(audition).mod_swap.lock().unwrap().take();
    r.session.select_patch(first).unwrap();
    assert!(r.voice(audition).mod_swap.lock().unwrap().take().is_some());
    assert!(
        r.voice(0).mod_swap.lock().unwrap().take().is_none(),
        "the track's voice is left alone"
    );
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

    assert!(
        !r.session.set_track_patch(0, second).unwrap(),
        "a patch selected"
    );
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

    assert!(!r.session.set_track_patch(0, first).unwrap());
    assert_eq!(r.session.history_labels(), history, "already played");

    assert!(r.session.set_track_patch(0, u64::MAX).is_err());
    // A material is a node with a runtime id, and not a patch.
    let material = r.session.pool().materials[0].id;
    assert!(r.session.set_track_patch(0, material).is_err());
    assert_eq!(r.session.history_labels(), history);
    assert_eq!(r.session.read(|d| d.tree().serialise()), tree);
    assert_eq!(played(&r).id, first);
    assert_eq!(current(&r).id, second, "and the current patch stays");
}

#[test]
fn with_its_track_selected_the_current_patch_follows_what_it_plays() {
    let r = rig();
    r.session.set_param("grain.size", 80.0).unwrap();
    let first = played(&r).id;
    let second = r.session.new_patch().unwrap();
    r.session.set_param("grain.size", 120.0).unwrap();
    r.session.select_track(0).unwrap();
    assert_eq!(current(&r).id, first);
    assert_eq!(get(&r, "grain.size"), 80.0);

    assert!(
        r.session.set_track_patch(0, second).unwrap(),
        "the current patch moved"
    );
    assert_eq!(current(&r).id, second);
    assert_eq!(
        get(&r, "grain.size"),
        120.0,
        "the patch view shows the pick"
    );
    assert_eq!(
        r.voice(0).bank.get_by_id("grain.size"),
        Some(120.0),
        "and the track's voice plays it"
    );

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Plays"));
    assert_eq!(current(&r).id, first);
    assert_eq!(get(&r, "grain.size"), 80.0, "undo returns the view too");
    assert_eq!(r.voice(0).bank.get_by_id("grain.size"), Some(80.0));
    r.session.redo().unwrap();
    assert_eq!(current(&r).id, second);
    assert_eq!(get(&r, "grain.size"), 120.0);
}

#[test]
fn with_a_patch_selected_set_plays_leaves_the_patch_being_shaped() {
    let r = rig();
    r.session.set_param("grain.size", 80.0).unwrap();
    let first = played(&r).id;
    let second = r.session.new_patch().unwrap();
    r.session.select_patch(first).unwrap();

    r.session.set_track_patch(0, second).unwrap();
    assert_eq!(current(&r).id, first, "the patch being shaped stays");
    assert_eq!(get(&r, "grain.size"), 80.0);
    r.session.undo().unwrap();
    assert_eq!(current(&r).id, first);
}

// Story d: Rename, Delete and Duplicate.

fn undo_label(r: &Rig) -> Option<String> {
    r.session.history().0
}

/// A patch with something in every part a copy must carry: a value, an
/// effect, an LFO and an envelope, a link onto each (one from a fixed node,
/// one from the effect), and a controller map.
fn dressed(r: &Rig) -> u64 {
    r.session.set_param("grain.size", 80.0).unwrap();
    let n = r.session.fx_add("patch", "chorus").unwrap();
    r.session
        .set_param(&format!("fx.{n}.chorus.mix"), 0.4)
        .unwrap();
    r.session.add_lfo().unwrap();
    r.session.add_envelope().unwrap();
    let m = r.session.modulation().0;
    r.session
        .link_param("grain.size", m.lfos[0].id, 0.25, 0.75)
        .unwrap();
    r.session
        .link_param(&format!("fx.{n}.chorus.mix"), m.envelopes[0].id, 0.0, 0.5)
        .unwrap();
    r.session
        .set_controller_map(Some(&crate::mapping::MapRef {
            id: 7,
            name: "LPD8".into(),
        }))
        .unwrap();
    current(r).id
}

/// What a patch sounds like, ids and all, for comparing two patches: its
/// values, its modulation with the modulator ids taken out (each patch has
/// its own), and its controller map.
fn sound(r: &Rig) -> (Vec<f32>, String, Option<String>) {
    let plan = r.session.plan();
    let mut m = plan.modulation.clone();
    let ids: Vec<u64> = m
        .lfos
        .iter()
        .map(|l| l.id)
        .chain(m.envelopes.iter().map(|e| e.id))
        .collect();
    for l in &mut m.lfos {
        l.id = 0;
    }
    for e in &mut m.envelopes {
        e.id = 0;
    }
    for link in m.links.values_mut() {
        // Which of the patch's own modulators, by place.
        link.source = ids.iter().position(|i| *i == link.source).unwrap() as u64;
    }
    (
        plan.patch,
        serde_json::to_string(&m).unwrap(),
        r.session.controller_map().map(|m| m.name),
    )
}

#[test]
fn rename_is_one_step_named_for_the_old_name_and_undo_restores_it() {
    let r = rig();
    let id = current(&r).id;
    assert!(r.session.rename_patch(id, "  Bass  ").unwrap());
    assert_eq!(names(&r), ["Bass"], "trimmed");
    assert_eq!(undo_label(&r).as_deref(), Some("Rename Patch 1"));
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Rename Patch 1"));
    assert_eq!(names(&r), ["Patch 1"]);
    assert_eq!(undo_label(&r), None, "it was one step");
}

#[test]
fn an_empty_or_unchanged_name_makes_no_step() {
    let r = rig();
    let id = current(&r).id;
    let before = r.session.history_labels();
    assert!(!r.session.rename_patch(id, "").unwrap());
    assert!(!r.session.rename_patch(id, "   ").unwrap());
    assert!(!r.session.rename_patch(id, "Patch 1").unwrap());
    assert!(!r.session.rename_patch(id, " Patch 1 ").unwrap());
    assert_eq!(r.session.history_labels(), before);
    assert_eq!(names(&r), ["Patch 1"]);
    assert!(r.session.rename_patch(u64::MAX, "x").is_err());
}

#[test]
fn delete_is_one_step_and_undo_brings_the_patch_back_whole_in_its_place() {
    let r = rig();
    let first = current(&r).id;
    r.session.new_patch().unwrap();
    let doomed = dressed(&r);
    let kept = sound(&r);
    r.session.new_patch().unwrap();
    r.session.select_patch(doomed).unwrap();
    assert_eq!(names(&r), ["Patch 1", "Patch 2", "Patch 3"]);

    assert!(r.session.delete_patch(doomed).unwrap(), "it was current");
    assert_eq!(names(&r), ["Patch 1", "Patch 3"]);
    assert_eq!(undo_label(&r).as_deref(), Some("Delete Patch 2"));
    assert_eq!(current(&r).id, first, "the track's patch takes over");
    assert_eq!(get(&r, "grain.size"), default_of("grain.size"));
    assert!(shard_dsp::fx::order_of(&r.bank).is_empty());

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Delete Patch 2"));
    assert_eq!(names(&r), ["Patch 1", "Patch 2", "Patch 3"], "in its place");
    let back = listed(&r)[1].id;
    r.session.select_patch(back).unwrap();
    assert_eq!(sound(&r), kept, "with its chain, modulators and links");
    assert_eq!(get(&r, "grain.size"), 80.0);
    assert_eq!(undo_label(&r).as_deref(), Some("New Patch"), "one step");

    assert_eq!(r.session.redo().unwrap().as_deref(), Some("Delete Patch 2"));
    assert_eq!(names(&r), ["Patch 1", "Patch 3"], "redo deletes it again");
    assert_eq!(current(&r).id, first);
}

#[test]
fn deleting_a_patch_that_is_not_current_leaves_the_current_one() {
    let r = rig();
    let second = r.session.new_patch().unwrap();
    let third = r.session.new_patch().unwrap();
    r.session.set_param("grain.size", 120.0).unwrap();
    assert!(!r.session.delete_patch(second).unwrap());
    assert_eq!(current(&r).id, third);
    assert_eq!(get(&r, "grain.size"), 120.0);
}

#[test]
fn the_last_patch_and_the_played_patch_cannot_be_deleted() {
    let r = rig();
    let first = current(&r).id;
    assert!(!listed(&r)[0].deletable);
    assert!(r.session.delete_patch(first).is_err(), "the last patch");
    let second = r.session.new_patch().unwrap();
    let before = r.session.history_labels();
    assert!(r.session.delete_patch(first).is_err(), "the track plays it");
    assert_eq!(r.session.history_labels(), before, "and no step");
    let all = listed(&r);
    assert!(!all[0].deletable && all[1].deletable);

    r.session.set_track_patch(0, second).unwrap();
    let all = listed(&r);
    assert!(all[0].deletable && !all[1].deletable);
    assert!(r.session.delete_patch(second).is_err());
    r.session.delete_patch(first).unwrap();
    assert_eq!(names(&r), ["Patch 2"]);
}

#[test]
fn duplicate_is_one_step_a_copy_after_the_original_and_becomes_current() {
    let r = rig();
    let original = dressed(&r);
    let theirs = sound(&r);
    r.session.new_patch().unwrap();
    r.session.select_patch(original).unwrap();

    let copy = r.session.duplicate_patch(original).unwrap();
    assert_eq!(names(&r), ["Patch 1", "Patch 1 copy", "Patch 2"]);
    assert_eq!(current(&r).id, copy, "the copy is current");
    assert_eq!(undo_label(&r).as_deref(), Some("Duplicate Patch 1"));
    assert_eq!(sound(&r), theirs, "the same sound");

    // The copy's links follow its own modulators, never the original's.
    let plan = r.session.plan();
    let own: Vec<u64> = plan
        .modulation
        .lfos
        .iter()
        .map(|l| l.id)
        .chain(plan.modulation.envelopes.iter().map(|e| e.id))
        .collect();
    r.session.select_patch(original).unwrap();
    let orig = r.session.plan().modulation;
    for link in plan.modulation.links.values() {
        assert!(own.contains(&link.source), "{link:?} is not the copy's");
    }
    assert!(orig.lfos.iter().all(|l| !own.contains(&l.id)));
    assert_eq!(plan.modulation.links.len(), 2);

    assert_ne!(r.session.duplicate_patch(original).unwrap(), copy);
    assert_eq!(names(&r)[1], "Patch 1 copy 2", "a second copy is numbered");
    r.session.undo().unwrap();

    assert_eq!(
        r.session.undo().unwrap().as_deref(),
        Some("Duplicate Patch 1")
    );
    assert_eq!(names(&r), ["Patch 1", "Patch 2"]);
    assert_eq!(
        r.session.redo().unwrap().as_deref(),
        Some("Duplicate Patch 1")
    );
    assert_eq!(names(&r), ["Patch 1", "Patch 1 copy", "Patch 2"], "redo");
}

#[test]
fn a_patch_whose_material_was_removed_duplicates_reading_nothing() {
    let r = rig();
    let original = current(&r).id;
    let drone = r.session.pool().materials[0].id;
    r.session.remove_material(drone).unwrap();
    let wires = r.session.pool().wires;
    assert_eq!((wires.material, wires.grain), (None, None));

    r.session.duplicate_patch(original).unwrap();
    assert_eq!(names(&r), ["Patch 1", "Patch 1 copy"]);
    let wires = r.session.pool().wires;
    assert_eq!(
        (wires.material, wires.grain),
        (None, None),
        "the same nothing"
    );
}

#[test]
fn a_copy_reads_the_same_materials() {
    let r = rig();
    let original = current(&r).id;
    r.session.add_generator("grain").unwrap();
    let before = r.session.pool().wires;
    assert!(before.material.is_some() && before.grain.is_some());
    r.session.duplicate_patch(original).unwrap();
    assert_eq!(r.session.pool().wires, before);
    assert_eq!(r.session.pool().materials.len(), 1, "no material copied");
}

#[test]
fn editing_the_copy_leaves_the_original_alone() {
    let r = rig();
    let original = dressed(&r);
    let theirs = sound(&r);
    let copy = r.session.duplicate_patch(original).unwrap();

    r.session.set_param("grain.size", 200.0).unwrap();
    r.session.fx_add("patch", "delay").unwrap();
    let lfo = r.session.modulation().0.lfos[0].clone();
    r.session
        .set_lfo(crate::modulation::LfoRecord {
            rate: 3.0,
            ..lfo.clone()
        })
        .unwrap();
    r.session.unlink_param("grain.size").unwrap();
    r.session
        .remove_envelope(r.session.modulation().0.envelopes[0].id)
        .unwrap();
    r.session.set_controller_map(None).unwrap();
    assert_ne!(sound(&r), theirs);

    r.session.select_patch(original).unwrap();
    assert_eq!(sound(&r), theirs, "the original is as it was");
    assert_eq!(get(&r, "grain.size"), 80.0);
    r.session.select_patch(copy).unwrap();
    assert_eq!(get(&r, "grain.size"), 200.0);
}

#[test]
fn both_patches_save_and_open_as_they_were() {
    let path = temp("duplicate");
    let a = rig();
    let original = dressed(&a);
    let theirs = sound(&a);
    a.session.new_patch().unwrap();
    a.session.duplicate_patch(original).unwrap();
    a.session.set_param("grain.size", 150.0).unwrap();
    let copied = sound(&a);
    a.session.save(&path).unwrap();

    let b = rig();
    let report = b.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    assert_eq!(names(&b), ["Patch 1", "Patch 1 copy", "Patch 2"]);
    let all = listed(&b);
    b.session.select_patch(all[0].id).unwrap();
    assert_eq!(sound(&b), theirs);
    b.session.select_patch(all[1].id).unwrap();
    assert_eq!(sound(&b), copied);
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn a_copy_of_a_patch_missing_from_the_order_still_sits_after_it() {
    let r = rig();
    let first = current(&r).id;
    r.session.new_patch().unwrap();
    // A file from before patches had an order.
    r.session
        .edit("Forget the order", None, |tx| {
            tx.set_order(
                format!("/{}", om::PATCHES).as_str(),
                om::PATCH_ORDER,
                Vec::<rhizome_core::NodeId>::new(),
            )
        })
        .unwrap();
    r.session.duplicate_patch(first).unwrap();
    assert_eq!(names(&r), ["Patch 1", "Patch 1 copy", "Patch 2"]);
}

#[test]
fn a_pick_and_a_selection_in_either_order_play_the_same() {
    // The track row's picker and the row's selection can reach Rust in
    // either order; what the track plays is the pick either way, and the
    // patch being shaped is whatever was selected last.
    for pick_first in [true, false] {
        let r = rig();
        r.session.set_param("fm.on", 1.0).unwrap();
        r.session.set_param("material.on", 0.0).unwrap();
        let first = r.session.patches()[0].id;
        let second = r.session.new_patch().unwrap();
        r.session.select_patch(first).unwrap();
        if pick_first {
            r.session.set_track_patch(0, second).unwrap();
            r.session.select_track(0).unwrap();
        } else {
            r.session.select_track(0).unwrap();
            r.session.set_track_patch(0, second).unwrap();
        }
        assert_eq!(current(&r).id, second, "pick first: {pick_first}");
        assert_eq!(r.voice(0).bank.get_by_id("fm.on"), Some(0.0));
        assert_eq!(r.voice(0).bank.get_by_id("material.on"), Some(1.0));
        assert_eq!(undo_label(&r).as_deref(), Some("Set Plays"));
    }
}
