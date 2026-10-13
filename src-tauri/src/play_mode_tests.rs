//! What Play plays (Georg, 2026-10-13). `Loop` and `Once` voice the current
//! patch alone on `AUDITION`, steps off, at unity, and nothing else: every
//! other voice and kit is handed nothing. A session choice, never an edit.

use std::sync::atomic::Ordering;

use shard_dsp::{Alone, Mix, KITS, VOICES};

use crate::session::{PlayMode, AUDITION};
use crate::session_tests::{melody_rig as rig, Rig};

fn active(r: &Rig, i: usize) -> bool {
    r.voice(i).active.load(Ordering::Relaxed)
}

fn kit_active(r: &Rig, k: usize) -> bool {
    r.voices.kits[k].active.load(Ordering::Relaxed)
}

fn size(r: &Rig, voice: usize) -> f32 {
    r.voice(voice).bank.get_by_id("grain.size").unwrap()
}

/// Two tracks on two patches, track 2's patch current, a drums track too.
fn song(r: &Rig) {
    r.session.set_param("grain.size", 80.0).unwrap();
    r.session.new_patch().unwrap();
    r.session.set_param("grain.size", 120.0).unwrap();
    r.session.add_track(false).unwrap();
    r.session.add_track(true).unwrap();
}

#[test]
fn song_is_the_default_and_plays_every_track() {
    let r = rig();
    song(&r);
    assert_eq!(r.session.play_mode(), PlayMode::Song);
    assert_eq!(r.voices.alone.load(Ordering::Relaxed), Alone::Song.bits());
    assert!(active(&r, 0) && active(&r, 1) && !active(&r, AUDITION));
    assert!(kit_active(&r, 0));
}

#[test]
fn alone_voices_the_current_patch_on_audition_even_when_a_track_plays_it() {
    for mode in [PlayMode::Loop, PlayMode::Once] {
        let r = rig();
        song(&r);
        // Track 2 plays the current patch, and is selected.
        r.session.select_track(1).unwrap();
        r.session.set_mute(1, true).unwrap();
        r.session.set_solo(0, true).unwrap();
        r.session.set_play_mode(mode);

        assert_eq!(r.voices.alone.load(Ordering::Relaxed), mode.alone().bits());
        for i in 0..VOICES {
            assert_eq!(active(&r, i), i == AUDITION, "{mode:?}: voice {i}");
        }
        assert!(
            (0..KITS).all(|k| !kit_active(&r, k)),
            "{mode:?}: a kit plays"
        );
        assert_eq!(size(&r, AUDITION), 120.0, "{mode:?}: the current patch");
        let steps = r.voice(AUDITION).steps.load();
        assert!(!steps.on, "{mode:?}: steps are off");
        assert_eq!(r.voice(AUDITION).gain(), 1.0, "{mode:?}: unity");
        assert_eq!(r.voices.shown(), Some(AUDITION));
        // No track's mute or solo reaches it: it is safe from solos.
        let mix = Mix::from_bits(r.voice(AUDITION).mix.load(Ordering::Relaxed));
        assert!(!mix.mute && !mix.solo && mix.solo_safe, "{mode:?}: {mix:?}");

        // Selecting another patch moves it; selecting a track too.
        let first = r.session.patches()[0].id;
        r.session.select_patch(first).unwrap();
        assert_eq!(size(&r, AUDITION), 80.0);
        assert!(active(&r, AUDITION) && !active(&r, 0) && !active(&r, 1));
        r.session.select_track(0).unwrap();
        assert!(active(&r, AUDITION) && !active(&r, 0));

        // Back to the song: the tracks play again, the audition is gone.
        r.session.set_play_mode(PlayMode::Song);
        assert!(active(&r, 0) && active(&r, 1) && !active(&r, AUDITION));
        assert!(kit_active(&r, 0));
        assert_eq!(size(&r, 0), 80.0);
        assert_eq!(size(&r, 1), 120.0);
    }
}

#[test]
fn alone_plays_even_with_every_voice_on_a_track() {
    let r = rig();
    while r.session.track_count() < VOICES {
        r.session.add_track(false).unwrap();
    }
    r.session.set_play_mode(PlayMode::Loop);
    assert!(active(&r, AUDITION));
    assert!((0..AUDITION).all(|i| !active(&r, i)));
    assert!(!r.voice(AUDITION).steps.load().on);
}

#[test]
fn the_mode_is_no_edit_no_unsaved_mark_and_not_in_the_file() {
    let r = rig();
    song(&r);
    let dir = std::env::temp_dir().join(format!("shard-playmode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mode.shard");
    r.session.save(&path).unwrap();
    let history = r.session.history_labels();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!r.session.is_unsaved());

    r.session.set_play_mode(PlayMode::Once);
    r.session.set_play_mode(PlayMode::Loop);
    assert_eq!(r.session.history_labels(), history, "the mode made a step");
    assert!(
        !r.session.is_unsaved(),
        "the mode marked the session unsaved"
    );
    r.session.save(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);

    // It is the session's: a new document and an open leave it be.
    r.session.new_document(2).unwrap();
    assert_eq!(r.session.play_mode(), PlayMode::Loop);
    assert!(active(&r, AUDITION) && !active(&r, 0));
    r.session.open(&path).unwrap();
    assert_eq!(r.session.play_mode(), PlayMode::Loop);
    assert!(active(&r, AUDITION) && !active(&r, 0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_mode_serialises_as_a_lowercase_word() {
    for (mode, word) in [
        (PlayMode::Song, "\"song\""),
        (PlayMode::Loop, "\"loop\""),
        (PlayMode::Once, "\"once\""),
    ] {
        assert_eq!(serde_json::to_string(&mode).unwrap(), word);
        assert_eq!(serde_json::from_str::<PlayMode>(word).unwrap(), mode);
    }
}

#[test]
fn through_the_callbacks_hand_offs_once_ends_by_itself_and_loop_does_not() {
    use shard_dsp::Engine;

    for (mode, ends) in [(PlayMode::Once, true), (PlayMode::Loop, false)] {
        let r = rig();
        song(&r);
        r.session.set_play_mode(mode);
        let mut e = Engine::new(48_000.0, 64);
        let mut held = crate::Held::new();
        let mut out = vec![0.0f32; 2048];
        // Play: the first block wakes the voice, then the pass runs.
        r.block(&mut e, &mut out, &mut held);
        e.set_playing(true);
        let mut heard = 0.0f32;
        let mut blocks = 0;
        while e.playing() && blocks < 3_000 {
            r.block(&mut e, &mut out, &mut held);
            heard = out.iter().fold(heard, |m, s| m.max(s.abs()));
            blocks += 1;
        }
        assert_eq!(!e.playing(), ends, "{mode:?}: after {blocks} blocks");
        assert!(heard > 0.01, "{mode:?}: the patch was silent");
        if ends {
            r.block(&mut e, &mut out, &mut held);
            assert!(out.iter().all(|s| *s == 0.0));
        }
    }
}
