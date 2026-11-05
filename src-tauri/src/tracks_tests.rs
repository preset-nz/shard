//! Several melody tracks (epic 28, story b): each track plays its own patch
//! on its own voice, Add Track and Remove Track are one step each, selecting
//! a patch never changes what a track plays, and a selected patch no track
//! plays is auditioned on the last voice.

use std::sync::atomic::Ordering;

use shard_dsp::rt::no_alloc;
use shard_dsp::{Engine, VOICES};

use crate::patch_tests::worst_jump;
use crate::session::AUDITION;
use crate::session_tests::{melody_rig as rig, Rig};
use crate::tracker::Step;

fn active(r: &Rig, i: usize) -> bool {
    r.voice(i).active.load(Ordering::Relaxed)
}

fn heard_on(r: &Rig, voice: usize, id: &str) -> f32 {
    r.voice(voice).bank.get_by_id(id).unwrap()
}

/// What every voice was handed: whether it plays, its values, its steps and
/// its fader.
fn snapshot(r: &Rig) -> Vec<(bool, Vec<f32>, shard_dsp::StepParams, f32)> {
    (0..VOICES)
        .map(|i| {
            let v = r.voice(i);
            let values = (0..v.bank.defs().len()).map(|n| v.bank.get(n)).collect();
            (active(r, i), values, v.steps.load(), v.gain())
        })
        .collect()
}

/// A second patch, its grain size 120, with a track playing it: track 2.
fn two_tracks(r: &Rig) -> (u64, u64) {
    r.session.set_param("grain.size", 80.0).unwrap();
    let first = r.session.patches()[0].id;
    let second = r.session.new_patch().unwrap();
    r.session.set_param("grain.size", 120.0).unwrap();
    assert_eq!(r.session.add_track(false).unwrap(), 1);
    (first, second)
}

#[test]
fn two_tracks_on_two_patches_each_play_their_own() {
    let r = rig();
    let (first, second) = two_tracks(&r);
    assert!(active(&r, 0) && active(&r, 1));
    assert_eq!(heard_on(&r, 0, "grain.size"), 80.0);
    assert_eq!(heard_on(&r, 1, "grain.size"), 120.0);
    let t = r.session.tracker();
    assert_eq!(t.tracks.len(), 2);
    assert_eq!(
        t.tracks[1].patch,
        Some(second),
        "a new track plays the current patch"
    );
    assert!(
        t.tracks[1].steps.iter().all(|s| *s == Step::default()),
        "and starts empty"
    );
    assert_eq!(t.tracks[1].length, 16);

    // Editing the current patch (the second) changes only its voice.
    r.session.set_param("grain.size", 150.0).unwrap();
    assert_eq!(heard_on(&r, 1, "grain.size"), 150.0);
    assert_eq!(heard_on(&r, 0, "grain.size"), 80.0);

    // Two tracks on one patch both follow an edit to it.
    r.session.set_track_patch(0, second).unwrap();
    assert_eq!(heard_on(&r, 0, "grain.size"), 150.0);
    r.session.set_param("grain.size", 60.0).unwrap();
    assert_eq!(heard_on(&r, 0, "grain.size"), 60.0);
    assert_eq!(heard_on(&r, 1, "grain.size"), 60.0);
    // And the first patch, played by nobody now, can go.
    assert!(r.session.patches()[0].deletable);
    let _ = first;
}

#[test]
fn each_voice_follows_its_own_tracks_steps_and_fader() {
    let r = rig();
    two_tracks(&r);
    let mut t = r.session.tracker();
    t.tracks[1].steps[3].on = true;
    t.tracks[1].steps[3].pitch = 7;
    t.tracks[1].length = 8;
    r.session.set_tracker(t.clone()).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Edit the steps"));
    let steps = r.voice(1).steps.load();
    assert!(steps.on);
    assert_eq!(
        (steps.pattern, steps.length, steps.pitches[3]),
        (1 << 3, 8, 7)
    );
    assert_ne!(r.voice(0).steps.load().pattern, steps.pattern);

    // A fader drag on one track is one step, "Set Gain", and reaches only
    // its voice.
    for g in [0.9, 0.7, 0.5] {
        t.tracks[1].gain = g;
        r.session.set_tracker(t.clone()).unwrap();
    }
    assert_eq!(r.voice(1).gain(), 0.5);
    assert_eq!(r.voice(0).gain(), 1.0);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Gain"));
    assert_eq!(r.voice(1).gain(), 1.0, "the whole drag was one step");
    assert_eq!(r.session.history().0.as_deref(), Some("Edit the steps"));
}

