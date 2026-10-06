use rhizome_core::{Node, NodeId, Value, ValueKind};
use rhizome_pom::{Document, MemoryStore, PresetRef};
use shard_dsp::arrangement;
use shard_dsp::fx::{self, Kind as Fx};
use shard_dsp::params::{Taper, PARAMS};

use super::*;
use crate::materials::DRONE_NAME;

fn new_session() -> Document<Shard> {
    Document::new(MemoryStore::default()).expect("Shard's model builds and seeds")
}

fn id(doc: &Document<Shard>, path: &str) -> NodeId {
    doc.tree()
        .at(path)
        .unwrap_or_else(|| panic!("no {path}"))
        .id()
}

/// A value as the bank holds it.
fn as_f32(v: &Value) -> f32 {
    match v {
        Value::Bool(b) => f32::from(u8::from(*b)),
        Value::Int(n) => *n as f32,
        Value::Float(x) => *x as f32,
        other => panic!("not a parameter value: {other:?}"),
    }
}

/// Where a patch parameter lives in a session, and its value there.
fn patch_value(doc: &Document<Shard>, id: &str) -> Option<f32> {
    let node = key_prefix(id);
    let at = if node == "patch" {
        "/patches/patch".to_string()
    } else {
        format!("/patches/patch/{node}")
    };
    doc.tree().at(at.as_str())?.value(id).map(|v| as_f32(&v))
}

fn arrangement_value(doc: &Document<Shard>, id: &str) -> Option<f32> {
    let bare = id.strip_prefix(arrangement::PREFIX)?;
    let node = key_prefix(bare);
    let a = doc.tree().at("/arrangements/arrangement")?;
    let n = if node == "track" {
        a.order(TRACKS).into_iter().next()?
    } else {
        a.child(node)?
    };
    n.value(bare).map(|v| as_f32(&v))
}

#[test]
fn a_new_session_holds_what_a_new_session_holds_today() {
    let doc = new_session();
    let tree = doc.tree();

    for name in PATCH_NODES {
        assert!(
            tree.at(format!("/patches/patch/{name}").as_str()).is_some(),
            "the patch has its {name}"
        );
    }
    let patch = tree.at("/patches/patch").unwrap();
    assert!(patch.order(CHAIN).is_empty(), "a new patch runs no effects");
    assert!(patch.order(MODULATORS).is_empty());

    // The drone, wired into both generators, as `Pool::with_drone`.
    let drone = tree
        .at("/materials")
        .unwrap()
        .children()
        .next()
        .expect("the drone");
    assert_eq!(
        drone.value("sample.label"),
        Some(Value::Text(DRONE_NAME.into()))
    );
    assert_eq!(drone.value("sample.path"), Some(Value::Text(String::new())));
    for g in READS_MATERIAL {
        let generator = patch.child(g).unwrap();
        assert_eq!(
            generator.resolve(&format!("{g}.source")).map(|n| n.id()),
            Some(drone.id()),
            "{g} reads the drone"
        );
    }

    let a = tree.at("/arrangements/arrangement").unwrap();
    for name in ["kit", "filter", "amp"] {
        assert!(a.child(name).is_some(), "the arrangement has its {name}");
    }
    assert_eq!(a.child("amp").unwrap().type_name(), MASTER);
    assert!(a.order(CHAIN).is_empty());
    assert_eq!(a.order(TRACKS).len(), Tracker::default().tracks.len());

    assert!(!doc.is_unsaved(), "a new session is not edited");
}

/// Every patch row a person edits has a home in the tree, at the table's
/// default. Slice B's projection relies on this to write the same bank.
#[test]
fn every_patch_parameter_has_a_home_at_its_default() {
    let doc = new_session();
    for def in PARAMS.iter() {
        if fx::parse_row(def.id).is_some() || fx::is_order_row(def.id) {
            continue;
        }
        let at = patch_value(&doc, def.id);
        if PLAYED.contains(&def.id) {
            assert_eq!(at, None, "{} is played, not stored", def.id);
            continue;
        }
        assert_eq!(at, Some(def.default), "{}", def.id);
    }
}

#[test]
fn every_arrangement_parameter_has_a_home_at_its_default() {
    let doc = new_session();
    for def in arrangement::params() {
        if fx::parse_row(def.id).is_some() || fx::is_order_row(def.id) {
            continue;
        }
        assert_eq!(
            arrangement_value(&doc, def.id),
            Some(def.default),
            "{}",
            def.id
        );
    }
}

