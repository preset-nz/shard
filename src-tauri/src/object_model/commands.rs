//! Shard's own POM commands: whole-track writes in the shape a model, a
//! script or a future CLI sends (`design/agent-tools.md`, "a tool is a
//! command"). Each runs against a `Document<Shard>` alone, with no session,
//! engine or webview, so the same payload does the same thing in the app's
//! agent panel, in a draft document and headless.
//!
//! - `shard.read_song`: the tempo, swing and every track, as JSON text.
//! - `shard.set_drums`: one drums track's whole grid, from General MIDI
//!   notes or lane names.
//! - `shard.set_melody`: one melody track's whole line, from MIDI notes.
//! - `shard.set_tempo`: tempo, and swing as a percentage.
//!
//! **A payload names its track by place, or `"new"`.** What the selected
//! track is belongs to the session, not the document, so a caller that wants
//! "the selected one" resolves it first. Left out, a write makes a new track.
//!
//! **A refusal is text for whoever sent it.** Every command is always
//! enabled, and refuses inside `run` with `Error::Payload`, whose message
//! says what to send instead: POM's `Disabled` carries only the id. Values
//! out of range are brought in, never refused, and the answer says so.

use rhizome_core::{Edit, NodeId, Ref};
use rhizome_pom::{Commands, Document, Error as PomError, Outcome};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use shard_dsp::params::PARAMS;
use shard_dsp::steps::{HOLD_MAX, PITCH_RANGE, STEPS, VELOCITY_MAX};
use shard_dsp::{KITS, VOICES};

use super::{arrangement_node, spec, to_value, Shard, ShardEdit, TRACK_PATCH};
use crate::tracker::{Kit, Step, Track, Tracker, LENGTHS, SWING_MAX};

pub const READ_SONG: &str = "shard.read_song";
pub const SET_DRUMS: &str = "shard.set_drums";
pub const SET_MELODY: &str = "shard.set_melody";
pub const SET_TEMPO: &str = "shard.set_tempo";

/// The note the built-in drone sounds at: A2, its lowest partial
/// (`source::drone`).
pub const DRONE_ROOT: i32 = 45;
/// The note a sample with no root is taken to sound at.
const UNKNOWN_ROOT: i32 = 60;

/// The velocity a hit or note sent without one plays at.
const DEFAULT_VELOCITY: u8 = 100;

/// Registers Shard's commands beside POM's built-in ones.
pub fn register(c: &mut Commands<Shard>) {
    let always = |_: &Document<Shard>, _: &Json| true;
    c.add(
        READ_SONG,
        |_| "Read the Song".into(),
        always,
        |d, _| Ok(Outcome::Text(song(d).to_string())),
    );
    c.add(
        SET_DRUMS,
        |_| "Set Drums".into(),
        always,
        |d, p| {
            set_drums(d, p)
                .map(Outcome::Text)
                .map_err(PomError::Payload)
        },
    );
    c.add(
        SET_MELODY,
        |_| "Set Melody".into(),
        always,
        |d, p| {
            set_melody(d, p)
                .map(Outcome::Text)
                .map_err(PomError::Payload)
        },
    );
    c.add(
        SET_TEMPO,
        |_| "Set Tempo".into(),
        always,
        |d, p| {
            set_tempo(d, p)
                .map(Outcome::Text)
                .map_err(PomError::Payload)
        },
    );
}

// Payloads. Forgiving: models write `midi_note` and `duration` unprompted.

/// Which track a write goes to: a place, or a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Place(usize),
    New,
}

/// `track` as a payload holds it: a number, `"new"`, or a number as text.
pub fn target(p: &Json) -> Result<Target, String> {
    match p.get("track") {
        None | Some(Json::Null) => Ok(Target::New),
        Some(Json::Number(n)) => n
            .as_u64()
            .map(|n| Target::Place(n as usize))
            .ok_or_else(|| format!("track {n} is not a place; tracks count from 0")),
        Some(Json::String(s)) if s.trim().eq_ignore_ascii_case("new") => Ok(Target::New),
        Some(Json::String(s)) => s
            .trim()
            .parse::<usize>()
            .map(Target::Place)
            .map_err(|_| format!("track \"{s}\" is neither a place (0, 1, …) nor \"new\"")),
        Some(other) => Err(format!(
            "track {other} is neither a place (0, 1, …) nor \"new\""
        )),
    }
}

