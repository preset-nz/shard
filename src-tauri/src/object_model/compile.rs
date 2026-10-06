//! The projection: the tree compiled into what the engine reads today
//! (epic 32, story 2, slice B).
//!
//! One patch. The N-patch `Plan` in `object-model.md`, with its own bank and
//! a crossfade on structural edits, waits for multi-patch. Until then a plan
//! is today's inputs, rebuilt from the tree after every change:
//!
//! - the patch's values, indexed as `PARAMS`, and the arrangement's, indexed
//!   as `arrangement::params()`. An effect node plays a pool instance by its
//!   place in the chain: the k-th of a kind is copy k, so the same tree
//!   always compiles to the same ids, which controller maps store;
//! - the modulators and links as `Modulation` records, built into a `ModSet`
//!   by the code that builds one today;
//! - the arrangement as a `Tracker`, and the materials as a `Pool`.
//!
//! Records carry runtime ids for modulators and materials, as the engine and
//! the material cache want. They are never saved; a node keeps its id for as
//! long as it lives, and an id is never handed out twice in a process.
//!
//! Nothing here runs on the audio thread. It hands over through the setters
//! the host already calls.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use rhizome_core::{Node, NodeId, On, Tree, Value};
use shard_dsp::arrangement;
use shard_dsp::fx::{self, Kind as Fx, Order, INSTANCES};
use shard_dsp::params::{ParamDef, PARAMS};
use shard_dsp::steps::STEPS;
use shard_dsp::{ModSet, ParamBank, StepParams};

use super::*;
use crate::materials::{MaterialRecord, Pool};
use crate::modulation::{EnvelopeRecord, LfoRecord, LinkRecord, Modulation, Refused};
use crate::tracker::{Kit, Step, Track, Tracker};

/// What the engine reads, compiled from a session.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The patch's values, indexed as `PARAMS`.
    pub patch: Vec<f32>,
    /// The arrangement's values, indexed as `arrangement::params()`.
    pub arrangement: Vec<f32>,
    pub modulation: Modulation,
    pub tracker: Tracker,
    pub pool: Pool,
    /// Runtime ids by node, for modulators and materials.
    ids: BTreeMap<NodeId, u64>,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            patch: PARAMS.iter().map(|p| p.default).collect(),
            arrangement: arrangement::params().iter().map(|p| p.default).collect(),
            modulation: Modulation::default(),
            tracker: Tracker::default(),
            pool: Pool::default(),
            ids: BTreeMap::new(),
        }
    }
}

impl Plan {
    /// Writes the patch's values into the live bank, all but the played rows,
    /// so a held brake stays held through an edit.
    pub fn write_patch(&self, bank: &ParamBank) {
        write(&self.patch, bank);
    }

    pub fn write_arrangement(&self, bank: &ParamBank) {
        write(&self.arrangement, bank);
    }

    pub fn steps(&self) -> StepParams {
        self.tracker.params()
    }

    /// The `ModSet` for `Engine::set_modulation`, and what it refused.
    pub fn mod_set(&self) -> (ModSet, Vec<Refused>) {
        self.modulation.build()
    }

    /// The runtime id of a modulator or material node.
    pub fn id_of(&self, node: NodeId) -> Option<u64> {
        self.ids.get(&node).copied()
    }

    /// The node a runtime id names.
    pub fn node_of(&self, id: u64) -> Option<NodeId> {
        self.ids.iter().find(|(_, i)| **i == id).map(|(n, _)| *n)
    }
}

fn write(values: &[f32], bank: &ParamBank) {
    for (i, (v, def)) in values.iter().zip(bank.defs()).enumerate() {
        if !PLAYED.contains(&def.id) {
            bank.set(i, *v);
        }
    }
}

/// Never reused in a process, so a cache keyed by a material's id can't
/// confuse two documents.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Ids kept from the last compile, and the ones this compile hands out.
struct Ids<'p> {
    old: &'p BTreeMap<NodeId, u64>,
    new: BTreeMap<NodeId, u64>,
}

