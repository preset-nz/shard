//! The projection's contracts: a session compiles to what the engine reads
//! today, by the code paths that make it today.

use rhizome_core::{NodeId, Tree, Value};
use rhizome_pom::{Document, MemoryStore};
use shard_dsp::arrangement;
use shard_dsp::fx::{self, Kind as Fx};
use shard_dsp::params::{index_of, PARAMS};
use shard_dsp::steps::{HOLD_MAX, STEPS, VELOCITY_MAX};
use shard_dsp::ParamBank;

use super::*;
use crate::materials::Pool;
use crate::tracker::{Kit, Step, Track, Tracker};

fn new_session() -> Document<Shard> {
    Document::new(MemoryStore::default()).expect("Shard's model builds and seeds")
}

fn id(doc: &Document<Shard>, path: &str) -> NodeId {
    doc.tree()
        .at(path)
        .unwrap_or_else(|| panic!("no {path}"))
        .id()
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

fn bank_bits(bank: &ParamBank) -> Vec<u32> {
    (0..bank.defs().len())
        .map(|i| bank.get(i).to_bits())
        .collect()
}

#[test]
fn a_new_session_compiles_to_todays_banks_bit_for_bit() {
    let doc = new_session();
    let plan = doc.projection();
    assert_eq!(bits(&plan.patch), bank_bits(&ParamBank::new()));
    assert_eq!(
        bits(&plan.arrangement),
        bank_bits(&ParamBank::for_table(arrangement::params()))
    );
    assert_eq!(plan.tracker, Tracker::default());
    assert_eq!(plan.steps(), Tracker::default().params());
    assert_eq!(plan.modulation, crate::modulation::Modulation::default());
}

#[test]
fn a_new_session_reads_the_drone_in_both_generators() {
    let plan = new_session().projection().clone();
    let today = Pool::with_drone();
    assert_eq!(plan.pool.materials.len(), 1);
    let (ours, theirs) = (&plan.pool.materials[0], &today.materials[0]);
    assert_eq!(
        (
            &ours.name,
            &ours.path,
            ours.octave,
            ours.trim_start,
            ours.trim_end,
            ours.root
        ),
        (
            &theirs.name,
            &theirs.path,
            theirs.octave,
            theirs.trim_start,
            theirs.trim_end,
            theirs.root
        )
    );
    assert_eq!(plan.pool.wires.material, Some(ours.id));
    assert_eq!(plan.pool.wires.grain, Some(ours.id));
}

/// The effect chain compiles to the bank today's palette code writes for the
/// same chain: the k-th of a kind is copy k.
#[test]
fn a_chain_compiles_as_the_palette_writes_it_today() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let ((), _) = doc
        .edit("Chain", |tx| {
            tx.add_effect(patch, Fx::Drive)?;
            let first = tx.add_effect(patch, Fx::Chorus)?;
            let second = tx.add_effect(patch, Fx::Chorus)?;
            tx.set_value(first, "chorus.mix", Value::Float(0.25))?;
            tx.set_value(second, "chorus.mix", Value::Float(0.75))?;
            tx.set_value(second, "chorus.on", Value::Bool(false))
        })
        .unwrap();

    let today = ParamBank::new();
    fx::add_to_chain(&today, Fx::Drive).unwrap();
    let a = fx::add_to_chain(&today, Fx::Chorus).unwrap();
    let b = fx::add_to_chain(&today, Fx::Chorus).unwrap();
    today.set_by_id(&fx::row_id(a, "chorus.mix"), 0.25);
    today.set_by_id(&fx::row_id(b, "chorus.mix"), 0.75);
    today.set_by_id(&fx::row_id(b, "chorus.on"), 0.0);

    assert_eq!(bits(&doc.projection().patch), bank_bits(&today));
}

