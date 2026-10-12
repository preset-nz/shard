//! Drums tracks (epic 28, story d): a track type of its own in the one
//! order, each on its own kit. Add and Remove are one step each, a drums
//! track never moves a melody track's voice, its mute and solo are a
//! track's, and a saved file carries every drums track as it was.

use std::sync::atomic::Ordering;

use shard_dsp::{Engine, Mix, KITS};

use crate::object_model as om;
use crate::session::{self, MixState};
use crate::session_tests::{rig, Rig};
use crate::tracker::{Kit, Slot};

fn kit_active(r: &Rig, k: usize) -> bool {
    r.voices.kits[k].active.load(Ordering::Relaxed)
}

fn kit_mix(r: &Rig, k: usize) -> Mix {
    Mix::from_bits(r.voices.kits[k].mix.load(Ordering::Relaxed))
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shard-drums-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("drums.shard")
}

#[test]
fn a_new_session_has_its_drums_on_kit_zero_switched_off() {
    let r = rig();
    assert_eq!(r.session.slots(), vec![Slot::Melody(0), Slot::Drums(0)]);
    assert!(kit_active(&r, 0), "the drums track's kit plays");
    assert!((1..KITS).all(|k| !kit_active(&r, k)));
    let (grid, sound) = r.voices.kits[0].bank.load();
    assert_eq!(grid, shard_dsp::KitPattern::default());
    assert!(!sound.on, "as the Drums card started");
}

#[test]
fn add_drums_is_one_undo_step_and_refused_once_every_kit_has_one() {
    let r = rig();
    let before = r.session.history_labels().0.len();
    assert_eq!(r.session.add_track(true).unwrap(), 2);
    assert_eq!(r.session.history().0.as_deref(), Some("Add Drums"));
    assert_eq!(r.session.history_labels().0.len(), before + 1);
    let t = r.session.tracker();
    assert_eq!(t.slots()[2], Slot::Drums(1));
    let (grid, sound, gain) = t.kit(1).unwrap();
    assert!(sound.on, "an added drums track is on");
    assert!(grid.hits.iter().flatten().all(|h| *h == 0), "and empty");
    assert_eq!((grid.length, gain), (16, 1.0));
    assert!(kit_active(&r, 1));
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add Drums"));
    assert!(!kit_active(&r, 1), "its kit goes with it");
    r.session.redo().unwrap();
    assert!(kit_active(&r, 1));

    while r.session.tracker().drums().count() < KITS {
        r.session.add_track(true).unwrap();
    }
    let history = r.session.history_labels();
    assert!(r.session.add_track(true).is_err());
    assert_eq!(r.session.history_labels(), history, "and no step");
    assert!((0..KITS).all(|k| kit_active(&r, k)));
    // A melody track can still be added: the pools are separate.
    r.session.add_track(false).unwrap();
}

#[test]
fn a_drums_track_never_moves_a_melody_tracks_voice() {
    // Melody, drums, then a melody track added after the drums: it plays
    // on voice 1, not 2, and the drums stay on kit 0.
    let r = rig();
    let second = r.session.new_patch().unwrap();
    r.session.set_param("grain.size", 120.0).unwrap();
    assert_eq!(r.session.add_track(false).unwrap(), 2);
    assert_eq!(
        r.session.slots(),
        vec![Slot::Melody(0), Slot::Drums(0), Slot::Melody(1)]
    );
    assert!(r.voice(1).active.load(Ordering::Relaxed));
    assert!(!r.voice(2).active.load(Ordering::Relaxed));
    assert_eq!(r.voice(1).bank.get_by_id("grain.size"), Some(120.0));
    assert_eq!(r.session.tracker().tracks[2].patch, Some(second));
    // The melody track's steps reach voice 1.
    let mut t = r.session.tracker();
    t.tracks[2].steps[5].on = true;
    r.session.set_tracker(t).unwrap();
    assert_eq!(r.voice(1).steps.load().pattern, 1 << 5);
    // Removing the drums track leaves voice 1 where it is.
    r.session.remove_track(1).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Remove Drums 1"));
    assert!(!kit_active(&r, 0));
    assert_eq!(r.voice(1).steps.load().pattern, 1 << 5);
}

