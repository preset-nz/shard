//! Global LFOs (roadmap row 43): LFOs beside the patches that every patch
//! linking one follows, as the session holds them and hands them to the
//! engine. One shared LFO, edited once; never a copy per voice.

use std::path::Path;

use shard_dsp::{Engine, ParamBank};

use crate::modulation::{LinkRecord, Modulation};
use crate::object_model::{self as om, ShardEdit};
use crate::session_tests::{rig, Rig};

/// The modulation patch `id` (a runtime id) compiles to.
fn modulation_of(r: &Rig, id: u64) -> Modulation {
    let plan = r.session.plan();
    let node = plan.node_of(id).expect("a patch");
    plan.patch_plan(node).expect("compiled").modulation.clone()
}

fn link_of(r: &Rig, patch: u64, param: &str) -> Option<LinkRecord> {
    modulation_of(r, patch).links.get(param).copied()
}

fn only_global(r: &Rig) -> u64 {
    let globals = r.session.globals();
    assert_eq!(globals.len(), 1, "{globals:?}");
    globals[0].id
}

#[test]
fn a_global_lfo_adds_and_removes_in_one_step_each_and_is_not_a_patchs() {
    let r = rig();
    r.session.add_global_lfo().unwrap();
    let globals = r.session.globals();
    assert_eq!(globals.len(), 1);
    assert_eq!(globals[0].name, "Global LFO 1");
    assert!(r.session.modulation().0.lfos.is_empty(), "not the patch's");

    // A patch's own LFO counts only the patch's: it is still "LFO 1".
    r.session.add_lfo().unwrap();
    assert_eq!(r.session.modulation().0.lfos[0].name, "LFO 1");
    r.session.add_global_lfo().unwrap();
    assert_eq!(r.session.globals()[1].name, "Global LFO 2");

    let first = r.session.globals()[0].id;
    r.session.remove_lfo(first).unwrap();
    assert_eq!(r.session.globals().len(), 1);
    assert_eq!(
        r.session.undo().unwrap().as_deref(),
        Some("Remove global LFO")
    );
    assert_eq!(r.session.globals()[0].name, "Global LFO 1", "back, first");
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add global LFO"));
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add LFO"));
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add global LFO"));
    assert!(r.session.globals().is_empty());
}

#[test]
fn editing_a_global_lfo_hands_the_engine_one_new_set_and_no_patch_set() {
    let r = rig();
    r.session.add_global_lfo().unwrap();
    let mut lfo = r.session.globals()[0].clone();
    drop(r.voices.globals_swap.lock().unwrap().take());
    drop(r.voice(0).mod_swap.lock().unwrap().take());
    lfo.rate = 3.5;
    r.session.set_lfo(lfo).unwrap();
    assert_eq!(r.session.globals()[0].rate, 3.5);
    assert!(r.voices.globals_swap.lock().unwrap().take().is_some());
    assert!(
        r.voice(0).mod_swap.lock().unwrap().take().is_none(),
        "no patch changed"
    );
    assert_eq!(r.session.history().0.as_deref(), Some("Change global LFO"));
}

#[test]
fn a_link_to_a_new_global_lfo_is_one_undo_step() {
    let r = rig();
    let patch = r.session.current_patch().unwrap();
    r.session
        .link_new("grain.size", "global-lfo", 0.25, 0.75)
        .unwrap();
    let g = only_global(&r);
    assert_eq!(r.session.globals()[0].name, "Global LFO 1");
    assert_eq!(link_of(&r, patch, "grain.size").unwrap().source, g);
    assert!(r.session.modulation().0.lfos.is_empty());
    assert!(r.session.modulation().1.is_empty(), "nothing refused");
    assert_eq!(
        r.session.undo().unwrap().as_deref(),
        Some("Link to a new global LFO")
    );
    assert!(r.session.globals().is_empty());
    assert!(r.session.modulation().0.links.is_empty());
    assert!(r
        .session
        .link_new("arrangement.filter.cutoff", "global-lfo", 0.0, 1.0)
        .is_err());
    assert!(r.session.globals().is_empty(), "a refusal adds nothing");
}

/// Patch A (the first track's) and a new patch B, both linking global G.
fn two_patches_on_one_global(r: &Rig) -> (u64, u64, u64) {
    let a = r.session.current_patch().unwrap();
    r.session
        .link_new("grain.size", "global-lfo", 0.1, 0.9)
        .unwrap();
    let g = only_global(r);
    let b = r.session.new_patch().unwrap();
    assert_eq!(r.session.current_patch(), Some(b));
    r.session.link_param("fm.index", g, 0.0, 0.5).unwrap();
    (a, b, g)
}

