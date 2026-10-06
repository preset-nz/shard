//! Shard's object model on rhizome (epic 32, story 2; guidance
//! `projects/shard/design/object-model.md`): its kinds, their roles and
//! rules, and the verbs that build a session.
//!
//! - `/patches/<patch>`: the sound. Its fixed nodes (the generators, the
//!   envelope, tape, filter and output) are children named for their kind.
//!   The effects it runs are children listed in its order `chain`, its LFOs
//!   and mod envelopes children listed in `modulators`. A link is a binding
//!   from a parameter onto one of those, carrying `lo` and `hi`.
//! - `/arrangements/arrangement`: the level above the patch. Tempo and swing;
//!   tracks in the order `tracks`, each step a child named by its number and
//!   made only once it differs from a blank step; the kit, its beats made the
//!   same way; its own chain, filter and output (a `master`, named `amp`).
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
    Edit, Node, NodeId, NodeType, On, Origin, Ref, Result, Shape, Value, ValueKind, ValueSpec,
};
use rhizome_pom::{Kinds, NodeValues, ObjectModel};
use serde_json::json;
use shard_dsp::arrangement;
use shard_dsp::fx::{self, Kind as Fx};
use shard_dsp::kit::HIT_MAX;
use shard_dsp::modulation::{Shape as LfoShape, MAX_RATE_HZ, MIN_RATE_HZ};
use shard_dsp::params::{ParamDef, Taper, GENERATORS, PARAMS};
use shard_dsp::sources::OCTAVE_RANGE;
use shard_dsp::steps::{HOLD_MAX, NUDGE_MAX_MS, PITCH_RANGE, STEPS, VELOCITY_MAX};

use crate::materials::Pool;
use crate::modulation::{new_envelope, MAX_STAGE_MS, NEW_RATE_HZ, NEW_SHAPE};
use crate::tracker::{Kit, Step, Track, Tracker, LENGTHS, SWING_MAX, TEMPO_MAX, TEMPO_MIN};

#[cfg(test)]
mod tests;

pub const EXTENSION: &str = "shard";

pub const PATCHES: &str = "patches";
pub const ARRANGEMENTS: &str = "arrangements";
pub const MATERIALS: &str = "materials";

pub const PATCH: &str = "patch";
pub const ARRANGEMENT: &str = "arrangement";
pub const TRACK: &str = "track";
pub const STEP: &str = "step";
pub const KIT: &str = "kit";
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

/// Played, not edited: a held button or a pedal writes them to the bank, and
/// a document never stores them.
pub const PLAYED: &[&str] = &["tape.brake", "tape.reverse"];

/// A patch's fixed nodes, each a child named for its kind.
pub const PATCH_NODES: &[&str] = &["material", "grain", "fm", "env", "tape", "filter", "amp"];

/// The generators that read a material, through `<kind>.source`.
pub const READS_MATERIAL: &[&str] = &["material", "grain"];

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
    Kit,
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
        .filter(|p| key_prefix(p.id) == prefix && !PLAYED.contains(&p.id))
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
        _ => ValueSpec::float(
            def.id,
            f64::from(def.min)..=f64::from(def.max),
            f64::from(def.default),
        ),
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
        Under::Kit => parent.type_name() == KIT,
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