/// A number the model may send as an integer, a float or a string.
fn number(v: &Json) -> Option<f64> {
    match v {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f: &f64| f.is_finite())
}

#[derive(Debug, Deserialize)]
struct Hit {
    step: Json,
    #[serde(default, alias = "note")]
    midi_note: Option<Json>,
    #[serde(default, alias = "drum", alias = "voice", alias = "instrument")]
    lane: Option<String>,
    #[serde(default)]
    velocity: Option<Json>,
}

#[derive(Debug, Deserialize)]
struct Note {
    step: Json,
    #[serde(alias = "midi_note", alias = "pitch")]
    note: Json,
    #[serde(default)]
    velocity: Option<Json>,
    #[serde(default)]
    hold: Option<Json>,
    /// In quarter notes, as the eval's shape has it: 0.25 is one step.
    #[serde(default)]
    duration: Option<Json>,
}

fn list<T: for<'de> Deserialize<'de>>(p: &Json, key: &str) -> Result<Vec<T>, String> {
    let v = p
        .get(key)
        .ok_or_else(|| format!("missing \"{key}\": send the whole track as a list"))?;
    serde_json::from_value(v.clone()).map_err(|e| format!("\"{key}\" doesn't read: {e}"))
}

/// The length a payload asks for, snapped to one a track can have, or the
/// shortest that holds `last`, the last step written, or `keep`.
fn length_of(p: &Json, last: Option<usize>, keep: u32) -> u32 {
    let snap = |n: u32| {
        LENGTHS
            .iter()
            .copied()
            .min_by_key(|l| l.abs_diff(n))
            .unwrap_or(16)
    };
    match p.get("length").and_then(number) {
        Some(n) => snap(n.max(0.0).round() as u32),
        None => {
            let need = last.map_or(0, |s| s as u32 + 1);
            if need <= keep {
                keep
            } else {
                LENGTHS
                    .iter()
                    .copied()
                    .find(|l| *l >= need)
                    .unwrap_or(STEPS as u32)
            }
        }
    }
}

fn velocity(v: Option<&Json>) -> u8 {
    v.and_then(number)
        .map_or(DEFAULT_VELOCITY, |n| n.round().clamp(0.0, 127.0) as u8)
        .min(VELOCITY_MAX)
}

/// A step as sent, if it is one: a whole number from 0.
fn step_of(v: &Json) -> Option<usize> {
    number(v)
        .filter(|n| *n >= 0.0 && n.fract() == 0.0)
        .map(|n| n as usize)
}

// Drums.

/// The lane a General MIDI drum note plays on, if any.
pub fn lane_of_note(note: i64) -> Option<usize> {
    match note {
        35 | 36 => Some(0),
        37..=40 => Some(1),
        42 | 44 | 46 | 49..=59 => Some(2),
        _ => None,
    }
}

fn lane_of_name(name: &str) -> Option<usize> {
    let n = name.trim().to_ascii_lowercase();
    if n.contains("kick") || n.contains("bass drum") || n == "bd" {
        Some(0)
    } else if n.contains("snare") || n.contains("clap") || n.contains("rim") || n == "sd" {
        Some(1)
    } else if n.contains("hat")
        || n.contains("hh")
        || n.contains("cymbal")
        || n.contains("ride")
        || n.contains("crash")
    {
        Some(2)
    } else {
        None
    }
}

const LANES: [&str; 3] = ["kick", "snare", "hat"];

