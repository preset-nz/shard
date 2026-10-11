//! The session's contracts, with no sound card: every write goes through the
//! tree, one undo step per gesture, and the engine's inputs follow.
//!
//! The undo tests here were `lib.rs`'s, against Shard's own history; they now
//! hold rhizome's to the same behaviour.

use std::sync::{Arc, Mutex};

use shard_dsp::arrangement;
use shard_dsp::fx;
use shard_dsp::steps::StepBank;
use shard_dsp::{ParamBank, ReadingBank};

use crate::session::{Feeds, Session};
use crate::source;
use crate::tracker::{self, Tracker};

/// A session with no sound card, and the engine inputs it feeds.
pub(crate) struct Rig {
    pub(crate) session: Session,
    pub(crate) bank: Arc<ParamBank>,
    pub(crate) arrangement: Arc<ParamBank>,
    pub(crate) steps: Arc<StepBank>,
    pub(crate) mod_swap: Arc<Mutex<Option<shard_dsp::ModSet>>>,
    pub(crate) swap: Arc<Mutex<crate::session::SourceSlots>>,
    pub(crate) readings: Arc<ReadingBank>,
}

pub(crate) fn rig() -> Rig {
    let sample_rate = 48_000.0;
    let bank = Arc::new(ParamBank::new());
    let arrangement = Arc::new(ParamBank::for_table(arrangement::params()));
    let steps = Arc::new(StepBank::new(Tracker::default().params()));
    let mod_swap = Arc::new(Mutex::new(None));
    let swap = Arc::new(Mutex::new([None, None]));
    let readings = Arc::new(ReadingBank::new());
    let feeds = Feeds {
        bank: Arc::clone(&bank),
        arrangement: Arc::clone(&arrangement),
        steps: Arc::clone(&steps),
        mod_swap: Arc::clone(&mod_swap),
        swap: Arc::clone(&swap),
        readings: Arc::clone(&readings),
    };
    let drone = Arc::new(source::startup_drone(sample_rate));
    let session = Session::new(feeds, drone, sample_rate).expect("a session starts");
    Rig {
        session,
        bank,
        arrangement,
        steps,
        mod_swap,
        swap,
        readings,
    }
}

fn get(r: &Rig, id: &str) -> f32 {
    r.bank.get_by_id(id).unwrap()
}

#[test]
fn undo_puts_a_parameter_back_and_redo_changes_it_again() {
    let r = rig();
    let was = get(&r, "grain.size");
    r.session.set_param("grain.size", 123.0).unwrap();
    assert_eq!(get(&r, "grain.size"), 123.0);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Size"));
    assert_eq!(get(&r, "grain.size"), was);
    assert_eq!(r.session.redo().unwrap().as_deref(), Some("Set Size"));
    assert_eq!(get(&r, "grain.size"), 123.0);
    assert!(r.session.redo().unwrap().is_none());
}

#[test]
fn dragging_a_slider_is_one_undo() {
    let r = rig();
    let was = get(&r, "grain.size");
    for v in 50..150 {
        r.session.set_param("grain.size", v as f32).unwrap();
    }
    assert_eq!(
        r.session.history().0.as_deref(),
        Some("Set Size"),
        "Undo reads “Undo Set Size”"
    );
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Size"));
    assert_eq!(get(&r, "grain.size"), was);
    assert!(
        r.session.undo().unwrap().is_none(),
        "the drag was a single step"
    );
}

#[test]
fn setting_a_value_to_what_it_already_is_is_not_an_edit() {
    let r = rig();
    let now = get(&r, "grain.size");
    r.session.set_param("grain.size", now).unwrap();
    assert!(r.session.undo().unwrap().is_none());
    assert!(!r.session.read(|d| d.is_unsaved()));
}

#[test]
fn a_value_out_of_range_is_brought_into_it_as_the_bank_did() {
    let r = rig();
    r.session.set_param("grain.size", 1.0e9).unwrap();
    let max = shard_dsp::params::PARAMS
        .iter()
        .find(|p| p.id == "grain.size")
        .unwrap()
        .max;
    assert_eq!(get(&r, "grain.size"), max);
}

#[test]
fn held_gestures_are_not_edits_and_reach_the_bank() {
    let r = rig();
    r.session.set_param("tape.brake", 1.0).unwrap();
    r.session.set_param("tape.reverse", 1.0).unwrap();
    assert_eq!(get(&r, "tape.brake"), 1.0);
    assert!(r.session.undo().unwrap().is_none());

    // An edit while the brake is held leaves it held.
    r.session.set_param("grain.size", 80.0).unwrap();
    assert_eq!(get(&r, "tape.brake"), 1.0);
    assert_eq!(get(&r, "tape.reverse"), 1.0);
}

