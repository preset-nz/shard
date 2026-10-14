//! The brief commands: a whole track from a few choices, written by the
//! generators in `crate::music` (`projects/shard/design/agent-formats.md`,
//! "The lean: the model writes a brief").
//!
//! - `shard.make_drums`: a drums track from a style, density and glitch.
//! - `shard.make_bass`: a bass line from a key, mode, progression and figure.
//! - `shard.make_arp`: an arpeggio from the same, a pattern and a rate.
//! - `shard.list_drum_styles`, `shard.list_progressions`: what there is to
//!   choose from, with each one's feel.
//!
//! They speak `commands`' vocabulary (tracks by the names Shard shows,
//! `"new"`, steps from 1) and write through `set_drums` and `set_melody`,
//! so a brief and a note list land the same way. Every choice is seeded; a
//! request without a seed gets one from its own text, and the answer says
//! which, so asking again with another seed is another take.

use rhizome_pom::{Commands, Document, Error as PomError, Outcome};
use serde_json::{json, Map, Value as Json};

use super::commands::{self, number, Role, Target};
use super::Shard;
use crate::music::drums::{self, STYLES};
use crate::music::line::{self, Figure, Harmony, Pattern, OCTAVES_MAX, RATES};
use crate::music::{self, Mode, Progression, Register, PROGRESSIONS};

pub const MAKE_DRUMS: &str = "shard.make_drums";
pub const MAKE_BASS: &str = "shard.make_bass";
pub const MAKE_ARP: &str = "shard.make_arp";
pub const LIST_DRUM_STYLES: &str = "shard.list_drum_styles";
pub const LIST_PROGRESSIONS: &str = "shard.list_progressions";

/// The bars a generated track may run: what a track's lengths allow.
pub const BARS: [usize; 3] = [1, 2, 4];
/// How long a chord may last, in bars.
pub const CHORD_BARS: [f64; 4] = [0.5, 1.0, 2.0, 4.0];

pub fn register(c: &mut Commands<Shard>) {
    let always = |_: &Document<Shard>, _: &Json| true;
    let text = |r: Result<String, String>| r.map(Outcome::Text).map_err(PomError::Payload);
    c.add(
        MAKE_DRUMS,
        |_| "Make Drums".into(),
        always,
        move |d, p| text(make_drums(d, p)),
    );
    c.add(
        MAKE_BASS,
        |_| "Make Bass".into(),
        always,
        move |d, p| text(make_line(d, p, Role::Bass)),
    );
    c.add(
        MAKE_ARP,
        |_| "Make Arp".into(),
        always,
        move |d, p| text(make_line(d, p, Role::Arp)),
    );
    c.add(
        LIST_DRUM_STYLES,
        |_| "List Drum Styles".into(),
        always,
        |_, _| Ok(Outcome::Text(drum_styles().to_string())),
    );
    c.add(
        LIST_PROGRESSIONS,
        |_| "List Progressions".into(),
        always,
        |_, p| Ok(Outcome::Text(progressions(p).to_string())),
    );
}

// Reading a brief.

fn text_of<'a>(p: &'a Json, key: &str) -> Option<&'a str> {
    p.get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// One of a list, by name: refused with the list when it names none.
fn choice<T: Copy>(
    p: &Json,
    key: &str,
    parse: impl Fn(&str) -> Option<T>,
    names: &[&str],
    default: Option<T>,
) -> Result<T, String> {
    match text_of(p, key) {
        Some(t) => parse(t)
            .ok_or_else(|| format!("\"{t}\" is not a {key}; send one of {}", names.join(", "))),
        None => default.ok_or_else(|| format!("missing \"{key}\": one of {}", names.join(", "))),
    }
}

/// A fraction from 0 to 1, read as a percentage when above 1.
fn amount(p: &Json, key: &str) -> Option<f32> {
    p.get(key).and_then(number).map(|n| {
        let n = if n > 1.0 { n / 100.0 } else { n };
        n.clamp(0.0, 1.0) as f32
    })
}

/// The bars asked for, snapped to 1, 2 or 4.
fn bars_of(p: &Json) -> Option<usize> {
    p.get("bars").and_then(number).map(|n| match n {
        n if n <= 1.0 => 1,
        n if n <= 2.0 => 2,
        _ => 4,
    })
}

/// The seed sent, or one made from the rest of the request: its fields in
/// sorted order, leaving out the seed and which track it goes to.
fn seed_of(p: &Json) -> (u32, bool) {
    if let Some(n) = p.get("seed").and_then(number) {
        return ((n.abs().round() as u64 % (1 << 32)) as u32, true);
    }
    let mut keys: Vec<(&String, &Json)> = p
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| *k != "seed" && *k != "track")
                .collect()
        })
        .unwrap_or_default();
    keys.sort_by(|a, b| a.0.cmp(b.0));
    let text: String = keys.iter().map(|(k, v)| format!("{k}={v};")).collect();
    (music::seed_of(&text), false)
}

fn seed_text(seed: u32, sent: bool) -> String {
    if sent {
        format!("seed {seed}")
    } else {
        format!("seed {seed} (none was sent; send another for another take)")
    }
}

