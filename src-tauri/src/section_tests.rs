//! A patch has no predefined sections (Georg, 2026-10-14: "writing into a
//! file needs to create the nodes it needs ... there should not be
//! predefined areas"). A write makes the section it needs, in its own undo
//! step; an absent section reads as its defaults; a generator is in the patch
//! while its node is, and its switch only mutes it.

use shard_dsp::params::PARAMS;
use shard_dsp::ParamBank;

use crate::object_model::patch_node;
use crate::session_tests::{rig, Rig};

/// The current patch's sections, by name, in the tree's order.
fn held(r: &Rig) -> Vec<String> {
    r.session.read(|d| {
        patch_node(d.tree(), r.session.plan().current)
            .map(|p| p.children().map(|n| n.name().to_string()).collect())
            .unwrap_or_default()
    })
}

fn has(r: &Rig, kind: &str) -> bool {
    held(r).iter().any(|n| n == kind)
}

fn get(r: &Rig, id: &str) -> f32 {
    r.bank.get_by_id(id).unwrap()
}

fn default_of(id: &str) -> f32 {
    PARAMS.iter().find(|p| p.id == id).unwrap().default
}

fn undo_label(r: &Rig) -> Option<String> {
    r.session.history().0
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shard-sections-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("saved.shard")
}

#[test]
fn a_new_patch_holds_only_its_material_and_sounds_as_before() {
    let r = rig();
    r.session.new_patch().unwrap();
    assert_eq!(held(&r), ["material"]);
    assert_eq!(r.session.generators(), ["material"]);
    // The bank a new patch had when it held every section.
    let fresh = ParamBank::new();
    for (i, def) in PARAMS.iter().enumerate() {
        assert_eq!(r.bank.get(i), fresh.get(i), "{}", def.id);
    }
    let pool = r.session.pool();
    assert_eq!(pool.wires.material, Some(pool.materials[0].id), "the drone");
    assert_eq!(pool.wires.grain, None);
}

#[test]
fn a_write_to_an_absent_section_makes_it_and_one_undo_takes_both() {
    let r = rig();
    assert!(!has(&r, "filter"));
    r.session.set_param("filter.cutoff", 640.0).unwrap();
    assert!(has(&r, "filter"));
    assert_eq!(get(&r, "filter.cutoff"), 640.0);
    assert_eq!(undo_label(&r).as_deref(), Some("Set Cutoff"));

    // A drag is still one step: the section is made by its first move.
    r.session.set_param("filter.cutoff", 700.0).unwrap();
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Set Cutoff"));
    assert!(!has(&r, "filter"), "the section went with the value");
    assert_eq!(get(&r, "filter.cutoff"), default_of("filter.cutoff"));
    assert_eq!(undo_label(&r), None, "it was one step");

    assert_eq!(r.session.redo().unwrap().as_deref(), Some("Set Cutoff"));
    assert!(has(&r, "filter"));
    assert_eq!(get(&r, "filter.cutoff"), 700.0);
}

#[test]
fn a_write_the_absent_section_already_reads_makes_nothing() {
    let r = rig();
    r.session
        .set_param("filter.cutoff", default_of("filter.cutoff"))
        .unwrap();
    r.session.set_param("noise.on", 0.0).unwrap();
    assert_eq!(held(&r), ["material"]);
    assert_eq!(undo_label(&r), None, "no step");
    assert!(!r.session.is_unsaved());
}

#[test]
fn a_write_to_an_absent_generator_makes_it_present_and_reading_the_drone() {
    let r = rig();
    r.session.set_param("grain.on", 1.0).unwrap();
    assert_eq!(r.session.generators(), ["material", "grain"]);
    let pool = r.session.pool();
    assert_eq!(pool.wires.grain, Some(pool.materials[0].id));
}

