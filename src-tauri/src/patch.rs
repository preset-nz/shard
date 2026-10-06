//! The document.
//!
//! A `.shard` file holds two levels (Georg, 2026-09-14: "so the tracker is
//! one level up from the patch"). The **tracker** decides when things play:
//! tempo, swing and tracks of steps, see `tracker.rs`. The **patch** is the
//! sound: every parameter value, the LFOs and what follows them, node
//! presets, and a reference to the material. Not the material itself —
//! samples stay where they are, so a document is a few kilobytes of text you
//! can read and diff.
//!
//! **The arrangement's chain sits above the patch too,** as `arrangement`:
//! its drive, crush, ring, filter and output, the patch's fader and the
//! limiter, by id like the patch's values. See `shard_dsp::arrangement`.
//!
//! One patch today. Several patches inline in the same file is the node API's
//! decision 23, and tracks name the patch they play when that lands.
//!
//! **Parameters are stored by id, never by index.** The table's order is an
//! implementation detail and will change; the ids are a wire format and will
//! not. A patch written today has to load after a parameter is inserted in the
//! middle of the table, and this is the only thing that makes that true.
//!
//! Forward and backward compatible by construction: an id this build does not
//! know is ignored, and an id missing from the file keeps its default. Both
//! are reported so a silent partial load is impossible to mistake for a clean
//! one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shard_dsp::params::PARAMS;
use shard_dsp::ParamBank;

use crate::mapping::MapRef;
use crate::materials::{Pool, Wires};
use crate::modulation::{Modulation, Refused};
use crate::tracker::Tracker;

/// Bumped only for a change old builds cannot read. Adding parameters does
/// not need it, because unknown and missing ids are both handled.
pub const VERSION: u32 = 3;

/// The whole file: the tracker, the material pool, and the patch it plays.
#[derive(Debug, Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    /// Absent gives the default tracker: off, 120 bpm, four on the floor.
    #[serde(default)]
    pub tracker: Tracker,
    /// The material pool, as `materials` and `next_material_id` beside the
    /// tracker, since a pool outlives any one patch. See `materials.rs`.
    #[serde(flatten)]
    pub pool: Pool,
    /// The arrangement's values by id, above the patch. Absent gives its
    /// defaults: every arrangement effect off, unity gains, the limiter on.
    #[serde(default)]
    pub arrangement: BTreeMap<String, f32>,
    pub patch: Patch,
}

/// The sound.
#[derive(Debug, Serialize, Deserialize)]
pub struct Patch {
    /// Parameter values by id. A map, not a list, so order never matters.
    pub params: BTreeMap<String, f32>,
    /// Which material in the document's pool each generator reads, by id.
    /// Octave and trim rest on the materials, not here.
    #[serde(default, skip_serializing_if = "Wires::is_empty")]
    pub wires: Wires,
    /// Node presets, by node and then by name, kept as written: nothing reads
    /// the old format's presets any more.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub presets: serde_json::Value,
    /// The controller map this patch plays with, by stable id. App-wide
    /// data referred to, never copied in. Absent leaves the active map alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_map: Option<MapRef>,
    /// The LFOs, and which parameter follows which, as `lfos` and `links`
    /// beside `params`. Document data beside the values, never mixed into
    /// them: the values stay the hand's. See `modulation.rs`.
    #[serde(flatten)]
    pub modulation: Modulation,
}

/// What happened on load. Surfaced rather than logged, because a patch that
/// half-applied and a patch that applied cleanly must not look the same.
#[derive(Serialize, Default)]
pub struct LoadReport {
    pub applied: usize,
    /// Ids in the file that this build does not have. Usually a newer patch.
    pub unknown: Vec<String>,
    /// Ids this build has that the file did not carry. Left at their defaults.
    pub missing: Vec<String>,
    pub sample_path: Option<String>,
    /// Set when the sample could not be found where the patch said it was.
    pub sample_missing: bool,
    /// LFOs and links the engine could not use. They stay in the document.
    pub refused: Vec<Refused>,
    /// The name of a controller map the patch wanted and this Mac does not
    /// have. The active map is left alone.
    pub map_missing: Option<String>,
}

