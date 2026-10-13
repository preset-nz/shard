//! Shard's object model on rhizome (epic 32, story 2; guidance
//! `projects/shard/design/object-model.md`): its kinds, their roles and
//! rules, and the verbs that build a session.
//!
//! - `/patches/<patch>`: the sounds, listed in the order `order` (epic 10).
//!   A patch's name is its `patch.label`; its sections (the generators,
//!   the envelope, tape, filters and output) are children named for their
//!   kind, one of each at most. None is predefined: a write makes the
//!   section it needs (`ShardEdit::section`), and a section that is absent
//!   reads as its defaults, except a generator, which is in the patch only
//!   while its node is (Georg, 2026-10-14).
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
//!   voice or kit *k* (`tracker::Slot`). A track of either kind may be
//!   ducked by another (roadmap row 41): its reference `<kind>.duck` names
//!   the key track's node, beside its depth, release and Key only switch
//!   (`duck_keys`). Then the arrangement's own chain,
//!   filter and output (a `master`, named `amp`).
//! - `/materials/<sample>`: WAVs and the built-in drone. A generator reads
//!   one through its reference `<kind>.source`.
//! - `/globals/<lfo>`: the global LFOs (roadmap row 43), listed in the
//!   category's order `modulators`. Any patch's parameter may link one, and
//!   every patch that does follows the same LFO. A category of its own, so a
//!   file saved before it opens unchanged: rhizome makes a category a file
//!   lacks. Only LFOs live here; a mod envelope fires on its patch's notes.
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
use rhizome_pom::{Aggregate, Kinds, NodeValues, NodeValuesState, ObjectModel, Report};
use serde_json::{json, Value as Json};
use shard_dsp::arrangement::{self, TRACK_GAIN_DEFAULT, TRACK_GAIN_MAX};
use shard_dsp::duck::{DEPTH_DEFAULT, RELEASE_DEFAULT_MS, RELEASE_MAX_MS, RELEASE_MIN_MS};
use shard_dsp::fx::{self, Kind as Fx};
use shard_dsp::kit::{DECAY_MAX, DECAY_MIN, HIT_MAX, TUNE_RANGE};
use shard_dsp::modulation::{Shape as LfoShape, MAX_RATE_HZ, MIN_RATE_HZ};
use shard_dsp::params::{ParamDef, Taper, Unit, GENERATORS, PARAMS};
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

/// A material's regions as the tree holds them: a list of `{start, end}`, or
/// null for none, so a material with no regions is a default node.
pub fn regions_value(regions: &[crate::materials::Region]) -> Value {
    if regions.is_empty() {
        return Value::Shaped(serde_json::Value::Null);
    }
    Value::Shaped(serde_json::Value::Array(
        regions
            .iter()
            .map(|r| json!({ "start": float(r.start), "end": float(r.end) }))
            .collect(),
    ))
}

pub use compile::{
    arrangement_node, as_f32, compile, global_lfos, home, instance_of, locate, patch_node, patches,
    patches_in, played_by, read_tracker, to_value, track_patch, tracks, write_values, Home,
    PatchPlan, Plan,
};

pub const EXTENSION: &str = "shard";

pub const PATCHES: &str = "patches";
pub const ARRANGEMENTS: &str = "arrangements";
pub const MATERIALS: &str = "materials";
/// The global LFOs' category.
pub const GLOBALS: &str = "globals";

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
/// The order of a patch's LFOs and mod envelopes, and of the global LFOs.
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

/// A track's sidechain (roadmap row 41), by its kind: `by` the reference to
/// the track whose steps dip its volume (unset, or naming a track that has
/// gone, it is not ducked), how far a full-velocity step dips it (0 to 1,
/// silence), how long the dip takes to come back (ms), and Key only:
/// silent, its steps still keying whatever it ducks. Edits, saved in the
/// tree; a file without them opens not ducked and heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuckKeys {
    pub by: &'static str,
    pub depth: &'static str,
    pub release: &'static str,
    pub key_only: &'static str,
}

