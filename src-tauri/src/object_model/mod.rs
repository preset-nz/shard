//! Shard's object model on rhizome (epic 32, story 2; guidance
//! `projects/shard/design/object-model.md`): its kinds, their roles and
//! rules, and the verbs that build a session.
//!
//! - `/patches/<patch>`: the sounds, listed in the order `order` (epic 10).
//!   A patch's name is its `patch.label`; its fixed nodes (the generators,
//!   the envelope, tape, filter and output) are children named for their kind.
//!   The effects it runs are children listed in its order `chain`, its LFOs
//!   and mod envelopes children listed in `modulators`. A link is a binding
//!   from a parameter onto one of those, carrying `lo` and `hi`.
//! - `/arrangements/arrangement`: the level above the patch. Tempo and swing;
//!   tracks of two kinds in the one order `tracks` (epic 28). A melody
//!   `track` plays the patch its reference `track.patch` names (the first
//!   patch when it names none) at its fader `track.gain`, each step a child
//!   named by its number and made only once it differs from a blank step. A
//!   `drums` track (story d) holds its kit's switch, fader, tune, decay and
//!   length, its beats made the same way. The *k*-th of each kind plays on
//!   voice or kit *k* (`tracker::Slot`). Then the arrangement's own chain,
//!   filter and output (a `master`, named `amp`).
//! - `/materials/<sample>`: WAVs and the built-in drone. A generator reads
//!   one through its reference `<kind>.source`.
//!
//! **A value's key is its parameter id,** less any `arrangement.` prefix, so
//! `params.rs` stays the one place ids are written. An effect's keys are its
//! kind's (`chorus.mix`); which pool instance a node plays is decided when the
//! tree is compiled. The table keeps what the tree does not: taper, unit and
//! smoothing.
//!
//! Brake and reverse are not in the tree. They are played, not edited.

use rhizome_core::{
    Changeset, Edit, Node, NodeId, NodeType, On, Origin, Ref, Result, Shape, Tree, Value,
    ValueKind, ValueSpec,
};
use rhizome_pom::{Kinds, NodeValues, ObjectModel};
use serde_json::{json, Value as Json};
use shard_dsp::arrangement::{self, TRACK_GAIN_DEFAULT, TRACK_GAIN_MAX};
use shard_dsp::fx::{self, Kind as Fx};
use shard_dsp::kit::{DECAY_MAX, DECAY_MIN, HIT_MAX, TUNE_RANGE};
use shard_dsp::modulation::{Shape as LfoShape, MAX_RATE_HZ, MIN_RATE_HZ};
use shard_dsp::params::{ParamDef, Taper, GENERATORS, PARAMS};
use shard_dsp::sources::OCTAVE_RANGE;
use shard_dsp::steps::{HOLD_MAX, NUDGE_MAX_MS, PITCH_RANGE, STEPS, VELOCITY_MAX};

use crate::materials::Pool;
use crate::modulation::{pluck_curve, MODES, NEW_RATE_HZ, NEW_SHAPE};
use crate::tracker::{Kit, Step, Track, Tracker, LENGTHS, SWING_MAX, TEMPO_MAX, TEMPO_MIN};
use shard_dsp::KITS;

mod compile;
#[cfg(test)]
mod compile_tests;
#[cfg(test)]
mod tests;

/// An `f32` as the `f64` its shortest decimal names, so a file says `0.4`
/// rather than `0.4000000059604645`. It reads back as the same `f32`. Every
/// float the tree holds, ranges and defaults included, goes through it, so a
/// value set to its default compares equal to it.
pub fn float(v: f32) -> f64 {
    v.to_string().parse().unwrap_or(f64::from(v))
}

pub use compile::{
    arrangement_node, as_f32, compile, instance_of, locate, patch_node, patches, patches_in,
    played_by, read_tracker, to_value, track_patch, tracks, write_values, PatchPlan, Plan,
};

pub const EXTENSION: &str = "shard";

pub const PATCHES: &str = "patches";
pub const ARRANGEMENTS: &str = "arrangements";
pub const MATERIALS: &str = "materials";

pub const PATCH: &str = "patch";
pub const ARRANGEMENT: &str = "arrangement";
pub const TRACK: &str = "track";
pub const STEP: &str = "step";
/// A drums track (epic 28, story d).
pub const DRUMS: &str = "drums";
pub const BEAT: &str = "beat";
pub const MASTER: &str = "master";
pub const LFO: &str = "lfo";
pub const MOD_ENV: &str = "mod-env";
pub const SAMPLE: &str = "sample";

/// The order of a patch's or the arrangement's effects.
pub const CHAIN: &str = "chain";
/// The order of a patch's LFOs and mod envelopes.
pub const MODULATORS: &str = "modulators";
/// The order of the arrangement's tracks.
pub const TRACKS: &str = "tracks";
/// The order of the materials, as they were added.
pub const POOL: &str = "pool";
/// The order of the patches, as they were added (epic 10), so "Patch 10" never
/// sorts before "Patch 2".
pub const PATCH_ORDER: &str = "order";