fn names<T>(all: &[T], name: impl Fn(&T) -> &'static str) -> Vec<&'static str> {
    all.iter().map(name).collect()
}

// Drums.

fn make_drums(d: &mut Document<Shard>, p: &Json) -> Result<String, String> {
    let style_names = names(STYLES, |s| s.name);
    let style = choice(p, "style", drums::style, &style_names, None)?;
    let bars = bars_of(p).unwrap_or(2);
    let density = amount(p, "density").unwrap_or(0.5);
    let glitch = amount(p, "glitch").unwrap_or(style.glitch);
    let (seed, sent) = seed_of(p);
    let lanes = drums::generate(style, bars, density, glitch, seed);
    let mut hits = Vec::new();
    for step in 0..bars * 16 {
        for (l, lane) in drums::LANES.iter().enumerate() {
            let v = lanes.lane(l)[step];
            if v > 0 {
                hits.push(json!({ "step": step + 1, "lane": lane, "velocity": v }));
            }
        }
    }
    let payload = json!({
        "track": p.get("track").cloned().unwrap_or(Json::Null),
        "length": bars * 16,
        "hits": hits,
    });
    let wrote = commands::set_drums(d, &payload)?;
    Ok(format!(
        "{wrote}. Played {} over {bars} bar{}, density {density:.2}, glitch {glitch:.2}, {}; it is written for about {} bpm",
        style.name,
        if bars == 1 { "" } else { "s" },
        seed_text(seed, sent),
        style.bpm,
    ))
}

pub fn drum_styles() -> Json {
    Json::Array(
        STYLES
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "description": s.describe,
                    "tags": s.tags,
                    "bpm": s.bpm,
                })
            })
            .collect(),
    )
}

// Lines.

/// The chord a degree makes in a key and mode, as a name: "Am", "F", "Bdim".
fn chord_name(key: i32, mode: Mode, degree: i32) -> String {
    let t = mode.triad(degree);
    let quality = match (t[1] - t[0], t[2] - t[0]) {
        (3, 6) => "dim",
        (3, _) => "m",
        (4, 8) => "aug",
        _ => "",
    };
    format!("{}{quality}", music::pitch_name(key + t[0]))
}

fn make_line(d: &mut Document<Shard>, p: &Json, role: Role) -> Result<String, String> {
    let key_text = text_of(p, "key").ok_or("missing \"key\": a note name such as A, F# or Bb")?;
    let (key, key_mode) = music::parse_key(key_text)?;
    let prog_text = text_of(p, "progression").ok_or(
        "missing \"progression\": a name from list_progressions (\"i-bVI-bVII\"), or roman numerals such as \"i iv v\"",
    )?;
    let (degrees, entry) = music::chords(prog_text)?;
    let mode_names = names(&Mode::ALL, |m| m.name());
    let fallback = key_mode
        .or(entry.map(Progression::home))
        .unwrap_or(Mode::Minor);
    let mode = choice(p, "mode", Mode::from_name, &mode_names, Some(fallback))?;

    let chord_bars = p
        .get("chord_bars")
        .and_then(number)
        .map(|n| {
            CHORD_BARS
                .iter()
                .copied()
                .min_by(|a, b| (a - n).abs().total_cmp(&(b - n).abs()))
                .unwrap_or(1.0)
        })
        .unwrap_or(if degrees.len() <= 4 { 1.0 } else { 0.5 });
    let bars = bars_of(p).unwrap_or_else(|| {
        let need = (degrees.len() as f64 * chord_bars).ceil() as usize;
        BARS.iter().copied().find(|b| *b >= need).unwrap_or(4)
    });
    let chord_steps = (chord_bars * 16.0) as usize;
    let length = bars * 16;
    let heard = length.div_ceil(chord_steps).min(degrees.len());
    let h = Harmony {
        key,
        mode,
        degrees: degrees.clone(),
        chord_steps,
        length,
    };
    let (seed, sent) = seed_of(p);
    let reg_names = names(&Register::ALL, |r| r.name());

    let (notes, what) = match role {
        Role::Arp => {
            let pat_names = names(&Pattern::ALL, |p| p.name());
            let pattern = choice(p, "pattern", Pattern::from_name, &pat_names, None)?;
            let rate = p
                .get("rate")
                .and_then(number)
                .map_or(16, |r| if r < 12.0 { 8 } else { 16 });
            let octaves = p
                .get("octaves")
                .and_then(number)
                .map_or(1, |o| o.round().clamp(1.0, f64::from(OCTAVES_MAX)) as u8);
            let register = choice(
                p,
                "register",
                Register::from_name,
                &reg_names,
                Some(Register::Mid),
            )?;
            let notes = line::arp(&h, pattern, rate, octaves, register, seed);
            let what = format!(
                "{} {} arp in {rate}ths over {octaves} octave{}, {} register",
                if pattern == Pattern::Up || pattern == Pattern::UpDown {
                    "an"
                } else {
                    "a"
                },
                pattern.name(),
                if octaves == 1 { "" } else { "s" },
                register.name()
            );
            (notes, what)
        }
        _ => {
            let fig_names = names(&Figure::ALL, |f| f.name());
            let figure = choice(p, "figure", Figure::from_name, &fig_names, None)?;
            let register = choice(
                p,
                "register",
                Register::from_name,
                &reg_names,
                Some(Register::Low),
            )?;
            let notes = line::bass(&h, figure, register, seed);
            (
                notes,
                format!("{} bass, {} register", figure.name(), register.name()),
            )
        }
    };

    // Which track, before writing, to say if it plays the drone.
    let tracker = d.projection().tracker.clone();
    let target = commands::resolve(&tracker, p, false)?;
    let drone = match target {
        Target::Place(i) => {
            commands::plays_drone(d, d.projection().track_patches.get(i).copied().flatten())
                .then(|| commands::track_names(&tracker)[i].clone())
        }
        Target::New => None,
    };

    let steps: Vec<Json> = notes
        .iter()
        .map(|n| json!({ "step": n.step + 1, "note": n.note, "velocity": n.velocity, "hold": n.hold }))
        .collect();
    let payload = json!({
        "track": p.get("track").cloned().unwrap_or(Json::Null),
        "length": length,
        "steps": steps,
    });
    let wrote = commands::melody(d, &payload, role)?;

    let chords: Vec<String> = degrees[..heard]
        .iter()
        .map(|g| chord_name(key, mode, *g))
        .collect();
    let (lo, hi) = (
        notes.iter().map(|n| n.note).min().unwrap_or(0),
        notes.iter().map(|n| n.note).max().unwrap_or(0),
    );
    let name = entry.map_or(prog_text, |e| e.name);
    let mut out = format!(
        "{wrote}. Played {what} over {name} in {} {} ({}), {} a chord, notes {} to {}, {}",
        music::pitch_name(key),
        mode.name(),
        chords.join(" "),
        match chord_bars {
            0.5 => "half a bar".to_string(),
            1.0 => "a bar".to_string(),
            b => format!("{b} bars"),
        },
        music::note_name(lo),
        music::note_name(hi),
        seed_text(seed, sent),
    );
    if heard < degrees.len() {
        out += &format!(
            ". Only the first {heard} of its {} chords fit in {bars} bar{}; send chord_bars 0.5 or more bars for all of them",
            degrees.len(),
            if bars == 1 { "" } else { "s" }
        );
    }
    if let Some(track) = drone {
        out += &format!(
            ". {track} plays the built-in drone, not a {} sound; send track \"new\" for a track with a sound of its own",
            if role == Role::Arp { "arp" } else { "bass" }
        );
    }
    Ok(out)
}

