//! Mute and solo through the session (epic 28, story c): a press is no edit
//! and no unsaved change, it reaches the engine and lands on the next bar,
//! the switches follow their track, and a saved file carries them.

use rhizome_core::Value;
use shard_dsp::{Engine, Mix};

use crate::object_model as om;
use crate::session::MixState;
use crate::session_tests::{melody_rig as rig, Rig};

fn mix_of(r: &Rig, voice: usize) -> Mix {
    Mix::from_bits(
        r.voice(voice)
            .mix
            .load(std::sync::atomic::Ordering::Relaxed),
    )
}

fn pressed(r: &Rig) -> Vec<(bool, bool)> {
    r.session.mix().iter().map(|m| (m.mute, m.solo)).collect()
}

/// Three tracks, all on the first patch.
fn three_tracks(r: &Rig) {
    r.session.add_track(false).unwrap();
    r.session.add_track(false).unwrap();
    assert_eq!(r.session.track_count(), 3);
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shard-mix-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("mix.shard")
}

#[test]
fn a_mute_or_a_solo_is_no_undo_step_and_no_unsaved_change() {
    let r = rig();
    r.session.set_param("grain.size", 80.0).unwrap();
    r.session.save(&temp("clean")).unwrap();
    let history = r.session.history_labels();
    assert!(!r.session.is_unsaved());

    r.session.set_mute(0, true).unwrap();
    r.session.set_solo(0, true).unwrap();
    assert_eq!(pressed(&r), vec![(true, true)]);
    assert_eq!(r.session.history_labels(), history, "a press made a step");
    assert!(
        !r.session.is_unsaved(),
        "a press marked the session unsaved"
    );
    // It reached the voice.
    assert_eq!(
        mix_of(&r, 0),
        Mix {
            mute: true,
            solo: true,
            solo_safe: false
        }
    );
    // And undo steps the document, not the switches.
    assert!(r.session.undo().unwrap().is_some());
    assert_eq!(pressed(&r), vec![(true, true)]);
    // A click flips what Rust holds, so two presses always cancel.
    r.session.toggle_mute(0).unwrap();
    r.session.toggle_solo(0).unwrap();
    assert_eq!(pressed(&r), vec![(false, false)]);
    r.session.toggle_mute(0).unwrap();
    assert_eq!(pressed(&r), vec![(true, false)]);
    assert_eq!(r.session.history_labels().0.len(), history.0.len() - 1);
}

#[test]
fn save_then_open_restores_mutes_and_solos() {
    let path = temp("round-trip");
    let a = rig();
    three_tracks(&a);
    a.session.set_mute(0, true).unwrap();
    a.session.set_solo(2, true).unwrap();
    a.session.save(&path).unwrap();
    assert!(!a.session.is_unsaved(), "saving marks the session saved");
    // The live tree never had them written.
    a.session.read(|d| {
        for t in om::tracks(d.tree()) {
            assert_ne!(t.value(om::TRACK_MUTE), Some(Value::Bool(true)));
            assert_ne!(t.value(om::TRACK_SOLO), Some(Value::Bool(true)));
        }
    });

    let b = rig();
    let report = b.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert_eq!(
        pressed(&b),
        vec![(true, false), (false, false), (false, true)]
    );
    assert!(mix_of(&b, 0).mute && mix_of(&b, 2).solo && !mix_of(&b, 1).mute);
    assert!(!b.session.is_unsaved());
    assert!(
        b.session.undo().unwrap().is_none(),
        "an opened file has no history"
    );

    // Undo and redo after opening leave the switches alone.
    b.session.set_param("grain.size", 90.0).unwrap();
    b.session.undo().unwrap();
    b.session.redo().unwrap();
    assert_eq!(
        pressed(&b),
        vec![(true, false), (false, false), (false, true)]
    );

    // The file follows the session, not what the tree was loaded with: an
    // unmute saved over the loaded mute sticks.
    b.session.set_mute(0, false).unwrap();
    b.session.save(&path).unwrap();
    let c = rig();
    c.session.open(&path).unwrap();
    assert_eq!(
        pressed(&c),
        vec![(false, false), (false, false), (false, true)]
    );
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn a_new_session_starts_with_nothing_muted() {
    let r = rig();
    r.session.set_mute(0, true).unwrap();
    r.session.new_document(2).unwrap();
    // A new session's melody track and drums track.
    assert_eq!(pressed(&r), vec![(false, false); 2]);
    assert!(!mix_of(&r, 0).mute);
}

#[test]
fn the_switches_follow_their_track_through_remove_and_undo() {
    let r = rig();
    three_tracks(&r);
    r.session.set_mute(1, true).unwrap();
    r.session.set_solo(2, true).unwrap();
    r.session.remove_track(1).unwrap();
    // Track 3 moved up to voice 1, its solo with it.
    assert_eq!(pressed(&r), vec![(false, false), (false, true)]);
    assert!(mix_of(&r, 1).solo && !mix_of(&r, 1).mute);
    // The voice left over is no track: safe from solos, as an audition is.
    assert!(mix_of(&r, 2).solo_safe);

    r.session.undo().unwrap();
    assert_eq!(
        pressed(&r),
        vec![(false, false), (true, false), (false, true)],
        "undoing Remove Track brings back its mute"
    );
    assert!(mix_of(&r, 1).mute && mix_of(&r, 2).solo);
}

/// Blocks until the transport's bar count reaches `bar`, recording what the
/// session reports for track `track` after each.
fn play_until(
    r: &Rig,
    e: &mut Engine,
    held: &mut crate::Held,
    bar: u64,
    track: usize,
) -> Vec<(Option<u64>, MixState)> {
    let mut out = vec![0.0f32; 512];
    let mut seen = Vec::new();
    while e.bar().unwrap_or(0) < bar {
        r.block(e, &mut out, held);
        seen.push((e.bar(), r.session.mix()[track]));
    }
    seen
}

#[test]
fn a_press_through_the_session_lands_on_the_next_bar() {
    let r = rig();
    r.session.add_track(false).unwrap();
    let mut e = Engine::new(48_000.0, 64);
    e.set_voice_active(0, false);
    let mut held = crate::Held::new();
    let mut out = vec![0.0f32; 512];
    // Stopped: at once.
    r.session.set_mute(0, true).unwrap();
    r.block(&mut e, &mut out, &mut held);
    let m = r.session.mix()[0];
    assert!(m.mute && m.heard_mute, "stopped, a mute applies at once");
    r.session.set_mute(0, false).unwrap();
    r.block(&mut e, &mut out, &mut held);
    assert!(!r.session.mix()[0].heard_mute);

    e.set_playing(true);
    for _ in 0..40 {
        r.block(&mut e, &mut out, &mut held);
    }
    assert_eq!(e.bar(), Some(0));
    r.session.set_mute(0, true).unwrap();
    r.session.set_solo(1, true).unwrap();
    let seen = play_until(&r, &mut e, &mut held, 1, 0);
    let (before, after) = seen.split_at(seen.len() - 1);
    assert!(
        before.iter().all(|(_, m)| m.mute && !m.heard_mute),
        "the mute waited for the bar"
    );
    assert!(after[0].1.heard_mute, "and landed on it");
    assert!(r.session.mix()[1].heard_solo);

    // Removing a track while playing: track 2 arrives on voice 0 and its
    // solo is heard there at once, not on the bar.
    r.session.remove_track(0).unwrap();
    r.block(&mut e, &mut out, &mut held);
    assert_eq!(e.bar(), Some(1));
    let m = r.session.mix()[0];
    assert!(m.solo && m.heard_solo && !m.heard_mute);
}