pub const TRACK_DUCK: DuckKeys = DuckKeys {
    by: "track.duck",
    depth: "track.duck-depth",
    release: "track.duck-release",
    key_only: "track.key-only",
};
pub const DRUMS_DUCK: DuckKeys = DuckKeys {
    by: "drums.duck",
    depth: "drums.duck-depth",
    release: "drums.duck-release",
    key_only: "drums.key-only",
};

/// A track node's sidechain keys, by its kind.
pub fn duck_keys(track: &Node<'_>) -> DuckKeys {
    if track.type_name() == DRUMS {
        DRUMS_DUCK
    } else {
        TRACK_DUCK
    }
}

/// A track's sidechain values, declared on its kind.
fn with_duck(t: NodeType, keys: DuckKeys) -> NodeType {
    t.float(keys.depth, 0.0..=1.0, float(DEPTH_DEFAULT))
        .described(
            "Duck depth",
            "How far a full-strength step on the key track dips this track; 1 is silence.",
        )
        .float(
            keys.release,
            float(RELEASE_MIN_MS)..=float(RELEASE_MAX_MS),
            float(RELEASE_DEFAULT_MS),
        )
        .described("Duck release", "How long the dip takes to come back.")
        .unit("ms")
        .bool(keys.key_only, false)
        .described(
            "Key only",
            "Silences this track while its steps still dip whatever it ducks.",
        )
        .reference(keys.by)
}

/// A track is ducked by another track, never by itself.
fn duck_holds(n: &Node<'_>) -> std::result::Result<(), String> {
    match n.resolve(duck_keys(n).by) {
        Some(k) if !is_track(&k) => Err(format!("a track is ducked by a track, not {}", k.path())),
        Some(k) if k.id() == n.id() => Err("a track can't be ducked by itself".into()),
        _ => Ok(()),
    }
}

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

/// A patch's sections, each a child named for its kind, one at most. A new
/// patch has only what it was written (its material); the first write to a
/// section's row makes it (`ShardEdit::section`). An absent section reads as
/// its defaults, and an absent generator (`GENERATORS`) as off: a generator is
/// in the patch while its node is, and Remove deletes it.
pub const PATCH_NODES: &[&str] = &[
    "material", "grain", "fm", "noise", "dust", "env", "tape", "filter", "ladder", "amp",
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
    /// A patch, or the globals category: LFOs.
    PatchOrGlobals,
}

/// One kind as Shard declares it.
pub struct KindDecl {
    pub node_type: NodeType,
    pub role: Role,
    /// Never removed on its own: one of a patch's sections other than a
    /// generator, or the arrangement's output.
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
    let spec = match def.taper {
        Taper::Stepped(_) if def.id.ends_with(".on") => ValueSpec::bool(def.id, def.default >= 0.5),
        Taper::Stepped(_) => {
            ValueSpec::int(def.id, def.min as i64..=def.max as i64, def.default as i64)
        }
        _ => ValueSpec::float(def.id, float(def.min)..=float(def.max), float(def.default)),
    }
    .described(def.name, def.description);
    let spec = match unit_text(def.unit) {
        Some(u) => spec.unit(u),
        None => spec,
    };
    if def.steps.is_empty() {
        spec
    } else {
        spec.names(def.steps)
    }
}

/// A unit as the pages and tooltips write it. Percent rows hold a fraction (0
/// to 1) in the tree, and rhizome-docs shows a "%" value scaled by 100.
fn unit_text(unit: Unit) -> Option<&'static str> {
    match unit {
        Unit::Ms => Some("ms"),
        Unit::Seconds => Some("s"),
        Unit::Hz => Some("Hz"),
        Unit::Semitones => Some("st"),
        Unit::Octaves => Some("oct"),
        Unit::Db => Some("dB"),
        Unit::Percent => Some("%"),
        Unit::None => None,
    }
}

/// A kind's words, written by hand where no switch row speaks for it.
fn words(t: NodeType, label: &str, summary: &str, group: &str) -> NodeType {
    t.label(label).summary(summary).group(group)
}

