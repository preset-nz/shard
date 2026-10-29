//! The projection: the tree compiled into what the engine reads today
//! (epic 32, story 2, slice B).
//!
//! A session holds several patches (epic 10) and several tracks (epic 28),
//! each track playing one patch on its own voice. Every patch is compiled
//! (`patch_plan`), and which patch each track plays (`track_patches`), so the
//! session hands each voice its track's patch without compiling again. A
//! plan is rebuilt from the tree after every change:
//!
//! - **the current patch,** `current`: the one patch-scoped commands edit and
//!   the patch view shows, never what plays. Compiled, that is the patch the
//!   first track plays (`track_patch`). The session's own copy is selected
//!   onto its current patch with `Plan::select`: the projection is rhizome's
//!   and can't hear about a selection, which is session state, not the
//!   document's;
//! - the patch's values, indexed as `PARAMS`, and the arrangement's, indexed
//!   as `arrangement::params()`. An effect node plays a pool instance by its
//!   place in the chain: the k-th of a kind is copy k, so the same tree
//!   always compiles to the same ids, which controller maps store;
//! - the modulators and links as `Modulation` records, built into a `ModSet`
//!   by the code that builds one today, and the global LFOs as `LfoRecord`s,
//!   built into the engine's one `Globals` set;
//! - the arrangement as a `Tracker`, each drums track's lane nodes beside
//!   it (`kit_lanes`), and the materials as a `Pool`.
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
use shard_dsp::{Globals, ModSet, ParamBank};

use super::*;
use crate::materials::{MaterialRecord, Pool, Wires};
use crate::modulation::{
    build_globals, EnvelopeRecord, LfoRecord, LinkRecord, Modulation, Refused,
};
use crate::tracker::{Duck, Kit, KitLane, Step, Track, Tracker};

/// What the engine reads, compiled from a session.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The patch `patch`, `modulation` and the pool's wires are: the one the
    /// first track plays, or the session's current one once it is selected.
    pub current: Option<NodeId>,
    /// The patch each track plays, by place, as `tracker.tracks` lists them.
    pub track_patches: Vec<Option<NodeId>>,
    /// Each drums track's lane nodes, the *k*-th for kit *k*, in the order
    /// its `Kit::lanes` lists them: what the session gives each a slot by.
    pub kit_lanes: Vec<Vec<NodeId>>,
    /// The current patch's values, indexed as `PARAMS`.
    pub patch: Vec<f32>,
    /// The generators the current patch holds (`GENERATORS`), in table
    /// order: the ones the patch view lists and draws.
    pub generators: Vec<&'static str>,
    /// The arrangement's values, indexed as `arrangement::params()`.
    pub arrangement: Vec<f32>,
    /// The current patch's LFOs, envelopes and links.
    pub modulation: Modulation,
    /// The global LFOs, in their order.
    pub globals: Vec<LfoRecord>,
    pub tracker: Tracker,
    /// Every material, wired as the current patch wires them.
    pub pool: Pool,
    /// Every patch compiled, in the patches' order.
    patches: Vec<(NodeId, PatchPlan)>,
    /// Runtime ids by node, for patches, modulators and materials.
    ids: BTreeMap<NodeId, u64>,
}

/// One patch as a voice plays it.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchPlan {
    /// Indexed as `PARAMS`.
    pub values: Vec<f32>,
    /// The generators it holds, in table order.
    pub generators: Vec<&'static str>,
    pub modulation: Modulation,
    pub wires: Wires,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            current: None,
            track_patches: Vec::new(),
            kit_lanes: Vec::new(),
            patch: PARAMS.iter().map(|p| p.default).collect(),
            generators: Vec::new(),
            arrangement: arrangement::params().iter().map(|p| p.default).collect(),
            modulation: Modulation::default(),
            globals: Vec::new(),
            tracker: Tracker::default(),
            pool: Pool::default(),
            patches: Vec::new(),
            ids: BTreeMap::new(),
        }
    }
}