impl Ids<'_> {
    fn of(&mut self, node: NodeId) -> u64 {
        let id = self
            .new
            .get(&node)
            .or_else(|| self.old.get(&node))
            .copied()
            .unwrap_or_else(|| NEXT_ID.fetch_add(1, Ordering::Relaxed));
        self.new.insert(node, id);
        id
    }
}

/// A value as the bank holds it.
pub fn as_f32(v: &Value) -> Option<f32> {
    match v {
        Value::Bool(b) => Some(f32::from(u8::from(*b))),
        Value::Int(n) => Some(*n as f32),
        Value::Float(x) => Some(*x as f32),
        _ => None,
    }
}

fn float(node: &Node<'_>, key: &str) -> f32 {
    node.value(key).as_ref().and_then(as_f32).unwrap_or(0.0)
}

fn int(node: &Node<'_>, key: &str) -> i64 {
    match node.value(key) {
        Some(Value::Int(n)) => n,
        _ => 0,
    }
}

fn text(node: &Node<'_>, key: &str) -> String {
    match node.value(key) {
        Some(Value::Text(s) | Value::Choice(s)) => s,
        _ => String::new(),
    }
}

/// A chain's effect nodes by pool instance, and its order.
struct Chain<'t> {
    by_instance: [Option<Node<'t>>; INSTANCES],
    order: Order,
}

impl<'t> Chain<'t> {
    fn of(owner: Option<Node<'t>>) -> Self {
        let mut chain = Chain {
            by_instance: [None; INSTANCES],
            order: Order::EMPTY,
        };
        let Some(owner) = owner else { return chain };
        let mut copies = [0u8; Fx::ALL.len()];
        for node in owner.order(CHAIN) {
            let Some(kind) = Fx::from_name(node.type_name()) else {
                continue;
            };
            let k = Fx::ALL.iter().position(|x| *x == kind).expect("in ALL");
            let copy = copies[k];
            copies[k] += 1;
            if let Some(n) = fx::POOL.iter().position(|&(x, c)| x == kind && c == copy) {
                chain.by_instance[n] = Some(node);
                chain.order.push(n);
            }
        }
        chain
    }

    fn instance_of(&self, node: NodeId) -> Option<usize> {
        self.by_instance
            .iter()
            .position(|n| n.is_some_and(|n| n.id() == node))
    }
}

/// Fills a table's values: instance rows from the chain, order rows from its
/// order, every other row from the node `fixed` finds for its id's prefix.
fn fill<'t>(
    table: &[ParamDef],
    strip: &str,
    chain: &Chain<'t>,
    fixed: impl Fn(&str) -> Option<Node<'t>>,
    out: &mut Vec<f32>,
) {
    let order = chain.order.to_values();
    out.clear();
    for def in table {
        let id = def.id.strip_prefix(strip).unwrap_or(def.id);
        let value = if let Some((n, template)) = fx::parse_row(def.id) {
            chain.by_instance[n]
                .and_then(|node| node.value(template))
                .as_ref()
                .and_then(as_f32)
        } else if fx::is_order_row(def.id) {
            id.strip_prefix("fx.order.")
                .and_then(|p| p.parse::<usize>().ok())
                .map(|p| order[p])
        } else if PLAYED.contains(&id) {
            None
        } else {
            fixed(key_prefix(id))
                .and_then(|node| node.value(id))
                .as_ref()
                .and_then(as_f32)
        };
        out.push(value.unwrap_or(def.default));
    }
}