/// A kind with a switch row (`<kind>.on`) is named and summed up by it: the
/// row's name is the label, its description the summary.
fn words_from_switch(t: NodeType, rows: &[ParamDef], group: &str) -> NodeType {
    match rows.iter().find(|r| r.id.ends_with(".on")) {
        Some(sw) => words(t, sw.name, sw.description, group),
        None => t,
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
        ValueSpec::float("lo", 0.0..=1.0, 0.0).described(
            "Low",
            "Where the linked control sits when the modulator is at its lowest, from the bottom to the top of its range.",
        ),
        ValueSpec::float("hi", 0.0..=1.0, 1.0).described(
            "High",
            "Where the linked control sits when the modulator is at its highest, from the bottom to the top of its range.",
        ),
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
        Under::PatchOrGlobals => {
            parent.type_name() == PATCH || (parent.is_category() && parent.name() == GLOBALS)
        }
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

/// Whether a node is a global LFO: an LFO in the globals category.
pub fn is_global(node: &Node<'_>) -> bool {
    node.type_name() == LFO
        && node
            .parent()
            .is_some_and(|p| p.is_category() && p.name() == GLOBALS)
}

/// Links onto a node: each onto a continuous parameter (a set of choices
/// can't follow a modulator, as `LinkError::Stepped` says, and a modulator's
/// own settings are not parameters), at most one per value, and only from a
/// modulator of the node's own patch or a global LFO. The arrangement's
/// nodes follow nothing yet, globals included.
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
        let own = patch.is_some() && b.source.parent().map(|p| p.id()) == patch;
        let global = patch.is_some() && is_global(&b.source);
        if !own && !global {
            return Err(format!(
                "{key} can follow only a modulator of its own patch or a global LFO"
            ));
        }
    }
    Ok(())
}

/// What a preset's skipped list says of a link it left out because it follows
/// a modulator in another patch: the linked value's key, then this.
pub const OTHER_PATCH_LINK: &str = " ← an LFO in another patch";

/// `NodeValues` for a preset, except that a link `links_hold` would refuse
/// (a source that is neither a modulator of this node's patch nor a global
/// LFO) is left out and reported, so the rest of the preset still applies
/// rather than the whole edit failing at its check.
struct PresetValues(NodeValues);

impl Aggregate for PresetValues {
    type State = NodeValuesState;

    fn get(&self, node: Node<'_>) -> NodeValuesState {
        self.0.get(node)
    }

    fn set(&self, tx: &mut Edit<'_>, node: NodeId, state: &NodeValuesState) -> Result<Report> {
        let patch = tx.at(node).and_then(|n| {
            if n.type_name() == PATCH {
                Some(n.id())
            } else {
                n.parent()
                    .filter(|p| p.type_name() == PATCH)
                    .map(|p| p.id())
            }
        });
        let mut kept = state.clone();
        kept.bindings.clear();
        let mut left_out = Vec::new();
        for b in &state.bindings {
            // A source that is gone is the inner apply's to report.
            let usable = tx.at(b.source).is_none_or(|s| {
                patch.is_some() && (s.parent().map(|p| p.id()) == patch || is_global(&s))
            });
            if usable {
                kept.bindings.push(b.clone());
            } else {
                let key = match &b.on {
                    On::Value(k) => k.clone(),
                    On::Slot(k) => k.clone(),
                };
                left_out.push(format!("{key}{OTHER_PATCH_LINK}"));
            }
        }
        let mut report = self.0.set(tx, node, &kept)?;
        report.skipped.extend(left_out);
        Ok(report)
    }

    fn matches(&self, current: &NodeValuesState, preset: &NodeValuesState) -> bool {
        self.0.matches(current, preset)
    }