impl Plan {
    /// Makes `patch` the one this plan plays: its values, modulation and
    /// wires. False, and nothing changes, when it compiled no such patch.
    pub fn select(&mut self, patch: NodeId) -> bool {
        let Some((_, p)) = self.patches.iter().find(|(n, _)| *n == patch) else {
            return false;
        };
        self.patch.clone_from(&p.values);
        self.generators.clone_from(&p.generators);
        self.modulation = p.modulation.clone();
        self.pool.wires = p.wires;
        self.current = Some(patch);
        true
    }

    /// A patch as it compiled, if it did.
    pub fn patch_plan(&self, patch: NodeId) -> Option<&PatchPlan> {
        self.patches
            .iter()
            .find(|(n, _)| *n == patch)
            .map(|(_, p)| p)
    }

    /// Writes the patch's values into the live bank, all but the played rows,
    /// so a held brake stays held through an edit.
    pub fn write_patch(&self, bank: &ParamBank) {
        write_values(&self.patch, bank);
    }

    pub fn write_arrangement(&self, bank: &ParamBank) {
        write_values(&self.arrangement, bank);
    }

    /// The `ModSet` for `Engine::set_modulation`, and what it refused.
    pub fn mod_set(&self) -> (ModSet, Vec<Refused>) {
        self.modulation.build(&self.globals)
    }

    /// The global LFOs for `Engine::set_globals`, and what they refused.
    pub fn global_set(&self) -> (Globals, Vec<Refused>) {
        build_globals(&self.globals)
    }

    /// Everything in the current patch's modulation and the globals that the
    /// engine refuses, as a load or the UI reports it.
    pub fn refused(&self) -> Vec<Refused> {
        let mut refused = self.mod_set().1;
        refused.extend(self.global_set().1);
        refused
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

/// Values into a bank indexed as they are, all but the played rows.
pub fn write_values(values: &[f32], bank: &ParamBank) {
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
        } else if PLAYED.contains(&def.id) {
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
                regions: match s.value("sample.regions") {
                    Some(Value::Shaped(j)) => serde_json::from_value(j).unwrap_or_default(),
                    _ => Vec::new(),
                },
            });
        }
    }

    // The global LFOs before the patches, so their ids are the same ones a
    // patch's links name.
    plan.globals = global_lfos(tree)
        .into_iter()
        .map(|n| lfo_record(n, &mut ids))
        .collect();

    plan.patches = patches(tree)
        .into_iter()
        .map(|p| {
            ids.of(p.id());
            (p.id(), compile_patch(p, &mut ids))
        })
        .collect();
    // The patch the first track plays, or the defaults with no patch at all.
    plan.patch.clear();
    plan.patch.extend(PARAMS.iter().map(|p| p.default));
    plan.generators.clear();
    plan.modulation = Modulation::default();
    plan.current = None;
    if let Some(p) = track_patch(tree) {
        plan.select(p.id());
    }

    let arr_chain = Chain::of(arr);
    fill(
        arrangement::params(),
        arrangement::PREFIX,
        &arr_chain,
        |prefix| arr?.child(prefix),
        &mut plan.arrangement,
    );
    plan.tracker = arr.map(read_tracker).unwrap_or_default();
    plan.track_patches = tracks(tree)
        .into_iter()
        .map(|t| played_by(tree, t).map(|p| p.id()))
        .collect();
    for (t, patch) in plan.tracker.tracks.iter_mut().zip(&plan.track_patches) {
        t.patch = patch.map(|p| ids.of(p));
    }
    let drums: Vec<Node<'_>> = tracks(tree)
        .into_iter()
        .filter(|t| t.type_name() == DRUMS)
        .collect();
    plan.kit_lanes = drums
        .iter()
        .map(|t| lanes(*t).iter().map(|l| l.id()).collect())
        .collect();
    // What each lane plays from the pool, by runtime id, as a track's patch.
    let kits = plan
        .tracker
        .tracks
        .iter_mut()
        .filter_map(|t| t.drums.as_mut());
    for (kit, t) in kits.zip(&drums) {
        for (lane, l) in kit.lanes.iter_mut().zip(lanes(*t)) {
            lane.material = l.resolve(LANE_MATERIAL).map(|s| ids.of(s.id()));
        }
    }

    plan.ids = ids.new;
}