#[test]
fn reordering_a_chain_moves_the_sound_with_the_place() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let ((first, second), _) = doc
        .edit("Chain", |tx| {
            let first = tx.add_effect(patch, Fx::Chorus)?;
            let second = tx.add_effect(patch, Fx::Chorus)?;
            tx.set_value(second, "chorus.mix", Value::Float(0.75))?;
            Ok((first, second))
        })
        .unwrap();
    doc.edit("Move", |tx| tx.set_order(patch, CHAIN, [second, first]))
        .unwrap();

    let plan = doc.projection();
    let copy0 = fx::POOL
        .iter()
        .position(|&(k, c)| k == Fx::Chorus && c == 0)
        .unwrap();
    let mix0 = index_of(&fx::row_id(copy0, "chorus.mix")).unwrap();
    assert_eq!(
        plan.patch[mix0], 0.75,
        "the first chorus in the chain is copy 0"
    );
    let order = fx::Order::sanitise(std::array::from_fn(|p| {
        plan.patch[index_of(&fx::order_id(p)).unwrap()]
    }));
    assert_eq!(order.len(), 2);
    assert_eq!(order.as_slice()[0] as usize, copy0);
}

#[test]
fn the_arrangement_chain_compiles_under_its_prefix() {
    let mut doc = new_session();
    let arr = id(&doc, "/arrangements/arrangement");
    let (delay, _) = doc
        .edit("Add Delay", |tx| tx.add_effect(arr, Fx::Delay))
        .unwrap();
    doc.edit("Set", |tx| {
        tx.set_value(delay, "delay.mix", Value::Float(0.5))
    })
    .unwrap();

    let today = ParamBank::for_table(arrangement::params());
    let mut order = fx::Order::EMPTY;
    let n = order.add(Fx::Delay).unwrap();
    for (p, v) in order.to_values().iter().enumerate() {
        today.set_by_id(&format!("{}{}", arrangement::PREFIX, fx::order_id(p)), *v);
    }
    today.set_by_id(
        &format!("{}{}", arrangement::PREFIX, fx::row_id(n, "delay.mix")),
        0.5,
    );
    assert_eq!(bits(&doc.projection().arrangement), bank_bits(&today));
    assert_eq!(
        bits(&doc.projection().patch),
        bank_bits(&ParamBank::new()),
        "the patch doesn't hear the arrangement's chain"
    );
}

/// Today's tracker, written into a tree and read back, is itself.
#[test]
fn a_tracker_round_trips_through_the_tree() {
    let mut first = Track {
        on: true,
        length: 32,
        ..Track::default()
    };
    first.steps[0] = Step {
        on: true,
        pitch: -7,
        hold: HOLD_MAX,
        velocity: 40,
        nudge: -12,
    };
    first.steps[STEPS - 1] = Step {
        on: true,
        ..Step::default()
    };
    // Off, but with a pitch kept for when it comes back on.
    first.steps[5] = Step {
        pitch: 12,
        ..Step::default()
    };
    let mut kit = Kit {
        length: 16,
        ..Kit::default()
    };
    kit.snare[3] = VELOCITY_MAX;
    kit.hat[63] = 1;
    let tracker = Tracker {
        tempo: 87.0,
        swing: 0.12,
        tracks: vec![first, Track::default()],
        kit,
    };

    let registry = new_session().tree().registry().clone();
    let mut tree = Tree::new(registry);
    let (arr, _) = tree
        .edit("Arrangement", |tx| tx.add_arrangement(&tracker))
        .unwrap();
    assert_eq!(read_tracker(tree.get(arr).unwrap()), tracker);
}