#[test]
fn adding_and_removing_an_effect_undo_cleanly_and_keep_its_settings() {
    let r = rig();
    let n = r.session.fx_add("patch", "delay").unwrap();
    let time = fx::row_id(n, "delay.time");
    r.session.set_param(&time, 777.0).unwrap();
    r.session.fx_remove("patch", n).unwrap();
    assert!(fx::order_of(&r.bank).is_empty());

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Remove Delay"));
    assert!(fx::order_of(&r.bank).contains(n), "back in the chain");
    assert_eq!(get(&r, &time), 777.0);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Time"));
    assert_ne!(get(&r, &time), 777.0);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add Delay"));
    assert!(fx::order_of(&r.bank).is_empty());
}

#[test]
fn moving_an_effect_reorders_the_chain_the_engine_runs() {
    let r = rig();
    let drive = r.session.fx_add("patch", "drive").unwrap();
    let crush = r.session.fx_add("patch", "crush").unwrap();
    assert_eq!(
        fx::order_of(&r.bank).as_slice(),
        &[drive as u8, crush as u8]
    );
    assert_eq!(r.session.fx_move("patch", crush, -1).unwrap(), Some(crush));
    assert_eq!(
        fx::order_of(&r.bank).as_slice(),
        &[crush as u8, drive as u8]
    );
    assert_eq!(
        r.session.fx_move("patch", crush, -1).unwrap(),
        None,
        "already first"
    );
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Move Crush"));
}

#[test]
fn an_effect_moved_past_its_own_kind_takes_the_other_copy() {
    // The k-th of a kind plays copy k, so the moved delay's settings travel
    // to the first copy, and the answer says so: the selection follows it.
    let r = rig();
    let first = r.session.fx_add("patch", "delay").unwrap();
    let second = r.session.fx_add("patch", "delay").unwrap();
    let id = |n: usize| format!("fx.{n}.delay.mix");
    r.session.set_param(&id(second), 0.9).unwrap();
    assert_eq!(r.session.fx_move("patch", second, -1).unwrap(), Some(first));
    assert_eq!(get(&r, &id(first)), 0.9, "its settings came with it");
    assert_ne!(get(&r, &id(second)), 0.9);
}

#[test]
fn the_arrangement_is_undone_with_the_patch() {
    let r = rig();
    r.session.set_param("arrangement.track.gain", 0.25).unwrap();
    assert_eq!(
        r.arrangement.get_by_id("arrangement.track.gain"),
        Some(0.25)
    );
    r.session.set_param("grain.size", 99.0).unwrap();
    r.session.undo().unwrap();
    r.session.undo().unwrap();
    assert_eq!(r.arrangement.get_by_id("arrangement.track.gain"), Some(1.0));
}

#[test]
fn the_arrangement_chain_is_its_own() {
    let r = rig();
    let n = r.session.fx_add("arrangement", "reverb").unwrap();
    let mix = format!("{}{}", arrangement::PREFIX, fx::row_id(n, "reverb.mix"));
    r.session.set_param(&mix, 0.4).unwrap();
    assert_eq!(r.arrangement.get_by_id(&mix), Some(0.4));
    assert!(
        fx::order_of(&r.bank).is_empty(),
        "the patch's chain is untouched"
    );
}

#[test]
fn undo_restores_the_tracker_the_engine_plays() {
    let r = rig();
    let mut t = r.session.tracker();
    t.tempo = 90.0;
    r.session.set_tracker(t).unwrap();
    assert_eq!(r.steps.load().tempo_bpm, 90.0);
    r.session.undo().unwrap();
    assert_eq!(r.session.tracker().tempo, Tracker::default().tempo);
    assert_eq!(r.steps.load().tempo_bpm, Tracker::default().tempo);
}

#[test]
fn a_pattern_edit_is_one_undo_and_one_that_changes_nothing_is_none() {
    let r = rig();
    let before = r.session.tracker();
    r.session
        .edit_tracker(&tracker::Edit::ClearBar { bar: 0 })
        .unwrap();
    assert!(!r.session.tracker().tracks[0].steps[0].on);
    assert_eq!(r.steps.load().pattern & 0xFFFF, 0, "the engine hears it");
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Clear a bar"));
    assert_eq!(r.session.tracker(), before);
    assert_eq!(r.steps.load().pattern & 0xFFFF, 0x1111);

    // A bar that is already empty, or does not exist, leaves no step.
    r.session
        .edit_tracker(&tracker::Edit::ClearBar { bar: 3 })
        .unwrap();
    r.session
        .edit_tracker(&tracker::Edit::ClearBar { bar: 9 })
        .unwrap();
    assert!(r.session.undo().unwrap().is_none());
}

