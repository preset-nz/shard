//! The sidechain through the session (roadmap row 41): *Ducked by* is a
//! reference to the key track's node, one undo step; Depth, Release and Key
//! only are edits; removing the key un-ducks in the same step; a saved file
//! keeps it all; and the engine is handed it through each track's slot.

use rhizome_core::{Ref, Value};
use shard_dsp::{Engine, Key};

use crate::object_model as om;
use crate::session_tests::{rig, Rig};
use crate::tracker::Duck;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shard-duck-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("duck.shard")
}

fn by(r: &Rig) -> Vec<Option<usize>> {
    r.session
        .tracker()
        .tracks
        .iter()
        .map(|t| t.duck.by)
        .collect()
}

fn voice_key(r: &Rig, voice: usize) -> Option<Key> {
    r.voice(voice).duck.load().key
}

#[test]
fn ducked_by_is_one_undo_step_and_reaches_the_engine() {
    // A new session: Track 1 on voice 0, Drums 1 on kit 0.
    let r = rig();
    assert_eq!(by(&r), vec![None, None], "a new session ducks nothing");
    assert!(r.session.set_track_duck(0, Some(1)).unwrap());
    assert_eq!(by(&r), vec![Some(1), None]);
    assert_eq!(r.session.history().0.as_deref(), Some("Set Ducked by"));
    assert_eq!(voice_key(&r, 0), Some(Key::Kit(0)));
    assert!(
        !r.session.set_track_duck(0, Some(1)).unwrap(),
        "the same pick again is no step"
    );
    // A drums track can be ducked too, by a melody track.
    r.session.set_track_duck(1, Some(0)).unwrap();
    assert_eq!(r.voices.kits[0].duck.load().key, Some(Key::Voice(0)));

    r.session.undo().unwrap();
    r.session.undo().unwrap();
    assert_eq!(by(&r), vec![None, None]);
    assert_eq!(voice_key(&r, 0), None);
    r.session.redo().unwrap();
    assert_eq!(voice_key(&r, 0), Some(Key::Kit(0)));
    r.session.set_track_duck(0, None).unwrap();
    assert_eq!(by(&r), vec![None, None]);
}

#[test]
fn a_track_cant_be_ducked_by_itself() {
    let r = rig();
    assert!(r.session.set_track_duck(0, Some(0)).is_err());
    assert!(
        r.session.set_track_duck(0, Some(9)).is_err(),
        "no such track"
    );
    // Nor can the tree be made to say so.
    let t = r.session.read(|d| om::tracks(d.tree())[0].id());
    let refused = r.session.edit("Self", None, |tx| {
        tx.set_ref(t, om::TRACK_DUCK.by, Ref::here(t))
    });
    assert!(refused.is_err());
    assert_eq!(by(&r), vec![None, None]);
}

#[test]
fn removing_the_key_unducks_and_undo_brings_it_back() {
    let r = rig();
    r.session.add_track(false).unwrap(); // Track 2, place 2, voice 1.
    r.session.set_track_duck(0, Some(2)).unwrap();
    r.session.set_track_duck(2, Some(1)).unwrap();
    assert_eq!(voice_key(&r, 0), Some(Key::Voice(1)));

    // Removing Drums 1 moves Track 2 down a place; its key goes, and Track
    // 1's key follows Track 2 to its new place.
    r.session.remove_track(1).unwrap();
    assert_eq!(by(&r), vec![Some(1), None]);
    assert_eq!(voice_key(&r, 0), Some(Key::Voice(1)));
    assert_eq!(voice_key(&r, 1), None);
    // No reference to a track that has gone is left in the tree.
    r.session.read(|d| {
        let t = om::tracks(d.tree())[1];
        assert!(t.reference(om::TRACK_DUCK.by).is_none());
    });
    r.session.undo().unwrap();
    assert_eq!(by(&r), vec![Some(2), None, Some(1)]);
    assert_eq!(voice_key(&r, 1), Some(Key::Kit(0)));

    // Removing Track 2, the key of Track 1.
    r.session.remove_track(2).unwrap();
    assert_eq!(by(&r), vec![None, None]);
    assert_eq!(voice_key(&r, 0), None);
    r.session.undo().unwrap();
    assert_eq!(by(&r), vec![Some(2), None, Some(1)]);
    assert_eq!(voice_key(&r, 0), Some(Key::Voice(1)));
}

