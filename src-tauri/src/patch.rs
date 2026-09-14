//! The document.
//!
//! A patch is the sound: every parameter value, the LFOs and what follows
//! them, node presets, and a reference to the material. Not the material itself — samples stay
//! where they are, so a patch is a few kilobytes of text you can read and diff.
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
use crate::modulation::{Modulation, Refused};
use crate::presets::Presets;

/// Bumped only for a change old builds cannot read. Adding parameters does
/// not need it, because unknown and missing ids are both handled.
pub const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Patch {
    pub version: u32,
    /// Parameter values by id. A map, not a list, so order never matters.
    pub params: BTreeMap<String, f32>,
    /// Where the material was. Absolute, because this is a personal tool and
    /// samples live wherever they live.
    #[serde(default)]
    pub sample_path: Option<String>,
    /// Node presets, by node and then by name. Document data, so they travel
    /// with the patch. See `presets.rs`.
    #[serde(default, skip_serializing_if = "Presets::is_empty")]
    pub presets: Presets,
    /// The controller map this patch plays with, by stable id. App-wide
    /// data referred to, never copied in. Absent leaves the active map alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_map: Option<MapRef>,
    /// The LFOs, and which parameter follows which, as top-level `lfos` and
    /// `links`. Document data beside the values, never mixed into them: the
    /// values stay the hand's. See `modulation.rs`.
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

impl Patch {
    pub fn capture(bank: &ParamBank, sample_path: Option<String>) -> Self {
        Self {
            version: VERSION,
            params: PARAMS
                .iter()
                .enumerate()
                .map(|(i, p)| (p.id.to_string(), bank.get(i)))
                .collect(),
            sample_path,
            presets: Presets::default(),
            controller_map: None,
            modulation: Modulation::default(),
        }
    }

    /// Write every value into the bank. Values are clamped by the bank itself,
    /// so a hand-edited patch with a nonsense number cannot put one into the
    /// audio thread.
    pub fn apply(&self, bank: &ParamBank) -> LoadReport {
        let mut report = LoadReport {
            sample_path: self.sample_path.clone(),
            ..Default::default()
        };

        for (id, value) in &self.params {
            if bank.set_by_id(id, *value) {
                report.applied += 1;
            } else {
                report.unknown.push(id.clone());
            }
        }

        for p in PARAMS {
            if !self.params.contains_key(p.id) {
                report.missing.push(p.id.to_string());
            }
        }

        report
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let patch: Patch = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if patch.version > VERSION {
            return Err(format!(
                "patch is version {}, this build reads up to {VERSION}",
                patch.version
            ));
        }
        Ok(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_parameter() {
        let bank = ParamBank::new();
        // Move everything off its default so a failure to save or load shows.
        for (i, p) in PARAMS.iter().enumerate() {
            bank.set(i, p.min + (p.max - p.min) * 0.37);
        }
        let before: Vec<f32> = (0..PARAMS.len()).map(|i| bank.get(i)).collect();

        let patch = Patch::capture(&bank, None);
        let text = patch.to_json().unwrap();

        let fresh = ParamBank::new();
        let report = Patch::from_json(&text).unwrap().apply(&fresh);

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
        let text = r#"{"version":1,"params":{"grain.size":200.0,"grain.nonsense":1.0}}"#;
        let bank = ParamBank::new();
        let report = Patch::from_json(text).unwrap().apply(&bank);
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
        let text = r#"{"version":1,"params":{"grain.size":200.0}}"#;
        let bank = ParamBank::new();
        Patch::from_json(text).unwrap().apply(&bank);
        let density = &PARAMS[shard_dsp::params::index_of("grain.density").unwrap()];
        assert_eq!(bank.get_by_id("grain.density"), Some(density.default));
    }

    #[test]
    fn order_in_the_file_does_not_matter() {
        // The reason values are keyed by id. Inserting a parameter in the
        // middle of the table must not shift what an old patch loads.
        let a = r#"{"version":1,"params":{"grain.size":200.0,"amp.gain":0.5}}"#;
        let b = r#"{"version":1,"params":{"amp.gain":0.5,"grain.size":200.0}}"#;
        let (ba, bb) = (ParamBank::new(), ParamBank::new());
        Patch::from_json(a).unwrap().apply(&ba);
        Patch::from_json(b).unwrap().apply(&bb);
        for (i, def) in PARAMS.iter().enumerate() {
            assert_eq!(ba.get(i), bb.get(i), "{}", def.id);
        }
    }

    #[test]
    fn a_hand_edited_nonsense_value_is_clamped() {
        // Patches are text and will be edited by hand. Nothing out of range
        // may reach the audio thread.
        let text = r#"{"version":1,"params":{"grain.density":1e30,"amp.gain":-5.0}}"#;
        let bank = ParamBank::new();
        Patch::from_json(text).unwrap().apply(&bank);
        let d = &PARAMS[shard_dsp::params::index_of("grain.density").unwrap()];
        assert_eq!(bank.get_by_id("grain.density"), Some(d.max));
        assert_eq!(bank.get_by_id("amp.gain"), Some(0.0));
    }

    #[test]
    fn a_newer_version_is_refused_with_a_reason() {
        let text = r#"{"version":999,"params":{}}"#;
        let err = Patch::from_json(text).unwrap_err();
        assert!(err.contains("999"), "unhelpful error: {err}");
    }

    #[test]
    fn the_sample_path_survives_the_round_trip() {
        let bank = ParamBank::new();
        let patch = Patch::capture(&bank, Some("/tmp/x.wav".into()));
        let back = Patch::from_json(&patch.to_json().unwrap()).unwrap();
        assert_eq!(back.sample_path.as_deref(), Some("/tmp/x.wav"));
    }

    #[test]
    fn presets_travel_with_the_patch() {
        let bank = ParamBank::new();
        let mut patch = Patch::capture(&bank, None);
        patch
            .presets
            .save(&bank, &Default::default(), "grain", "cloud")
            .unwrap();
        let back = Patch::from_json(&patch.to_json().unwrap()).unwrap();
        assert_eq!(back.presets, patch.presets);
        assert_eq!(back.presets.names("grain"), vec!["cloud"]);
    }

    #[test]
    fn lfos_and_links_travel_with_the_patch() {
        use crate::modulation::{LfoRecord, LinkRecord};

        let bank = ParamBank::new();
        let mut patch = Patch::capture(&bank, None);
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
                lfo: 2,
                lo: 0.6,
                hi: 0.2,
            },
        );

        let text = patch.to_json().unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(json["lfos"].is_array(), "lfos is a top-level list: {text}");
        assert!(
            json["links"]["grain.position"].is_object(),
            "links sit beside params: {text}"
        );

        let back = Patch::from_json(&text).unwrap();
        assert_eq!(back.modulation, patch.modulation);
    }

    #[test]
    fn a_patch_with_no_sample_still_loads() {
        let bank = ParamBank::new();
        let patch = Patch::capture(&bank, None);
        let back = Patch::from_json(&patch.to_json().unwrap()).unwrap();
        assert!(back.sample_path.is_none());
        assert!(!back.apply(&bank).sample_missing);
    }
}