#[test]
fn removing_a_global_drops_its_links_in_every_patch_and_undo_brings_them_back() {
    let r = rig();
    let (a, b, g) = two_patches_on_one_global(&r);
    assert_eq!(link_of(&r, a, "grain.size").unwrap().source, g);
    assert_eq!(link_of(&r, b, "fm.index").unwrap().source, g);

    r.session.remove_lfo(g).unwrap();
    assert!(r.session.globals().is_empty());
    assert!(link_of(&r, a, "grain.size").is_none());
    assert!(link_of(&r, b, "fm.index").is_none());

    assert_eq!(
        r.session.undo().unwrap().as_deref(),
        Some("Remove global LFO")
    );
    // Back as it was. A node that returns gets a new runtime id.
    let g = only_global(&r);
    assert_eq!(link_of(&r, a, "grain.size").unwrap().source, g);
    let back = link_of(&r, b, "fm.index").unwrap();
    assert_eq!((back.source, back.lo, back.hi), (g, 0.0, 0.5));
}

#[test]
fn a_patchs_own_lfo_takes_its_links_with_it_too() {
    // The rule removing a global follows: rhizome unbinds what followed a
    // removed node, in the same step.
    let r = rig();
    r.session.link_new("grain.size", "lfo", 0.0, 1.0).unwrap();
    let lfo = r.session.modulation().0.lfos[0].id;
    r.session.remove_lfo(lfo).unwrap();
    assert!(r.session.modulation().0.links.is_empty());
    r.session.undo().unwrap();
    let m = r.session.modulation().0;
    assert_eq!(m.links["grain.size"].source, m.lfos[0].id);
}

#[test]
fn duplicating_a_patch_keeps_its_links_on_the_same_global() {
    let r = rig();
    let (_, b, g) = two_patches_on_one_global(&r);
    let copy = r.session.duplicate_patch(b).unwrap();
    assert_eq!(link_of(&r, copy, "fm.index").unwrap().source, g);
    assert_eq!(only_global(&r), g, "no copy of the global");
}

#[test]
fn only_a_patch_follows_a_global_and_only_an_lfo_is_global() {
    let r = rig();
    r.session.add_global_lfo().unwrap();
    let (arr_filter, global) = r.session.read(|d| {
        let t = d.tree();
        let arr = om::arrangement_node(t).unwrap();
        (
            arr.child("filter").unwrap().id(),
            om::global_lfos(t)[0].id(),
        )
    });
    // No mod envelope in the globals: the kind lives in patches only.
    let refused = r.session.edit("x", None, |tx| {
        tx.add(format!("/{}", om::GLOBALS).as_str(), om::MOD_ENV, "mod-env")
    });
    assert!(refused.is_err(), "a global envelope was let in");
    // The arrangement's rows follow nothing yet, a global included.
    let refused = r.session.edit("x", None, |tx| {
        tx.link(arr_filter, "filter.cutoff", global, 0.0, 1.0)
    });
    assert!(refused.is_err(), "the arrangement followed a global");
    assert!(r.session.undo().unwrap().as_deref() == Some("Add global LFO"));

    // A link from patch A to patch B's own LFO is still refused.
    let a = r.session.current_patch().unwrap();
    r.session.new_patch().unwrap();
    r.session.add_lfo().unwrap();
    let b_lfo = r.session.modulation().0.lfos[0].id;
    r.session.select_patch(a).unwrap();
    assert!(r.session.link_param("grain.size", b_lfo, 0.0, 1.0).is_err());
}

#[test]
fn a_preset_follows_a_global_in_any_patch_and_reports_one_that_has_gone() {
    let r = rig();
    let a = r.session.current_patch().unwrap();
    r.session
        .link_new("grain.size", "global-lfo", 0.2, 0.6)
        .unwrap();
    let g = only_global(&r);
    r.session.save_preset("grain", "Wander").unwrap();

    // Applied in another patch, the link comes along: same global, same ends.
    let b = r.session.new_patch().unwrap();
    let report = r.session.apply_preset("grain", "Wander").unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    let l = link_of(&r, b, "grain.size").unwrap();
    assert_eq!((l.source, l.lo, l.hi), (g, 0.2, 0.6));

    // The global gone, the preset applies its values and reports the link
    // where it reports a patch LFO that has gone.
    r.session.remove_lfo(g).unwrap();
    let report = r.session.apply_preset("grain", "Wander").unwrap();
    assert!(
        report.unknown.iter().any(|u| u.contains("grain.size")),
        "{:?}",
        report.unknown
    );
    assert!(link_of(&r, b, "grain.size").is_none());

    // The same for a patch's own LFO, removed.
    r.session.select_patch(a).unwrap();
    r.session.link_new("grain.size", "lfo", 0.2, 0.6).unwrap();
    let lfo = r.session.modulation().0.lfos[0].id;
    r.session.save_preset("grain", "Own").unwrap();
    r.session.remove_lfo(lfo).unwrap();
    let report = r.session.apply_preset("grain", "Own").unwrap();
    assert!(
        report.unknown.iter().any(|u| u.contains("grain.size")),
        "{:?}",
        report.unknown
    );
}