/// Compiles `tree` into `plan`, keeping the runtime ids of nodes it already
/// knew.
pub fn compile(tree: &Tree, plan: &mut Plan) {
    let old = std::mem::take(&mut plan.ids);
    let mut ids = Ids {
        old: &old,
        new: BTreeMap::new(),
    };

    let patch = patch_node(tree);
    let arr = arrangement_node(tree);

    // Materials first, so a wire can name one.
    plan.pool = Pool::default();
    if let Some(materials) = tree.at(format!("/{MATERIALS}").as_str()) {
        for s in materials.order(POOL) {
            let path = text(&s, "sample.path");
            let root = match s.value("sample.root") {
                Some(Value::Shaped(j)) => j.as_u64().and_then(|r| u8::try_from(r).ok()),
                _ => None,
            };
            plan.pool.materials.push(MaterialRecord {
                id: ids.of(s.id()),
                name: text(&s, "sample.label"),
                path: (!path.is_empty()).then_some(path),
                octave: int(&s, "sample.octave") as f32,
                trim_start: float(&s, "sample.start"),
                trim_end: float(&s, "sample.end"),
                root,
            });
        }
    }

    let chain = Chain::of(patch);
    fill(
        &PARAMS,
        "",
        &chain,
        |prefix| {
            let p = patch?;
            if prefix == PATCH {
                Some(p)
            } else {
                p.child(prefix)
            }
        },
        &mut plan.patch,
    );

    plan.modulation = Modulation::default();
    if let Some(p) = patch {
        for g in READS_MATERIAL {
            let wired = p
                .child(g)
                .and_then(|n| n.resolve(&format!("{g}.source")))
                .map(|s| ids.of(s.id()));
            match *g {
                "material" => plan.pool.wires.material = wired,
                _ => plan.pool.wires.grain = wired,
            }
        }
        plan.modulation = modulation(p, &chain, &mut ids);
    }

    let arr_chain = Chain::of(arr);
    fill(
        arrangement::params(),
        arrangement::PREFIX,
        &arr_chain,
        |prefix| {
            let a = arr?;
            if prefix == TRACK {
                a.order(TRACKS).into_iter().next()
            } else {
                a.child(prefix)
            }
        },
        &mut plan.arrangement,
    );
    plan.tracker = arr.map(read_tracker).unwrap_or_default();

    plan.ids = ids.new;
}

fn modulation(patch: Node<'_>, chain: &Chain<'_>, ids: &mut Ids<'_>) -> Modulation {
    let mut m = Modulation::default();
    for node in patch.order(MODULATORS) {
        let id = ids.of(node.id());
        match node.type_name() {
            LFO => m.lfos.push(LfoRecord {
                id,
                name: text(&node, "lfo.label"),
                rate: float(&node, "lfo.rate"),
                shape: text(&node, "lfo.shape"),
                phase: float(&node, "lfo.phase"),
            }),
            MOD_ENV => m.envelopes.push(EnvelopeRecord {
                id,
                name: text(&node, "mod-env.label"),
                attack: float(&node, "mod-env.attack"),
                decay: float(&node, "mod-env.decay"),
                sustain: float(&node, "mod-env.sustain"),
                release: float(&node, "mod-env.release"),
            }),
            _ => {}
        }
    }
    for target in std::iter::once(patch).chain(patch.children()) {
        let instance = chain.instance_of(target.id());
        for b in target.bindings() {
            let On::Value(key) = b.on else { continue };
            let param = match instance {
                Some(n) => fx::row_id(n, key),
                None if Fx::from_name(target.type_name()).is_some() => continue,
                None => key.clone(),
            };
            let end = |k: &str| b.value(k).as_ref().and_then(as_f32).unwrap_or(0.0);
            m.links.insert(
                param,
                LinkRecord {
                    source: ids.of(b.source.id()),
                    lo: end("lo"),
                    hi: end("hi"),
                },
            );
        }
    }
    m
}