#[test]
fn switching_the_mode_is_not_an_edit_and_undo_does_not_flip_it() {
    let r = rig();
    // What the mode switch does: turn the track on through `set_tracker`.
    let mut on = r.session.tracker();
    on.tracks[0].on = true;
    r.session.set_tracker(on).unwrap();
    assert!(r.steps.load().on, "the engine plays the steps");
    assert!(
        r.session.undo().unwrap().is_none(),
        "the mode is not an edit"
    );
    assert!(!r.session.read(|d| d.is_unsaved()));

    // A real edit, then undo: the steps go back and the track stays on.
    r.session
        .edit_tracker(&tracker::Edit::ClearBar { bar: 0 })
        .unwrap();
    r.session.undo().unwrap();
    assert!(r.session.tracker().tracks[0].on, "undo left the mode alone");
    assert!(r.steps.load().on);
    assert!(
        r.session.tracker().tracks[0].steps[0].on,
        "and restored the steps"
    );
}

#[test]
fn undo_restores_lfos_and_a_refused_edit_leaves_no_step() {
    let r = rig();
    r.session.add_lfo().unwrap();
    assert_eq!(r.session.modulation().0.lfos.len(), 1);
    assert_eq!(r.session.modulation().0.lfos[0].name, "LFO 1");
    assert!(r.session.remove_lfo(999_999).is_err());
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add LFO"));
    assert!(r.session.modulation().0.lfos.is_empty());
    assert!(r.session.undo().unwrap().is_none());
}

#[test]
fn a_link_reaches_the_engine() {
    let r = rig();
    r.session.add_lfo().unwrap();
    let lfo = r.session.modulation().0.lfos[0].id;
    r.mod_swap.lock().unwrap().take();
    r.session.link_param("grain.size", lfo, 0.25, 0.75).unwrap();
    assert!(
        r.mod_swap.lock().unwrap().take().is_some(),
        "a new set waits"
    );
    let (m, refused) = r.session.modulation();
    assert!(refused.is_empty());
    assert_eq!(m.links["grain.size"].source, lfo);

    // Changing a value hands no new set over.
    r.session.set_param("grain.size", 80.0).unwrap();
    assert!(r.mod_swap.lock().unwrap().take().is_none());

    r.session.unlink_param("grain.size").unwrap();
    assert!(r.session.modulation().0.links.is_empty());
}

#[test]
fn a_new_edit_after_undo_drops_redo() {
    let r = rig();
    r.session.set_param("grain.size", 10.0).unwrap();
    r.session.undo().unwrap();
    r.session.set_param("grain.pitch", 3.0).unwrap();
    assert!(r.session.redo().unwrap().is_none());
}

#[test]
fn resetting_the_sound_restores_defaults_in_one_undoable_step() {
    let r = rig();
    r.session.set_param("grain.size", 123.0).unwrap();
    let n = r.session.fx_add("patch", "delay").unwrap();
    r.session
        .set_param(&fx::row_id(n, "delay.time"), 777.0)
        .unwrap();
    // Things the reset must leave alone.
    r.session.set_param("arrangement.track.gain", 0.25).unwrap();
    let mut t = r.session.tracker();
    t.tempo = 90.0;
    r.session.set_tracker(t).unwrap();

    assert!(r.session.reset_sound().unwrap());
    for (i, p) in r.bank.defs().iter().enumerate() {
        assert_eq!(r.bank.get(i), p.default, "{}", p.id);
    }
    assert!(
        fx::order_of(&r.bank).is_empty(),
        "the chain empties with it"
    );
    assert_eq!(
        r.arrangement.get_by_id("arrangement.track.gain"),
        Some(0.25)
    );
    assert_eq!(r.session.tracker().tempo, 90.0);

    assert_eq!(
        r.session.undo().unwrap().as_deref(),
        Some("Reset the sound")
    );
    assert_eq!(get(&r, "grain.size"), 123.0);
    assert!(fx::order_of(&r.bank).contains(n), "the chain is back");
    assert_eq!(get(&r, &fx::row_id(n, "delay.time")), 777.0);
}

#[test]
fn resetting_a_sound_that_is_already_default_is_not_an_edit() {
    let r = rig();
    assert!(!r.session.reset_sound().unwrap());
    assert!(r.session.undo().unwrap().is_none());
}

#[test]
fn a_preset_applies_a_nodes_sound_and_never_its_switch() {
    let r = rig();
    r.session.set_param("grain.size", 80.0).unwrap();
    r.session.save_preset("grain", "Small").unwrap();
    assert!(
        r.session.save_preset("grain", "Small").is_err(),
        "a name in use"
    );
    assert_eq!(r.session.preset_names("grain"), vec!["Small".to_string()]);

    r.session.set_param("grain.size", 200.0).unwrap();
    r.session.set_param("grain.on", 0.0).unwrap();
    let report = r.session.apply_preset("grain", "Small").unwrap();
    assert!(report.applied > 0);
    assert_eq!(get(&r, "grain.size"), 80.0);
    assert_eq!(get(&r, "grain.on"), 0.0, "the switch stays as it was");
}