#[test]
fn a_pattern_edit_on_one_track_leaves_the_others() {
    let r = rig();
    two_tracks(&r);
    let before = r.session.tracker().tracks[0].clone();
    let mut t = r.session.tracker();
    t.tracks[1].steps[0].on = true;
    r.session.set_tracker(t).unwrap();
    r.session
        .edit_tracker(1, &crate::tracker::Edit::RepeatBar { from: 0 })
        .unwrap();
    assert_eq!(r.session.tracker().tracks[0].steps, before.steps);
    // Edits on two tracks are two steps, even inside the coalescing window.
    let mut t = r.session.tracker();
    t.tracks[0].steps[5].on = !t.tracks[0].steps[5].on;
    r.session.set_tracker(t).unwrap();
    let mut t = r.session.tracker();
    t.tracks[1].steps[5].on = !t.tracks[1].steps[5].on;
    r.session.set_tracker(t).unwrap();
    r.session.undo().unwrap();
    assert_ne!(
        r.session.tracker().tracks[0].steps,
        before.steps,
        "undo took back the second track's edit only"
    );
}

#[test]
fn a_stale_tracker_sent_back_never_adds_or_removes_a_track() {
    let r = rig();
    let stale = r.session.tracker();
    r.session.add_track(false).unwrap();
    let mut t = stale;
    t.tempo = 99.0;
    r.session.set_tracker(t).unwrap();
    assert_eq!(r.session.tracker().tracks.len(), 2, "the new track stays");
    let mut more = r.session.tracker();
    more.tracks.push(crate::tracker::Track::blank());
    r.session.set_tracker(more).unwrap();
    assert_eq!(r.session.tracker().tracks.len(), 2, "and none is added");
}

#[test]
fn add_track_is_one_undo_step_and_refused_once_every_voice_has_one() {
    let r = rig();
    let before = r.session.history_labels().0.len();
    r.session.add_track(false).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Add Track"));
    assert_eq!(r.session.history_labels().0.len(), before + 1);
    assert!(r.session.is_unsaved());
    assert!(active(&r, 1));
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add Track"));
    assert_eq!(r.session.tracker().tracks.len(), 1);
    assert!(!active(&r, 1), "its voice goes with it");
    r.session.redo().unwrap();
    assert_eq!(r.session.tracker().tracks.len(), 2);

    while r.session.track_count() < VOICES {
        r.session.add_track(false).unwrap();
    }
    let history = r.session.history_labels();
    assert!(r.session.add_track(false).is_err());
    assert_eq!(r.session.history_labels(), history, "and no step");
    assert!((0..VOICES).all(|i| active(&r, i)));
}

#[test]
fn removing_a_middle_track_keeps_the_later_ones_and_undo_brings_it_back() {
    let r = rig();
    let patches: Vec<u64> = (0..2).map(|_| r.session.new_patch().unwrap()).collect();
    r.session.add_track(false).unwrap();
    r.session.add_track(false).unwrap();
    r.session.set_track_patch(1, patches[0]).unwrap();
    r.session.set_track_patch(2, patches[1]).unwrap();
    let mut t = r.session.tracker();
    t.tracks[1].steps[1].on = true;
    t.tracks[2].steps[2].on = true;
    t.tracks[2].length = 32;
    r.session.set_tracker(t).unwrap();
    let before = r.session.tracker();

    r.session.remove_track(1).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Remove Track 2"));
    let after = r.session.tracker();
    assert_eq!(after.tracks.len(), 2);
    assert_eq!(after.tracks[1].steps, before.tracks[2].steps);
    assert_eq!(after.tracks[1].length, 32);
    assert_eq!(after.tracks[1].patch, Some(patches[1]));
    assert_eq!(after.tracks[0], before.tracks[0]);
    assert!(!active(&r, 2), "the last voice falls silent");
    assert_eq!(
        r.voice(1).steps.load().pattern,
        1 << 2,
        "the later track moved down"
    );

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Remove Track 2"));
    assert_eq!(r.session.tracker(), before, "back with its steps and patch");
    assert!(active(&r, 2));
}