/// The arrangement as a tracker: the inverse of `ShardEdit::add_arrangement`.
pub fn read_tracker(arr: Node<'_>) -> Tracker {
    let tracks = arr
        .order(TRACKS)
        .into_iter()
        .map(|t| {
            let mut steps = vec![Step::default(); STEPS];
            for s in t.children().filter(|c| c.type_name() == STEP) {
                let Some(n) = step_number(&s) else { continue };
                steps[n - 1] = Step {
                    on: s.value("step.on") == Some(Value::Bool(true)),
                    pitch: int(&s, "step.pitch") as i8,
                    hold: int(&s, "step.hold") as u8,
                    velocity: int(&s, "step.velocity") as u8,
                    nudge: int(&s, "step.nudge") as i8,
                };
            }
            Track {
                // The mode's, not the document's: the session sets it.
                on: false,
                length: int(&t, "track.length") as u32,
                steps,
            }
        })
        .collect();

    let mut kit = Kit {
        length: Kit::default().length,
        kick: vec![0; STEPS],
        snare: vec![0; STEPS],
        hat: vec![0; STEPS],
    };
    if let Some(k) = arr.child(KIT) {
        kit.length = int(&k, "kit.length") as u32;
        for b in k.children().filter(|c| c.type_name() == BEAT) {
            let Some(n) = step_number(&b) else { continue };
            kit.kick[n - 1] = int(&b, "beat.kick") as u8;
            kit.snare[n - 1] = int(&b, "beat.snare") as u8;
            kit.hat[n - 1] = int(&b, "beat.hat") as u8;
        }
    }

    Tracker {
        tempo: float(&arr, "arrangement.tempo"),
        swing: float(&arr, "arrangement.swing"),
        tracks,
        kit,
    }
}

pub fn patch_node(tree: &Tree) -> Option<Node<'_>> {
    tree.at(format!("/{PATCHES}").as_str())
        .and_then(|c| c.children().find(|n| n.type_name() == PATCH))
}

pub fn arrangement_node(tree: &Tree) -> Option<Node<'_>> {
    tree.at(format!("/{ARRANGEMENTS}").as_str())
        .and_then(|c| c.children().find(|n| n.type_name() == ARRANGEMENT))
}

/// Where a parameter id lives: the node and its key. Patch ids and
/// `arrangement.` ids alike. `None` for an order row, a played row, an effect
/// instance no node plays, or an id the tree has no home for.
pub fn locate(tree: &Tree, id: &str) -> Option<(NodeId, String)> {
    let (owner, bare) = match id.strip_prefix(arrangement::PREFIX) {
        Some(bare) => (arrangement_node(tree)?, bare),
        None => (patch_node(tree)?, id),
    };
    if fx::is_order_row(id) || PLAYED.contains(&bare) {
        return None;
    }
    if let Some((n, template)) = fx::parse_row(id) {
        let node = Chain::of(Some(owner)).by_instance[n]?;
        return Some((node.id(), template.to_string()));
    }
    let prefix = key_prefix(bare);
    let node = match (owner.type_name(), prefix) {
        (PATCH, PATCH) => owner,
        (ARRANGEMENT, TRACK) => owner.order(TRACKS).into_iter().next()?,
        _ => owner.child(prefix)?,
    };
    node.node_type()?.spec(bare)?;
    Some((node.id(), bare.to_string()))
}

/// The pool instance an effect node plays, from its place in its chain.
pub fn instance_of(tree: &Tree, node: NodeId) -> Option<usize> {
    let owner = tree.get(node)?.parent();
    Chain::of(owner).instance_of(node)
}

/// A bank value as the tree holds it under `spec`: a switch as a bool, a
/// stepped row rounded, and every number brought into range, as the bank
/// clamps.
pub fn to_value(spec: &rhizome_core::ValueSpec, v: f32) -> Option<Value> {
    if !v.is_finite() {
        return None;
    }
    let v = super::float(v);
    let (lo, hi) = spec.range.unwrap_or((f64::MIN, f64::MAX));
    match spec.kind {
        rhizome_core::ValueKind::Bool => Some(Value::Bool(v >= 0.5)),
        rhizome_core::ValueKind::Int => Some(Value::Int(v.round().clamp(lo, hi) as i64)),
        rhizome_core::ValueKind::Float => Some(Value::Float(v.clamp(lo, hi))),
        _ => None,
    }
}