fn set_drums(d: &mut Document<Shard>, p: &Json) -> Result<String, String> {
    let hits: Vec<Hit> = list(p, "hits")?;
    let target = target(p)?;
    let now = d.projection().tracker.clone();
    let place = match target {
        Target::Place(i) => {
            let t = now.tracks.get(i).ok_or_else(|| no_track(&now, i))?;
            if !t.is_drums() {
                return Err(format!(
                    "track {i} is a melody track; use set_melody for it, or leave track out to write a new drums track"
                ));
            }
            Some(i)
        }
        Target::New => {
            if now.drums().count() >= KITS {
                return Err(format!(
                    "the song already has {KITS} drums tracks, the most it holds; write over one of them by its track number"
                ));
            }
            None
        }
    };

    let mut grid = [[0u8; STEPS]; 3];
    let (mut ignored_notes, mut bad, mut odd_lanes, mut past) =
        (Vec::new(), 0usize, 0usize, Vec::new());
    let mut written = Vec::new();
    for h in &hits {
        let Some(step) = step_of(&h.step) else {
            bad += 1;
            continue;
        };
        let by_name = h.lane.as_deref().and_then(lane_of_name);
        let note = h
            .midi_note
            .as_ref()
            .and_then(number)
            .map(|n| n.round() as i64);
        let lane = match (by_name, note) {
            (Some(l), _) => Some(l),
            (None, Some(n)) => {
                let l = lane_of_note(n);
                if l.is_none() {
                    ignored_notes.push(n);
                }
                l
            }
            (None, None) if h.lane.is_some() => {
                odd_lanes += 1;
                None
            }
            (None, None) => {
                bad += 1;
                None
            }
        };
        let Some(lane) = lane else { continue };
        if step >= STEPS {
            past.push(step);
            continue;
        }
        let v = velocity(h.velocity.as_ref());
        if v > 0 {
            grid[lane][step] = grid[lane][step].max(v);
            written.push(step);
        }
    }
    let keep = place.map_or(16, |i| now.tracks[i].length);
    let length = length_of(p, written.iter().max().copied(), keep);
    let cut = written.iter().filter(|s| **s as u32 >= length).count();
    for row in &mut grid {
        for hit in row.iter_mut().skip(length as usize) {
            *hit = 0;
        }
    }

    let kit_of = |old: Option<&Kit>| Kit {
        on: true,
        kick: grid[0].to_vec(),
        snare: grid[1].to_vec(),
        hat: grid[2].to_vec(),
        ..old.cloned().unwrap_or_else(Kit::blank)
    };
    let arr = arr_of(d)?;
    let place = match place {
        Some(i) => {
            let mut next = now.clone();
            let t = &mut next.tracks[i];
            t.drums = Some(kit_of(t.drums.as_ref()));
            t.length = length;
            let next = next.sanitised();
            d.edit("Set Drums", |tx| tx.write_tracker(arr, &next))
                .map_err(|e| e.to_string())?;
            i
        }
        None => {
            let track = Track {
                length,
                drums: Some(kit_of(None)),
                ..Track::blank()
            };
            d.edit("Set Drums", |tx| tx.add_track(arr, &track).map(|_| ()))
                .map_err(|e| e.to_string())?;
            now.tracks.len()
        }
    };

    let counts: Vec<String> = (0..3)
        .map(|l| {
            let n = grid[l].iter().filter(|h| **h > 0).count();
            format!("{} {n}", LANES[l])
        })
        .collect();
    let mut out = format!(
        "drums track {place} written, switched on: {length} steps, {}",
        counts.join(", ")
    );
    ignored_notes.sort_unstable();
    ignored_notes.dedup();
    let notes: Vec<String> = ignored_notes.iter().map(i64::to_string).collect();
    if !notes.is_empty() {
        out += &format!(
            ". Ignored notes {} (only kick 35/36, snare 37-40, hats and cymbals 42/44/46/49-59 play)",
            notes.join(", ")
        );
    }
    if odd_lanes > 0 {
        out += &format!(". Ignored {odd_lanes} hits on a drum that isn't kick, snare or hat");
    }
    if bad > 0 {
        out += &format!(". Skipped {bad} hits without a step or a drum");
    }
    if cut > 0 || !past.is_empty() {
        out += &format!(
            ". Dropped {} hits past the track's {length} steps",
            cut + past.len()
        );
    }
    if target == Target::New {
        // Without this a model revising its answer writes another new track.
        out += &format!(". To change it, write over track {place}");
    }
    Ok(out)
}

