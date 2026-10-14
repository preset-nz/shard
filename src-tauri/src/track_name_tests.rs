//! Tracks have names of their own: `track.label` and `drums.label` are tree
//! values, so a rename is an undo step and a file keeps it, and absent or
//! empty reads as the track's place ("Track 2"). The agent addresses a track
//! by either (`commands::resolve`).

use serde_json::json;

use crate::object_model::commands::{self, Target};
use crate::session_tests::rig;
use crate::tracker::{Track, Tracker};

fn names(r: &crate::session_tests::Rig) -> Vec<String> {
    r.session.tracker().names()
}

fn text(r: &crate::session_tests::Rig) -> String {
    r.session.read(|d| d.tree().serialise())
}

/// Tracks "Track 1", "Drums 1", "Track 2", carrying `labels` in that order.
fn song(labels: [&str; 3]) -> Tracker {
    let named = |mut t: Track, l: &str| {
        t.label = l.to_string();
        t
    };
    Tracker {
        tracks: vec![
            named(Track::blank(), labels[0]),
            named(Track::blank_drums(), labels[1]),
            named(Track::blank(), labels[2]),
        ],
        ..Tracker::default()
    }
}

#[test]
fn a_track_without_a_label_shows_its_place_and_writes_nothing() {
    let r = rig();
    assert_eq!(names(&r), ["Track 1", "Drums 1"]);
    assert!(
        !text(&r).contains("track.label") && !text(&r).contains("drums.label"),
        "absence is the default; nothing is written for it"
    );
    // The label that isn't set isn't written by a rename to the place, either.
    assert!(!r.session.rename_track(0, "Track 1").unwrap());
    assert!(!r.session.rename_track(0, "  ").unwrap());
    assert_eq!(r.session.undo().unwrap(), None);
}

#[test]
fn a_rename_is_one_undo_step_and_undo_puts_the_old_name_back() {
    let r = rig();
    assert!(r.session.rename_track(0, "  Sub bass ").unwrap());
    assert_eq!(names(&r), ["Sub bass", "Drums 1"]);
    assert_eq!(r.session.tracker().tracks[0].label, "Sub bass");
    let (undo, _) = r.session.history_labels();
    assert_eq!(undo, ["Rename Track"]);

    assert!(r.session.rename_track(0, "Low end").unwrap());
    assert!(r.session.rename_track(1, "Beat").unwrap());
    assert_eq!(names(&r), ["Low end", "Beat"]);

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Rename Track"));
    assert_eq!(names(&r), ["Low end", "Drums 1"]);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Rename Track"));
    assert_eq!(names(&r), ["Sub bass", "Drums 1"]);
    r.session.redo().unwrap();
    assert_eq!(names(&r), ["Low end", "Drums 1"]);
}

#[test]
fn an_empty_name_or_the_place_clears_the_label() {
    let r = rig();
    r.session.rename_track(0, "Sub bass").unwrap();
    assert!(!r.session.rename_track(0, "Sub bass").unwrap(), "no step");
    assert!(r.session.rename_track(0, "").unwrap());
    assert_eq!(names(&r), ["Track 1", "Drums 1"]);
    assert!(!text(&r).contains("Sub bass"));

    r.session.rename_track(1, "Beat").unwrap();
    assert!(r.session.rename_track(1, "Drums 1").unwrap());
    assert_eq!(names(&r), ["Track 1", "Drums 1"]);
    r.session.undo().unwrap();
    assert_eq!(names(&r), ["Track 1", "Beat"]);
}

#[test]
fn a_rename_never_takes_another_tracks_name_or_place() {
    let r = rig();
    r.session.add_track(false).unwrap();
    r.session.rename_track(0, "Sub bass").unwrap();
    for refused in ["sub BASS", "Track 2", "Track 1", "new", "3"] {
        let e = r.session.rename_track(1, refused).unwrap_err();
        assert!(e.contains(refused), "{e}");
    }
    assert_eq!(names(&r), ["Sub bass", "Drums 1", "Track 2"]);
    // Its own place is its own.
    assert!(!r.session.rename_track(2, "Track 2").unwrap());
    assert!(r.session.rename_track(9, "x").is_err());
}