#[test]
fn removing_a_drums_track_is_one_step_and_undo_brings_its_grid_back() {
    let r = rig();
    r.session.add_track(true).unwrap();
    let mut t = r.session.tracker();
    let kit = t.tracks[2].drums.as_mut().unwrap();
    kit.snare[6] = 90;
    kit.tune = 3.0;
    t.tracks[2].length = 8;
    r.session.set_tracker(t).unwrap();
    let before = r.session.tracker();

    // The first drums track goes; the second moves down onto kit 0.
    r.session.remove_track(1).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Remove Drums 1"));
    let (grid, sound) = r.voices.kits[0].bank.load();
    assert_eq!((grid.length, grid.hits[1][6], sound.tune), (8, 90, 3.0));
    assert!(!kit_active(&r, 1));

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Remove Drums 1"));
    assert_eq!(r.session.tracker(), before);
    assert!(kit_active(&r, 1));
    assert_eq!(r.voices.kits[1].bank.load().0.hits[1][6], 90);
}

#[test]
fn the_last_melody_track_stays_but_the_drums_can_go() {
    let r = rig();
    let history = r.session.history_labels();
    assert!(r.session.remove_track(0).is_err());
    assert_eq!(r.session.history_labels(), history);
    r.session.remove_track(1).unwrap();
    assert_eq!(r.session.slots(), vec![Slot::Melody(0)]);
}

#[test]
fn a_drums_tracks_settings_are_one_step_each() {
    let r = rig();
    let mut t = r.session.tracker();
    for tune in [1.0, 2.0, 3.0] {
        t.tracks[1].drums.as_mut().unwrap().tune = tune;
        r.session.set_tracker(t.clone()).unwrap();
    }
    assert_eq!(r.session.history().0.as_deref(), Some("Set Tune"));
    assert_eq!(r.voices.kits[0].bank.load().1.tune, 3.0);
    for gain in [0.8, 0.6] {
        t.tracks[1].gain = gain;
        r.session.set_tracker(t.clone()).unwrap();
    }
    assert_eq!(r.voices.kits[0].gain(), 0.6);
    t.tracks[1].drums.as_mut().unwrap().on = true;
    r.session.set_tracker(t.clone()).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Set Drums"));
    assert!(r.voices.kits[0].bank.load().1.on);
    t.tracks[1].drums.as_mut().unwrap().kick[3] = 127;
    r.session.set_tracker(t).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Edit the steps"));
    let labels: Vec<String> = r.session.history_labels().0;
    assert_eq!(
        labels[labels.len() - 4..],
        ["Set Tune", "Set Gain", "Set Drums", "Edit the steps"],
        "a drag is one step"
    );
}

#[test]
fn selecting_a_drums_track_leaves_the_current_patch_and_auditions_nothing() {
    let r = rig();
    let first = r.session.patches()[0].id;
    let lone = r.session.new_patch().unwrap();
    assert!(r.voice(session::AUDITION).active.load(Ordering::Relaxed));
    assert!(
        !r.session.select_track(1).unwrap(),
        "no patch to read again"
    );
    assert_eq!(r.session.current_patch(), Some(lone));
    assert!(!r.voice(session::AUDITION).active.load(Ordering::Relaxed));
    assert!(
        r.session.set_track_patch(1, first).is_err(),
        "drums play no patch"
    );
}

#[test]
fn mute_and_solo_on_drums_tracks_are_a_tracks() {
    // A solo on the melody track leaves both drums tracks unsoloed on their
    // kits, and a solo on a drums track the melody track: the engine's one
    // rule does the silencing. Nothing is an edit.
    let r = rig();
    r.session.add_track(true).unwrap();
    let history = r.session.history_labels();
    r.session.set_solo(0, true).unwrap();
    assert!(r.voice(0).mix.load(Ordering::Relaxed) & 2 != 0);
    assert_eq!(kit_mix(&r, 0), Mix::default());
    r.session.set_solo(0, false).unwrap();
    r.session.set_mute(2, true).unwrap();
    r.session.set_solo(1, true).unwrap();
    assert!(kit_mix(&r, 1).mute && kit_mix(&r, 0).solo);
    assert!(
        !kit_mix(&r, 2).solo && kit_mix(&r, 2).solo_safe,
        "no track there"
    );
    assert_eq!(r.session.history_labels(), history);

    // Through the callback's hand-offs: stopped, they apply at once.
    let mut e = Engine::new(48_000.0, 64);
    e.set_voice_active(0, false);
    let mut held = crate::Held::new();
    let mut out = vec![0.0f32; 512];
    r.block(&mut e, &mut out, &mut held);
    assert!(e.kit_audible(0) && !e.kit_audible(1) && !e.voice_audible(0));
    let m = r.session.mix();
    assert!(m[1].heard_solo && m[2].heard_mute && !m[0].heard_solo);
}