/// A patch's name, as the scene view shows it. Empty in a file written
/// before patches had names, which shows the patch's place instead.
pub const PATCH_LABEL: &str = "patch.label";
/// The patch a track plays, a reference to it. Unset, or naming a patch that
/// has gone, the track plays the first patch.
pub const TRACK_PATCH: &str = "track.patch";
/// A track's fader, 0 to `TRACK_GAIN_MAX` (epic 28).
pub const TRACK_GAIN: &str = "track.gain";
/// A track's mute and solo (epic 28, story c). Session state, not edits:
/// the live tree never has them written. Saving writes them into a copy of
/// the tree, and opening reads them back (`session.rs`), so they are the
/// file's and never the history's.
pub const TRACK_MUTE: &str = "track.mute";
pub const TRACK_SOLO: &str = "track.solo";
/// A drums track's, the same way.
pub const DRUMS_MUTE: &str = "drums.mute";
pub const DRUMS_SOLO: &str = "drums.solo";

/// Whether a node is a track of either kind, as the order `tracks` lists.
pub fn is_track(n: &Node<'_>) -> bool {
    matches!(n.type_name(), TRACK | DRUMS)
}

/// A track node's mute and solo keys, by its kind.
pub fn mix_keys(track: &Node<'_>) -> (&'static str, &'static str) {
    if track.type_name() == DRUMS {
        (DRUMS_MUTE, DRUMS_SOLO)
    } else {
        (TRACK_MUTE, TRACK_SOLO)
    }
}

/// Played, not edited: a held button or a pedal writes them to the bank, and
/// a document never stores them. The tape's gestures, on the arrangement's
/// table since they slow every track (epic 28, Georg, 2026-10-12).
pub const PLAYED: &[&str] = &["arrangement.tape.brake", "arrangement.tape.reverse"];

/// A patch's fixed nodes, each a child named for its kind.
pub const PATCH_NODES: &[&str] = &[
    "material", "grain", "fm", "env", "tape", "filter", "ladder", "amp",
];

/// The generators that read a material, through `<kind>.source`.
pub const READS_MATERIAL: &[&str] = &["material", "grain"];

/// A patch's name: its label, or "Patch N" by its place when it has none.
pub fn patch_name(patch: &Node<'_>, index: usize) -> String {
    match patch.value(PATCH_LABEL) {
        Some(Value::Text(t)) if !t.trim().is_empty() => t,
        _ => format!("Patch {}", index + 1),
    }
}

/// The first "Patch N" no patch in `category` (`/patches`) is named yet,
/// counting on from how many there are.
pub fn next_patch_name(category: Option<Node<'_>>) -> String {
    let taken: Vec<String> = category
        .map(patches_in)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, p)| patch_name(p, i))
        .collect();
    let mut n = taken.len() + 1;
    while taken.contains(&format!("Patch {n}")) {
        n += 1;
    }
    format!("Patch {n}")
}

/// Whether a node is listed in its parent's order `order`.
fn listed_in(n: &Node<'_>, order: &str) -> std::result::Result<(), String> {
    let listed = n
        .parent()
        .is_some_and(|p| p.order(order).iter().any(|m| m.id() == n.id()));
    if listed {
        Ok(())
    } else {
        Err(format!("{} is missing from {order}", n.path()))
    }
}

/// What a kind is for. Roles carry the conventions in CLAUDE.md, and
/// `tests.rs` holds every kind to its role's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Makes sound: its switch, then Gain. Generators are summed.
    Generator,
    /// Shapes sound: its switch, then Mix.
    Effect,
    /// A node with settings and no switch: tape, the output, the patch itself.
    Fixed,
    /// What a link follows: an LFO or a mod envelope.
    Modulator,
    /// Holds the rest: the arrangement, a track and its steps, a sample.
    Structure,
}

/// Where a kind may live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Under {
    Category,
    Patch,
    Arrangement,
    /// A patch or the arrangement: effects and the filter.
    Either,
    Track,
    Drums,
}

/// One kind as Shard declares it.
pub struct KindDecl {
    pub node_type: NodeType,
    pub role: Role,
    /// One per parent, made with it, never removed on its own.
    pub fixed: bool,
    pub max_per_parent: Option<usize>,
}

fn key_prefix(id: &str) -> &str {
    id.split_once('.').map_or(id, |(p, _)| p)
}

/// The patch table's rows for one node, in table order.
fn patch_rows(prefix: &str) -> Vec<ParamDef> {
    PARAMS
        .iter()
        .filter(|p| key_prefix(p.id) == prefix)
        .copied()
        .collect()
}

/// The arrangement table's rows for one node, in table order, ids bare.
fn arrangement_rows(prefix: &str) -> Vec<ParamDef> {
    arrangement::params()
        .iter()
        .filter_map(|p| {
            let bare = p.id.strip_prefix(arrangement::PREFIX)?;
            (key_prefix(bare) == prefix).then_some(ParamDef { id: bare, ..*p })
        })
        .collect()
}

/// An effect kind's rows. An added effect arrives on, as in `fx::rows`.
fn effect_rows(kind: Fx) -> Vec<ParamDef> {
    kind.rows()
        .iter()
        .map(|row| {
            let mut def = *row;
            if row.id.ends_with(".on") {
                def.default = 1.0;
            }
            def
        })
        .collect()
}

/// A parameter row as a value. A switch is a bool, any other stepped row an
/// int (the table holds every stepped row to whole steps), the rest floats.
pub fn spec(def: &ParamDef) -> ValueSpec {
    match def.taper {
        Taper::Stepped(_) if def.id.ends_with(".on") => ValueSpec::bool(def.id, def.default >= 0.5),
        Taper::Stepped(_) => {
            ValueSpec::int(def.id, def.min as i64..=def.max as i64, def.default as i64)
        }
        _ => ValueSpec::float(def.id, float(def.min)..=float(def.max), float(def.default)),
    }
}