#[test]
fn a_label_survives_save_and_open_and_the_tracker_projection_carries_it() {
    let dir = std::env::temp_dir().join(format!("shard-track-names-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("named.shard");

    let a = rig();
    a.session.rename_track(0, "Sub bass").unwrap();
    a.session.rename_track(1, "Beat").unwrap();
    a.session.save(&path).unwrap();

    let b = rig();
    b.session.open(&path).unwrap();
    assert_eq!(names(&b), ["Sub bass", "Beat"]);
    let t = b.session.tracker();
    assert_eq!(
        (t.tracks[0].label.as_str(), t.tracks[1].label.as_str()),
        ("Sub bass", "Beat")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn editing_the_steps_never_undoes_a_rename() {
    let r = rig();
    // A tracker read before the rename, sent back after it.
    let stale = r.session.tracker();
    r.session.rename_track(0, "Sub bass").unwrap();
    let mut next = stale;
    next.tempo = 100.0;
    r.session.set_tracker(next).unwrap();
    assert_eq!(names(&r), ["Sub bass", "Drums 1"]);
}

#[test]
fn resolve_takes_a_place_or_a_label_in_any_case() {
    let t = song(["Sub bass", "", "Lead"]);
    let one = |track: serde_json::Value, drums: bool| {
        commands::resolve(&t, &json!({ "track": track }), drums)
    };
    assert_eq!(one(json!("sub bass"), false), Ok(Target::Place(0)));
    assert_eq!(one(json!("  LEAD "), false), Ok(Target::Place(2)));
    assert_eq!(
        one(json!("Track 2"), false),
        Ok(Target::Place(2)),
        "its place still works"
    );
    assert_eq!(one(json!("Drums 1"), true), Ok(Target::Place(1)));
    assert_eq!(
        one(json!(2), false),
        Ok(Target::Place(2)),
        "a bare number is a place"
    );
    // A label of the other kind is told so, naming it both ways.
    let e = one(json!("Sub bass"), true).unwrap_err();
    assert!(e.contains("Sub bass (Track 1) is a melody track"), "{e}");
    // Not a track: the song is listed with both names.
    let e = one(json!("Wobble"), false).unwrap_err();
    assert!(
        e.contains("Sub bass (Track 1), Drums 1, Lead (Track 2)"),
        "{e}"
    );
}

#[test]
fn resolve_refuses_a_shared_label_or_one_that_is_anothers_place() {
    let shared = song(["Bass", "", "bass"]);
    let e = commands::resolve(&shared, &json!({ "track": "BASS" }), false).unwrap_err();
    assert!(e.contains("more than one track"), "{e}");
    assert!(
        e.contains("Bass (Track 1)") && e.contains("bass (Track 2)"),
        "{e}"
    );

    // Track 1 called "Track 2", which is also track 3's place.
    let colliding = song(["Track 2", "", ""]);
    let e = commands::resolve(&colliding, &json!({ "track": "Track 2" }), false).unwrap_err();
    assert!(e.contains("more than one track"), "{e}");
    assert!(e.contains("Track 2 (Track 1)"), "{e}");
    // The other place of each still gets through.
    assert_eq!(
        commands::resolve(&colliding, &json!({ "track": "Track 1" }), false),
        Ok(Target::Place(0))
    );
}

#[test]
fn a_track_that_is_called_its_own_place_is_not_ambiguous() {
    let t = song(["Track 1", "", ""]);
    assert_eq!(
        commands::resolve(&t, &json!({ "track": "track 1" }), false),
        Ok(Target::Place(0))
    );
}

#[test]
fn song_gives_each_tracks_place_and_its_label_when_it_has_one() {
    let r = rig();
    r.session.rename_track(0, "Sub bass").unwrap();
    let song = r.session.read(commands::song);
    let tracks = song["tracks"].as_array().unwrap();
    assert_eq!(tracks[0]["name"], "Track 1");
    assert_eq!(tracks[0]["label"], "Sub bass");
    assert_eq!(tracks[1]["name"], "Drums 1");
    assert!(
        tracks[1].get("label").is_none(),
        "an unnamed track has none"
    );
}