fn compile_patch(p: Node<'_>, ids: &mut Ids<'_>) -> PatchPlan {
    let chain = Chain::of(Some(p));
    let mut values = Vec::with_capacity(PARAMS.len());
    fill(
        &PARAMS,
        "",
        &chain,
        |prefix| {
            if prefix == PATCH {
                Some(p)
            } else {
                p.child(prefix)
            }
        },
        &mut values,
    );
    // A generator is in the patch while its node is: one it lacks is off,
    // whatever its switch's default.
    let mut generators = Vec::new();
    for &g in GENERATORS {
        if p.child(g).is_some() {
            generators.push(g);
        } else if let Some(i) = shard_dsp::params::index_of(&format!("{g}.on")) {
            values[i] = 0.0;
        }
    }
    let mut wires = Wires::default();
    for g in READS_MATERIAL {
        let wired = p
            .child(g)
            .and_then(|n| n.resolve(&format!("{g}.source")))
            .map(|s| ids.of(s.id()));
        match *g {
            "material" => wires.material = wired,
            _ => wires.grain = wired,
        }
    }
    PatchPlan {
        values,
        generators,
        modulation: modulation(p, &chain, ids),
        wires,
    }
}

fn modulation(patch: Node<'_>, chain: &Chain<'_>, ids: &mut Ids<'_>) -> Modulation {
    let mut m = Modulation::default();
    for node in patch.order(MODULATORS) {
        match node.type_name() {
            LFO => m.lfos.push(lfo_record(node, ids)),
            MOD_ENV => m.envelopes.push(EnvelopeRecord {
                id: ids.of(node.id()),
                name: text(&node, "mod-env.label"),
                mode: text(&node, "mod-env.mode"),
                curve: match node.value("mod-env.curve") {
                    Some(Value::Shaped(j)) => serde_json::from_value(j).unwrap_or_default(),
                    _ => Default::default(),
                },
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

fn lfo_record(node: Node<'_>, ids: &mut Ids<'_>) -> LfoRecord {
    LfoRecord {
        id: ids.of(node.id()),
        name: text(&node, "lfo.label"),
        rate: float(&node, "lfo.rate"),
        shape: text(&node, "lfo.shape"),
        phase: float(&node, "lfo.phase"),
    }
}

/// The global LFOs, in their order (roadmap row 43). None in a file saved
/// before there were any: rhizome makes the category empty.
pub fn global_lfos(tree: &Tree) -> Vec<Node<'_>> {
    tree.at(format!("/{GLOBALS}").as_str())
        .map(|c| {
            c.order(MODULATORS)
                .into_iter()
                .filter(|n| n.type_name() == LFO)
                .collect()
        })
        .unwrap_or_default()
}

/// A track's sidechain, of either kind: its values, and its key resolved to
/// that track's place in `order`. A key that has gone, or is the track
/// itself, is none.
fn read_duck(t: Node<'_>, order: &[Node<'_>]) -> Duck {
    let keys = duck_keys(&t);
    let by = t
        .resolve(keys.by)
        .filter(|k| k.id() != t.id())
        .and_then(|k| order.iter().position(|o| o.id() == k.id()));
    Duck {
        by,
        depth: float(&t, keys.depth),
        release: float(&t, keys.release),
        key_only: t.value(keys.key_only) == Some(Value::Bool(true)),
    }
}

/// A drums track node as a track: the inverse of `ShardEdit::write_track`.
fn read_drums(t: Node<'_>) -> Track {
    let kit = Kit {
        on: t.value("drums.on") == Some(Value::Bool(true)),
        tune: float(&t, "drums.tune"),
        decay: float(&t, "drums.decay"),
        lanes: lanes(t)
            .into_iter()
            .map(|l| {
                let mut hits = vec![0; STEPS];
                for b in l.children().filter(|c| c.type_name() == BEAT) {
                    let Some(n) = step_number(&b) else { continue };
                    hits[n - 1] = int(&b, BEAT_VELOCITY) as u8;
                }
                KitLane {
                    label: text(&l, LANE_LABEL),
                    kind: text(&l, LANE_KIND),
                    hits,
                    material: None,
                }
            })
            .collect(),
    };
    Track {
        length: int(&t, "drums.length") as u32,
        gain: float(&t, "drums.gain"),
        drums: Some(kit),
        ..Track::blank()
    }
}

/// The arrangement as a tracker: the inverse of `ShardEdit::add_arrangement`.
pub fn read_tracker(arr: Node<'_>) -> Tracker {
    let order = arr.order(TRACKS);
    let tracks = order
        .iter()
        .map(|&t| {
            let duck = read_duck(t, &order);
            if t.type_name() == DRUMS {
                return Track {
                    duck,
                    ..read_drums(t)
                };
            }
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
                length: int(&t, "track.length") as u32,
                gain: float(&t, TRACK_GAIN),
                // A runtime id, which `compile` fills in.
                patch: None,
                steps,
                drums: None,
                duck,
            }
        })
        .collect();

    Tracker {
        tempo: float(&arr, "arrangement.tempo"),
        swing: float(&arr, "arrangement.swing"),
        tracks,
    }
}

/// The patches in `category` (`/patches`), in their order. A patch the
/// order misses, in a file the tree's check would refuse, still plays, after
/// the listed ones.
pub fn patches_in(category: Node<'_>) -> Vec<Node<'_>> {
    let mut out: Vec<Node<'_>> = category
        .order(PATCH_ORDER)
        .into_iter()
        .filter(|n| n.type_name() == PATCH)
        .collect();
    for c in category.children().filter(|n| n.type_name() == PATCH) {
        if !out.iter().any(|n| n.id() == c.id()) {
            out.push(c);
        }
    }
    out
}

/// The session's patches, in their order.
pub fn patches(tree: &Tree) -> Vec<Node<'_>> {
    tree.at(format!("/{PATCHES}").as_str())
        .map(patches_in)
        .unwrap_or_default()
}

/// A drums track's lanes, in its order `lanes`: the grid's.
pub fn lanes(drums: Node<'_>) -> Vec<Node<'_>> {
    drums
        .order(LANES)
        .into_iter()
        .filter(|l| l.type_name() == LANE)
        .collect()
}

/// The arrangement's tracks of both kinds, in their order. The *k*-th of
/// each kind plays on voice or kit *k* (`tracker::slots`).
pub fn tracks(tree: &Tree) -> Vec<Node<'_>> {
    arrangement_node(tree)
        .map(|a| a.order(TRACKS))
        .unwrap_or_default()
}

/// The patch a track plays: the one its `track.patch` names, or the first
/// patch when it names none, or one that has gone. None for a drums track,
/// which plays no patch.
pub fn played_by<'t>(tree: &'t Tree, track: Node<'t>) -> Option<Node<'t>> {
    if track.type_name() != TRACK {
        return None;
    }
    track
        .resolve(TRACK_PATCH)
        .filter(|p| p.type_name() == PATCH)
        .or_else(|| patches(tree).into_iter().next())
}

/// The patch the first melody track plays (`played_by`), or the first patch.
pub fn track_patch(tree: &Tree) -> Option<Node<'_>> {
    match tracks(tree).into_iter().find(|t| t.type_name() == TRACK) {
        Some(t) => played_by(tree, t),
        None => patches(tree).into_iter().next(),
    }
}

/// The patch an edit acts on: `current` while it is a patch in the tree, and
/// the one the first track plays otherwise (after undoing the New Patch that made
/// it current, say).
pub fn patch_node(tree: &Tree, current: Option<NodeId>) -> Option<Node<'_>> {
    current
        .and_then(|id| tree.get(id))
        .filter(|n| n.type_name() == PATCH)
        .or_else(|| track_patch(tree))
}