impl Document {
    pub fn new(tracker: Tracker, patch: Patch) -> Self {
        Self {
            version: VERSION,
            tracker,
            pool: Pool::default(),
            arrangement: BTreeMap::new(),
            patch,
        }
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let doc: Document = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if doc.version > VERSION {
            return Err(format!(
                "document is version {}, this build reads up to {VERSION}",
                doc.version
            ));
        }
        Ok(doc)
    }
}

/// Every arrangement value by id, for saving.
pub fn capture_arrangement(bank: &ParamBank) -> BTreeMap<String, f32> {
    bank.defs()
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.to_string(), bank.get(i)))
        .collect()
}

/// Replace the arrangement with a document's. Every row starts from its
/// default, so a document without an arrangement gets the default one rather
/// than whatever was open before. Answers with the ids it did not know.
pub fn apply_arrangement(values: &BTreeMap<String, f32>, bank: &ParamBank) -> Vec<String> {
    for (i, p) in bank.defs().iter().enumerate() {
        bank.set(i, p.default);
    }
    values
        .iter()
        .filter(|(id, value)| !bank.set_by_id(id, **value))
        .map(|(id, _)| id.clone())
        .collect()
}

impl Patch {
    pub fn capture(bank: &ParamBank, wires: Wires) -> Self {
        Self {
            params: PARAMS
                .iter()
                .enumerate()
                .map(|(i, p)| (p.id.to_string(), bank.get(i)))
                .collect(),
            wires,
            presets: serde_json::Value::Null,
            controller_map: None,
            modulation: Modulation::default(),
        }
    }

