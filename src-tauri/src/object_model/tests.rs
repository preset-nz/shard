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

/// The first patch's section `kind`, made in an edit of its own: a new
/// patch holds only its material.
fn section(doc: &mut Document<Shard>, kind: &str) -> NodeId {
    let patch = id(doc, "/patches/patch");
    doc.edit("Section", |tx| tx.section(patch, kind)).unwrap().0
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

    // Only what it was written: its material. Every other section is made
    // by the first write that needs it.
    let patch = tree.at("/patches/patch").unwrap();
    let held: Vec<&str> = patch.children().map(|n| n.name()).collect();
    assert_eq!(held, ["material"], "a new patch holds only its material");
    assert!(patch.order(CHAIN).is_empty(), "a new patch runs no effects");
    assert!(patch.order(MODULATORS).is_empty());

    // The drone, wired into the material, as `Pool::with_drone`.
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
    assert_eq!(
        patch
            .child("material")
            .unwrap()
            .resolve("material.source")
            .map(|n| n.id()),
        Some(drone.id()),
        "the material reads the drone"
    );

    let a = tree.at("/arrangements/arrangement").unwrap();
    for name in ["filter", "amp"] {
        assert!(a.child(name).is_some(), "the arrangement has its {name}");
    }
    // A melody track, then a drums track switched off (story d), with the
    // beat today's Drums lane had.
    let kinds: Vec<&str> = a.order(TRACKS).iter().map(|t| t.type_name()).collect();
    assert_eq!(kinds, [TRACK, DRUMS]);
    let drums = a.order(TRACKS)[1];
    assert_eq!(drums.value("drums.on"), Some(Value::Bool(false)));
    assert!(drums.children().any(|b| b.type_name() == BEAT));
    assert!(a.child("kit").is_none(), "no kit beside the tracks");
    assert_eq!(a.child("amp").unwrap().type_name(), MASTER);
    assert!(a.order(CHAIN).is_empty());
    assert_eq!(a.order(TRACKS).len(), Tracker::default().tracks.len());

    assert!(!doc.is_unsaved(), "a new session is not edited");
}

/// Every patch row a person edits has a home in the tree, made by the write
/// when its section is absent, and an absent section compiles to the
/// table's defaults, but for a generator, which is off.
#[test]
fn every_patch_parameter_has_a_home_and_absence_reads_as_its_default() {
    let doc = new_session();
    let plan = doc.projection();
    for (i, def) in PARAMS.iter().enumerate() {
        if fx::parse_row(def.id).is_some() || fx::is_order_row(def.id) {
            continue;
        }
        assert!(
            home(doc.tree(), None, def.id).is_some(),
            "{} has a home",
            def.id
        );
        let prefix = key_prefix(def.id);
        let absent_generator = GENERATORS.contains(&prefix)
            && def.id == format!("{prefix}.on")
            && doc
                .tree()
                .at(format!("/patches/patch/{prefix}").as_str())
                .is_none();
        let want = if absent_generator { 0.0 } else { def.default };
        assert_eq!(plan.patch[i], want, "{}", def.id);
    }
}

#[test]
fn every_arrangement_parameter_has_a_home_at_its_default() {
    let doc = new_session();
    for def in arrangement::params() {
        if fx::parse_row(def.id).is_some() || fx::is_order_row(def.id) {
            continue;
        }
        if PLAYED.contains(&def.id) {
            assert_eq!(
                arrangement_value(&doc, def.id),
                None,
                "{} is played, not stored",
                def.id
            );
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
    // A patch itself is added and removed whole (epic 10); keeping the last
    // one is the Delete command's.
    section(&mut doc, "filter");
    for path in ["/patches/patch/filter", "/arrangements/arrangement/amp"] {
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
    // A generator is in the patch while its node is: Remove deletes it, and
    // there is one of each at most.
    let grain = section(&mut doc, "grain");
    assert!(
        doc.edit("Add", |tx| tx.add(patch, "grain", "grain-2"))
            .is_err(),
        "one grain per patch"
    );
    assert!(doc.edit("Remove", |tx| tx.remove(grain)).is_ok());
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
        ("/patches/patch", DRUMS),
        ("/patches/patch", BEAT),
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
    let grain = section(&mut doc, "grain");
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
    assert!(
        doc.edit("Link", |tx| tx.link(env, "mod-env.curve", lfo, 0.0, 1.0))
            .is_err(),
        "a modulator's own settings are not parameters the engine can link"
    );

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
    assert!(doc.has_presets(PATCH) && doc.has_presets("amp"));
    assert!(!doc.has_presets(LFO) && !doc.has_presets(TRACK));
}

#[test]
fn a_session_saves_and_opens_as_rhizome_text() {
    let mut doc = new_session();
    let patch = id(&doc, "/patches/patch");
    let (lfo, _) = doc.edit("Add LFO", |tx| tx.add_lfo(patch)).unwrap();
    let grain = section(&mut doc, "grain");
    doc.edit("Link", |tx| tx.link(grain, "grain.size", lfo, 0.0, 0.5))
        .unwrap();
    doc.edit("Add", |tx| tx.add_effect(patch, Fx::Reverb))
        .unwrap();

    let text = doc.tree().serialise();
    let (back, report) = rhizome_core::Tree::load(&text, doc.tree().registry().clone()).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(back.serialise(), text);
}

/// Every generator and effect is in exactly one category, and the table names
/// only kinds that exist, so the docs index never lists a kind twice or
/// under a stale name.
#[test]
fn every_generator_and_effect_has_one_category() {
    let kinds = kinds();
    for d in &kinds {
        let t = &d.node_type;
        let homes = CATEGORIES
            .iter()
            .filter(|(_, ks)| ks.contains(&t.name()))
            .count();
        // By group, not role: the drums track is a generator to the engine
        // but an arrangement node on the page.
        let browsable = matches!(t.group_tag(), Some("generator" | "effect"));
        assert_eq!(homes, usize::from(browsable), "{}", t.name());
        assert_eq!(t.category_tag().is_some(), browsable, "{}", t.name());
    }
    for (c, ks) in CATEGORIES {
        for k in *ks {
            assert!(
                kinds.iter().any(|d| d.node_type.name() == *k),
                "{c} names {k}, which is no kind"
            );
        }
    }
}

/// Every node the work area lists is named by its kind: the webview holds no
/// label of its own. The expected words are what the cards have always shown.
#[test]
fn every_node_the_webview_lists_has_a_label() {
    let words = node_words();
    let label_of = |id: &str| {
        words
            .iter()
            .find(|w| w.id == id)
            .unwrap_or_else(|| panic!("{id} has no words"))
    };
    let shown = [
        (PATCH, "Patch"),
        ("material", "Sample"),
        ("grain", "Granular"),
        ("fm", "FM"),
        ("noise", "Noise"),
        ("dust", "Dust"),
        ("env", "Envelope"),
        ("filter", "Filter"),
        ("ladder", "Ladder"),
        ("amp", "Output"),
        ("tape", "Tape"),
        (MASTER, "Output"),
    ];
    for (id, label) in shown {
        assert_eq!(label_of(id).label, label, "{id}");
    }
    for name in PATCH_NODES {
        assert!(!label_of(name).label.is_empty(), "{name}");
    }
    for kind in Fx::ALL {
        let w = label_of(kind.name());
        assert_eq!(w.label, kind.label(), "{}", kind.name());
    }
    for w in &words {
        assert!(!w.label.trim().is_empty(), "{} has an empty label", w.id);
        assert!(!w.summary.trim().is_empty(), "{} has no summary", w.id);
    }
}