#[test]
fn add_switch_off_and_remove_a_generator() {
    let r = rig();
    assert!(r.session.add_generator("grain").unwrap());
    assert_eq!(r.session.generators(), ["material", "grain"]);
    assert_eq!(get(&r, "grain.on"), 1.0, "added switched on");
    assert_eq!(undo_label(&r).as_deref(), Some("Add Granular"));
    assert!(
        !r.session.add_generator("grain").unwrap(),
        "one of each: a second add makes no step"
    );

    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Add Granular"));
    assert_eq!(r.session.generators(), ["material"]);
    assert_eq!(undo_label(&r), None, "Add was one step");
    r.session.redo().unwrap();

    // Off mutes it, and it stays.
    r.session.set_param("grain.size", 80.0).unwrap();
    r.session.set_param("grain.on", 0.0).unwrap();
    assert_eq!(r.session.generators(), ["material", "grain"]);
    assert_eq!(get(&r, "grain.on"), 0.0);

    // Remove deletes it; undo brings it back with its settings.
    r.session.set_param("grain.on", 1.0).unwrap();
    assert!(r.session.remove_generator("grain").unwrap());
    assert_eq!(undo_label(&r).as_deref(), Some("Remove Granular"));
    assert_eq!(r.session.generators(), ["material"]);
    assert_eq!(get(&r, "grain.on"), 0.0, "absent is silent");
    assert_eq!(get(&r, "grain.size"), default_of("grain.size"));
    assert_eq!(r.session.pool().wires.grain, None);

    r.session.undo().unwrap();
    assert_eq!(r.session.generators(), ["material", "grain"]);
    assert_eq!(get(&r, "grain.on"), 1.0);
    assert_eq!(get(&r, "grain.size"), 80.0);
    assert!(
        r.session.pool().wires.grain.is_some(),
        "still reading the drone"
    );
}

#[test]
fn an_absent_generator_is_silent_though_its_switch_defaults_on() {
    let r = rig();
    assert_eq!(default_of("material.on"), 1.0);
    r.session.remove_generator("material").unwrap();
    assert_eq!(undo_label(&r).as_deref(), Some("Remove Sample"));
    assert!(r.session.generators().is_empty());
    assert_eq!(get(&r, "material.on"), 0.0);
    assert_eq!(r.session.pool().wires.material, None);

    // Writing its switch on makes it again, at its defaults: it reads the
    // drone, as a new patch's does.
    r.session.set_param("material.on", 1.0).unwrap();
    assert_eq!(r.session.generators(), ["material"]);
    assert_eq!(get(&r, "material.on"), 1.0);
    assert!(r.session.pool().wires.material.is_some());
}

#[test]
fn adding_a_material_wires_only_the_generators_there_are() {
    let r = rig();
    r.session.remove_generator("material").unwrap();
    let drone = r.session.pool().materials[0].id;
    r.session.remove_material(drone).unwrap();
    r.session.add_drone().unwrap();
    assert!(r.session.generators().is_empty(), "no generator made");

    // Picking one for a generator is a write to it, and makes it.
    let drone = r.session.pool().materials[0].id;
    r.session.wire_material("grain", Some(drone)).unwrap();
    assert_eq!(r.session.generators(), ["grain"]);
    assert_eq!(r.session.pool().wires.grain, Some(drone));
    assert_eq!(get(&r, "grain.on"), 0.0, "made at its defaults, off");
}

#[test]
fn a_preset_applied_to_an_absent_section_makes_it_in_one_step() {
    let r = rig();
    r.session.set_param("filter.cutoff", 640.0).unwrap();
    r.session.save_preset("filter", "Dark").unwrap();
    r.session.new_patch().unwrap();
    assert!(!has(&r, "filter"));
    assert_eq!(r.session.preset_names("filter"), ["Dark"]);

    r.session.apply_preset("filter", "Dark").unwrap();
    assert!(has(&r, "filter"));
    assert_eq!(get(&r, "filter.cutoff"), 640.0);
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Apply Preset"));
    assert!(!has(&r, "filter"), "one step took both");
    assert_eq!(undo_label(&r).as_deref(), Some("New Patch"));

    // Saving one from an absent section makes it the same way.
    r.session.save_preset("env", "Plain").unwrap();
    assert!(has(&r, "env"));
    assert_eq!(r.session.undo().unwrap().as_deref(), Some("Save Preset"));
    assert!(!has(&r, "env"));
}

#[test]
fn a_sparse_patch_saves_and_opens_as_it_was() {
    let r = rig();
    r.session.set_param("noise.color", -0.5).unwrap();
    r.session.set_param("material.on", 0.0).unwrap();
    let path = temp("sparse");
    r.session.save(&path).unwrap();
    let (sections, generators) = (held(&r), r.session.generators());
    assert_eq!(sections, ["material", "noise"]);
    let values: Vec<f32> = (0..PARAMS.len()).map(|i| r.bank.get(i)).collect();

    let again = rig();
    let report = again.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert_eq!(held(&again), sections);
    assert_eq!(again.session.generators(), generators);
    let back: Vec<f32> = (0..PARAMS.len()).map(|i| again.bank.get(i)).collect();
    assert_eq!(back, values);
}