    /// Write every value into the bank. Values are clamped by the bank itself,
    /// so a hand-edited patch with a nonsense number cannot put one into the
    /// audio thread.
    pub fn apply(&self, bank: &ParamBank) -> LoadReport {
        let mut report = LoadReport::default();

        for (id, value) in &self.params {
            if bank.set_by_id(id, *value) {
                report.applied += 1;
            } else {
                report.unknown.push(id.clone());
            }
        }

        for p in PARAMS.iter() {
            if !self.params.contains_key(p.id) {
                report.missing.push(p.id.to_string());
            }
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document around a patch written as JSON, as a hand would.
    fn patch_of(params: &str) -> Patch {
        let text = format!(r#"{{"version":{VERSION},"patch":{{"params":{params}}}}}"#);
        Document::from_json(&text).unwrap().patch
    }

    fn round_trip(patch: Patch) -> Document {
        let text = Document::new(Tracker::default(), patch).to_json().unwrap();
        Document::from_json(&text).unwrap()
    }

    #[test]
    fn round_trips_every_parameter() {
        let bank = ParamBank::new();
        // Move everything off its default so a failure to save or load shows.
        for (i, p) in PARAMS.iter().enumerate() {
            bank.set(i, p.min + (p.max - p.min) * 0.37);
        }
        let before: Vec<f32> = (0..PARAMS.len()).map(|i| bank.get(i)).collect();

        let back = round_trip(Patch::capture(&bank, Wires::default()));

        let fresh = ParamBank::new();
        let report = back.patch.apply(&fresh);

        assert_eq!(report.applied, PARAMS.len());
        assert!(report.unknown.is_empty());
        assert!(report.missing.is_empty());
        for (i, expected) in before.iter().enumerate() {
            assert!(
                (fresh.get(i) - expected).abs() < 1e-6,
                "{} was {} not {expected}",
                PARAMS[i].id,
                fresh.get(i)
            );
        }
    }

    #[test]
    fn an_unknown_id_is_reported_not_swallowed() {
        let bank = ParamBank::new();
        let report = patch_of(r#"{"grain.size":200.0,"grain.nonsense":1.0}"#).apply(&bank);
        assert_eq!(report.applied, 1);
        assert_eq!(report.unknown, vec!["grain.nonsense"]);
        assert!(
            report.missing.len() > 5,
            "the rest should be reported missing"
        );
        assert_eq!(bank.get_by_id("grain.size"), Some(200.0));
    }

    #[test]
    fn a_missing_id_keeps_its_default() {
        let bank = ParamBank::new();
        patch_of(r#"{"grain.size":200.0}"#).apply(&bank);
        let density = &PARAMS[shard_dsp::params::index_of("grain.density").unwrap()];
        assert_eq!(bank.get_by_id("grain.density"), Some(density.default));
    }

    #[test]
    fn order_in_the_file_does_not_matter() {
        // The reason values are keyed by id. Inserting a parameter in the
        // middle of the table must not shift what an old patch loads.
        let a = patch_of(r#"{"grain.size":200.0,"amp.gain":0.5}"#);
        let b = patch_of(r#"{"amp.gain":0.5,"grain.size":200.0}"#);
        let (ba, bb) = (ParamBank::new(), ParamBank::new());
        a.apply(&ba);
        b.apply(&bb);
        for (i, def) in PARAMS.iter().enumerate() {
            assert_eq!(ba.get(i), bb.get(i), "{}", def.id);
        }
    }

    #[test]
    fn a_hand_edited_nonsense_value_is_clamped() {
        // Patches are text and will be edited by hand. Nothing out of range
        // may reach the audio thread.
        let bank = ParamBank::new();
        patch_of(r#"{"grain.density":1e30,"amp.gain":-5.0}"#).apply(&bank);
        let d = &PARAMS[shard_dsp::params::index_of("grain.density").unwrap()];
        assert_eq!(bank.get_by_id("grain.density"), Some(d.max));
        assert_eq!(bank.get_by_id("amp.gain"), Some(0.0));
    }

    #[test]
    fn a_newer_version_is_refused_with_a_reason() {
        let text = r#"{"version":999,"patch":{"params":{}}}"#;
        let err = Document::from_json(text).unwrap_err();
        assert!(err.contains("999"), "unhelpful error: {err}");
    }

    #[test]
    fn lfos_and_links_travel_with_the_patch() {
        use crate::modulation::{LfoRecord, LinkRecord};

        let bank = ParamBank::new();
        let mut patch = Patch::capture(&bank, Wires::default());
        patch.modulation.lfos.push(LfoRecord {
            id: 2,
            name: "wander".into(),
            rate: 0.3,
            shape: "smooth-random".into(),
            phase: 0.25,
        });
        patch.modulation.links.insert(
            "grain.position".into(),
            LinkRecord {
                source: 2,
                lo: 0.6,
                hi: 0.2,
            },
        );
        let saved = patch.modulation.clone();

        let text = Document::new(Tracker::default(), patch).to_json().unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(
            json["patch"]["lfos"].is_array(),
            "lfos sit in the patch: {text}"
        );
        assert!(
            json["patch"]["links"]["grain.position"].is_object(),
            "links sit beside params: {text}"
        );

        let back = Document::from_json(&text).unwrap();
        assert_eq!(back.patch.modulation, saved);
    }

    #[test]
    fn a_patch_with_no_sample_still_loads() {
        let bank = ParamBank::new();
        let back = round_trip(Patch::capture(&bank, Wires::default()));
        assert!(back.patch.wires.is_empty());
        assert!(!back.patch.apply(&bank).sample_missing);
    }

    #[test]
    fn the_tracker_sits_above_the_patch() {
        let bank = ParamBank::new();
        let tracker = Tracker {
            tempo: 87.0,
            tracks: vec![{
                let mut t = crate::tracker::Track {
                    length: 16,
                    ..Default::default()
                };
                for (i, step) in t.steps.iter_mut().enumerate() {
                    step.on = (43_690u64 >> i) & 1 == 1;
                }
                t.steps[3].pitch = -5;
                t.steps[8].pitch = 12;
                t.steps[15].pitch = -24;
                t.steps[2].hold = 4;
                t.steps[8].hold = 16;
                t
            }],
            ..Tracker::default()
        };
        let text = Document::new(tracker.clone(), Patch::capture(&bank, Wires::default()))
            .to_json()
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(
            json["tracker"]["tracks"].is_array(),
            "the tracker is top level: {text}"
        );
        assert!(
            json["patch"]["params"].is_object(),
            "the patch sits under it: {text}"
        );
        assert!(
            !json["patch"]["params"]
                .as_object()
                .unwrap()
                .keys()
                .any(|k| k.starts_with("seq.")),
            "no step lives in the patch: {text}"
        );
        // Each step is written as itself, readable by hand, and read back exactly.
        let steps = &json["tracker"]["tracks"][0]["steps"];
        assert_eq!(steps[0]["on"], false);
        assert_eq!(steps[1]["on"], true);
        assert_eq!(steps[3]["pitch"], -5);
        assert_eq!(steps[8]["hold"], 16);
        assert_eq!(Document::from_json(&text).unwrap().tracker, tracker);
    }

    #[test]
    fn a_document_without_a_tracker_gets_the_default_one() {
        let text = format!(r#"{{"version":{VERSION},"patch":{{"params":{{}}}}}}"#);
        assert_eq!(
            Document::from_json(&text).unwrap().tracker,
            Tracker::default()
        );
    }

    #[test]
    fn the_arrangement_sits_above_the_patch_and_round_trips() {
        let arr = ParamBank::for_table(shard_dsp::arrangement::params());
        arr.set_by_id("arrangement.fx.1.crush.on", 0.0);
        arr.set_by_id("arrangement.track.gain", 0.25);
        let mut doc = Document::new(Tracker::default(), patch_of("{}"));
        doc.arrangement = capture_arrangement(&arr);
        let text = doc.to_json().unwrap();
        let back = Document::from_json(&text).unwrap();

        let fresh = ParamBank::for_table(shard_dsp::arrangement::params());
        fresh.set_by_id("arrangement.filter.on", 1.0);
        let unknown = apply_arrangement(&back.arrangement, &fresh);
        assert!(unknown.is_empty(), "{unknown:?}");
        for (i, p) in arr.defs().iter().enumerate() {
            assert_eq!(fresh.get(i), arr.get(i), "{}", p.id);
        }
    }

    #[test]
    fn a_document_without_an_arrangement_gets_the_default_one() {
        let doc = Document::from_json(&round_trip(patch_of("{}")).to_json().unwrap()).unwrap();
        let bank = ParamBank::for_table(shard_dsp::arrangement::params());
        bank.set_by_id("arrangement.fx.1.crush.on", 0.0);
        apply_arrangement(&BTreeMap::new(), &bank);
        assert!(doc.arrangement.is_empty());
        // An added effect arrives on, so that is the default it goes back to.
        assert_eq!(bank.get_by_id("arrangement.fx.1.crush.on"), Some(1.0));
    }
}

/// Slice c of epic 32, run once: rewrites each old JSON file named in
/// `SHARD_CONVERT` (colon-separated) in place, in rhizome's format, by
/// replaying it through the session's own verbs. Deleted with this module.
#[test]
#[ignore]
fn convert_old_files() {
    use shard_dsp::fx;
    let Ok(paths) = std::env::var("SHARD_CONVERT") else {
        return;
    };
    for path in paths.split(':').filter(|p| !p.is_empty()) {
        let text = std::fs::read_to_string(path).expect("the file reads");
        let doc = Document::from_json(&text).expect("the file parses");
        assert!(doc.pool.materials.is_empty(), "{path}: has materials");
        assert!(doc.patch.presets.is_null(), "{path}: has presets");
        assert!(doc.patch.controller_map.is_none(), "{path}: has a map");
        let r = crate::session_tests::rig();
        let s = &r.session;
        s.set_tracker(doc.tracker.clone()).unwrap();
        for (layer, prefix, values) in [
            ("patch", "", &doc.patch.params),
            (
                "arrangement",
                shard_dsp::arrangement::PREFIX,
                &doc.arrangement,
            ),
        ] {
            // The chain first, in order, each old instance mapped to the copy
            // the session gives it.
            let mut order = [0.0; fx::ORDER_LEN];
            for (p, v) in order.iter_mut().enumerate() {
                *v = values
                    .get(&format!("{prefix}{}", fx::order_id(p)))
                    .copied()
                    .unwrap_or(0.0);
            }
            let mut copies = BTreeMap::new();
            for &n in fx::Order::sanitise(order).as_slice() {
                let kind = fx::kind_of(n as usize);
                let m = s.fx_add(layer, kind.name()).unwrap();
                copies.insert(n as usize, m);
            }
            for (id, &v) in values {
                if fx::is_order_row(id) {
                    continue;
                }
                let id = match fx::parse_row(id) {
                    Some((n, template)) => match copies.get(&n) {
                        Some(&m) => format!("{prefix}{}", fx::row_id(m, template)),
                        // An effect not in the chain: nothing plays it.
                        None => continue,
                    },
                    None => id.clone(),
                };
                s.set_param(&id, v)
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
            }
        }
        let m = &doc.patch.modulation;
        let mut ids = BTreeMap::new();
        let newest = |s: &crate::session::Session| {
            let (m, _) = s.modulation();
            m.lfos
                .iter()
                .map(|l| l.id)
                .chain(m.envelopes.iter().map(|e| e.id))
                .max()
                .unwrap()
        };
        for lfo in &m.lfos {
            s.add_lfo().unwrap();
            let id = newest(s);
            ids.insert(lfo.id, id);
            let mut lfo = lfo.clone();
            lfo.id = id;
            s.set_lfo(lfo).unwrap();
        }
        for env in &m.envelopes {
            s.add_envelope().unwrap();
            let id = newest(s);
            ids.insert(env.id, id);
            let mut env = env.clone();
            env.id = id;
            s.set_envelope(env).unwrap();
        }
        for (target, link) in &m.links {
            s.link_param(target, ids[&link.source], link.lo, link.hi)
                .unwrap();
        }
        s.save(std::path::Path::new(path)).unwrap();
        println!("converted {path}");
    }
}

/// Slice c's proof: each `old=new` pair in `SHARD_COMPARE` renders the same
/// four bars through the old loader and through the session. Deleted with
/// this module.
#[test]
#[ignore]
fn converted_files_sound_the_same() {
    use shard_dsp::{arrangement, Engine, ModSet, StepParams};
    fn render(bank: &ParamBank, arr: &ParamBank, steps: StepParams, set: ModSet) -> Vec<f32> {
        let mut e = Engine::new(48_000.0, 256);
        e.set_modulation(set);
        e.set_playing(true);
        let frames = (4.0 * 48_000.0 * 60.0 / steps.tempo_bpm * 4.0) as usize;
        let mut out = vec![0.0; 512];
        let mut all = Vec::new();
        while all.len() < frames * 2 {
            e.set_steps(steps);
            e.set_arrangement(arr, false);
            e.process_block(&mut out, bank);
            all.extend_from_slice(&out);
        }
        all
    }
    let Ok(pairs) = std::env::var("SHARD_COMPARE") else {
        return;
    };
    for pair in pairs.split(':').filter(|p| !p.is_empty()) {
        let (old, new) = pair.split_once('=').unwrap();
        let doc = Document::from_json(&std::fs::read_to_string(old).unwrap()).unwrap();
        let bank = ParamBank::new();
        doc.patch.apply(&bank);
        let arr = ParamBank::for_table(arrangement::params());
        apply_arrangement(&doc.arrangement, &arr);
        let mut steps = doc.tracker.clone().sanitised().params();
        steps.on = true;
        let (set, _) = doc.patch.modulation.build();
        let before = render(&bank, &arr, steps, set);

        let r = crate::session_tests::rig();
        let report = r.session.open(std::path::Path::new(new)).unwrap();
        assert!(report.unknown.is_empty(), "{new}: {:?}", report.unknown);
        let plan = r.session.plan();
        let mut steps = plan.steps();
        steps.on = true;
        let after = render(&r.bank, &r.arrangement, steps, plan.mod_set().0);

        assert_eq!(before.len(), after.len());
        let worst = before
            .iter()
            .zip(&after)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        let peak = before.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!("{new}: worst difference {worst:e}, peak {peak}");
        assert!(worst <= 1e-6, "{new}: differs by {worst}");
    }
}