#[test]
fn materials_are_listed_in_the_order_added_and_a_removal_undoes() {
    let r = rig();
    r.session.add_drone().unwrap();
    let pool = r.session.pool();
    assert_eq!(pool.materials.len(), 2);
    let (first, second) = (pool.materials[0].id, pool.materials[1].id);
    assert_eq!(pool.wires.material, Some(first), "the wires stay put");

    r.session.remove_material(first).unwrap();
    assert_eq!(
        r.session.pool().wires.material,
        None,
        "its generator is silent"
    );
    r.session.undo().unwrap();
    let back = r.session.pool();
    assert_eq!(back.materials.len(), 2);
    assert_eq!(back.materials[1].id, second, "the order holds");
    assert_eq!(back.wires.material, Some(back.materials[0].id));
}

#[test]
fn unwiring_a_generator_hands_it_silence() {
    let r = rig();
    r.swap.lock().unwrap()[0].take();
    r.session.wire_material("material", None).unwrap();
    let slot = r.swap.lock().unwrap()[0].take();
    assert_eq!(slot, Some(Vec::new()), "silence is an empty buffer");
    r.session.undo().unwrap();
    assert!(
        r.swap.lock().unwrap()[0]
            .take()
            .is_some_and(|s| !s.is_empty()),
        "undo hands the drone back"
    );
}