/// The progressions with a feel and in a mode, both optional; never none:
/// with nothing that matches both, the nearest, saying so.
pub fn progressions(p: &Json) -> Json {
    let feel = text_of(p, "feel").map(str::to_ascii_lowercase);
    let mode = text_of(p, "mode").and_then(Mode::from_name);
    let has_feel = |e: &&Progression| feel.as_ref().is_none_or(|f| e.tags.iter().any(|t| t == f));
    let in_mode = |e: &&Progression| mode.is_none_or(|m| e.modes.contains(&m));
    let both: Vec<&Progression> = PROGRESSIONS
        .iter()
        .filter(has_feel)
        .filter(in_mode)
        .collect();
    let (list, note) = if !both.is_empty() {
        (both, None)
    } else {
        let feel_only: Vec<&Progression> = PROGRESSIONS.iter().filter(has_feel).collect();
        if !feel_only.is_empty() {
            (feel_only, Some("none of that feel is written for that mode; these have the feel, and any progression plays in any mode"))
        } else {
            let mode_only: Vec<&Progression> = PROGRESSIONS.iter().filter(in_mode).collect();
            if !mode_only.is_empty() {
                (
                    mode_only,
                    Some("no progression has that feel; these are the ones for that mode"),
                )
            } else {
                (
                    PROGRESSIONS.iter().collect(),
                    Some("nothing matched; these are all of them"),
                )
            }
        }
    };
    let mut out = Map::new();
    out.insert(
        "progressions".into(),
        Json::Array(
            list.iter()
                .map(|e| {
                    json!({
                        "name": e.name,
                        "description": e.describe(),
                    })
                })
                .collect(),
        ),
    );
    if let Some(n) = note {
        out.insert("note".into(), json!(n));
    }
    out.insert("feels".into(), json!(music::feels()));
    Json::Object(out)
}

/// The choices a brief tool offers, for its schema.
pub fn style_names() -> Vec<&'static str> {
    names(STYLES, |s| s.name)
}

pub fn progression_names() -> Vec<&'static str> {
    names(PROGRESSIONS, |p| p.name)
}

pub fn mode_names() -> Vec<&'static str> {
    names(&Mode::ALL, |m| m.name())
}

pub fn figure_names() -> Vec<&'static str> {
    names(&Figure::ALL, |f| f.name())
}

pub fn pattern_names() -> Vec<&'static str> {
    names(&Pattern::ALL, |p| p.name())
}

pub fn register_names() -> Vec<&'static str> {
    names(&Register::ALL, |r| r.name())
}

pub fn rates() -> [u8; 2] {
    RATES
}