// Melody.

/// A MIDI note's frequency.
pub fn midi_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

/// The MIDI note nearest a frequency.
pub fn hz_midi(hz: f32) -> i32 {
    (69.0 + 12.0 * (hz / 440.0).log2()).round() as i32
}

fn value(p: &crate::object_model::PatchPlan, id: &str) -> Option<f32> {
    PARAMS
        .iter()
        .position(|d| d.id == id)
        .and_then(|i| p.values.get(i).copied())
}

/// The note a step at pitch 0 sounds on a patch: FM's frequency when FM
/// plays, else the wired material's root (the drone's A2, or C4 for a sample
/// with none), moved by the material's octave. Answers whether it is a guess.
pub fn root_of(d: &Document<Shard>, patch: Option<NodeId>) -> (i32, bool) {
    let plan = d.projection();
    let Some(p) = patch.and_then(|n| plan.patch_plan(n)) else {
        return (DRONE_ROOT, false);
    };
    if p.generators.contains(&"fm") && value(p, "fm.on").unwrap_or(0.0) > 0.5 {
        return (hz_midi(value(p, "fm.freq").unwrap_or(110.0)), false);
    }
    let Some(m) = p.wires.material.and_then(|id| plan.pool.get(id)) else {
        return (DRONE_ROOT, false);
    };
    let octave = 12 * m.octave.round() as i32;
    match (m.root, &m.path) {
        (Some(r), _) => (i32::from(r) + octave, false),
        (None, None) => (DRONE_ROOT + octave, false),
        (None, Some(_)) => (UNKNOWN_ROOT + octave, true),
    }
}

/// `note − root`, folded by octaves into the pitch range, and whether it
/// had to be.
fn pitch_from(note: i32, root: i32) -> (i8, bool) {
    let range = i32::from(PITCH_RANGE);
    let mut p = note - root;
    let folded = !(-range..=range).contains(&p);
    while p > range {
        p -= 12;
    }
    while p < -range {
        p += 12;
    }
    (p as i8, folded)
}

/// The root a new track's patch gets: the lowest note when the line spans
/// two octaves or less, else its middle; within FM's frequency range.
fn new_root(notes: &[i32]) -> i32 {
    let (Some(lo), Some(hi)) = (notes.iter().min(), notes.iter().max()) else {
        return DRONE_ROOT;
    };
    let root = if hi - lo <= i32::from(PITCH_RANGE) {
        *lo
    } else {
        (lo + hi) / 2
    };
    // fm.freq runs 20 Hz to 2 kHz: MIDI 16 to 95.
    root.clamp(16, 95)
}