    fn portable(&self, state: NodeValuesState) -> NodeValuesState {
        self.0.portable(state)
    }
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
    .described("Name", "What the patch is called in the scene view.")
    .shaped(
        "patch.map",
        Shape::optional(Shape::record([("id", Shape::Int), ("name", Shape::Text)])),
    )
    .described(
        "Controller map",
        "The controller map this patch plays with, by its id in the app's own list.",
    );
    let patch = words(
        patch,
        "Patch",
        "One sound: its generators, effects and settings, played by one or more tracks.",
        "patch",
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

    // A patch's sections. A generator is added and removed by the tree; the
    // rest stay once made.
    for &name in PATCH_NODES {
        let rows = patch_rows(name);
        let mut t = with_rows(NodeType::new(name), &rows);
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
        t = match role {
            Role::Generator => words_from_switch(t, &rows, "generator"),
            Role::Effect => words_from_switch(t, &rows, "effect"),
            _ => words(
                t,
                "Tape",
                "How the tape slows, stops or runs backwards when a brake or reverse is played.",
                "patch",
            ),
        };
        if name == "material" {
            // The row says "Material"; the work area has always called the
            // generator Sample.
            t = t.label("Sample");
        }
        if name == "amp" {
            t = words(
                t,
                "Output",
                "The patch's master level, after every generator and effect.",
                "patch",
            );
        }
        let under = if name == "filter" {
            Under::Either
        } else {
            Under::Patch
        };
        let fixed = role != Role::Generator;
        push(checked(t, under, none), role, fixed, Some(1));
    }

    // The palette.
    for kind in Fx::ALL {
        let rows = effect_rows(kind);
        let t = with_rows(NodeType::new(kind.name()), &rows);
        let t = words_from_switch(t, &rows, "effect");
        push(
            checked(t, Under::Either, none),
            Role::Effect,
            false,
            Some(fx::COPIES),
        );
    }

    // Modulators, the sources a link follows. An LFO lives in a patch, or in
    // the globals category, listed in its order there.
    let lfo = NodeType::new(LFO)
        .in_categories(&[PATCHES, GLOBALS])
        .text("lfo.label", "")
        .described("Name", "What the LFO is called in the list of modulators.")
        .float(
            "lfo.rate",
            float(MIN_RATE_HZ)..=float(MAX_RATE_HZ),
            float(NEW_RATE_HZ),
        )
        .described("Rate", "How many times a second the wave goes round.")
        .unit("Hz")
        .choice("lfo.shape", &LfoShape::NAMES, NEW_SHAPE)
        .described(
            "Shape",
            "The wave it draws: smooth, stepped, random, or one of the curves.",
        )
        .float("lfo.phase", 0.0..=1.0, 0.0)
        .described(
            "Phase",
            "Where in the wave it starts, as a fraction of one turn.",
        )
        .bindable(link_values());
    let lfo = words(
        lfo,
        "LFO",
        "A slow wave that a control can follow, so it drifts, wobbles or jumps by itself.",
        "modulator",
    );
    push(
        checked(lfo, Under::PatchOrGlobals, |n| {
            if is_global(n) {
                listed_in(n, MODULATORS)
            } else {
                Ok(())
            }
        }),
        Role::Modulator,
        false,
        None,
    );

    let mut curve = ValueSpec::shaped("mod-env.curve", curve_shape());
    curve.default = Value::Shaped(curve_json(&pluck_curve()));
    let env = NodeType::new(MOD_ENV)
        .in_categories(&[PATCHES])
        .text("mod-env.label", "")
        .described(
            "Name",
            "What the envelope is called in the list of modulators.",
        )
        .choice("mod-env.mode", &MODES, MODES[0])
        .described(
            "Mode",
            "ADSR draws the curve with four sliders; free lets you place the points yourself.",
        )
        .value(curve.described(
            "Curve",
            "The shape a linked control follows over the length of a note.",
        ))
        .bindable(link_values());
    let env = words(
        env,
        "Mod envelope",
        "A curve drawn over each note that a control can follow, so it swells, dips or snaps back as the note plays.",
        "modulator",
    );
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
        .described("Tempo", "How fast the steps play, in beats a minute.")
        .unit("BPM")
        .float(
            "arrangement.swing",
            0.0..=float(SWING_MAX),
            float(blank.swing),
        )
        .described(
            "Swing",
            "How far the even steps are pushed late, as a fraction of a sixteenth; 0 is straight.",
        );
    let arr = words(
        arr,
        "Arrangement",
        "The level above the patches: tempo, swing and the tracks of steps that play them.",
        "arrangement",
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
        .described("Level", "The track's fader; 1 leaves the patch as it is.")
        .int("track.length", 4..=STEPS as i64, i64::from(track.length))
        .described(
            "Length",
            "How many steps the track plays before it goes round again.",
        )
        .unit("steps")
        .bool(TRACK_MUTE, false)
        .described(
            "Mute",
            "Silences the track. Kept in the file, not in the undo history.",
        )
        .bool(TRACK_SOLO, false)
        .described(
            "Solo",
            "Plays only the soloed tracks. Kept in the file, not in the undo history.",
        )
        .reference(TRACK_PATCH);
    let t = with_duck(t, TRACK_DUCK);
    let t = words(
        t,
        "Track",
        "A row of steps that plays one patch as a melody.",
        "arrangement",
    );
    push(
        checked(t, Under::Arrangement, |n| {
            length_holds(n, "track.length")?;
            duck_holds(n)?;
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
        .described("On", "Whether the step plays a note.")
        .int(
            "step.pitch",
            -i64::from(PITCH_RANGE)..=i64::from(PITCH_RANGE),
            i64::from(step.pitch),
        )
        .described("Pitch", "How far the note is moved from the sample's own pitch.")
        .unit("st")
        .int("step.hold", 0..=i64::from(HOLD_MAX), i64::from(step.hold))
        .described(
            "Hold",
            "How many sixteenths the note is held before its release, when the patch's Length is Hold.",
        )
        .int(
            "step.velocity",
            0..=i64::from(VELOCITY_MAX),
            i64::from(step.velocity),
        )
        .described("Velocity", "How hard the note is hit, as MIDI counts it.")
        .int(
            "step.nudge",
            -i64::from(NUDGE_MAX_MS)..=i64::from(NUDGE_MAX_MS),
            i64::from(step.nudge),
        )
        .described("Nudge", "Moves the note earlier or later than the grid.")
        .unit("ms");
    let t = words(
        t,
        "Step",
        "One position in a track's row: whether it plays, and how.",
        "arrangement",
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
        .described("On", "Whether the drum track sounds.")
        .float(
            "drums.gain",
            0.0..=float(TRACK_GAIN_MAX),
            float(TRACK_GAIN_DEFAULT),
        )
        .described("Level", "The track's fader; 1 leaves the kit as it is.")
        .float(
            "drums.tune",
            float(-TUNE_RANGE)..=float(TUNE_RANGE),
            float(kit.tune),
        )
        .described("Tune", "Moves the whole kit up or down in pitch.")
        .unit("st")
        .float(
            "drums.decay",
            float(DECAY_MIN)..=float(DECAY_MAX),
            float(kit.decay),
        )
        .described(
            "Decay",
            "How long each drum rings; below 1 is tighter, above is longer.",
        )
        .int("drums.length", 4..=STEPS as i64, i64::from(track.length))
        .described(
            "Length",
            "How many steps the track plays before it goes round again.",
        )
        .unit("steps")
        .bool(DRUMS_MUTE, false)
        .described(
            "Mute",
            "Silences the track. Kept in the file, not in the undo history.",
        )
        .bool(DRUMS_SOLO, false)
        .described(
            "Solo",
            "Plays only the soloed tracks. Kept in the file, not in the undo history.",
        );
    let t = with_duck(t, DRUMS_DUCK);
    let t = words(
        t,
        "Drums",
        "A drum kit played by a grid of kick, snare and hat hits.",
        "arrangement",
    );
    push(
        checked(t, Under::Arrangement, |n| {
            length_holds(n, "drums.length")?;
            duck_holds(n)
        }),
        Role::Generator,
        false,
        Some(KITS),
    );

    let hit = |key: &str, label: &str, what: &str| {
        ValueSpec::int(key, 0..=i64::from(HIT_MAX), 0).described(label, what)
    };
    let t = NodeType::new(BEAT)
        .in_categories(&[ARRANGEMENTS])
        .value(hit(
            "beat.kick",
            "Kick",
            "How hard the kick hits on this step; 0 is silent.",
        ))
        .value(hit(
            "beat.snare",
            "Snare",
            "How hard the snare hits on this step; 0 is silent.",
        ))
        .value(hit(
            "beat.hat",
            "Hat",
            "How hard the hat hits on this step; 0 is silent.",
        ));
    let t = words(
        t,
        "Beat",
        "One position in a drums track's grid: how hard each drum hits there.",
        "arrangement",
    );
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
    let t = words(
        t,
        "Output",
        "The arrangement's master level and limiter, after every track and effect.",
        "arrangement",
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
        .described(
            "Name",
            "What the sample is called in the list of materials.",
        )
        // Empty for the built-in drone, which has no file.
        .text("sample.path", "")
        .described(
            "File",
            "Where the WAV lives on disk; empty for the built-in drone.",
        )
        .int(
            "sample.octave",
            -(OCTAVE_RANGE as i64)..=OCTAVE_RANGE as i64,
            0,
        )
        .described(
            "Octave",
            "Moves the sample up or down whole octaves before anything plays it.",
        )
        .unit("oct")
        .float("sample.start", 0.0..=1.0, 0.0)
        .described("Start", "Where playback begins, as a fraction of the file.")
        .float("sample.end", 0.0..=1.0, 1.0)
        .described("End", "Where playback ends, as a fraction of the file.")
        // The MIDI note it sounds at, when known.
        .shaped("sample.root", Shape::optional(Shape::Int))
        .described(
            "Root note",
            "The MIDI note the sample sounds at, when known.",
        )
        // Marked regions, as fractions of the file (row 49); null for none.
        .shaped(
            "sample.regions",
            Shape::optional(Shape::list(Shape::record([
                ("start", Shape::Float),
                ("end", Shape::Float),
            ]))),
        )
        .described(
            "Regions",
            "Marked stretches of the file, as fractions of its length.",
        );
    let t = words(
        t,
        "Material",
        "A WAV file, or the built-in drone, that a generator reads and plays.",
        "material",
    );
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
            .category(MATERIALS, Origin::Loaded)
            .category(GLOBALS, Origin::Loaded);
        for decl in kinds() {
            // Every node the parameter table backs keeps presets: the
            // generators and effects, and the patch, tape and output too.
            let presets = matches!(decl.role, Role::Generator | Role::Effect | Role::Fixed);
            let mut kind = k.kind(decl.node_type);
            if decl.fixed {
                kind = kind.not_deletable().not_duplicable();
            } else if decl.role == Role::Generator {
                // One per patch: removed and added again, never copied
                // beside itself.
                kind = kind.not_duplicable();
            }
            if let Some(n) = decl.max_per_parent {
                kind = kind.max_per_parent(n);
            }
            // A preset is a node's sound: its values and links, never its
            // switch, so applying one never switches a node in or out, and
            // never a patch's name, a drums track's mute and solo, or its
            // sidechain, which is routing, not sound.
            if presets {
                kind.presets(PresetValues(
                    NodeValues::new()
                        .skip(|key| {
                            key.ends_with(".on")
                                || key == PATCH_LABEL
                                || key == DRUMS_MUTE
                                || key == DRUMS_SOLO
                                || key == DRUMS_DUCK.depth
                                || key == DRUMS_DUCK.release
                                || key == DRUMS_DUCK.key_only
                        })
                        .with_bindings(),
                ));
            }
        }
    }

    /// Rebuilt whole after every change: a session is small, and compiling
    /// it is cheap beside a block of audio.
    fn project(tree: &Tree, into: &mut Plan, _: Option<&Changeset>) {
        compile(tree, into);
    }

    /// A new session: one patch at its defaults, "Patch 1", its material
    /// reading the built-in drone, and the arrangement as a new tracker has it,
    /// its track playing that patch.
    fn seed(tx: &mut Edit<'_>, _: &()) -> Result<()> {
        // The drone first, so the new patch's material reads it.
        tx.add_pool(&Pool::with_drone())?;
        let patch = tx.new_patch()?;
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
    /// An empty patch: no sections and an empty chain, at the end of the
    /// patches' order. Its node is named `name`; its label is left empty.
    fn add_patch(&mut self, name: &str) -> Result<NodeId>;
    /// What New Patch adds: a patch at its defaults named "Patch N" holding
    /// only its material, which reads the built-in drone when the pool still
    /// has it. The material is on by default, so a new patch sounds.
    fn new_patch(&mut self) -> Result<NodeId>;
    /// `patch`'s section `kind` (`grain`, `env`), made when it has none: what
    /// every write to a section's row goes through, in the write's own edit.
    /// A generator made here that reads a material reads the built-in drone,
    /// as a new patch's material does. Made, a generator is present but its
    /// switch is its default; Add Generator switches it on.
    fn section(&mut self, patch: NodeId, kind: &str) -> Result<NodeId>;
    /// An effect at the end of `owner`'s chain, a patch's or the arrangement's.
    fn add_effect(&mut self, owner: NodeId, kind: Fx) -> Result<NodeId>;
    fn add_lfo(&mut self, patch: NodeId) -> Result<NodeId>;
    /// A global LFO, at the end of the globals' order.
    fn add_global_lfo(&mut self) -> Result<NodeId>;
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
    /// A track's sidechain depth, release and Key only, writing only what
    /// differs. Who keys it is `Session::set_track_duck`'s, never written
    /// here.
    fn write_duck(&mut self, node: NodeId, keys: DuckKeys, track: &Track) -> Result<()>;
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
        order.push(patch);
        self.set_order(patches.as_str(), PATCH_ORDER, order)?;
        Ok(patch)
    }

    fn new_patch(&mut self) -> Result<NodeId> {
        let label = next_patch_name(self.at(format!("/{PATCHES}").as_str()));
        let patch = self.add_patch(PATCH)?;
        self.set_value(patch, PATCH_LABEL, Value::Text(label))?;
        self.section(patch, "material")?;
        Ok(patch)
    }

    fn section(&mut self, patch: NodeId, kind: &str) -> Result<NodeId> {
        if let Some(n) = self.at(patch).and_then(|p| p.child(kind)) {
            return Ok(n.id());
        }
        let node = self.add(patch, kind, kind)?;
        if READS_MATERIAL.contains(&kind) {
            // The drone is the material with no file.
            let drone = self.at(format!("/{MATERIALS}").as_str()).and_then(|m| {
                m.order(POOL)
                    .into_iter()
                    .find(
                        |s| matches!(s.value("sample.path"), Some(Value::Text(p)) if p.is_empty()),
                    )
                    .map(|s| s.id())
            });
            if let Some(drone) = drone {
                self.set_ref(node, format!("{kind}.source").as_str(), Ref::here(drone))?;
            }
        }
        Ok(node)
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

    fn add_global_lfo(&mut self) -> Result<NodeId> {
        let globals = format!("/{GLOBALS}");
        let node = self.add_unique(globals.as_str(), LFO, LFO)?;
        self.append_to_order(globals.as_str(), MODULATORS, node)?;
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
            self.write_duck(node, DRUMS_DUCK, track)?;
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
        self.write_duck(node, TRACK_DUCK, track)?;
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

    fn write_duck(&mut self, node: NodeId, keys: DuckKeys, track: &Track) -> Result<()> {
        let d = &track.duck;
        self.put(node, keys.depth, Value::Float(float(d.depth)))?;
        self.put(node, keys.release, Value::Float(float(d.release)))?;
        self.put(node, keys.key_only, Value::Bool(d.key_only))
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
            if !m.regions.is_empty() {
                self.set_value(node, "sample.regions", regions_value(&m.regions))?;
            }
            self.append_to_order(format!("/{MATERIALS}").as_str(), POOL, node)?;
            out.push((m.id, node));
        }
        Ok(out)
    }
}