pub fn arrangement_node(tree: &Tree) -> Option<Node<'_>> {
    tree.at(format!("/{ARRANGEMENTS}").as_str())
        .and_then(|c| c.children().find(|n| n.type_name() == ARRANGEMENT))
}

/// Where a write to a parameter lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Home {
    /// A node the tree holds.
    Node(NodeId),
    /// A section `patch` lacks, made by the write (`ShardEdit::section`).
    Section { patch: NodeId, kind: String },
}

impl Home {
    /// The node to write, making the section first when it is absent: in
    /// the write's own edit, so one undo step takes both away.
    pub fn node(&self, tx: &mut rhizome_core::Edit<'_>) -> rhizome_core::Result<NodeId> {
        match self {
            Home::Node(n) => Ok(*n),
            Home::Section { patch, kind } => tx.section(*patch, kind),
        }
    }

    /// Whether `value` for `key` is what the absent section already reads
    /// (its default, or off for a generator's switch), so writing it would
    /// make a node and change nothing heard.
    pub fn reads_already(&self, key: &str, value: &Value) -> bool {
        let Home::Section { kind, .. } = self else {
            return false;
        };
        let absent = if GENERATORS.contains(&kind.as_str()) && key == format!("{kind}.on") {
            Some(0.0)
        } else {
            PARAMS.iter().find(|p| p.id == key).map(|p| p.default)
        };
        absent.is_some() && as_f32(value) == absent
    }
}