#[test]
fn a_drums_tracks_mute_lands_on_the_bar_through_the_session() {
    let r = rig();
    let mut e = Engine::new(48_000.0, 64);
    e.set_voice_active(0, false);
    let mut held = crate::Held::new();
    let mut out = vec![0.0f32; 512];
    e.set_playing(true);
    for _ in 0..40 {
        r.block(&mut e, &mut out, &mut held);
    }
    assert_eq!(e.bar(), Some(0));
    r.session.set_mute(1, true).unwrap();
    let mut seen: Vec<MixState> = Vec::new();
    while e.bar().unwrap_or(0) < 1 {
        r.block(&mut e, &mut out, &mut held);
        seen.push(r.session.mix()[1]);
    }
    let (before, after) = seen.split_at(seen.len() - 1);
    assert!(before.iter().all(|m| m.mute && !m.heard_mute), "it waited");
    assert!(after[0].heard_mute, "and landed on the bar");
}

#[test]
fn several_drums_tracks_save_and_open_as_they_were() {
    let path = temp("round-trip");
    let a = rig();
    a.session.add_track(true).unwrap();
    a.session.add_track(false).unwrap();
    a.session.add_track(true).unwrap();
    let mut t = a.session.tracker();
    {
        let k = t.tracks[1].drums.as_mut().unwrap();
        k.on = true;
        k.decay = 2.5;
        k.hat[40] = 48;
    }
    t.tracks[1].length = 64;
    {
        let k = t.tracks[2].drums.as_mut().unwrap();
        k.tune = -7.0;
        k.kick[0] = 127;
    }
    t.tracks[2].gain = 1.6;
    t.tracks[2].length = 4;
    t.tracks[4].drums = Some(Kit {
        on: false,
        ..Kit::blank()
    });
    a.session.set_tracker(t).unwrap();
    a.session.set_mute(2, true).unwrap();
    a.session.set_solo(4, true).unwrap();
    let saved = a.session.tracker();
    a.session.save(&path).unwrap();
    // The live tree never had the switches written.
    a.session.read(|d| {
        for n in om::tracks(d.tree()) {
            let (mute, solo) = om::mix_keys(&n);
            assert_ne!(n.value(mute), Some(rhizome_core::Value::Bool(true)));
            assert_ne!(n.value(solo), Some(rhizome_core::Value::Bool(true)));
        }
    });

    let b = rig();
    let report = b.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    let opened = b.session.tracker();
    assert_eq!(opened.slots(), saved.slots());
    for (o, s) in opened.tracks.iter().zip(&saved.tracks) {
        assert_eq!(
            (o.length, o.gain, &o.steps, &o.drums),
            (s.length, s.gain, &s.steps, &s.drums)
        );
    }
    let pressed: Vec<(bool, bool)> = b.session.mix().iter().map(|m| (m.mute, m.solo)).collect();
    assert_eq!(
        pressed,
        vec![
            (false, false),
            (false, false),
            (true, false),
            (false, false),
            (false, true)
        ]
    );
    assert!((0..3).all(|k| kit_active(&b, k)) && !kit_active(&b, 3));
    assert_eq!(b.voices.kits[1].gain(), 1.6);
    assert!(kit_mix(&b, 1).mute && kit_mix(&b, 2).solo);
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn a_stale_patch_sent_back_with_a_fader_drag_is_still_set_gain() {
    // What a track plays is never `set_tracker`'s, so a tracker read before
    // a Plays pick still drags the fader as one "Set Gain".
    let r = rig();
    let mut t = r.session.tracker();
    t.tracks[0].patch = Some(u64::MAX);
    for gain in [0.9, 0.8] {
        t.tracks[0].gain = gain;
        r.session.set_tracker(t.clone()).unwrap();
    }
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Gain"));
    assert_eq!(r.voice(0).gain(), 1.0, "the whole drag was one step");
}