fn with_rows(mut t: NodeType, rows: &[ParamDef]) -> NodeType {
    for def in rows {
        t = t.value(spec(def));
    }
    t
}

fn link_values() -> Vec<ValueSpec> {
    vec![
        ValueSpec::float("lo", 0.0..=1.0, 0.0),
        ValueSpec::float("hi", 0.0..=1.0, 1.0),
    ]
}

/// The step number a step or beat is named by, 1 to `STEPS`.
pub fn step_number(node: &Node<'_>) -> Option<usize> {
    node.name()
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=STEPS).contains(n))
}

fn lives_under(node: &Node<'_>, under: Under) -> std::result::Result<(), String> {
    let parent = node.parent().ok_or("it has no parent")?;
    let ok = match under {
        Under::Category => parent.is_category(),
        Under::Patch => parent.type_name() == PATCH,
        Under::Arrangement => parent.type_name() == ARRANGEMENT,
        Under::Either => matches!(parent.type_name(), PATCH | ARRANGEMENT),
        Under::Track => parent.type_name() == TRACK,
        Under::Drums => parent.type_name() == DRUMS,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "a {} can't live in {}",
            node.type_name(),
            parent.path()
        ))
    }
}

/// Links onto a node: each onto a continuous parameter (a set of choices
/// can't follow a modulator, as `LinkError::Stepped` says, and a modulator's
/// own settings are not parameters), at most one per value, and only from a
/// modulator of the node's own patch.
fn links_hold(node: &Node<'_>) -> std::result::Result<(), String> {
    let bindings = node.bindings();
    if bindings.is_empty() {
        return Ok(());
    }
    if is_modulator(node) {
        return Err(format!("a {} can't follow a modulator", node.type_name()));
    }
    let patch = if node.type_name() == PATCH {
        Some(node.id())
    } else {
        node.parent()
            .filter(|p| p.type_name() == PATCH)
            .map(|p| p.id())
    };
    let mut seen: Vec<&str> = Vec::new();
    for b in bindings {
        let On::Value(key) = b.on else {
            return Err("a link is onto a value, not a slot".into());
        };
        let parameter = PARAMS.iter().any(|p| p.id == key)
            || Fx::from_name(node.type_name())
                .is_some_and(|k| k.rows().iter().any(|r| r.id == key));
        if !parameter {
            return Err(format!("{key} is not a parameter a modulator can move"));
        }
        let kind = node.node_type().and_then(|t| t.spec(key)).map(|s| s.kind);
        if kind != Some(ValueKind::Float) {
            return Err(format!(
                "{key} is a set of choices and cannot follow a modulator"
            ));
        }
        if seen.contains(&key.as_str()) {
            return Err(format!("{key} already follows a modulator"));
        }
        seen.push(key);
        if b.source.parent().map(|p| p.id()) != patch {
            return Err(format!(
                "{key} can follow only a modulator of its own patch"
            ));
        }
    }
    Ok(())
}