/// Where a write to parameter `id` lands: the node and its key. Patch ids
/// (on the patch `patch_node` finds from `current`) and `arrangement.` ids
/// alike. A patch section the patch lacks is `Home::Section`, made by the
/// write. `None` for an order row, a played row, an effect instance no node
/// plays, or an id the tree has no home for.
pub fn home(tree: &Tree, current: Option<NodeId>, id: &str) -> Option<(Home, String)> {
    let (owner, bare) = match id.strip_prefix(arrangement::PREFIX) {
        Some(bare) => (arrangement_node(tree)?, bare),
        None => (patch_node(tree, current)?, id),
    };
    if fx::is_order_row(id) || PLAYED.contains(&id) {
        return None;
    }
    if let Some((n, template)) = fx::parse_row(id) {
        let node = Chain::of(Some(owner)).by_instance[n]?;
        return Some((Home::Node(node.id()), template.to_string()));
    }
    let prefix = key_prefix(bare);
    let home = match (owner.type_name(), prefix) {
        (PATCH, PATCH) => Home::Node(owner.id()),
        (_, prefix) => match owner.child(prefix) {
            Some(node) => Home::Node(node.id()),
            None if owner.type_name() == PATCH && PATCH_NODES.contains(&prefix) => Home::Section {
                patch: owner.id(),
                kind: prefix.to_string(),
            },
            None => return None,
        },
    };
    let kind = match &home {
        Home::Node(n) => tree.get(*n)?.type_name(),
        Home::Section { kind, .. } => kind.as_str(),
    };
    tree.registry().node_type(kind)?.spec(bare)?;
    Some((home, bare.to_string()))
}

/// Where a parameter id lives now: the node and its key, as `home` finds it,
/// and `None` for a section the patch lacks. For reads; a write goes through
/// `home`, which makes the section.
pub fn locate(tree: &Tree, current: Option<NodeId>, id: &str) -> Option<(NodeId, String)> {
    match home(tree, current, id)? {
        (Home::Node(n), key) => Some((n, key)),
        (Home::Section { .. }, _) => None,
    }
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