/// An added effect holds its kind's rows at the defaults an added pool
/// instance has, switched on.
#[test]
fn an_added_effect_is_its_pool_instance_at_rest() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    for kind in Fx::ALL {
        let (node, _) = doc.edit("Add", |tx| tx.add_effect(patch, kind)).unwrap();
        let n = fx::POOL.iter().position(|(k, _)| *k == kind).unwrap();
        let tree = doc.tree();
        let effect = tree.get(node).unwrap();
        for row in kind.rows() {
            let instance = PARAMS
                .iter()
                .find(|p| p.id == fx::row_id(n, row.id))
                .unwrap();
            assert_eq!(
                effect.value(row.id).map(|v| as_f32(&v)),
                Some(instance.default),
                "{}",
                row.id
            );
        }
    }
    assert_eq!(
        doc.tree().at("/patches/patch").unwrap().order(CHAIN).len(),
        Fx::ALL.len()
    );
}

/// The int mapping of a stepped row is exact only if its steps are whole
/// numbers from a whole minimum.
#[test]
fn every_stepped_row_steps_in_whole_numbers() {
    for def in PARAMS.iter().chain(arrangement::params()) {
        if let Taper::Stepped(n) = def.taper {
            assert_eq!(def.min.fract(), 0.0, "{} starts on a whole number", def.id);
            assert_eq!(
                def.max - def.min,
                (n - 1) as f32,
                "{} has {n} whole steps",
                def.id
            );
        }
    }
}

/// CLAUDE.md's pattern, held over the declared kinds by role rather than by
/// id prefix: a generator's switch then Gain, an effect's switch then Mix,
/// and nothing else has a switch except a track.
#[test]
fn every_kind_leads_as_its_role_says() {
    for decl in kinds() {
        let t = &decl.node_type;
        let values = t.values();
        let switch = values.iter().find(|v| v.key.ends_with(".on"));
        match decl.role {
            Role::Generator | Role::Effect => {
                let first = &values[0];
                assert!(
                    first.key.ends_with(".on") && first.kind == ValueKind::Bool,
                    "{} leads with its switch",
                    t.name()
                );
                let node = first.key.trim_end_matches(".on");
                let level = if decl.role == Role::Generator {
                    "gain"
                } else {
                    "mix"
                };
                assert_eq!(
                    values[1].key,
                    format!("{node}.{level}"),
                    "{} leads with its {level}",
                    t.name()
                );
            }
            Role::Fixed | Role::Modulator => {
                assert!(switch.is_none(), "{} has no switch", t.name())
            }
            Role::Structure => {
                assert!(
                    switch.is_none() || t.name() == TRACK || t.name() == STEP,
                    "{} has no switch",
                    t.name()
                )
            }
        }
    }
}

#[test]
fn a_chain_holds_at_most_two_of_a_kind() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    for _ in 0..fx::COPIES {
        doc.edit("Add Chorus", |tx| tx.add_effect(patch, Fx::Chorus))
            .unwrap();
    }
    assert!(doc
        .edit("Add Chorus", |tx| tx.add_effect(patch, Fx::Chorus))
        .is_err());
}

#[test]
fn removing_an_effect_takes_it_out_of_the_chain() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let (chorus, _) = doc
        .edit("Add Chorus", |tx| tx.add_effect(patch, Fx::Chorus))
        .unwrap();
    doc.edit("Remove Chorus", |tx| tx.remove(chorus)).unwrap();
    assert!(doc
        .tree()
        .at("/patches/patch")
        .unwrap()
        .order(CHAIN)
        .is_empty());
}

#[test]
fn every_effect_is_in_its_chain() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let stray = doc.edit("Add", |tx| tx.add(patch, "chorus", "chorus"));
    assert!(stray.is_err(), "an effect outside the chain is refused");
}

#[test]
fn fixed_nodes_stay() {
    let mut doc = new_session();
    for path in [
        "/patches/patch/filter",
        "/patches/patch/grain",
        "/arrangements/arrangement/amp",
        "/arrangements/arrangement/kit",
    ] {
        let node = id(&doc, path);
        assert!(
            doc.edit("Remove", |tx| tx.remove(node)).is_err(),
            "{path} can't be removed"
        );
    }
    let patch = id(&doc, "/patches/patch");
    assert!(
        doc.edit("Add", |tx| tx.add(patch, "filter", "filter-2"))
            .is_err(),
        "one filter per patch"
    );
}

#[test]
fn kinds_live_only_where_they_belong() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let arr = id(&doc, "/arrangements/arrangement");
    let refused = [
        ("/patches", "filter"),
        ("/arrangements/arrangement", "grain"),
        ("/arrangements/arrangement", LFO),
        ("/patches/patch", KIT),
        ("/patches/patch", MASTER),
        ("/patches/patch", TRACK),
    ];
    for (parent, kind) in refused {
        let r = doc.edit("Add", |tx| tx.add(parent, kind, "stray"));
        assert!(r.is_err(), "a {kind} can't live in {parent}");
    }
    // The places they do belong.
    doc.edit("Add", |tx| tx.add_effect(arr, Fx::Delay)).unwrap();
    doc.edit("Add", |tx| tx.add_lfo(patch)).unwrap();
}