#[test]
fn globals_and_their_links_save_and_open_clean() {
    let dir = std::env::temp_dir().join(format!("shard-globals-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("globals.shard");
    let r = rig();
    two_patches_on_one_global(&r);
    let mut lfo = r.session.globals()[0].clone();
    lfo.rate = 0.7;
    lfo.shape = "triangle".into();
    r.session.set_lfo(lfo.clone()).unwrap();
    r.session.save(&path).unwrap();

    let o = rig();
    let report = o.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    assert!(!o.session.is_unsaved());
    let globals = o.session.globals();
    assert_eq!(globals.len(), 1);
    let g2 = globals[0].id;
    assert_eq!(
        (
            globals[0].name.as_str(),
            globals[0].rate,
            globals[0].shape.as_str()
        ),
        (lfo.name.as_str(), 0.7, "triangle")
    );
    let patches = o.session.patches();
    assert_eq!(patches.len(), 2);
    let (a2, b2) = (patches[0].id, patches[1].id);
    assert_eq!(link_of(&o, a2, "grain.size").unwrap().source, g2);
    assert_eq!(link_of(&o, b2, "fm.index").unwrap().source, g2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn files_from_before_globals_open_unchanged_with_none() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("tests/fixtures/trip-hop-acid.shard")];
    for e in std::fs::read_dir(root.join("../evals/agent/references")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == om::EXTENSION) {
            files.push(p);
        }
    }
    assert!(files.len() >= 3, "{files:?}");
    let dir = std::env::temp_dir().join(format!("shard-old-globals-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (n, path) in files.iter().enumerate() {
        let before = std::fs::read(path).unwrap();
        let r = rig();
        let report = r.session.open(path).unwrap();
        assert!(report.unknown.is_empty(), "{path:?}: {:?}", report.unknown);
        assert!(report.refused.is_empty(), "{path:?}: {:?}", report.refused);
        assert!(!r.session.is_unsaved(), "{path:?} opened unsaved");
        assert!(r.session.globals().is_empty());
        assert_eq!(std::fs::read(path).unwrap(), before, "{path:?} changed");

        // On the opened file, not a fresh session (the ladder, d5d187f): a
        // row links a new global, and both survive a save and a reopen.
        r.session
            .link_new("grain.size", "global-lfo", 0.1, 0.9)
            .unwrap();
        let patch = r.session.current_patch().unwrap();
        let place = r
            .session
            .patches()
            .iter()
            .position(|p| p.id == patch)
            .unwrap();
        let saved = dir.join(format!("{n}.shard"));
        r.session.save(&saved).unwrap();
        let again = rig();
        let report = again.session.open(&saved).unwrap();
        assert!(report.unknown.is_empty(), "{path:?}: {:?}", report.unknown);
        assert!(report.refused.is_empty(), "{path:?}: {:?}", report.refused);
        let g = only_global(&again);
        let patch = again.session.patches()[place].id;
        assert_eq!(link_of(&again, patch, "grain.size").unwrap().source, g);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn two_tracks_on_two_patches_read_one_global_alike_every_block() {
    // Through the callback's own hand-offs: track 1 on patch A, a second
    // melody track on patch B, both following global G at the same ends.
    // Each voice's heard value is the same in every block, through patch A's
    // set being rebuilt and G's rate being edited mid-run.
    let r = rig();
    let a = r.session.current_patch().unwrap();
    r.session
        .link_new("grain.size", "global-lfo", 0.1, 0.9)
        .unwrap();
    let g = only_global(&r);
    let b = r.session.new_patch().unwrap();
    r.session.link_param("grain.size", g, 0.1, 0.9).unwrap();
    // B's own wobble elsewhere, so its set is not A's.
    r.session
        .link_new("grain.density", "lfo", 0.0, 1.0)
        .unwrap();
    r.session.set_param("grain.size", 300.0).unwrap();
    r.session.add_track(false).unwrap();
    let second = r.session.track_count() - 1;
    r.session.set_track_patch(second, b).unwrap();
    let slots = r.session.slots();
    let voice = |track: usize| match slots[track] {
        crate::tracker::Slot::Melody(k) => k,
        _ => panic!("track {track} is a melody track"),
    };
    let (va, vb) = (voice(0), voice(second));
    assert_ne!(va, vb);

    let mut e = Engine::new(48_000.0, 128);
    e.set_playing(true);
    let mut out = vec![0.0f32; 512];
    let mut held = crate::Held::new();
    let heard = [ParamBank::new(), ParamBank::new()];
    let size = shard_dsp::params::index_of("grain.size").unwrap();
    let mut seen = Vec::new();
    for block in 0..600 {
        if block == 200 {
            // Patch A edited: a new LFO in it rebuilds its set.
            r.session.select_patch(a).unwrap();
            r.session.add_lfo().unwrap();
        }
        if block == 400 {
            let mut lfo = r.session.globals()[0].clone();
            lfo.rate = 4.0;
            r.session.set_lfo(lfo).unwrap();
        }
        r.block(&mut e, &mut out, &mut held);
        for (h, v) in heard.iter().zip([va, vb]) {
            e.publish_voice_heard(v, &r.voices.voice[v].bank, h);
        }
        let (x, y) = (heard[0].get(size), heard[1].get(size));
        assert_eq!(x, y, "block {block}: {x} and {y}");
        seen.push(x);
    }
    assert!(e.global_value(g).is_some(), "the engine has the global");
    let (lo, hi) = seen
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
    assert!(hi > lo * 1.2, "the global barely moved: {lo}..{hi}");
}