#[test]
fn links_compile_to_the_records_that_build_a_mod_set_today() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let grain = id(&doc, "/patches/patch/grain");
    let ((lfo, env), _) = doc
        .edit("Modulate", |tx| {
            let lfo = tx.add_lfo(patch)?;
            tx.set_value(lfo, "lfo.rate", Value::Float(2.0))?;
            tx.set_value(lfo, "lfo.shape", Value::Choice("sine".into()))?;
            let env = tx.add_mod_env(patch)?;
            let chorus = tx.add_effect(patch, Fx::Chorus)?;
            tx.link(grain, "grain.position", lfo, 0.2, 0.8)?;
            tx.link(chorus, "chorus.mix", env, 1.0, 0.0)?;
            tx.link(patch, "patch.hold", lfo, 0.0, 0.5)?;
            Ok((lfo, env))
        })
        .unwrap();

    let plan = doc.projection();
    let (lfo, env) = (plan.id_of(lfo).unwrap(), plan.id_of(env).unwrap());
    let m = &plan.modulation;
    assert_eq!(m.lfos.len(), 1);
    assert_eq!((m.lfos[0].id, m.lfos[0].rate), (lfo, 2.0));
    assert_eq!(m.lfos[0].shape, "sine");
    assert_eq!(m.envelopes.len(), 1);
    assert_eq!(m.envelopes[0].id, env);

    let copy0 = fx::POOL
        .iter()
        .position(|&(k, c)| k == Fx::Chorus && c == 0)
        .unwrap();
    let chorus_row = fx::row_id(copy0, "chorus.mix");
    let links: Vec<_> = m
        .links
        .iter()
        .map(|(k, l)| (k.as_str(), l.source, l.lo, l.hi))
        .collect();
    let mut expected = vec![
        ("grain.position", lfo, 0.2, 0.8),
        (chorus_row.as_str(), env, 1.0, 0.0),
        ("patch.hold", lfo, 0.0, 0.5),
    ];
    expected.sort_by_key(|(k, ..)| *k);
    assert_eq!(links, expected);

    let (_, refused) = plan.mod_set();
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn a_node_keeps_its_runtime_id_and_a_new_one_never_reuses_one() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let grain = id(&doc, "/patches/patch/grain");
    let (lfo, _) = doc.edit("Add LFO", |tx| tx.add_lfo(patch)).unwrap();
    let before = doc.projection().id_of(lfo).unwrap();

    doc.edit("Set", |tx| {
        tx.set_value(grain, "grain.size", Value::Float(80.0))
    })
    .unwrap();
    assert_eq!(doc.projection().id_of(lfo), Some(before));

    doc.edit("Remove", |tx| tx.remove(lfo)).unwrap();
    assert_eq!(doc.projection().id_of(lfo), None);
    let (again, _) = doc.edit("Add LFO", |tx| tx.add_lfo(patch)).unwrap();
    assert_ne!(doc.projection().id_of(again), Some(before));
}

#[test]
fn compiling_the_same_tree_twice_gives_the_same_plan() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    doc.edit("Build", |tx| {
        let lfo = tx.add_lfo(patch)?;
        let reverb = tx.add_effect(patch, Fx::Reverb)?;
        tx.link(reverb, "reverb.mix", lfo, 0.0, 1.0)
    })
    .unwrap();
    let mut again = doc.projection().clone();
    compile(doc.tree(), &mut again);
    assert_eq!(&again, doc.projection());
}

#[test]
fn undo_recompiles() {
    let mut doc = new_session();
    let grain = id(&doc, "/patches/patch/grain");
    let before = doc.projection().clone();
    doc.edit("Set", |tx| {
        tx.set_value(grain, "grain.size", Value::Float(80.0))
    })
    .unwrap();
    let size = index_of("grain.size").unwrap();
    assert_eq!(doc.projection().patch[size], 80.0);
    doc.undo().unwrap();
    assert_eq!(doc.projection(), &before);
}

/// Brake and reverse are played. Writing a compiled plan after an edit must
/// not lift a finger off the reel.
#[test]
fn writing_a_plan_leaves_a_held_brake_held() {
    let mut doc = new_session();
    let grain = id(&doc, "/patches/patch/grain");
    let bank = ParamBank::new();
    bank.set_by_id("tape.brake", 1.0);
    bank.set_by_id("tape.reverse", 1.0);

    doc.edit("Set", |tx| {
        tx.set_value(grain, "grain.size", Value::Float(80.0))
    })
    .unwrap();
    doc.projection().write_patch(&bank);

    assert_eq!(bank.get_by_id("tape.brake"), Some(1.0));
    assert_eq!(bank.get_by_id("tape.reverse"), Some(1.0));
    assert_eq!(bank.get_by_id("grain.size"), Some(80.0));
    for def in PARAMS.iter().filter(|d| !PLAYED.contains(&d.id)) {
        if def.id != "grain.size" {
            assert_eq!(bank.get_by_id(def.id), Some(def.default), "{}", def.id);
        }
    }
}