#[test]
fn depth_release_and_key_only_are_edits_each_with_its_name() {
    let r = rig();
    r.session.set_track_duck(0, Some(1)).unwrap();
    let change = |f: &dyn Fn(&mut Duck)| {
        let mut t = r.session.tracker();
        f(&mut t.tracks[0].duck);
        r.session.set_tracker(t).unwrap();
        r.session.history().0
    };
    assert_eq!(change(&|d| d.depth = 0.5).as_deref(), Some("Set Depth"));
    assert_eq!(
        change(&|d| d.release = 400.0).as_deref(),
        Some("Set Release")
    );
    assert_eq!(
        change(&|d| d.key_only = true).as_deref(),
        Some("Set Key only")
    );
    // A switch is a click: two are two steps, and one undo takes back one.
    let steps = r.session.history_labels().0.len();
    change(&|d| d.key_only = false);
    change(&|d| d.key_only = true);
    assert_eq!(r.session.history_labels().0.len(), steps + 2);
    r.session.undo().unwrap();
    assert!(!r.session.tracker().tracks[0].duck.key_only);
    r.session.redo().unwrap();
    let d = r.voice(0).duck.load();
    assert_eq!((d.depth, d.release_ms, d.key_only), (0.5, 400.0, true));
    // A drag is one step, not one a move.
    let steps = r.session.history_labels().0.len();
    change(&|d| d.depth = 0.4);
    change(&|d| d.depth = 0.3);
    assert_eq!(r.session.history_labels().0.len(), steps + 1);
    // Who ducks it is never `set_tracker`'s: a stale pick sent back is
    // ignored.
    change(&|d| d.by = None);
    assert_eq!(by(&r), vec![Some(1), None]);
    // And a fader drag is still "Set Gain".
    let mut t = r.session.tracker();
    t.tracks[0].gain = 0.5;
    r.session.set_tracker(t).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Set Gain"));
}

#[test]
fn a_saved_file_keeps_the_sidechain_and_opens_clean() {
    let path = temp("round-trip");
    let a = rig();
    a.session.add_track(false).unwrap();
    a.session.set_track_duck(0, Some(1)).unwrap();
    a.session.set_track_duck(2, Some(0)).unwrap();
    let mut t = a.session.tracker();
    t.tracks[0].duck.depth = 0.4;
    t.tracks[0].duck.release = 80.0;
    t.tracks[1].duck.key_only = true;
    a.session.set_tracker(t).unwrap();
    a.session.save(&path).unwrap();

    let b = rig();
    let report = b.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert_eq!(
        b.session
            .tracker()
            .tracks
            .iter()
            .map(|t| t.duck)
            .collect::<Vec<_>>(),
        a.session
            .tracker()
            .tracks
            .iter()
            .map(|t| t.duck)
            .collect::<Vec<_>>()
    );
    assert!(b.voices.kits[0].duck.load().key_only);

    // With its key removed, a file names no track that has gone.
    b.session.remove_track(0).unwrap();
    b.session.save(&path).unwrap();
    let c = rig();
    let report = c.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert_eq!(by(&c), vec![None, None]);
}

#[test]
fn a_file_from_before_opens_not_ducked_and_heard() {
    let r = rig();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/trip-hop-acid.shard"
    );
    let report = r.session.open(std::path::Path::new(path)).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    for t in r.session.tracker().tracks {
        assert_eq!(t.duck, Duck::default());
    }
    r.session.read(|d| {
        for t in om::tracks(d.tree()) {
            let keys = om::duck_keys(&t);
            assert_eq!(t.value(keys.key_only), Some(Value::Bool(false)));
        }
    });
}

#[test]
fn the_engine_dips_where_the_session_says() {
    // Track 1 ducked by Drums 1, the kit switched on and Key only: the
    // voice dips on the kick, through the callback's own hand-offs.
    let r = rig();
    r.session.set_track_duck(0, Some(1)).unwrap();
    let mut t = r.session.tracker();
    t.tracks[1].drums.as_mut().unwrap().on = true;
    t.tracks[1].duck.key_only = true;
    r.session.set_tracker(t).unwrap();
    let mut e = Engine::new(48_000.0, 64);
    e.set_playing(true);
    let mut out = vec![0.0f32; 256];
    let mut held = crate::Held::new();
    let mut lowest = 1.0f32;
    for _ in 0..200 {
        r.block(&mut e, &mut out, &mut held);
        lowest = lowest.min(e.voice_duck(0));
        assert_eq!(e.kit_duck(0), 1.0, "the kit is ducked by nothing");
    }
    assert!(lowest < 0.3, "the voice never dipped: {lowest}");
}