/// An order lists exactly the children `belongs` picks.
fn order_holds(
    node: &Node<'_>,
    order: &str,
    belongs: impl Fn(&Node<'_>) -> bool,
) -> std::result::Result<(), String> {
    let listed = node.order(order);
    if let Some(stray) = listed.iter().find(|n| !belongs(n)) {
        return Err(format!("{} can't be in {order}", stray.path()));
    }
    if let Some(missing) = node
        .children()
        .find(|c| belongs(c) && !listed.iter().any(|n| n.id() == c.id()))
    {
        return Err(format!("{} is missing from {order}", missing.path()));
    }
    Ok(())
}

/// A chain runs at most `fx::ORDER_LEN` effects, as the engine does.
fn chain_fits(node: &Node<'_>) -> std::result::Result<(), String> {
    if node.order(CHAIN).len() > fx::ORDER_LEN {
        Err(format!("a chain runs at most {} effects", fx::ORDER_LEN))
    } else {
        Ok(())
    }
}

/// A mod envelope's curve as rhizome checks it, the shape ux-kit's curve
/// editor uses: points with a basis and an optional tension, and an optional
/// sustain index.
fn curve_shape() -> Shape {
    Shape::record([
        (
            "points",
            Shape::list(Shape::record([
                ("x", Shape::Float),
                ("y", Shape::Float),
                (
                    "basis",
                    Shape::choice(&["constant", "linear", "monotone", "catmull-rom"]),
                ),
                ("tension", Shape::optional(Shape::Float)),
            ])),
        ),
        ("sustain", Shape::optional(Shape::Int)),
    ])
}

/// A curve as the tree holds it. Written by hand rather than through serde,
/// which would widen an `f32` to the `f64` that is not its shortest decimal:
/// every float goes through `float`, so a save says `0.005` and a curve
/// written back as it was compares equal.
pub fn curve_json(curve: &preset_math::Curve) -> Json {
    let points: Vec<Json> = curve
        .points
        .iter()
        .map(|p| {
            let mut o = serde_json::Map::new();
            o.insert("x".into(), json!(float(p.x)));
            o.insert("y".into(), json!(float(p.y)));
            let basis = serde_json::to_value(p.basis).unwrap_or(Json::Null);
            o.insert("basis".into(), basis);
            if p.tension != 0.0 {
                o.insert("tension".into(), json!(float(p.tension)));
            }
            Json::Object(o)
        })
        .collect();
    let mut o = serde_json::Map::new();
    o.insert("points".into(), Json::Array(points));
    if let Some(i) = curve.sustain {
        o.insert("sustain".into(), json!(i));
    }
    Json::Object(o)
}

/// A mod envelope's curve must be one the engine can play: sorted, with x and
/// y both in 0 to 1, and with a sustain mark that exists. In ADSR
/// mode it must also be the shape the four sliders draw, because the editor
/// shows sliders for it and a slider cannot say anything else.
fn envelope_holds(node: &Node<'_>) -> std::result::Result<(), String> {
    let Some(Value::Shaped(json)) = node.value("mod-env.curve") else {
        return Err("a mod envelope needs a curve".into());
    };
    let curve: preset_math::Curve =
        serde_json::from_value(json).map_err(|e| format!("the curve is not readable: {e}"))?;
    let adsr = matches!(node.value("mod-env.mode"), Some(Value::Choice(m)) if m == "adsr");

    let pts = &curve.points;
    if pts.is_empty() {
        return Err("a curve needs at least one point".into());
    }
    let mut last = 0.0f32;
    for (i, p) in pts.iter().enumerate() {
        if p.x < 0.0 {
            return Err(format!("point {i} is before the start (x {})", p.x));
        }
        if p.x < last {
            return Err(format!(
                "point {i} is before the one ahead of it (x {} after {last})",
                p.x
            ));
        }
        if p.x > 1.0 {
            return Err(format!("point {i} is past the end of the note (x {})", p.x));
        }
        if !(0.0..=1.0).contains(&p.y) {
            return Err(format!("point {i} is outside 0 to 1 (y {})", p.y));
        }
        last = p.x;
    }
    if let Some(s) = curve.sustain.filter(|s| *s >= pts.len()) {
        return Err(format!(
            "the sustain mark is on point {s}, and there are {} points",
            pts.len()
        ));
    }
    if adsr {
        // Four sliders can say only this shape, so anything else is a free
        // curve and the mode must say so.
        let why = if pts.len() != 4 {
            Some(format!("it has {} points, not four", pts.len()))
        } else if curve.sustain != Some(2) {
            Some(format!(
                "its sustain mark is {:?}, not on the third point",
                curve.sustain
            ))
        } else if (pts[0].x, pts[0].y) != (0.0, 0.0) {
            Some("it does not start at 0 at the start".into())
        } else if pts[1].y != 1.0 {
            Some("its attack does not reach 1".into())
        } else if pts[3].y != 0.0 {
            Some("its release does not end at 0".into())
        } else {
            None
        };
        if let Some(why) = why {
            return Err(format!(
                "this is not an ADSR curve ({why}); a free curve needs the free mode"
            ));
        }
    }
    Ok(())
}

fn is_effect(node: &Node<'_>) -> bool {
    Fx::from_name(node.type_name()).is_some()
}

/// Whether a node is an effect in a chain.
pub fn is_effect_node(node: &Node<'_>) -> bool {
    is_effect(node)
}

fn is_modulator(node: &Node<'_>) -> bool {
    matches!(node.type_name(), LFO | MOD_ENV)
}

fn length_holds(node: &Node<'_>, key: &str) -> std::result::Result<(), String> {
    match node.value(key) {
        Some(Value::Int(n)) if LENGTHS.iter().any(|l| i64::from(*l) == n) => Ok(()),
        other => Err(format!("{key} must be one of {LENGTHS:?}, not {other:?}")),
    }
}

fn checked(
    t: NodeType,
    under: Under,
    extra: impl Fn(&Node<'_>) -> std::result::Result<(), String> + Send + Sync + 'static,
) -> NodeType {
    t.check(move |n| {
        lives_under(n, under)?;
        links_hold(n)?;
        extra(n)
    })
}

fn none(_: &Node<'_>) -> std::result::Result<(), String> {
    Ok(())
}

/// Every kind Shard declares, in the order it registers them.
pub fn kinds() -> Vec<KindDecl> {
    let mut out = Vec::new();
    let mut push = |node_type, role, fixed, max_per_parent| {
        out.push(KindDecl {
            node_type,
            role,
            fixed,
            max_per_parent,
        })
    };

    // The patch, and its own settings: what sets a note's length, and the
    // controller map it plays with (app data, referred to by id). A session
    // holds as many as it likes, listed in the order `PATCH_ORDER` (epic 10).
    let patch = with_rows(
        NodeType::new(PATCH).in_categories(&[PATCHES]),
        &patch_rows("patch"),
    )
    .text(PATCH_LABEL, "")
    .shaped(
        "patch.map",
        Shape::optional(Shape::record([("id", Shape::Int), ("name", Shape::Text)])),
    );
    push(
        // Not held to the order, as a sample is to the pool's: a file saved
        // before patches had one opens and edits as it did, its patch listed
        // after the ordered ones (`patches_in`), and the next patch added
        // writes the whole order.
        checked(patch, Under::Category, |n| {
            order_holds(n, CHAIN, is_effect)?;
            chain_fits(n)?;
            order_holds(n, MODULATORS, is_modulator)
        }),
        Role::Fixed,
        // Added and removed whole. Keeping the last one, and the one a track
        // plays, is `Session::delete_patch`'s, not the tree's.
        false,
        None,
    );

    // A patch's fixed nodes.
    for &name in PATCH_NODES {
        let mut t = with_rows(NodeType::new(name), &patch_rows(name));
        if READS_MATERIAL.contains(&name) {
            t = t.reference(format!("{name}.source").as_str());
        }
        let role = if GENERATORS.contains(&name) {
            Role::Generator
        } else if t.spec(&format!("{name}.on")).is_some() {
            Role::Effect
        } else {
            Role::Fixed
        };
        let under = if name == "filter" {
            Under::Either
        } else {
            Under::Patch
        };
        push(checked(t, under, none), role, true, Some(1));
    }

    // The palette.
    for kind in Fx::ALL {
        let t = with_rows(NodeType::new(kind.name()), &effect_rows(kind));
        push(
            checked(t, Under::Either, none),
            Role::Effect,
            false,
            Some(fx::COPIES),
        );
    }

    // Modulators, the sources a link follows.
    let lfo = NodeType::new(LFO)
        .in_categories(&[PATCHES])
        .text("lfo.label", "")
        .float(
            "lfo.rate",
            float(MIN_RATE_HZ)..=float(MAX_RATE_HZ),
            float(NEW_RATE_HZ),
        )
        .choice("lfo.shape", &LfoShape::NAMES, NEW_SHAPE)
        .float("lfo.phase", 0.0..=1.0, 0.0)
        .bindable(link_values());
    push(
        checked(lfo, Under::Patch, none),
        Role::Modulator,
        false,
        None,
    );

    let mut curve = ValueSpec::shaped("mod-env.curve", curve_shape());
    curve.default = Value::Shaped(curve_json(&pluck_curve()));
    let env = NodeType::new(MOD_ENV)
        .in_categories(&[PATCHES])
        .text("mod-env.label", "")
        .choice("mod-env.mode", &MODES, MODES[0])
        .value(curve)
        .bindable(link_values());
    push(
        checked(env, Under::Patch, envelope_holds),
        Role::Modulator,
        false,
        None,
    );

    // The arrangement: the tracker's tempo and swing, over its tracks.
    let blank = Tracker::default();
    let arr = NodeType::new(ARRANGEMENT)
        .in_categories(&[ARRANGEMENTS])
        .float(
            "arrangement.tempo",
            float(TEMPO_MIN)..=float(TEMPO_MAX),
            float(blank.tempo),
        )
        .float(
            "arrangement.swing",
            0.0..=float(SWING_MAX),
            float(blank.swing),
        );
    push(
        checked(arr, Under::Category, |n| {
            order_holds(n, CHAIN, is_effect)?;
            chain_fits(n)?;
            order_holds(n, TRACKS, is_track)
        }),
        Role::Structure,
        true,
        Some(1),
    );

    // A track, with its fader, the patch it plays (epic 28) and its mute and
    // solo, which only a saved file carries (`TRACK_MUTE`).
    let track = Track::default();
    let t = NodeType::new(TRACK)
        .in_categories(&[ARRANGEMENTS])
        .float(
            TRACK_GAIN,
            0.0..=float(TRACK_GAIN_MAX),
            float(TRACK_GAIN_DEFAULT),
        )
        .int("track.length", 4..=STEPS as i64, i64::from(track.length))
        .bool(TRACK_MUTE, false)
        .bool(TRACK_SOLO, false)
        .reference(TRACK_PATCH);
    push(
        checked(t, Under::Arrangement, |n| {
            length_holds(n, "track.length")?;
            match n.resolve(TRACK_PATCH) {
                Some(p) if p.type_name() != PATCH => {
                    Err(format!("a track plays a patch, not {}", p.path()))
                }
                _ => Ok(()),
            }
        }),
        Role::Structure,
        false,
        None,
    );

    let step = Step::default();
    let t = NodeType::new(STEP)
        .in_categories(&[ARRANGEMENTS])
        .bool("step.on", step.on)
        .int(
            "step.pitch",
            -i64::from(PITCH_RANGE)..=i64::from(PITCH_RANGE),
            i64::from(step.pitch),
        )
        .int("step.hold", 0..=i64::from(HOLD_MAX), i64::from(step.hold))
        .int(
            "step.velocity",
            0..=i64::from(VELOCITY_MAX),
            i64::from(step.velocity),
        )
        .int(
            "step.nudge",
            -i64::from(NUDGE_MAX_MS)..=i64::from(NUDGE_MAX_MS),
            i64::from(step.nudge),
        );
    push(
        checked(t, Under::Track, |n| {
            step_number(n)
                .map(|_| ())
                .ok_or_else(|| format!("a step is named 1 to {STEPS}"))
        }),
        Role::Structure,
        false,
        None,
    );

    // A drums track (story d): a generator, so its switch and then its
    // level, which is its fader; its kit's sound, its length, its mute and
    // solo (only a saved file carries them), and its grid as beats.
    let kit = Kit::default();
    let t = NodeType::new(DRUMS)
        .in_categories(&[ARRANGEMENTS])
        .bool("drums.on", kit.on)
        .float(
            "drums.gain",
            0.0..=float(TRACK_GAIN_MAX),
            float(TRACK_GAIN_DEFAULT),
        )
        .float(
            "drums.tune",
            float(-TUNE_RANGE)..=float(TUNE_RANGE),
            float(kit.tune),
        )
        .float(
            "drums.decay",
            float(DECAY_MIN)..=float(DECAY_MAX),
            float(kit.decay),
        )
        .int("drums.length", 4..=STEPS as i64, i64::from(track.length))
        .bool(DRUMS_MUTE, false)
        .bool(DRUMS_SOLO, false);
    push(
        checked(t, Under::Arrangement, |n| length_holds(n, "drums.length")),
        Role::Generator,
        false,
        Some(KITS),
    );

    let hit = |key: &str| ValueSpec::int(key, 0..=i64::from(HIT_MAX), 0);
    let t = NodeType::new(BEAT)
        .in_categories(&[ARRANGEMENTS])
        .value(hit("beat.kick"))
        .value(hit("beat.snare"))
        .value(hit("beat.hat"));
    push(
        checked(t, Under::Drums, |n| {
            step_number(n)
                .map(|_| ())
                .ok_or_else(|| format!("a beat is named 1 to {STEPS}"))
        }),
        Role::Structure,
        false,
        None,
    );

    // The arrangement's output and limiter, last of all.
    let t = with_rows(
        NodeType::new(MASTER).in_categories(&[ARRANGEMENTS]),
        &arrangement_rows("amp"),
    );
    push(
        checked(t, Under::Arrangement, none),
        Role::Fixed,
        true,
        Some(1),
    );

    // Materials.
    let t = NodeType::new(SAMPLE)
        .in_categories(&[MATERIALS])
        .text("sample.label", "")
        // Empty for the built-in drone, which has no file.
        .text("sample.path", "")
        .int(
            "sample.octave",
            -(OCTAVE_RANGE as i64)..=OCTAVE_RANGE as i64,
            0,
        )
        .float("sample.start", 0.0..=1.0, 0.0)
        .float("sample.end", 0.0..=1.0, 1.0)
        // The MIDI note it sounds at, when known.
        .shaped("sample.root", Shape::optional(Shape::Int));
    push(
        checked(t, Under::Category, |n| listed_in(n, POOL)),
        Role::Structure,
        false,
        None,
    );

    out
}

/// Shard's object model.
pub struct Shard;

impl ObjectModel for Shard {
    const NAME: &'static str = "Shard";
    const EXTENSION: &'static str = EXTENSION;
    type Projection = Plan;
    type Context = ();

    fn kinds(k: &mut Kinds, _: &()) {
        k.category(PATCHES, Origin::Loaded)
            .category(ARRANGEMENTS, Origin::Loaded)
            .category(MATERIALS, Origin::Loaded);
        for decl in kinds() {
            // Every node the parameter table backs keeps presets: the
            // generators and effects, and the patch, tape and output too.
            let presets = matches!(decl.role, Role::Generator | Role::Effect | Role::Fixed);
            let mut kind = k.kind(decl.node_type);
            if decl.fixed {
                kind = kind.not_deletable().not_duplicable();
            }
            if let Some(n) = decl.max_per_parent {
                kind = kind.max_per_parent(n);
            }
            // A preset is a node's sound: its values and links, never its
            // switch, so applying one never switches a node in or out, and
            // never a patch's name or a drums track's mute and solo.
            if presets {
                kind.presets(
                    NodeValues::new()
                        .skip(|key| {
                            key.ends_with(".on")
                                || key == PATCH_LABEL
                                || key == DRUMS_MUTE
                                || key == DRUMS_SOLO
                        })
                        .with_bindings(),
                );
            }
        }
    }

    /// Rebuilt whole after every change: a session is small, and compiling
    /// it is cheap beside a block of audio.
    fn project(tree: &Tree, into: &mut Plan, _: Option<&Changeset>) {
        compile(tree, into);
    }

    /// A new session: the built-in drone in both generators, one patch at
    /// its defaults, "Patch 1", and the arrangement as a new tracker has it,
    /// its track playing that patch.
    fn seed(tx: &mut Edit<'_>, _: &()) -> Result<()> {
        let pool = Pool::with_drone();
        let samples = tx.add_pool(&pool)?;
        let patch = tx.new_patch()?;
        for (name, wired) in [
            ("material", pool.wires.material),
            ("grain", pool.wires.grain),
        ] {
            let sample = wired.and_then(|id| samples.iter().find(|(m, _)| *m == id));
            if let Some((_, node)) = sample {
                tx.wire(patch, name, *node)?;
            }
        }
        let arr = tx.add_arrangement(&Tracker::default())?;
        let first = tx
            .at(arr)
            .and_then(|a| a.order(TRACKS).into_iter().next())
            .map(|t| t.id());
        if let Some(track) = first {
            tx.set_ref(track, TRACK_PATCH, Ref::here(patch))?;
        }
        Ok(())
    }
}

/// Shard's verbs. Each composes into the caller's edit, so a verb is one undo
/// step however many writes it makes.
pub trait ShardEdit {
    /// A patch at its defaults, with its fixed nodes and an empty chain, at
    /// the end of the patches' order. Its node is named `name`; its label is
    /// left empty.
    fn add_patch(&mut self, name: &str) -> Result<NodeId>;
    /// What New Patch adds: a patch at its defaults named "Patch N", the
    /// built-in drone (when the pool still has it) in both generators that
    /// read a material, as a new session's patch has it.
    fn new_patch(&mut self) -> Result<NodeId>;
    /// `generator` (`material` or `grain`) of `patch` reads `sample`.
    fn wire(&mut self, patch: NodeId, generator: &str, sample: NodeId) -> Result<()>;
    /// An effect at the end of `owner`'s chain, a patch's or the arrangement's.
    fn add_effect(&mut self, owner: NodeId, kind: Fx) -> Result<NodeId>;
    fn add_lfo(&mut self, patch: NodeId) -> Result<NodeId>;
    fn add_mod_env(&mut self, patch: NodeId) -> Result<NodeId>;
    /// `key` on `target` follows `source`, from `lo` to `hi` of its range.
    fn link(&mut self, target: NodeId, key: &str, source: NodeId, lo: f32, hi: f32) -> Result<()>;
    /// The arrangement and its tracks written from a tracker, with its
    /// filter and output at their defaults.
    fn add_arrangement(&mut self, tracker: &Tracker) -> Result<NodeId>;
    /// A track at the end of the arrangement's order, written from `track`:
    /// a `drums` node for a drums track, else a melody `track`, whose patch
    /// is left unset (the first patch).
    fn add_track(&mut self, arr: NodeId, track: &Track) -> Result<NodeId>;
    /// Makes the arrangement's tempo, swing, and its tracks' steps or grids,
    /// lengths, faders and kits what `tracker` says, writing only what
    /// differs. Tracks are matched by place, and none is added or removed:
    /// that is Add Track's and Remove Track's, so a stale tracker sent back
    /// can't undo one; a place whose kind differs (a stale tracker from
    /// before one was added) is skipped. What a track plays is
    /// `track.patch`'s, never written here.
    fn write_tracker(&mut self, arr: NodeId, tracker: &Tracker) -> Result<()>;
    /// One track's steps or grid, length, fader and kit, writing only what
    /// differs.
    fn write_track(&mut self, node: NodeId, track: &Track) -> Result<()>;
    /// Sets `key` to `want`, or resets it when `want` is its default, and
    /// writes nothing when it already is.
    fn put(&mut self, node: NodeId, key: &str, want: Value) -> Result<()>;
    /// Every material in a pool, with the node each became.
    fn add_pool(&mut self, pool: &Pool) -> Result<Vec<(u64, NodeId)>>;
}

/// A step or beat: made when it stops being blank, removed when it becomes
/// blank again, its fields written where they differ.
trait PutCell {
    fn put_cell<const N: usize>(
        &mut self,
        owner: NodeId,
        kind: &str,
        index: usize,
        blank: bool,
        fields: [(&str, Value); N],
    ) -> Result<()>;
}

impl PutCell for Edit<'_> {
    fn put_cell<const N: usize>(
        &mut self,
        owner: NodeId,
        kind: &str,
        index: usize,
        blank: bool,
        fields: [(&str, Value); N],
    ) -> Result<()> {
        let name = (index + 1).to_string();
        let cell = self.at(owner).and_then(|o| o.child(&name)).map(|c| c.id());
        match (cell, blank) {
            (None, true) => Ok(()),
            (Some(c), true) => self.remove(c),
            (cell, false) => {
                let c = match cell {
                    Some(c) => c,
                    None => self.add(owner, kind, &name)?,
                };
                for (key, value) in fields {
                    self.put(c, key, value)?;
                }
                Ok(())
            }
        }
    }
}

/// A node name from a free-text one: what a path allows, the rest as `-`.
fn node_name(text: &str) -> String {
    let name: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.trim_matches('.').is_empty() {
        "untitled".into()
    } else {
        name
    }
}

fn int(n: impl Into<i64>) -> Value {
    Value::Int(n.into())
}

impl ShardEdit for Edit<'_> {
    fn add_patch(&mut self, name: &str) -> Result<NodeId> {
        let patches = format!("/{PATCHES}");
        let mut order: Vec<NodeId> = self
            .at(patches.as_str())
            .map(|c| patches_in(c).iter().map(|p| p.id()).collect())
            .unwrap_or_default();
        let patch = self.add_unique(patches.as_str(), PATCH, name)?;
        for kind in PATCH_NODES {
            self.add(patch, kind, kind)?;
        }
        order.push(patch);
        self.set_order(patches.as_str(), PATCH_ORDER, order)?;
        Ok(patch)
    }

    fn new_patch(&mut self) -> Result<NodeId> {
        let label = next_patch_name(self.at(format!("/{PATCHES}").as_str()));
        let patch = self.add_patch(PATCH)?;
        self.set_value(patch, PATCH_LABEL, Value::Text(label))?;
        // The drone is the material with no file.
        let drone = self.at(format!("/{MATERIALS}").as_str()).and_then(|m| {
            m.order(POOL)
                .into_iter()
                .find(|s| matches!(s.value("sample.path"), Some(Value::Text(p)) if p.is_empty()))
                .map(|s| s.id())
        });
        if let Some(drone) = drone {
            for g in READS_MATERIAL {
                self.wire(patch, g, drone)?;
            }
        }
        Ok(patch)
    }

    fn wire(&mut self, patch: NodeId, generator: &str, sample: NodeId) -> Result<()> {
        let g = self
            .at(patch)
            .and_then(|p| p.child(generator))
            .map(|n| n.id());
        match g {
            Some(g) => self.set_ref(g, format!("{generator}.source").as_str(), Ref::here(sample)),
            None => Ok(()),
        }
    }

    fn add_effect(&mut self, owner: NodeId, kind: Fx) -> Result<NodeId> {
        let node = self.add_unique(owner, kind.name(), kind.name())?;
        self.append_to_order(owner, CHAIN, node)?;
        Ok(node)
    }

    fn add_lfo(&mut self, patch: NodeId) -> Result<NodeId> {
        let node = self.add_unique(patch, LFO, LFO)?;
        self.append_to_order(patch, MODULATORS, node)?;
        Ok(node)
    }

    fn add_mod_env(&mut self, patch: NodeId) -> Result<NodeId> {
        let node = self.add_unique(patch, MOD_ENV, MOD_ENV)?;
        self.append_to_order(patch, MODULATORS, node)?;
        Ok(node)
    }

    fn link(&mut self, target: NodeId, key: &str, source: NodeId, lo: f32, hi: f32) -> Result<()> {
        self.bind(
            target,
            On::Value(key.to_string()),
            source,
            [
                ("lo", Value::Float(float(lo))),
                ("hi", Value::Float(float(hi))),
            ],
        )
    }

    fn add_arrangement(&mut self, tracker: &Tracker) -> Result<NodeId> {
        let arr = self.add(
            format!("/{ARRANGEMENTS}").as_str(),
            ARRANGEMENT,
            ARRANGEMENT,
        )?;
        self.add(arr, "filter", "filter")?;
        self.add(arr, MASTER, "amp")?;
        for track in &tracker.tracks {
            self.add_track(arr, track)?;
        }
        self.write_tracker(arr, tracker)?;
        Ok(arr)
    }

    fn add_track(&mut self, arr: NodeId, track: &Track) -> Result<NodeId> {
        let kind = if track.is_drums() { DRUMS } else { TRACK };
        let node = self.add_unique(arr, kind, kind)?;
        self.append_to_order(arr, TRACKS, node)?;
        self.write_track(node, track)?;
        Ok(node)
    }

    fn write_tracker(&mut self, arr: NodeId, tracker: &Tracker) -> Result<()> {
        self.put(arr, "arrangement.tempo", Value::Float(float(tracker.tempo)))?;
        self.put(arr, "arrangement.swing", Value::Float(float(tracker.swing)))?;

        let existing: Vec<NodeId> = self
            .at(arr)
            .map(|a| a.order(TRACKS).iter().map(|n| n.id()).collect())
            .unwrap_or_default();
        for (node, track) in existing.iter().zip(&tracker.tracks) {
            let drums = self.at(*node).map(|n| n.type_name() == DRUMS);
            if drums == Some(track.is_drums()) {
                self.write_track(*node, track)?;
            }
        }
        Ok(())
    }

    fn write_track(&mut self, node: NodeId, track: &Track) -> Result<()> {
        if let Some(kit) = &track.drums {
            self.put(node, "drums.on", Value::Bool(kit.on))?;
            self.put(node, "drums.gain", Value::Float(float(track.gain)))?;
            self.put(node, "drums.tune", Value::Float(float(kit.tune)))?;
            self.put(node, "drums.decay", Value::Float(float(kit.decay)))?;
            self.put(node, "drums.length", int(track.length))?;
            for i in 0..STEPS {
                let hit = |row: &Vec<u8>| row.get(i).copied().unwrap_or(0);
                let hits = [hit(&kit.kick), hit(&kit.snare), hit(&kit.hat)];
                let fields = [
                    ("beat.kick", int(hits[0])),
                    ("beat.snare", int(hits[1])),
                    ("beat.hat", int(hits[2])),
                ];
                self.put_cell(node, BEAT, i, hits == [0; 3], fields)?;
            }
            return Ok(());
        }
        self.put(node, "track.length", int(track.length))?;
        self.put(node, TRACK_GAIN, Value::Float(float(track.gain)))?;
        let blank = Step::default();
        for i in 0..STEPS {
            let s = track.steps.get(i).copied().unwrap_or(blank);
            let fields = [
                ("step.on", Value::Bool(s.on)),
                ("step.pitch", int(s.pitch)),
                ("step.hold", int(s.hold)),
                ("step.velocity", int(s.velocity)),
                ("step.nudge", int(s.nudge)),
            ];
            self.put_cell(node, STEP, i, s == blank, fields)?;
        }
        Ok(())
    }

    fn put(&mut self, node: NodeId, key: &str, want: Value) -> Result<()> {
        let Some((now, default)) = self.at(node).map(|n| {
            let default = n
                .node_type()
                .and_then(|t| t.spec(key))
                .map(|s| s.default.clone());
            (n.value(key), default)
        }) else {
            return Ok(());
        };
        if now.as_ref() == Some(&want) {
            Ok(())
        } else if default.as_ref() == Some(&want) {
            self.reset(node, key)
        } else {
            self.set_value(node, key, want)
        }
    }

    fn add_pool(&mut self, pool: &Pool) -> Result<Vec<(u64, NodeId)>> {
        let mut out = Vec::new();
        for m in &pool.materials {
            let node = self.add_unique(
                format!("/{MATERIALS}").as_str(),
                SAMPLE,
                &node_name(&m.name),
            )?;
            self.set_value(node, "sample.label", Value::Text(m.name.clone()))?;
            if let Some(path) = &m.path {
                self.set_value(node, "sample.path", Value::Text(path.clone()))?;
            }
            if m.octave != 0.0 {
                self.set_value(node, "sample.octave", int(m.octave as i64))?;
            }
            if m.trim_start != 0.0 {
                self.set_value(node, "sample.start", Value::Float(float(m.trim_start)))?;
            }
            if m.trim_end != 1.0 {
                self.set_value(node, "sample.end", Value::Float(float(m.trim_end)))?;
            }
            if let Some(root) = m.root {
                self.set_value(node, "sample.root", Value::Shaped(json!(root)))?;
            }
            self.append_to_order(format!("/{MATERIALS}").as_str(), POOL, node)?;
            out.push((m.id, node));
        }
        Ok(out)
    }
}