/// Links onto a node: each onto a continuous value (a set of choices can't
/// follow a modulator, as `LinkError::Stepped` says), at most one per value,
/// and only from a modulator of the node's own patch.
fn links_hold(node: &Node<'_>) -> std::result::Result<(), String> {
    let patch = if node.type_name() == PATCH {
        Some(node.id())
    } else {
        node.parent()
            .filter(|p| p.type_name() == PATCH)
            .map(|p| p.id())
    };
    let mut seen: Vec<&str> = Vec::new();
    for b in node.bindings() {
        let On::Value(key) = b.on else {
            return Err("a link is onto a value, not a slot".into());
        };
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

fn is_effect(node: &Node<'_>) -> bool {
    Fx::from_name(node.type_name()).is_some()
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
    // controller map it plays with (app data, referred to by id).
    let patch = with_rows(
        NodeType::new(PATCH).in_categories(&[PATCHES]),
        &patch_rows("patch"),
    )
    .shaped(
        "patch.map",
        Shape::optional(Shape::record([("id", Shape::Int), ("name", Shape::Text)])),
    );
    push(
        checked(patch, Under::Category, |n| {
            order_holds(n, CHAIN, is_effect)?;
            order_holds(n, MODULATORS, is_modulator)
        }),
        Role::Fixed,
        false,
        // One patch until multi-patch.
        Some(1),
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
            f64::from(MIN_RATE_HZ)..=f64::from(MAX_RATE_HZ),
            f64::from(NEW_RATE_HZ),
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

    let e = new_envelope(0);
    let stage =
        |key: &str, ms: f32| ValueSpec::float(key, 0.0..=f64::from(MAX_STAGE_MS), f64::from(ms));
    let env = NodeType::new(MOD_ENV)
        .in_categories(&[PATCHES])
        .text("mod-env.label", "")
        .value(stage("mod-env.attack", e.attack))
        .value(stage("mod-env.decay", e.decay))
        .float("mod-env.sustain", 0.0..=1.0, f64::from(e.sustain))
        .value(stage("mod-env.release", e.release))
        .bindable(link_values());
    push(
        checked(env, Under::Patch, none),
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
            f64::from(TEMPO_MIN)..=f64::from(TEMPO_MAX),
            f64::from(blank.tempo),
        )
        .float(
            "arrangement.swing",
            0.0..=f64::from(SWING_MAX),
            f64::from(blank.swing),
        );
    push(
        checked(arr, Under::Category, |n| {
            order_holds(n, CHAIN, is_effect)?;
            order_holds(n, TRACKS, |c| c.type_name() == TRACK)
        }),
        Role::Structure,
        true,
        Some(1),
    );

    // A track, with the patch's fader (one today, read from the first track,
    // as only the first track sounds).
    let track = Track::default();
    let t = NodeType::new(TRACK)
        .in_categories(&[ARRANGEMENTS])
        .bool("track.on", track.on);
    let t = with_rows(t, &arrangement_rows("track")).int(
        "track.length",
        4..=STEPS as i64,
        i64::from(track.length),
    );
    push(
        checked(t, Under::Arrangement, |n| length_holds(n, "track.length")),
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

    // The drum kit beside the tracks: a generator, its grid as beats.
    let t = with_rows(
        NodeType::new(KIT).in_categories(&[ARRANGEMENTS]),
        &arrangement_rows("kit"),
    )
    .int(
        "kit.length",
        4..=STEPS as i64,
        i64::from(Kit::default().length),
    );
    push(
        checked(t, Under::Arrangement, |n| length_holds(n, "kit.length")),
        Role::Generator,
        true,
        Some(1),
    );

    let hit = |key: &str| ValueSpec::int(key, 0..=i64::from(HIT_MAX), 0);
    let t = NodeType::new(BEAT)
        .in_categories(&[ARRANGEMENTS])
        .value(hit("beat.kick"))
        .value(hit("beat.snare"))
        .value(hit("beat.hat"));
    push(
        checked(t, Under::Kit, |n| {
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
        checked(t, Under::Category, none),
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
    type Projection = ();
    type Context = ();

    fn kinds(k: &mut Kinds, _: &()) {
        k.category(PATCHES, Origin::Loaded)
            .category(ARRANGEMENTS, Origin::Loaded)
            .category(MATERIALS, Origin::Loaded);
        for decl in kinds() {
            let presets = matches!(decl.role, Role::Generator | Role::Effect);
            let mut kind = k.kind(decl.node_type);
            if decl.fixed {
                kind = kind.not_deletable().not_duplicable();
            }
            if let Some(n) = decl.max_per_parent {
                kind = kind.max_per_parent(n);
            }
            // A preset is a node's sound: its values and links, never its
            // switch, so applying one never switches a node in or out.
            if presets {
                kind.presets(
                    NodeValues::new()
                        .skip(|key| key.ends_with(".on"))
                        .with_bindings(),
                );
            }
        }
    }

    /// A new session: the built-in drone in both generators, one patch at
    /// its defaults, and the arrangement as a new tracker has it.
    fn seed(tx: &mut Edit<'_>, _: &()) -> Result<()> {
        let pool = Pool::with_drone();
        let samples = tx.add_pool(&pool)?;
        let patch = tx.add_patch(PATCH)?;
        for (name, wired) in [
            ("material", pool.wires.material),
            ("grain", pool.wires.grain),
        ] {
            let sample = wired.and_then(|id| samples.iter().find(|(m, _)| *m == id));
            if let Some((_, node)) = sample {
                let generator = tx.at(patch).and_then(|p| p.child(name)).map(|n| n.id());
                if let Some(g) = generator {
                    tx.set_ref(g, format!("{name}.source").as_str(), Ref::here(*node))?;
                }
            }
        }
        tx.add_arrangement(&Tracker::default())?;
        Ok(())
    }
}

/// Shard's verbs. Each composes into the caller's edit, so a verb is one undo
/// step however many writes it makes.
pub trait ShardEdit {
    /// A patch at its defaults, with its fixed nodes and an empty chain.
    fn add_patch(&mut self, name: &str) -> Result<NodeId>;
    /// An effect at the end of `owner`'s chain, a patch's or the arrangement's.
    fn add_effect(&mut self, owner: NodeId, kind: Fx) -> Result<NodeId>;
    fn add_lfo(&mut self, patch: NodeId) -> Result<NodeId>;
    fn add_mod_env(&mut self, patch: NodeId) -> Result<NodeId>;
    /// `key` on `target` follows `source`, from `lo` to `hi` of its range.
    fn link(&mut self, target: NodeId, key: &str, source: NodeId, lo: f32, hi: f32) -> Result<()>;
    /// The arrangement, its tracks and kit written from a tracker, with its
    /// filter and output at their defaults.
    fn add_arrangement(&mut self, tracker: &Tracker) -> Result<NodeId>;
    /// Every material in a pool, with the node each became.
    fn add_pool(&mut self, pool: &Pool) -> Result<Vec<(u64, NodeId)>>;
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
        let patch = self.add_unique(format!("/{PATCHES}").as_str(), PATCH, name)?;
        for kind in PATCH_NODES {
            self.add(patch, kind, kind)?;
        }
        Ok(patch)
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
                ("lo", Value::Float(f64::from(lo))),
                ("hi", Value::Float(f64::from(hi))),
            ],
        )
    }

    fn add_arrangement(&mut self, tracker: &Tracker) -> Result<NodeId> {
        let blank = Tracker::default();
        let arr = self.add(
            format!("/{ARRANGEMENTS}").as_str(),
            ARRANGEMENT,
            ARRANGEMENT,
        )?;
        if tracker.tempo != blank.tempo {
            self.set_value(arr, "arrangement.tempo", Value::Float(tracker.tempo.into()))?;
        }
        if tracker.swing != blank.swing {
            self.set_value(arr, "arrangement.swing", Value::Float(tracker.swing.into()))?;
        }

        let fresh_track = Track::default();
        let blank_step = Step::default();
        for t in &tracker.tracks {
            let node = self.add_unique(arr, TRACK, TRACK)?;
            self.append_to_order(arr, TRACKS, node)?;
            if t.on != fresh_track.on {
                self.set_value(node, "track.on", Value::Bool(t.on))?;
            }
            if t.length != fresh_track.length {
                self.set_value(node, "track.length", int(t.length))?;
            }
            for (i, s) in t.steps.iter().take(STEPS).enumerate() {
                if *s == blank_step {
                    continue;
                }
                let step = self.add(node, STEP, &(i + 1).to_string())?;
                let fields = [
                    ("step.on", Value::Bool(s.on), s.on != blank_step.on),
                    ("step.pitch", int(s.pitch), s.pitch != blank_step.pitch),
                    ("step.hold", int(s.hold), s.hold != blank_step.hold),
                    (
                        "step.velocity",
                        int(s.velocity),
                        s.velocity != blank_step.velocity,
                    ),
                    ("step.nudge", int(s.nudge), s.nudge != blank_step.nudge),
                ];
                for (key, value, differs) in fields {
                    if differs {
                        self.set_value(step, key, value)?;
                    }
                }
            }
        }

        let kit = self.add(arr, KIT, KIT)?;
        if tracker.kit.length != Kit::default().length {
            self.set_value(kit, "kit.length", int(tracker.kit.length))?;
        }
        for i in 0..STEPS {
            let hits = [
                ("beat.kick", tracker.kit.kick.get(i)),
                ("beat.snare", tracker.kit.snare.get(i)),
                ("beat.hat", tracker.kit.hat.get(i)),
            ];
            if hits.iter().all(|(_, h)| h.copied().unwrap_or(0) == 0) {
                continue;
            }
            let beat = self.add(kit, BEAT, &(i + 1).to_string())?;
            for (key, h) in hits {
                let h = h.copied().unwrap_or(0);
                if h != 0 {
                    self.set_value(beat, key, int(h))?;
                }
            }
        }

        self.add(arr, "filter", "filter")?;
        self.add(arr, MASTER, "amp")?;
        Ok(arr)
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
                self.set_value(node, "sample.start", Value::Float(m.trim_start.into()))?;
            }
            if m.trim_end != 1.0 {
                self.set_value(node, "sample.end", Value::Float(m.trim_end.into()))?;
            }
            if let Some(root) = m.root {
                self.set_value(node, "sample.root", Value::Shaped(json!(root)))?;
            }
            out.push((m.id, node));
        }
        Ok(out)
    }
}