#[test]
fn a_step_is_named_by_its_number() {
    let mut doc = new_session();
    let track = doc
        .tree()
        .at("/arrangements/arrangement")
        .unwrap()
        .order(TRACKS)[0]
        .id();
    doc.edit("Step", |tx| tx.add(track, STEP, "64")).unwrap();
    for bad in ["0", "65", "first"] {
        assert!(
            doc.edit("Step", |tx| tx.add(track, STEP, bad)).is_err(),
            "{bad} is not a step"
        );
    }
}

#[test]
fn a_track_is_a_length_a_track_can_have() {
    let mut doc = new_session();
    let track = doc
        .tree()
        .at("/arrangements/arrangement")
        .unwrap()
        .order(TRACKS)[0]
        .id();
    doc.edit("Length", |tx| {
        tx.set_value(track, "track.length", Value::Int(32))
    })
    .unwrap();
    assert!(doc
        .edit("Length", |tx| tx.set_value(
            track,
            "track.length",
            Value::Int(12)
        ))
        .is_err());
}

#[test]
fn links_follow_the_rules_the_engine_has() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let grain = id(&doc, "/patches/patch/grain");
    let (lfo, _) = doc.edit("Add LFO", |tx| tx.add_lfo(patch)).unwrap();
    let (env, _) = doc.edit("Add", |tx| tx.add_mod_env(patch)).unwrap();

    doc.edit("Link", |tx| tx.link(grain, "grain.position", lfo, 0.2, 0.8))
        .unwrap();
    assert!(
        doc.edit("Link", |tx| tx.link(grain, "grain.position", env, 0.0, 1.0))
            .is_err(),
        "one source per parameter"
    );
    assert!(
        doc.edit("Link", |tx| tx.link(grain, "grain.window", lfo, 0.0, 1.0))
            .is_err(),
        "a stepped parameter can't follow a modulator"
    );
    assert!(
        doc.edit("Link", |tx| tx.link(grain, "grain.on", lfo, 0.0, 1.0))
            .is_err(),
        "nor can a switch"
    );
    doc.edit("Link", |tx| tx.link(patch, "patch.hold", env, 0.0, 1.0))
        .unwrap();

    let filter = id(&doc, "/arrangements/arrangement/filter");
    assert!(
        doc.edit("Link", |tx| tx.link(filter, "filter.cutoff", lfo, 0.0, 1.0))
            .is_err(),
        "the arrangement can't follow a patch's modulator"
    );
}

#[test]
fn a_preset_is_a_sound_and_never_its_switch() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let (chorus, _) = doc
        .edit("Add Chorus", |tx| tx.add_effect(patch, Fx::Chorus))
        .unwrap();
    doc.edit("Set", |tx| {
        tx.set_value(chorus, "chorus.mix", Value::Float(0.25))
    })
    .unwrap();
    doc.save_preset(chorus, "Thin").unwrap();

    doc.edit("Set", |tx| {
        tx.set_value(chorus, "chorus.mix", Value::Float(0.75))?;
        tx.set_value(chorus, "chorus.on", Value::Bool(false))
    })
    .unwrap();
    doc.apply_preset(chorus, &PresetRef::User("Thin".into()))
        .unwrap();

    let tree = doc.tree();
    let node: Node<'_> = tree.get(chorus).unwrap();
    assert_eq!(node.value("chorus.mix"), Some(Value::Float(0.25)));
    assert_eq!(
        node.value("chorus.on"),
        Some(Value::Bool(false)),
        "applying a preset never switches a node in or out"
    );
    assert!(doc.has_presets("grain") && doc.has_presets("filter"));
    assert!(!doc.has_presets(LFO) && !doc.has_presets(TRACK));
}

#[test]
fn a_session_saves_and_opens_as_rhizome_text() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let (lfo, _) = doc.edit("Add LFO", |tx| tx.add_lfo(patch)).unwrap();
    let grain = id(&doc, "/patches/patch/grain");
    doc.edit("Link", |tx| tx.link(grain, "grain.size", lfo, 0.0, 0.5))
        .unwrap();
    doc.edit("Add", |tx| tx.add_effect(patch, Fx::Reverb))
        .unwrap();

    let text = doc.tree().serialise();
    let (back, report) = rhizome_core::Tree::load(&text, doc.tree().registry().clone()).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(back.serialise(), text);
}
