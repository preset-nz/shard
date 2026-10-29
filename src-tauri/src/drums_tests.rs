//! Drums tracks (epic 28, story d): a track type of its own in the one
//! order, each on its own kit. Add and Remove are one step each, a drums
//! track never moves a melody track's voice, its mute and solo are a
//! track's, and a saved file carries every drums track as it was.

use std::sync::atomic::Ordering;

use shard_dsp::{Engine, Mix, KITS};

use crate::object_model as om;
use crate::session::{self, MixState};
use crate::session_tests::{rig, Rig};
use crate::tracker::{Kit, Slot, IN_ORDER};

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
    let (grid, sound, gain) = t.kit(1, &IN_ORDER).unwrap();
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
    kit.lanes[1].hits[6] = 90;
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
    t.tracks[1].drums.as_mut().unwrap().lanes[0].hits[3] = 127;
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
        k.lanes[2].hits[40] = 48;
    }
    t.tracks[1].length = 64;
    {
        let k = t.tracks[2].drums.as_mut().unwrap();
        k.tune = -7.0;
        k.lanes[0].hits[0] = 127;
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

/// Kit `k`'s lane changes waiting, taken as the callback takes them: which
/// slots are built (`Some(true)`), emptied (`Some(false)`) or untouched.
fn take_lane_swaps(r: &Rig, k: usize) -> Vec<Option<bool>> {
    let mut pending = r.voices.kits[k].lane_swap.lock().unwrap();
    pending
        .iter_mut()
        .map(|change| change.take().map(|lane| lane.is_some()))
        .collect()
}

/// The first drums track's lane nodes, in the grid's order.
fn lane_nodes(r: &Rig) -> Vec<rhizome_core::NodeId> {
    r.session.read(|d| {
        let drums = om::tracks(d.tree())
            .into_iter()
            .find(|t| t.type_name() == om::DRUMS)
            .unwrap();
        om::lanes(drums).iter().map(|l| l.id()).collect()
    })
}

#[test]
fn a_new_drums_track_is_written_with_four_lanes() {
    // A new session's and Add Track › Drums both: Kick, Snare, Hat closed
    // and Hat open, each a node in the track's order, the new session's
    // with today's beat on the first three.
    let r = rig();
    r.session.add_track(true).unwrap();
    let written = r.session.read(|d| {
        om::tracks(d.tree())
            .into_iter()
            .filter(|t| t.type_name() == om::DRUMS)
            .map(|t| {
                om::lanes(t)
                    .iter()
                    .map(|l| {
                        let text = |key| match l.value(key) {
                            Some(rhizome_core::Value::Text(s) | rhizome_core::Value::Choice(s)) => {
                                s
                            }
                            _ => String::new(),
                        };
                        (text(om::LANE_LABEL), text(om::LANE_KIND))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    });
    let four: Vec<(String, String)> = [
        ("Kick", "kick"),
        ("Snare", "snare"),
        ("Hat closed", "hat"),
        ("Hat open", "hat"),
    ]
    .iter()
    .map(|(l, k)| (l.to_string(), k.to_string()))
    .collect();
    assert_eq!(written, vec![four.clone(), four]);
    let t = r.session.tracker();
    let beat = t.tracks[1].drums.as_ref().unwrap();
    let p = shard_dsp::KitPattern::default();
    for (lane, slot) in beat.lanes.iter().zip(0..4) {
        assert_eq!(lane.hits[..], p.hits[slot][..], "{}", lane.label);
    }
    let added = t.tracks[2].drums.as_ref().unwrap();
    assert!(added.lanes.iter().all(|l| l.hits.iter().all(|h| *h == 0)));
    // Each lane built into its own slot, in order.
    assert_eq!(
        take_lane_swaps(&r, 1),
        [vec![Some(true); 4], vec![None; 4]].concat()
    );
}

#[test]
fn reordering_lanes_moves_no_slot_and_builds_nothing() {
    let r = rig();
    let before = r.voices.kits[0].bank.load().0;
    take_lane_swaps(&r, 0);
    let mut lanes = lane_nodes(&r);
    lanes.reverse();
    let drums = r.session.read(|d| om::tracks(d.tree())[1].id());
    r.session
        .edit("Move", None, |tx| tx.set_order(drums, om::LANES, lanes))
        .unwrap();
    let t = r.session.tracker();
    assert_eq!(
        t.tracks[1].drums.as_ref().unwrap().lanes[0].label,
        "Hat open",
        "the grid's order is the tree's"
    );
    assert_eq!(
        r.voices.kits[0].bank.load().0,
        before,
        "every hit in its slot"
    );
    assert_eq!(take_lane_swaps(&r, 0), vec![None; 8]);
    // The kick still keys what the track ducks, from its own slot.
    assert_eq!(r.voices.kits[0].key(), Some(0));
}

#[test]
fn removing_a_lane_empties_its_slot_and_rebuilds_nothing_else() {
    let r = rig();
    take_lane_swaps(&r, 0);
    let snare = lane_nodes(&r)[1];
    r.session
        .edit("Remove", None, |tx| tx.remove(snare))
        .unwrap();
    let mut want = vec![None; 8];
    want[1] = Some(false);
    assert_eq!(take_lane_swaps(&r, 0), want);
    let grid = r.voices.kits[0].bank.load().0;
    assert!(grid.hits[1].iter().all(|h| *h == 0), "its hits are gone");
    assert_eq!(grid.hits[2], shard_dsp::KitPattern::default().hits[2]);
    // A lane added later takes the free slot.
    r.session.undo().unwrap();
    assert_eq!(
        take_lane_swaps(&r, 0),
        want.iter().map(|w| w.map(|_| true)).collect::<Vec<_>>()
    );

    // Its track removed, every lane of its kit is let go.
    r.session.remove_track(1).unwrap();
    assert_eq!(
        take_lane_swaps(&r, 0),
        [vec![Some(false); 4], vec![None; 4]].concat()
    );
}

#[test]
fn a_hit_is_one_undo_step() {
    let r = rig();
    let before = r.session.tracker();
    let mut t = before.clone();
    t.tracks[1].drums.as_mut().unwrap().lanes[3].hits[5] = 90;
    let steps = r.session.history_labels().0.len();
    r.session.set_tracker(t).unwrap();
    assert_eq!(r.session.history_labels().0.len(), steps + 1);
    assert_eq!(r.voices.kits[0].bank.load().0.hits[3][5], 90);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Edit the steps"));
    assert_eq!(r.session.tracker(), before);
    assert_eq!(r.voices.kits[0].bank.load().0.hits[3][5], 0);
}

#[test]
fn a_removed_tracks_lanes_ring_out_before_they_go() {
    // Through the callback's hand-offs: a slot emptied waits while its kit
    // still sounds, then is let go off the audio thread.
    let r = rig();
    let mut t = r.session.tracker();
    t.tracks[1].drums.as_mut().unwrap().on = true;
    r.session.set_tracker(t).unwrap();
    let mut e = Engine::new(48_000.0, 64);
    e.set_playing(true);
    let mut out = vec![0.0f32; 512];
    let mut held = crate::Held::new();
    while !e.kit_sounding(0) {
        r.block(&mut e, &mut out, &mut held);
    }
    r.session.remove_track(1).unwrap();
    r.block(&mut e, &mut out, &mut held);
    assert!(e.kit_sounding(0), "still ringing");
    let waiting = |r: &Rig| {
        r.voices.kits[0]
            .lane_swap
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.is_some())
            .count()
    };
    assert_eq!(waiting(&r), 4, "its four lanes wait");
    for _ in 0..2000 {
        if !e.kit_sounding(0) && waiting(&r) == 0 {
            break;
        }
        r.block(&mut e, &mut out, &mut held);
    }
    assert_eq!(waiting(&r), 0, "and go once it is quiet");
}

/// The first drums track's lanes as the grid shows them: name and kind.
fn lane_names(r: &Rig) -> Vec<(String, String)> {
    let t = r.session.tracker();
    let kit = t.tracks[1].drums.as_ref().unwrap();
    kit.lanes
        .iter()
        .map(|l| (l.label.clone(), l.kind.clone()))
        .collect()
}

fn labels(r: &Rig) -> Vec<String> {
    lane_names(r).into_iter().map(|(l, _)| l).collect()
}

#[test]
fn add_lane_names_from_the_list_and_stops_at_eight() {
    let r = rig();
    take_lane_swaps(&r, 0);
    let steps = r.session.history_labels().0.len();
    assert_eq!(r.session.add_lane(1).unwrap(), 4);
    assert_eq!(r.session.history().0.as_deref(), Some("Add Lane"));
    assert_eq!(r.session.history_labels().0.len(), steps + 1);
    assert_eq!(
        lane_names(&r)[4],
        ("Clap".to_string(), "material".to_string()),
        "silent until a material is assigned"
    );
    // Built into the free slot, nothing else touched.
    let mut want = vec![None; 8];
    want[4] = Some(true);
    assert_eq!(take_lane_swaps(&r, 0), want);

    for _ in 0..3 {
        r.session.add_lane(1).unwrap();
    }
    assert_eq!(
        labels(&r)[4..],
        ["Clap", "Tom low", "Tom high", "Crash"].map(String::from)
    );
    assert!(r.session.add_lane(1).is_err(), "a kit holds eight");
    assert_eq!(labels(&r).len(), 8);
    assert!(
        r.session.add_lane(0).is_err(),
        "a melody track has no lanes"
    );

    // A name in use is skipped, and past the list a lane is numbered.
    r.session.remove_lane(1, 0).unwrap();
    assert_eq!(r.session.add_lane(1).unwrap(), 7);
    assert_eq!(labels(&r)[7], "Lane 8");
    r.session.rename_lane(1, 7, "Clap").unwrap();
    r.session.remove_lane(1, 0).unwrap();
    assert_eq!(r.session.add_lane(1).unwrap(), 7);
    assert_eq!(labels(&r)[7], "Lane 8");
}

#[test]
fn remove_lane_and_undo_give_back_its_hits_and_slot() {
    let r = rig();
    take_lane_swaps(&r, 0);
    let before = r.session.tracker();
    let hits = r.voices.kits[0].bank.load().0;
    r.session.remove_lane(1, 0).unwrap();
    assert_eq!(r.session.history().0.as_deref(), Some("Remove Kick"));
    assert_eq!(labels(&r), ["Snare", "Hat closed", "Hat open"]);
    assert!(r.voices.kits[0].bank.load().0.hits[0]
        .iter()
        .all(|h| *h == 0));
    // The lane after it keeps its slot.
    assert_eq!(
        r.voices.kits[0].bank.load().0.hits[1],
        hits.hits[1],
        "the snare plays from where it was"
    );
    take_lane_swaps(&r, 0);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Remove Kick"));
    assert_eq!(r.session.tracker(), before);
    assert_eq!(r.voices.kits[0].bank.load().0, hits);
    assert_eq!(r.voices.kits[0].key(), Some(0), "the kick keys again");
    let mut built = vec![None; 8];
    built[0] = Some(true);
    assert_eq!(take_lane_swaps(&r, 0), built);
}

#[test]
fn move_lane_is_one_named_step_and_rebuilds_nothing() {
    let r = rig();
    take_lane_swaps(&r, 0);
    let hits = r.voices.kits[0].bank.load().0;
    let steps = r.session.history_labels().0.len();
    assert_eq!(r.session.move_lane(1, 0, 1).unwrap(), Some(1));
    assert_eq!(r.session.history().0.as_deref(), Some("Move Kick"));
    assert_eq!(r.session.history_labels().0.len(), steps + 1);
    assert_eq!(labels(&r)[..2], ["Snare", "Kick"].map(String::from));
    assert_eq!(
        r.voices.kits[0].bank.load().0,
        hits,
        "every hit in its slot"
    );
    assert_eq!(take_lane_swaps(&r, 0), vec![None; 8]);
    assert_eq!(r.voices.kits[0].key(), Some(0));
    // An end stays put and makes no step.
    assert_eq!(r.session.move_lane(1, 3, 1).unwrap(), None);
    assert_eq!(r.session.move_lane(1, 0, -1).unwrap(), None);
    assert_eq!(r.session.history_labels().0.len(), steps + 1);
    r.session.undo().unwrap();
    assert_eq!(labels(&r)[..2], ["Kick", "Snare"].map(String::from));
}

#[test]
fn rename_lane_is_one_undo_step() {
    let r = rig();
    take_lane_swaps(&r, 0);
    let steps = r.session.history_labels().0.len();
    assert!(r.session.rename_lane(1, 2, "  Shaker ").unwrap());
    assert_eq!(labels(&r)[2], "Shaker");
    assert_eq!(r.session.history().0.as_deref(), Some("Rename Lane"));
    assert_eq!(r.session.history_labels().0.len(), steps + 1);
    assert!(
        !r.session.rename_lane(1, 2, "Shaker").unwrap(),
        "no change, no step"
    );
    assert!(r.session.rename_lane(1, 2, "  ").is_err());
    assert_eq!(r.session.history_labels().0.len(), steps + 1);
    assert_eq!(
        take_lane_swaps(&r, 0),
        vec![None; 8],
        "a name builds nothing"
    );
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Rename Lane"));
    assert_eq!(labels(&r)[2], "Hat closed");
}

/// A material on a lane (epic 49, story d): a mono 48 kHz WAV of distinct
/// samples in the pool, by its id.
fn add_wav(r: &Rig, name: &str) -> u64 {
    let dir = std::env::temp_dir().join(format!("shard-drums-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.wav"));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..4_800 {
        w.write_sample(((i as f32) * 0.013).sin() * 0.5 + (i % 7) as f32 * 0.01)
            .unwrap();
    }
    w.finalize().unwrap();
    r.session.add_material(path.to_str().unwrap()).unwrap();
    r.session
        .pool()
        .materials
        .iter()
        .find(|m| m.name == format!("{name}.wav"))
        .unwrap()
        .id
}

/// Kit `k`'s lane changes waiting, taken: each slot's new lane, an emptied
/// slot (`Some(None)`), or none.
fn take_lanes(r: &Rig, k: usize) -> Vec<Option<Option<Box<shard_dsp::kit::Lane>>>> {
    let mut pending = r.voices.kits[k].lane_swap.lock().unwrap();
    pending.iter_mut().map(Option::take).collect()
}

/// The slots of kit `k` with a lane built since the last look.
fn rebuilt(r: &Rig, k: usize) -> Vec<usize> {
    take_lane_swaps(r, k)
        .iter()
        .enumerate()
        .filter_map(|(v, c)| (*c == Some(true)).then_some(v))
        .collect()
}

/// What `lane` plays on a hit at full velocity, alone in a kit, `n` frames.
fn hit(lane: Box<shard_dsp::kit::Lane>, n: usize) -> Vec<f32> {
    let mut kit = shard_dsp::kit::Kit::new(48_000.0);
    kit.swap_lane(0, Some(lane));
    kit.hit(0, 1.0, &shard_dsp::KitParams::default());
    (0..n).map(|_| kit.process()).collect()
}

/// The built-in drone, a material with no file.
fn drone(r: &Rig) -> u64 {
    r.session
        .pool()
        .materials
        .iter()
        .find(|m| m.name == crate::materials::DRONE_NAME)
        .unwrap()
        .id
}

fn first_lane(r: &Rig) -> crate::tracker::KitLane {
    r.session.tracker().tracks[1].drums.as_ref().unwrap().lanes[0].clone()
}

#[test]
fn a_lane_set_to_a_material_plays_it_from_its_trim() {
    let r = rig();
    let id = add_wav(&r, "plays");
    let m = r.session.pool().get(id).cloned().unwrap();
    r.session
        .set_material(id, m.octave, 0.25, 0.75, m.root)
        .unwrap();
    take_lane_swaps(&r, 0);
    assert!(r
        .session
        .set_lane_plays(1, 0, "material", Some(id))
        .unwrap());
    assert_eq!(r.session.history().0.as_deref(), Some("Set Plays"));
    let lane = first_lane(&r);
    assert_eq!((lane.kind.as_str(), lane.material), ("material", Some(id)));
    // The same again is no step.
    let history = r.session.history_labels();
    assert!(!r
        .session
        .set_lane_plays(1, 0, "material", Some(id))
        .unwrap());
    assert_eq!(r.session.history_labels(), history);

    let mut lanes = take_lanes(&r, 0);
    assert!(lanes.iter().skip(1).all(Option::is_none), "only its slot");
    let built = lanes[0].take().flatten().expect("a lane built");
    assert_eq!(built.kind(), shard_dsp::kit::LaneKind::Material);
    let samples = r.session.loaded(id).unwrap().samples.clone();
    let (lo, hi) = shard_dsp::Reading {
        trim_start: 0.25,
        trim_end: 0.75,
        ..Default::default()
    }
    .window(samples.len());
    let out = hit(built, hi - lo + 100);
    for (i, s) in out[..hi - lo].iter().enumerate() {
        assert!((s - samples[lo + i]).abs() < 1e-6, "sample {i}");
    }
    assert!(out[hi - lo..].iter().all(|s| *s == 0.0), "then silence");
    // Its kick gone, nothing keys what the track ducks.
    assert_eq!(r.voices.kits[0].key(), None);
}

#[test]
fn changing_a_lanes_material_rebuilds_only_its_slot() {
    let r = rig();
    let a = add_wav(&r, "first");
    r.session
        .set_lane_plays(1, 0, "material", Some(drone(&r)))
        .unwrap();
    take_lane_swaps(&r, 0);
    r.session.set_lane_plays(1, 0, "material", Some(a)).unwrap();
    assert_eq!(rebuilt(&r, 0), [0]);
    r.session.set_lane_plays(1, 2, "material", Some(a)).unwrap();
    assert_eq!(rebuilt(&r, 0), [2]);
    // Back to a drum: the reference goes with it.
    r.session.set_lane_plays(1, 2, "snare", None).unwrap();
    assert_eq!(rebuilt(&r, 0), [2]);
    let lane = r.session.tracker().tracks[1].drums.clone().unwrap().lanes[2].clone();
    assert_eq!((lane.kind.as_str(), lane.material), ("snare", None));
    assert!(r.session.set_lane_plays(1, 2, "snare", Some(a)).is_err());
    assert!(r.session.set_lane_plays(1, 2, "material", None).is_err());
    assert!(
        r.session.set_lane_plays(0, 0, "kick", None).is_err(),
        "a melody track"
    );
}

#[test]
fn a_materials_trim_and_octave_rebuild_the_lanes_on_it_and_nothing_else() {
    let r = rig();
    let a = add_wav(&r, "trimmed");
    r.session.set_lane_plays(1, 0, "material", Some(a)).unwrap();
    r.session.set_lane_plays(1, 2, "material", Some(a)).unwrap();
    r.session
        .set_lane_plays(1, 1, "material", Some(drone(&r)))
        .unwrap();
    take_lane_swaps(&r, 0);
    let m = r.session.pool().get(a).cloned().unwrap();
    r.session
        .set_material(a, m.octave, 0.1, 0.6, m.root)
        .unwrap();
    assert_eq!(rebuilt(&r, 0), [0, 2]);
    // Its octave repitches the lanes on it too (Georg, 2026-10-30).
    r.session.set_material(a, 1.0, 0.1, 0.6, m.root).unwrap();
    assert_eq!(rebuilt(&r, 0), [0, 2]);
}

#[test]
fn removing_a_material_silences_its_lanes() {
    let r = rig();
    let a = add_wav(&r, "removed");
    r.session.set_lane_plays(1, 0, "material", Some(a)).unwrap();
    take_lane_swaps(&r, 0);
    r.session.remove_material(a).unwrap();
    assert_eq!(first_lane(&r).material, None);
    let mut lanes = take_lanes(&r, 0);
    let silent = lanes[0].take().flatten().expect("rebuilt, not emptied");
    assert_eq!(silent.kind(), shard_dsp::kit::LaneKind::Material);
    assert!(hit(silent, 1_000).iter().all(|s| *s == 0.0));
    assert!(lanes.iter().all(Option::is_none), "nothing else");
    // Undo brings the material back, and the lane plays it again.
    r.session.undo().unwrap();
    // (A node brought back by undo has a new runtime id.)
    assert!(first_lane(&r).material.is_some());
    let back = take_lanes(&r, 0)[0].take().flatten().unwrap();
    assert!(hit(back, 1_000).iter().any(|s| *s != 0.0));
}

#[test]
fn undoing_set_plays_brings_the_drum_back() {
    let r = rig();
    r.session
        .set_lane_plays(1, 0, "material", Some(drone(&r)))
        .unwrap();
    take_lane_swaps(&r, 0);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Plays"));
    let lane = first_lane(&r);
    assert_eq!((lane.kind.as_str(), lane.material), ("kick", None));
    let kick = take_lanes(&r, 0)[0].take().flatten().unwrap();
    assert_eq!(kick.kind(), shard_dsp::kit::LaneKind::Kick);
    assert_eq!(r.voices.kits[0].key(), Some(0), "the kick keys again");
    // Redo: the drone, which the pool holds as a material with no file.
    r.session.redo().unwrap();
    let drone = take_lanes(&r, 0)[0].take().flatten().unwrap();
    assert!(hit(drone, 1_000).iter().any(|s| *s != 0.0));
}

#[test]
fn back_to_kick_through_the_picker_and_a_stale_material_is_still_set_tune() {
    let r = rig();
    r.session
        .set_lane_plays(1, 0, "material", Some(drone(&r)))
        .unwrap();
    take_lane_swaps(&r, 0);
    assert!(r.session.set_lane_plays(1, 0, "kick", None).unwrap());
    let lane = first_lane(&r);
    assert_eq!((lane.kind.as_str(), lane.material), ("kick", None));
    let kick = take_lanes(&r, 0)[0].take().flatten().unwrap();
    assert_eq!(kick.kind(), shard_dsp::kit::LaneKind::Kick);
    assert_eq!(r.voices.kits[0].key(), Some(0));
    let history = r.session.history_labels();
    assert!(!r.session.set_lane_plays(1, 0, "kick", None).unwrap());
    assert_eq!(r.session.history_labels(), history);

    // What a lane plays is never `set_tracker`'s, so a tracker read before
    // a pick still drags Tune as one "Set Tune".
    let mut t = r.session.tracker();
    t.tracks[1].drums.as_mut().unwrap().lanes[0].material = Some(u64::MAX);
    for tune in [1.0, 2.0] {
        t.tracks[1].drums.as_mut().unwrap().tune = tune;
        r.session.set_tracker(t.clone()).unwrap();
    }
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Tune"));
    assert_eq!(r.voices.kits[0].bank.load().1.tune, 0.0, "one step");
}
