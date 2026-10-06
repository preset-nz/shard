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
    swap: Arc<Mutex<crate::session::SourceSlots>>,
}

pub(crate) fn rig() -> Rig {
    let sample_rate = 48_000.0;
    let bank = Arc::new(ParamBank::new());
    let arrangement = Arc::new(ParamBank::for_table(arrangement::params()));
    let steps = Arc::new(StepBank::new(Tracker::default().params()));
    let mod_swap = Arc::new(Mutex::new(None));
    let swap = Arc::new(Mutex::new([None, None]));
    let feeds = Feeds {
        bank: Arc::clone(&bank),
        arrangement: Arc::clone(&arrangement),
        steps: Arc::clone(&steps),
        mod_swap: Arc::clone(&mod_swap),
        swap: Arc::clone(&swap),
        readings: Arc::new(ReadingBank::new()),
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
    assert!(r.session.fx_move("patch", crush, -1).unwrap());
    assert_eq!(
        fx::order_of(&r.bank).as_slice(),
        &[crush as u8, drive as u8]
    );
    assert!(
        !r.session.fx_move("patch", crush, -1).unwrap(),
        "already first"
    );
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Move Crush"));
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