/// What a new melody track's patch plays: an FM voice at `root`, notes held
/// as long as their steps say. Settings from the pattern-to-shard skill's
/// bass, the one voice the first listening tests had.
fn voice_rows(root: i32) -> [(&'static str, f32); 5] {
    [
        ("fm.on", 1.0),
        ("fm.freq", midi_hz(root as f32)),
        ("fm.ratio", 1.0),
        ("fm.index", 2.4),
        // Hold: a step's hold is its length.
        ("patch.length", 2.0),
    ]
}

/// Writes patch row `id` on `patch`, making its section when absent, the
/// value brought into range as `Session::set_param` does.
fn put_row(tx: &mut Edit<'_>, patch: NodeId, id: &str, v: f32) -> rhizome_core::Result<()> {
    let prefix = id.split('.').next().unwrap_or(id);
    let node = if prefix == super::PATCH {
        patch
    } else {
        tx.section(patch, prefix)?
    };
    let Some(def) = PARAMS.iter().find(|p| p.id == id) else {
        return Ok(());
    };
    match to_value(&spec(def), v) {
        Some(want) => tx.put(node, id, want),
        None => Ok(()),
    }
}

fn set_melody(d: &mut Document<Shard>, p: &Json) -> Result<String, String> {
    // The eval's shape called them notes.
    let key = if p.get("steps").is_none() && p.get("notes").is_some() {
        "notes"
    } else {
        "steps"
    };
    let notes: Vec<Note> = list(p, key)?;
    let target = target(p)?;
    let now = d.projection().tracker.clone();
    let place = match target {
        Target::Place(i) => {
            let t = now.tracks.get(i).ok_or_else(|| no_track(&now, i))?;
            if t.is_drums() {
                return Err(format!(
                    "track {i} is a drums track; use set_drums for it, or leave track out to write a new melody track"
                ));
            }
            Some(i)
        }
        Target::New => {
            if now.melody().count() >= VOICES {
                return Err(format!(
                    "the song already has {VOICES} melody tracks, the most it holds; write over one of them by its track number"
                ));
            }
            None
        }
    };

    let mut bad = 0usize;
    let mut read: Vec<(usize, i32, u8, u8)> = Vec::new();
    for n in &notes {
        let (Some(step), Some(note)) = (step_of(&n.step), number(&n.note)) else {
            bad += 1;
            continue;
        };
        let hold = match (
            n.hold.as_ref().and_then(number),
            n.duration.as_ref().and_then(number),
        ) {
            (Some(h), _) => h.round(),
            (None, Some(q)) => (q * 4.0).round(),
            (None, None) => 1.0,
        }
        .clamp(1.0, f64::from(HOLD_MAX)) as u8;
        read.push((
            step,
            note.round() as i32,
            velocity(n.velocity.as_ref()),
            hold,
        ));
    }
    let past = read.iter().filter(|r| r.0 >= STEPS).count();
    read.retain(|r| r.0 < STEPS);
    let keep = place.map_or(16, |i| now.tracks[i].length);
    let length = length_of(p, read.iter().map(|r| r.0).max(), keep);
    let cut = read.iter().filter(|r| r.0 as u32 >= length).count();
    read.retain(|r| (r.0 as u32) < length);

    let root = match place {
        Some(i) => root_of(d, d.projection().track_patches.get(i).copied().flatten()),
        None => (
            new_root(&read.iter().map(|r| r.1).collect::<Vec<_>>()),
            false,
        ),
    };
    let mut steps = vec![Step::default(); STEPS];
    let (mut folded, mut doubled) = (0usize, 0usize);
    for &(step, note, velocity, hold) in &read {
        let (pitch, f) = pitch_from(note, root.0);
        folded += usize::from(f);
        if steps[step].on {
            doubled += 1;
        }
        steps[step] = Step {
            on: velocity > 0,
            pitch,
            hold,
            velocity,
            nudge: 0,
        };
    }
    let count = steps.iter().filter(|s| s.on).count();

    let arr = arr_of(d)?;
    let place = match place {
        Some(i) => {
            let mut next = now.clone();
            next.tracks[i].steps = steps;
            next.tracks[i].length = length;
            let next = next.sanitised();
            d.edit("Set Melody", |tx| tx.write_tracker(arr, &next))
                .map_err(|e| e.to_string())?;
            i
        }
        None => {
            let track = Track {
                length,
                steps,
                ..Track::blank()
            };
            let r = root.0;
            d.edit("Set Melody", |tx| {
                // Its own patch, so a sound for it never changes a track
                // already playing: the material taken out, FM in.
                let patch = tx.new_patch()?;
                let material = tx
                    .at(patch)
                    .and_then(|n| n.child("material"))
                    .map(|n| n.id());
                if let Some(m) = material {
                    tx.remove(m)?;
                }
                for (id, v) in voice_rows(r) {
                    put_row(tx, patch, id, v)?;
                }
                let node = tx.add_track(arr, &track)?;
                tx.set_ref(node, TRACK_PATCH, Ref::here(patch))
            })
            .map_err(|e| e.to_string())?;
            now.tracks.len()
        }
    };

    let mut out = format!(
        "melody track {place} written: {length} steps, {count} notes, root note {}{}",
        root.0,
        if target == Target::New {
            ", on a new FM patch of its own"
        } else {
            ""
        }
    );
    if root.1 {
        out += " (the sample's root isn't known, so C4 is assumed)";
    }
    if folded > 0 {
        out += &format!(
            ". Moved {folded} notes by octaves to within {PITCH_RANGE} semitones of the root"
        );
    }
    if doubled > 0 {
        out += &format!(
            ". {doubled} steps had two notes; a track plays one at a time, so the later one stayed"
        );
    }
    if bad > 0 {
        out += &format!(". Skipped {bad} steps without a step number or a note");
    }
    if cut + past > 0 {
        out += &format!(
            ". Dropped {} notes past the track's {length} steps",
            cut + past
        );
    }
    if target == Target::New {
        // Without this a model revising its answer writes another new track.
        out += &format!(". To change it, write over track {place}");
    }
    Ok(out)
}

// Tempo.

fn set_tempo(d: &mut Document<Shard>, p: &Json) -> Result<String, String> {
    let bpm = p
        .get("bpm")
        .and_then(number)
        .ok_or_else(|| "missing \"bpm\": the tempo in beats per minute".to_string())?;
    let mut next = d.projection().tracker.clone();
    next.tempo = bpm as f32;
    if let Some(s) = p.get("swing").and_then(number) {
        next.swing = swing_from_percent(s as f32);
    }
    let next = next.sanitised();
    let arr = arr_of(d)?;
    d.edit("Set Tempo", |tx| tx.write_tracker(arr, &next))
        .map_err(|e| e.to_string())?;
    let mut out = format!(
        "tempo is {} bpm, swing {} %",
        next.tempo,
        swing_percent(next.swing)
    );
    if (next.tempo - bpm as f32).abs() > 0.01 {
        out += &format!(" ({bpm} is outside 40 to 240)");
    }
    Ok(out)
}

/// Swing as the UI shows it, 50 % straight to 75 % heavy, as the tracker
/// holds it: of a sixteenth, 0 to `SWING_MAX`.
pub fn swing_from_percent(p: f32) -> f32 {
    ((p - 50.0) / 50.0).clamp(0.0, SWING_MAX)
}

pub fn swing_percent(s: f32) -> f32 {
    (s * 50.0 + 50.0).round()
}

// Reading.

/// The song in the shape the write commands take: tempo, swing in percent,
/// and every track with its notes or hits as MIDI.
pub fn song(d: &Document<Shard>) -> Json {
    let plan = d.projection();
    let t = &plan.tracker;
    let tracks: Vec<Json> = t
        .tracks
        .iter()
        .enumerate()
        .map(|(i, track)| match &track.drums {
            Some(kit) => {
                let mut hits = Vec::new();
                for step in 0..track.length as usize {
                    for (lane, row) in [&kit.kick, &kit.snare, &kit.hat].into_iter().enumerate() {
                        let v = row.get(step).copied().unwrap_or(0);
                        if v > 0 {
                            hits.push(json!({ "step": step, "drum": LANES[lane], "velocity": v }));
                        }
                    }
                }
                json!({
                    "track": i,
                    "kind": "drums",
                    "on": kit.on,
                    "length": track.length,
                    "hits": hits,
                })
            }
            None => {
                let (root, _) = root_of(d, plan.track_patches.get(i).copied().flatten());
                let notes: Vec<Json> = track
                    .steps
                    .iter()
                    .take(track.length as usize)
                    .enumerate()
                    .filter(|(_, s)| s.on)
                    .map(|(step, s)| {
                        json!({
                            "step": step,
                            "note": root + i32::from(s.pitch),
                            "velocity": s.velocity,
                            "hold": s.hold,
                        })
                    })
                    .collect();
                json!({
                    "track": i,
                    "kind": "melody",
                    "length": track.length,
                    "root": root,
                    "steps": notes,
                })
            }
        })
        .collect();
    json!({
        "tempo": t.tempo,
        "swing": swing_percent(t.swing),
        "tracks": tracks,
    })
}

fn arr_of(d: &Document<Shard>) -> Result<NodeId, String> {
    arrangement_node(d.tree())
        .map(|n| n.id())
        .ok_or_else(|| "the song has no arrangement".to_string())
}

fn no_track(t: &Tracker, i: usize) -> String {
    format!(
        "there is no track {i}: the song has tracks 0 to {}; leave track out to write a new one",
        t.tracks.len().saturating_sub(1)
    )
}