#[test]
fn the_last_track_cannot_be_removed() {
    let r = rig();
    let history = r.session.history_labels();
    assert!(r.session.remove_track(0).is_err());
    assert!(r.session.remove_track(3).is_err());
    assert_eq!(r.session.history_labels(), history);
}

#[test]
fn a_track_playing_a_patch_keeps_it_from_being_deleted() {
    let r = rig();
    let (_, second) = two_tracks(&r);
    let info = r.session.patches();
    assert!(info.iter().all(|p| p.played && !p.deletable));
    assert!(r.session.delete_patch(second).is_err());
}

#[test]
fn selecting_a_patch_never_changes_what_a_track_plays() {
    let r = rig();
    let (first, second) = two_tracks(&r);
    let third = r.session.new_patch().unwrap();
    r.session.select_track(0).unwrap();
    let tracks = |s: &[(bool, Vec<f32>, shard_dsp::StepParams, f32)]| s[..2].to_vec();
    let before = tracks(&snapshot(&r));
    for id in [first, second, third, first] {
        r.session.select_patch(id).unwrap();
        assert_eq!(tracks(&snapshot(&r)), before, "selecting {id}");
    }
    r.session.select_track(1).unwrap();
    assert_eq!(tracks(&snapshot(&r)), before);
}

#[test]
fn an_unplayed_patch_is_auditioned_while_selected_and_only_then() {
    let r = rig();
    two_tracks(&r);
    let lone = r.session.new_patch().unwrap();
    r.session.set_param("grain.size", 33.0).unwrap();
    assert!(active(&r, AUDITION), "a new patch is heard at once");
    assert_eq!(heard_on(&r, AUDITION, "grain.size"), 33.0);
    assert!(!r.voice(AUDITION).steps.load().on, "looping free");
    assert_eq!(r.voices.shown(), Some(AUDITION));
    assert!(r
        .session
        .patches()
        .iter()
        .any(|p| p.id == lone && p.auditioned));

    // A played patch is heard on its track, not auditioned.
    let second = r.session.patches()[1].id;
    r.session.select_patch(second).unwrap();
    assert!(!active(&r, AUDITION));
    assert_eq!(r.voices.shown(), Some(1));

    // Selecting a track ends an audition.
    r.session.select_patch(lone).unwrap();
    assert!(active(&r, AUDITION));
    r.session.select_track(0).unwrap();
    assert!(!active(&r, AUDITION));
    assert_eq!(r.voices.shown(), Some(0));

    // So does a track picking it, which then plays it on its own voice.
    r.session.select_patch(lone).unwrap();
    r.session.set_track_patch(1, lone).unwrap();
    assert!(!active(&r, AUDITION));
    assert_eq!(heard_on(&r, 1, "grain.size"), 33.0);
    assert!(r.session.patches().iter().all(|p| !p.auditioned));
}

#[test]
fn with_every_voice_on_a_track_nothing_is_auditioned() {
    let r = rig();
    while r.session.track_count() < VOICES {
        r.session.add_track(false).unwrap();
    }
    let lone = r.session.new_patch().unwrap();
    let last = r.voice(AUDITION).steps.load();
    assert!(last.on, "the last voice still plays its track");
    assert!(r.session.patches().iter().all(|p| !p.auditioned));
    assert_eq!(r.voices.shown(), None, "no voice plays the new patch");
    let _ = lone;
    // One track fewer frees the voice, and the selected patch is heard.
    r.session.remove_track(VOICES - 1).unwrap();
    assert!(active(&r, AUDITION));
    assert!(!r.voice(AUDITION).steps.load().on);
}