#[test]
fn a_session_saves_and_opens_with_its_sound() {
    let dir = std::env::temp_dir().join(format!("shard-session-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.shard");

    let a = rig();
    a.session.set_param("grain.size", 80.0).unwrap();
    let n = a.session.fx_add("patch", "chorus").unwrap();
    a.session
        .set_param(&fx::row_id(n, "chorus.mix"), 0.3)
        .unwrap();
    let mut t = a.session.tracker();
    t.tempo = 101.0;
    a.session.set_tracker(t).unwrap();
    a.session.save(&path).unwrap();
    assert!(!a.session.read(|d| d.is_unsaved()));

    let b = rig();
    let report = b.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert!(!report.sample_missing);
    for i in 0..a.bank.defs().len() {
        assert_eq!(a.bank.get(i), b.bank.get(i), "{}", a.bank.defs()[i].id);
    }
    assert_eq!(b.steps.load().tempo_bpm, 101.0);
    assert!(
        b.session.undo().unwrap().is_none(),
        "an opened file has no history"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_file_in_the_old_format_is_refused() {
    let r = rig();
    let dir = std::env::temp_dir().join(format!("shard-old-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.shard");
    std::fs::write(
        &path,
        r#"{"version":3,"tracker":{"tempo":82.0},"patch":{"params":{"grain.size":80.0}}}"#,
    )
    .unwrap();
    let before = get(&r, "grain.size");
    assert!(r.session.open(&path).is_err());
    assert_eq!(get(&r, "grain.size"), before, "nothing changed");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_refused_modulator_edit_changes_nothing() {
    let r = rig();
    r.session.add_lfo().unwrap();
    let before = r.session.modulation().0;
    let lfo = before.lfos[0].clone();
    let bad = [
        crate::modulation::LfoRecord {
            shape: "wobble".into(),
            ..lfo.clone()
        },
        crate::modulation::LfoRecord {
            name: "  ".into(),
            ..lfo.clone()
        },
        crate::modulation::LfoRecord {
            rate: f32::NAN,
            ..lfo.clone()
        },
    ];
    for b in bad {
        assert!(r.session.set_lfo(b.clone()).is_err(), "accepted {b:?}");
    }
    assert!(r
        .session
        .link_param("grain.window", lfo.id, 0.0, 0.5)
        .is_err());
    assert!(r
        .session
        .link_param("grain.nonsense", lfo.id, 0.0, 0.5)
        .is_err());
    assert!(r
        .session
        .link_param("grain.position", 99_999, 0.0, 0.5)
        .is_err());
    assert_eq!(r.session.modulation().0, before);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add LFO"));
}

#[test]
fn a_modulator_edit_brings_rate_phase_and_ends_into_range() {
    let r = rig();
    r.session.add_lfo().unwrap();
    let lfo = r.session.modulation().0.lfos[0].clone();
    r.session
        .set_lfo(crate::modulation::LfoRecord {
            rate: 1.0e6,
            phase: -2.0,
            ..lfo.clone()
        })
        .unwrap();
    r.session
        .link_param("grain.position", lfo.id, -3.0, 7.0)
        .unwrap();
    let m = r.session.modulation().0;
    assert_eq!(m.lfos[0].rate, shard_dsp::modulation::MAX_RATE_HZ);
    assert_eq!(m.lfos[0].phase, 0.0);
    let link = m.links["grain.position"];
    assert_eq!((link.lo, link.hi), (0.0, 1.0));
}

#[test]
fn the_patch_and_its_output_keep_presets_too() {
    let r = rig();
    r.session.set_param("amp.gain", 0.5).unwrap();
    r.session.save_preset("amp", "Quiet").unwrap();
    r.session.save_preset("patch", "Plain").unwrap();
    assert_eq!(r.session.preset_names("amp"), vec!["Quiet".to_string()]);
    r.session.set_param("amp.gain", 1.0).unwrap();
    r.session.apply_preset("amp", "Quiet").unwrap();
    assert_eq!(get(&r, "amp.gain"), 0.5);
}

#[test]
fn a_knob_on_an_effect_not_in_the_chain_moves_nothing() {
    let r = rig();
    assert!(r
        .session
        .set_param(&fx::row_id(5, "flanger.mix"), 0.5)
        .is_ok());
    assert!(r.session.undo().unwrap().is_none());
    assert!(r.session.set_param("grain.nonsense", 0.5).is_err());
}

#[test]
fn the_history_reads_as_the_edit_menu_says_it() {
    let r = rig();
    r.session.set_param("filter.cutoff", 800.0).unwrap();
    r.session.set_param("grain.size", 80.0).unwrap();
    let (undo, redo) = r.session.history_labels();
    assert_eq!(undo, ["Set Cutoff", "Set Size"], "oldest first");
    assert!(redo.is_empty());
    r.session.undo().unwrap();
    // rhizome calls the undo commit "Undo Set Size"; Redo names the edit.
    assert_eq!(r.session.history().1.as_deref(), Some("Set Size"));
    assert_eq!(r.session.history_labels().1, ["Set Size"]);
}

#[test]
fn every_change_is_told_with_the_session_free_to_read() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let r = rig();
    let session = Arc::new(r.session);
    let told = Arc::new(AtomicUsize::new(0));
    let (weak, count) = (Arc::downgrade(&session), Arc::clone(&told));
    // Reading the session here would deadlock if any of its locks were held.
    session.on_change(move || {
        let s = weak.upgrade().unwrap();
        let _ = (s.is_unsaved(), s.history());
        count.fetch_add(1, Ordering::SeqCst);
    });
    let dir = std::env::temp_dir().join(format!("shard-told-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("told.shard");

    session.set_param("filter.cutoff", 800.0).unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 1, "an edit");
    session.set_param("filter.cutoff", 800.0).unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 1, "no change, nothing told");
    session.undo().unwrap();
    session.redo().unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 3, "undo and redo");
    session.save(&path).unwrap();
    session.open(&path).unwrap();
    session.new_document(2).unwrap();
    assert_eq!(told.load(Ordering::SeqCst), 6, "save, open and new");
    session.save_preset("grain", "Small").unwrap();
    session.apply_preset("grain", "Small").unwrap();
    assert!(told.load(Ordering::SeqCst) >= 7, "presets");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_new_document_starts_over_untitled_with_no_history() {
    let r = rig();
    let dir = std::env::temp_dir().join(format!("shard-new-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("before.shard");
    r.session.set_param("filter.cutoff", 800.0).unwrap();
    r.session.fx_add("patch", "chorus").unwrap();
    r.session.save(&path).unwrap();
    assert_eq!(r.session.path().as_deref(), Some(path.as_path()));
    assert_eq!(r.session.untitled_number(), 1);

    r.session.new_document(2).unwrap();
    assert_eq!(r.session.path(), None);
    assert_eq!(r.session.untitled_number(), 2);
    assert!(!r.session.is_unsaved());
    assert_eq!(r.session.history(), (None, None));
    let default = shard_dsp::params::PARAMS
        .iter()
        .find(|p| p.id == "filter.cutoff")
        .unwrap()
        .default;
    assert_eq!(
        get(&r, "filter.cutoff"),
        default,
        "the engine hears the new one"
    );
    assert!(fx::order_of(&r.bank).is_empty(), "and its empty chain");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn undoing_back_to_what_was_saved_clears_the_unsaved_mark() {
    let r = rig();
    let dir = std::env::temp_dir().join(format!("shard-mark-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    r.session.save(&dir.join("mark.shard")).unwrap();
    assert!(!r.session.is_unsaved());
    r.session.set_param("filter.cutoff", 800.0).unwrap();
    assert!(r.session.is_unsaved());
    r.session.undo().unwrap();
    assert!(!r.session.is_unsaved());
    std::fs::remove_dir_all(&dir).ok();
}

// Mod envelopes: a curve and a mode, fitted to the note (epic 32, story 6).

use crate::modulation::EnvelopeRecord;
use preset_math::{Basis, Curve, CurvePoint};

fn pt(x: f32, y: f32, basis: Basis, tension: f32) -> CurvePoint {
    CurvePoint {
        x,
        y,
        basis,
        tension,
    }
}

/// A free curve with every feature the file must keep: a smooth basis, a
/// bent linear segment, a sustain mark off the third point, and floats that
/// are not exact in binary.
fn free_curve() -> Curve {
    Curve {
        points: vec![
            pt(0.0, 0.0, Basis::CatmullRom, 0.0),
            pt(0.013, 0.9, Basis::CatmullRom, 0.0),
            pt(0.4, 0.35, Basis::Linear, -0.3),
            pt(0.7, 0.35, Basis::Monotone, 0.0),
            pt(1.0, 0.0, Basis::Linear, 0.0),
        ],
        sustain: Some(3),
    }
}

fn first_envelope(r: &Rig) -> EnvelopeRecord {
    r.session.modulation().0.envelopes[0].clone()
}

/// Where the engine's envelope stands 20 ms into a one second pass, from the
/// set the session last handed over (or the one before it, if none waits).
fn engine_level(r: &Rig, held: &mut Option<shard_dsp::ModSet>) -> f32 {
    if let Some(set) = r.mod_swap.lock().unwrap().take() {
        *held = Some(set);
    }
    let set = held.as_mut().expect("the engine has a set");
    let id = first_envelope(r).id;
    set.clock_envelopes(960.0, 48_000.0, true);
    set.env_value(id).unwrap()
}

#[test]
fn a_new_envelope_is_the_pluck_in_adsr_mode() {
    let r = rig();
    r.session.add_envelope().unwrap();
    let mut e = first_envelope(&r);
    let want = crate::modulation::new_envelope(e.id);
    e.name = want.name.clone();
    assert_eq!(e, want);
    assert_eq!(e.mode, "adsr");
}

#[test]
fn a_drag_is_one_undo_step_and_undo_restores_the_curve_and_the_engine() {
    let r = rig();
    r.session.add_envelope().unwrap();
    let was = first_envelope(&r);
    let mut held = None;
    let level_was = engine_level(&r, &mut held);
    let steps_before = r.session.history_labels().0.len();

    r.session.envelope_gesture_begin(was.id).unwrap();
    let mut level_during = level_was;
    for i in 1..=8 {
        let mut e = was.clone();
        e.curve.points[1].x = 0.005 + 0.01 * i as f32;
        e.curve.points[2].x = e.curve.points[1].x + 0.3;
        e.curve.points[3].x = e.curve.points[2].x + 0.1;
        r.session.envelope_gesture_apply(e).unwrap();
        level_during = engine_level(&r, &mut held);
    }
    assert_ne!(level_during, level_was, "the engine heard the drag");
    assert_eq!(
        r.session.history_labels().0.len(),
        steps_before,
        "no step is cut while the gesture is open"
    );
    r.session.envelope_gesture_end().unwrap();

    let dragged = first_envelope(&r);
    assert_ne!(dragged.curve, was.curve);
    let (undo, _) = r.session.history_labels();
    assert_eq!(undo.len(), steps_before + 1, "one step for the whole drag");
    assert_eq!(undo.last().map(String::as_str), Some("Change envelope"));

    assert_eq!(
        r.session.undo().unwrap().as_deref(),
        Some("Change envelope")
    );
    assert_eq!(first_envelope(&r), was, "undo restores the curve");
    assert_eq!(engine_level(&r, &mut held), level_was, "and the engine");
    r.session.redo().unwrap();
    assert_eq!(first_envelope(&r), dragged);
}

#[test]
fn escape_leaves_no_step_and_puts_the_envelope_back() {
    let r = rig();
    r.session.add_envelope().unwrap();
    let was = first_envelope(&r);
    let mut held = None;
    let level_was = engine_level(&r, &mut held);
    let steps_before = r.session.history_labels().0.len();

    r.session.envelope_gesture_begin(was.id).unwrap();
    let mut e = was.clone();
    e.curve.points[1].x = 0.2;
    e.curve.points[2].x = 0.5;
    e.curve.points[3].x = 0.6;
    r.session.envelope_gesture_apply(e).unwrap();
    assert_ne!(engine_level(&r, &mut held), level_was);

    r.session.envelope_gesture_cancel().unwrap();
    assert_eq!(first_envelope(&r), was);
    assert_eq!(engine_level(&r, &mut held), level_was, "the engine too");
    assert_eq!(r.session.history_labels().0.len(), steps_before);
    assert!(r.session.history_labels().1.is_empty(), "nothing to redo");

    // A release after the cancel, and a second cancel, are harmless.
    r.session.envelope_gesture_end().unwrap();
    r.session.envelope_gesture_cancel().unwrap();
    assert!(r.session.envelope_gesture_apply(was).is_err());
}

#[test]
fn a_gesture_meeting_other_edits_neither_deadlocks_nor_panics() {
    let r = rig();
    r.session.add_envelope().unwrap();
    let was = first_envelope(&r);

    r.session.envelope_gesture_begin(was.id).unwrap();
    let mut e = was.clone();
    e.curve.points[3].x = 0.9;
    r.session.envelope_gesture_apply(e.clone()).unwrap();

    // Undo is refused while a drag is open, and says why.
    assert!(r.session.undo().is_err());
    // A knob turned mid-drag joins the drag's step.
    r.session.set_param("grain.size", 77.0).unwrap();
    // Another envelope, or none, is refused too.
    assert!(r
        .session
        .envelope_gesture_apply(EnvelopeRecord {
            id: 999_999,
            ..e.clone()
        })
        .is_err());

    // A new begin ends the old drag, keeping what it did as its own step.
    let steps = r.session.history_labels().0.len();
    r.session.envelope_gesture_begin(was.id).unwrap();
    assert_eq!(r.session.history_labels().0.len(), steps + 1);
    assert_eq!(first_envelope(&r).curve, e.curve, "kept, not cancelled");
    r.session.envelope_gesture_cancel().unwrap();
    assert_eq!(first_envelope(&r).curve, e.curve);

    // A drag on an envelope that does not exist never opens.
    assert!(r.session.envelope_gesture_begin(999_999).is_err());

    // A document replaced mid-drag drops the drag.
    r.session.envelope_gesture_begin(was.id).unwrap();
    r.session.new_document(2).unwrap();
    r.session.envelope_gesture_end().unwrap();
    r.session.add_envelope().unwrap();
    r.session.undo().unwrap();
}

#[test]
fn mode_edits_round_trip_and_nudges_coalesce() {
    let r = rig();
    r.session.add_envelope().unwrap();
    let mut e = first_envelope(&r);

    e.mode = "free".into();
    e.curve = free_curve();
    r.session.set_envelope(e.clone(), false).unwrap();
    assert_eq!(first_envelope(&r), e);

    // A free curve with a smooth middle and no sustain mark.
    e.curve = Curve {
        points: vec![
            pt(0.0, 0.0, Basis::Linear, 0.0),
            pt(0.5, 1.0, Basis::Monotone, 0.0),
            pt(1.0, 0.0, Basis::Linear, 0.0),
        ],
        sustain: None,
    };
    r.session.set_envelope(e.clone(), false).unwrap();
    assert_eq!(first_envelope(&r), e);

    let steps = r.session.history_labels().0.len();
    for i in 0..5 {
        e.curve.points[1].y = 0.5 + 0.1 * i as f32;
        r.session.set_envelope(e.clone(), true).unwrap();
    }
    assert_eq!(
        r.session.history_labels().0.len(),
        steps + 1,
        "coalesced nudges share a step"
    );
    r.session.undo().unwrap();
    assert_eq!(first_envelope(&r).curve.points[1].y, 1.0);
    r.session.undo().unwrap();
    assert_eq!(first_envelope(&r).curve, free_curve());
}

#[test]
fn a_free_curve_with_a_sustain_mark_saves_and_opens_bit_exactly() {
    let dir = std::env::temp_dir().join(format!("shard-envelope-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("curve.shard");

    let a = rig();
    a.session.add_envelope().unwrap();
    let mut e = first_envelope(&a);
    e.name = "Wobbly".into();
    e.mode = "free".into();
    e.curve = free_curve();
    a.session.set_envelope(e.clone(), false).unwrap();
    a.session.save(&path).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("catmull-rom"), "{text}");
    assert!(
        text.contains("0.013"),
        "floats are written as typed: {text}"
    );
    assert!(!text.contains("0.0130000"), "{text}");

    let b = rig();
    let report = b.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    let back = first_envelope(&b);
    assert_eq!(back.curve, free_curve(), "bit for bit");
    assert_eq!((back.mode.as_str(), back.name.as_str()), ("free", "Wobbly"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_envelope_the_tree_would_refuse_is_refused_with_its_reason() {
    let r = rig();
    r.session.add_envelope().unwrap();
    let base = first_envelope(&r);
    let adsr = |points: Vec<CurvePoint>, sustain: Option<usize>| EnvelopeRecord {
        curve: Curve { points, sustain },
        ..base.clone()
    };
    let free = |points: Vec<CurvePoint>, sustain: Option<usize>| EnvelopeRecord {
        mode: "free".into(),
        curve: Curve { points, sustain },
        ..base.clone()
    };
    let l = |x: f32, y: f32| pt(x, y, Basis::Linear, 0.0);
    let good = || vec![l(0.0, 0.0), l(0.1, 1.0), l(0.3, 0.4), l(0.5, 0.0)];

    let cases: Vec<(&str, EnvelopeRecord, &str)> = vec![
        (
            "adsr with five points",
            adsr(
                vec![
                    l(0.0, 0.0),
                    l(0.1, 1.0),
                    l(0.3, 0.4),
                    l(0.4, 0.4),
                    l(0.5, 0.0),
                ],
                Some(2),
            ),
            "it has 5 points, not four",
        ),
        (
            "adsr without a sustain mark",
            adsr(good(), None),
            "sustain mark is None",
        ),
        (
            "adsr with the sustain on the wrong point",
            adsr(good(), Some(1)),
            "sustain mark is Some(1)",
        ),
        (
            "adsr not starting at rest",
            adsr(
                vec![l(0.0, 0.2), l(0.1, 1.0), l(0.3, 0.4), l(0.5, 0.0)],
                Some(2),
            ),
            "does not start at 0",
        ),
        (
            "adsr not reaching the peak",
            adsr(
                vec![l(0.0, 0.0), l(0.1, 0.8), l(0.3, 0.4), l(0.5, 0.0)],
                Some(2),
            ),
            "attack does not reach 1",
        ),
        (
            "adsr not ending at rest",
            adsr(
                vec![l(0.0, 0.0), l(0.1, 1.0), l(0.3, 0.4), l(0.5, 0.1)],
                Some(2),
            ),
            "release does not end at 0",
        ),
        ("no points", free(vec![], None), "at least one point"),
        (
            "points out of order",
            free(vec![l(0.0, 0.0), l(0.5, 1.0), l(0.2, 0.0)], None),
            "before the one ahead of it",
        ),
        (
            "a point before the start",
            free(vec![l(-0.1, 0.0), l(0.5, 1.0)], None),
            "before the start",
        ),
        (
            "past the end of the note",
            free(vec![l(0.0, 0.0), l(1.5, 1.0)], None),
            "past the end",
        ),
        (
            "a level above 1",
            free(vec![l(0.0, 0.0), l(0.5, 1.2)], None),
            "outside 0 to 1",
        ),
        (
            "a sustain mark off the end",
            free(vec![l(0.0, 0.0), l(0.5, 1.0)], Some(2)),
            "sustain mark",
        ),
        (
            "an adsr drawn over more than the note",
            adsr(
                vec![l(0.0, 0.0), l(1.0, 1.0), l(3.0, 0.4), l(5.0, 0.0)],
                Some(2),
            ),
            "past the end",
        ),
    ];
    for (what, record, reason) in cases {
        let e = r
            .session
            .set_envelope(record.clone(), false)
            .expect_err(what);
        eprintln!("{what}: {e}");
        assert!(e.contains(reason), "{what}: wrong reason, got “{e}”");
        // The gesture path holds the same line, and leaves the gesture open.
        r.session.envelope_gesture_begin(base.id).unwrap();
        assert!(r.session.envelope_gesture_apply(record).is_err(), "{what}");
        r.session.envelope_gesture_cancel().unwrap();
        assert_eq!(first_envelope(&r), base, "{what} changed something");
    }

    // The names the webview sends are checked before the tree sees them.
    for bad in [
        EnvelopeRecord {
            mode: "wobble".into(),
            ..base.clone()
        },
        EnvelopeRecord {
            name: " ".into(),
            ..base.clone()
        },
    ] {
        let e = r.session.set_envelope(bad, false).unwrap_err();
        eprintln!("{e}");
    }
    assert_eq!(first_envelope(&r), base);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add envelope"));
}

#[test]
fn an_envelope_as_the_webview_sends_it_is_accepted() {
    // The JSON `EnvelopeEditor` sends: ux-kit's `adsrCurve` writes no
    // `tension`, and a free curve may have no `sustain` key at all.
    let r = rig();
    r.session.add_envelope().unwrap();
    let id = first_envelope(&r).id;
    let adsr: crate::modulation::EnvelopeRecord = serde_json::from_value(serde_json::json!({
        "id": id, "name": "Pluck", "mode": "adsr",
        "curve": {"points": [
            {"x": 0.0, "y": 0.0, "basis": "linear"},
            {"x": 0.01, "y": 1.0, "basis": "linear"},
            {"x": 0.2, "y": 0.4, "basis": "linear"},
            {"x": 0.5, "y": 0.0, "basis": "linear"}
        ], "sustain": 2}
    }))
    .unwrap();
    r.session.set_envelope(adsr.clone(), false).unwrap();
    assert_eq!(first_envelope(&r).curve, adsr.curve);

    let free: crate::modulation::EnvelopeRecord = serde_json::from_value(serde_json::json!({
        "id": id, "name": "Pluck", "mode": "free",
        "curve": {"points": [
            {"x": 0.0, "y": 0.0, "basis": "monotone"},
            {"x": 1.0, "y": 1.0, "basis": "catmull-rom"}
        ]}
    }))
    .unwrap();
    r.session.set_envelope(free.clone(), false).unwrap();
    let got = first_envelope(&r);
    assert_eq!(got.mode, "free");
    assert_eq!(got.curve, free.curve);
}