#[test]
fn several_tracks_save_and_open_as_they_were() {
    let a = rig();
    let (_, second) = two_tracks(&a);
    a.session.add_track(false).unwrap();
    let mut t = a.session.tracker();
    t.tracks[1].steps[9] = Step {
        on: true,
        pitch: -3,
        hold: 2,
        velocity: 70,
        nudge: 4,
    };
    t.tracks[1].length = 64;
    t.tracks[1].gain = 0.6;
    t.tracks[2].length = 4;
    t.tracks[2].gain = 1.4;
    a.session.set_tracker(t).unwrap();
    a.session
        .set_track_patch(2, a.session.patches()[0].id)
        .unwrap();
    let saved = a.session.tracker();

    let dir = std::env::temp_dir().join(format!("shard-tracks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tracks.shard");
    a.session.save(&path).unwrap();

    let b = rig();
    b.session.open(&path).unwrap();
    let opened = b.session.tracker();
    assert_eq!(opened.tracks.len(), 3);
    let names = |r: &Rig, t: &crate::tracker::Tracker| -> Vec<Option<String>> {
        let patches = r.session.patches();
        t.tracks
            .iter()
            .map(|t| {
                patches
                    .iter()
                    .find(|p| Some(p.id) == t.patch)
                    .map(|p| p.name.clone())
            })
            .collect()
    };
    assert_eq!(names(&b, &opened), names(&a, &saved));
    assert_eq!(names(&b, &opened)[1].as_deref(), Some("Patch 2"));
    for (o, s) in opened.tracks.iter().zip(&saved.tracks) {
        assert_eq!((o.length, o.gain, &o.steps), (s.length, s.gain, &s.steps));
    }
    assert!((0..3).all(|i| active(&b, i)) && !active(&b, 3));
    assert_eq!(b.voice(1).gain(), 0.6);
    assert_eq!(b.voice(1).bank.get_by_id("grain.size"), Some(120.0));
    let _ = second;
    std::fs::remove_dir_all(&dir).ok();
}

/// Three tracks sounding, each on its own patch: the first on the drone at
/// its demo pattern, the others on FM lines of their own.
fn three_sounding(r: &Rig) {
    for (track, freq) in [(1, 110.0), (2, 220.0)] {
        r.session.new_patch().unwrap();
        for (id, v) in [
            ("material.on", 0.0),
            ("fm.on", 1.0),
            ("fm.freq", freq),
            ("fm.index", 0.0),
            ("fm.feedback", 0.0),
            ("fm.drift", 0.0),
            ("fm.gain", 0.8),
        ] {
            r.session.set_param(id, v).unwrap();
        }
        r.session.add_track(false).unwrap();
        let mut t = r.session.tracker();
        for (i, s) in t.tracks[track].steps.iter_mut().enumerate().take(16) {
            s.on = i % (track + 1) == 0;
            s.pitch = (i % 5) as i8;
        }
        r.session.set_tracker(t).unwrap();
    }
    r.session.select_track(0).unwrap();
}

#[test]
fn adding_and_removing_tracks_while_playing_does_not_click() {
    let r = rig();
    three_sounding(&r);
    let steady = worst_jump(&r, 0, 600, |_| {});
    assert!(steady > 1e-4, "the tracks sound ({steady})");
    // Remove the middle track, so the last moves down a voice, and undo it
    // back; then a track added and undone at the end.
    let removed = worst_jump(&r, 53, 900, |n| {
        if n % 2 == 0 {
            r.session.remove_track(1).unwrap();
        } else {
            r.session.undo().unwrap();
        }
    });
    let added = worst_jump(&r, 53, 900, |n| {
        if n % 2 == 0 {
            r.session.add_track(false).unwrap();
        } else {
            r.session.undo().unwrap();
        }
    });
    assert!(
        removed < steady * 2.0,
        "removing jumped by {removed} against a steady {steady}"
    );
    assert!(
        added < steady * 2.0,
        "adding jumped by {added} against a steady {steady}"
    );
}

#[test]
fn a_track_moving_onto_a_voice_with_another_material_does_not_click() {
    // Track 2 plays the drone through Sample, every step; track 3 an FM line
    // with no material wired at all. Removing track 2 moves track 3 onto its
    // voice, whose source changes from the drone to nothing.
    let r = rig();
    r.session.new_patch().unwrap();
    r.session.add_track(false).unwrap();
    r.session.new_patch().unwrap();
    for (id, v) in [
        ("fm.on", 1.0),
        ("fm.freq", 110.0),
        ("fm.index", 0.0),
        ("fm.feedback", 0.0),
        ("fm.drift", 0.0),
        ("fm.gain", 0.8),
    ] {
        r.session.set_param(id, v).unwrap();
    }
    r.session.wire_material("material", None).unwrap();
    r.session.wire_material("grain", None).unwrap();
    r.session.add_track(false).unwrap();
    let mut t = r.session.tracker();
    for track in [1, 2] {
        for (i, s) in t.tracks[track].steps.iter_mut().enumerate().take(16) {
            s.on = i % 2 == 0;
        }
    }
    r.session.set_tracker(t).unwrap();
    r.session.select_track(0).unwrap();

    let steady = worst_jump(&r, 0, 600, |_| {});
    assert!(steady > 1e-4, "the tracks sound ({steady})");
    let moved = worst_jump(&r, 53, 900, |n| {
        if n % 2 == 0 {
            r.session.remove_track(1).unwrap();
        } else {
            r.session.undo().unwrap();
        }
    });
    assert!(
        moved < steady * 2.0,
        "moving across materials jumped by {moved} against a steady {steady}"
    );
}

#[test]
fn the_callbacks_hand_offs_stay_off_the_allocator_with_every_track() {
    // The guard counts in this test binary too: `lib.rs` installs it.
    let ((), caught) = no_alloc(|| drop(std::hint::black_box(vec![0u8; 64])));
    assert!(caught >= 2, "the guard is not counting ({caught})");

    let r = rig();
    three_sounding(&r);
    while r.session.track_count() < VOICES {
        r.session.add_track(false).unwrap();
    }
    // Every kit too (story d), after the melody tracks.
    while r.session.tracker().drums().count() < shard_dsp::KITS {
        r.session.add_track(true).unwrap();
    }
    let drone = r.session.pool().materials[0].id;
    let mut e = Engine::new(48_000.0, 128);
    e.set_voice_active(0, false);
    e.set_playing(true);
    let mut out = vec![0.0f32; 512];
    let mut held = crate::Held::new();
    for block in 0..400 {
        // The command thread between blocks: tracks removed and put back,
        // patches switched, a material unwired under a sounding voice (whose
        // swap waits for it to fade), edits and fader drags.
        match block % 50 {
            // The sidechain (roadmap row 41): a melody track ducked by the
            // first drums track, that one Key only by turns, and track 1
            // keying track 0.
            3 => {
                r.session.set_track_duck(1, Some(VOICES)).unwrap();
                r.session.set_track_duck(0, Some(1)).unwrap();
                let mut t = r.session.tracker();
                let d = &mut t.tracks[VOICES].duck;
                d.key_only = !d.key_only;
                t.tracks[1].duck.depth = (block % 10) as f32 * 0.1;
                r.session.set_tracker(t).unwrap();
            }
            // A global LFO (roadmap row 43): linked from a new one, its rate
            // edited, removed and put back, every set crossing through the
            // callback's own globals slots.
            22 => r
                .session
                .link_new("fm.index", "global-lfo", 0.0, 0.3)
                .unwrap(),
            24 => {
                let mut g = r.session.globals()[0].clone();
                g.rate = 1.0 + (block % 7) as f32;
                r.session.set_lfo(g).unwrap();
            }
            26 => {
                let g = r.session.globals()[0].id;
                r.session.remove_lfo(g).unwrap();
            }
            28 => drop(r.session.undo().unwrap()),
            5 => r.session.wire_material("material", None).unwrap(),
            7 => r.session.wire_material("material", Some(drone)).unwrap(),
            10 => r.session.remove_track(2).unwrap(),
            20 => drop(r.session.undo().unwrap()),
            30 => r
                .session
                .set_param("grain.size", 40.0 + block as f32 * 0.1)
                .unwrap(),
            40 => {
                let id = r.session.patches()[block / 50 % 3].id;
                r.session.set_track_patch(block / 50 % VOICES, id).unwrap();
            }
            // A drums track's grid, sound and mute, and one removed and back.
            43 => {
                let mut t = r.session.tracker();
                let last = t.tracks.len() - 1;
                let kit = t.tracks[last].drums.as_mut().unwrap();
                kit.lanes[0].hits[block % 16] = 127;
                kit.tune = (block % 24) as f32 - 12.0;
                r.session.set_tracker(t).unwrap();
                r.session.toggle_mute(last).unwrap();
            }
            45 => r.session.remove_track(VOICES).unwrap(),
            47 => drop(r.session.undo().unwrap()),
            _ => {}
        }
        let ((), caught) = no_alloc(|| {
            crate::hand_over(&mut e, &r.voices, &r.arrangement, &mut held);
            crate::render(&mut e, &r.voices, &r.heard, &mut out);
        });
        assert_eq!(
            caught, 0,
            "block {block} touched the allocator {caught} time(s)"
        );
        // What `meters` frees, off the audio thread.
        r.voices.collect_retired();
    }
}
