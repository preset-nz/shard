//! Node presets.
//!
//! A preset is one node's values under a name: every parameter whose id starts
//! with `<node>.`, except the node's own `.on` switch. Applying a preset changes
//! how a section sounds without switching it in or out.
//!
//! **Presets live in the patch** (Georg, 2026-09-13). They are document data:
//! saved with the `.shard` file, replaced when another patch loads, and gone if
//! a patch is closed unsaved. Keyed by node, then by name, with values by id,
//! for the same reason the patch keys by id: table order is not a wire format.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shard_dsp::params::PARAMS;
use shard_dsp::ParamBank;

/// One preset's values, by parameter id.
pub type Values = BTreeMap<String, f32>;

/// Long enough for anything a person types into a menu, short enough that a
/// menu stays a menu.
const MAX_NAME: usize = 60;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Presets(BTreeMap<String, BTreeMap<String, Values>>);

/// What an apply did. Surfaced rather than swallowed, like a patch load: a
/// preset saved before a rename must not look like it applied cleanly.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ApplyReport {
    pub applied: usize,
    /// Ids in the preset that were not applied: renamed since it was saved, or
    /// hand-edited to reach outside its node.
    pub unknown: Vec<String>,
}

impl Presets {
    pub fn is_empty(&self) -> bool {
        self.0.values().all(|names| names.is_empty())
    }

    /// The names saved for `node`, sorted.
    pub fn names(&self, node: &str) -> Vec<String> {
        self.0
            .get(node)
            .map(|names| names.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Store the node's current values under a new name.
    pub fn save(&mut self, bank: &ParamBank, node: &str, name: &str) -> Result<(), String> {
        let name = valid_name(name)?;
        let values = capture(bank, node)?;
        let names = self.0.entry(node.to_string()).or_default();
        if names.contains_key(&name) {
            return Err(format!(
                "a preset called “{name}” already exists here; update it instead"
            ));
        }
        names.insert(name, values);
        Ok(())
    }

    /// Overwrite an existing preset with the node's current values.
    pub fn update(&mut self, bank: &ParamBank, node: &str, name: &str) -> Result<(), String> {
        let values = capture(bank, node)?;
        let preset = self
            .0
            .get_mut(node)
            .and_then(|names| names.get_mut(name))
            .ok_or_else(|| missing(name))?;
        *preset = values;
        Ok(())
    }

    /// Write a preset's values back into the bank. The bank clamps, so a
    /// hand-edited preset cannot put a nonsense value on the audio thread.
    pub fn apply(&self, bank: &ParamBank, node: &str, name: &str) -> Result<ApplyReport, String> {
        let preset = self
            .0
            .get(node)
            .and_then(|names| names.get(name))
            .ok_or_else(|| missing(name))?;
        let mut report = ApplyReport::default();
        for (id, value) in preset {
            // A preset only ever writes its own node, and never its switch,
            // however the file was edited.
            if belongs(node, id) && bank.set_by_id(id, *value) {
                report.applied += 1;
            } else {
                report.unknown.push(id.clone());
            }
        }
        Ok(report)
    }
}

/// Whether `id` is one of `node`'s own values: under its prefix, not its switch.
fn belongs(node: &str, id: &str) -> bool {
    id.strip_prefix(node)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|rest| rest != "on")
}

fn capture(bank: &ParamBank, node: &str) -> Result<Values, String> {
    let values: Values = PARAMS
        .iter()
        .enumerate()
        .filter(|(_, p)| belongs(node, p.id))
        .map(|(i, p)| (p.id.to_string(), bank.get(i)))
        .collect();
    if values.is_empty() {
        Err(format!("there is no section called “{node}”"))
    } else {
        Ok(values)
    }
}

fn valid_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a preset needs a name".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!(
            "keep preset names to {MAX_NAME} characters or fewer"
        ));
    }
    Ok(name.to_string())
}

fn missing(name: &str) -> String {
    format!("there is no preset called “{name}” here")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(bank: &ParamBank, id: &str, v: f32) {
        assert!(bank.set_by_id(id, v), "no parameter {id}");
    }

    #[test]
    fn a_preset_holds_its_node_but_not_its_switch_or_its_neighbours() {
        let bank = ParamBank::new();
        let mut p = Presets::default();
        p.save(&bank, "crush", "gritty").unwrap();
        let values = &p.0["crush"]["gritty"];
        assert!(values.contains_key("crush.mix"));
        assert!(
            values.contains_key("crush.env.attack"),
            "the crush envelope belongs to crush"
        );
        assert!(
            !values.contains_key("crush.on"),
            "a preset must not switch a section"
        );
        assert!(values.keys().all(|id| id.starts_with("crush.")));
    }

    #[test]
    fn applying_puts_the_values_back_and_leaves_the_switch_alone() {
        let bank = ParamBank::new();
        set(&bank, "ring.mix", 0.75);
        set(&bank, "ring.freq", 900.0);
        let mut p = Presets::default();
        p.save(&bank, "ring", "bell").unwrap();

        set(&bank, "ring.mix", 0.25);
        set(&bank, "ring.freq", 50.0);
        set(&bank, "ring.on", 1.0);
        let report = p.apply(&bank, "ring", "bell").unwrap();

        assert!(report.unknown.is_empty());
        assert_eq!(bank.get_by_id("ring.mix"), Some(0.75));
        assert_eq!(bank.get_by_id("ring.freq"), Some(900.0));
        assert_eq!(
            bank.get_by_id("ring.on"),
            Some(1.0),
            "the switch is not part of it"
        );
    }

    #[test]
    fn save_refuses_a_duplicate_and_update_refuses_a_stranger() {
        let bank = ParamBank::new();
        let mut p = Presets::default();
        p.save(&bank, "grain", "cloud").unwrap();
        assert!(p.save(&bank, "grain", "cloud").is_err());
        assert!(
            p.save(&bank, "grain", "  cloud ").is_err(),
            "names are trimmed before they are compared"
        );
        assert!(p.update(&bank, "grain", "fog").is_err());

        set(&bank, "grain.size", 400.0);
        p.update(&bank, "grain", "cloud").unwrap();
        assert_eq!(p.0["grain"]["cloud"]["grain.size"], 400.0);
    }

    #[test]
    fn names_must_be_real_and_sections_must_exist() {
        let bank = ParamBank::new();
        let mut p = Presets::default();
        assert!(p.save(&bank, "grain", "   ").is_err());
        assert!(p.save(&bank, "grain", &"x".repeat(MAX_NAME + 1)).is_err());
        assert!(p.save(&bank, "nonsense", "x").is_err());
        assert!(p.names("grain").is_empty());
        assert!(p.is_empty());
    }

    #[test]
    fn an_edited_preset_cannot_reach_outside_its_node() {
        // Patches are text. A ring preset that names a grain value, its own
        // switch, or a value that no longer exists applies none of those, and
        // says so.
        let text =
            r#"{"ring":{"odd":{"ring.mix":0.5,"ring.on":1.0,"grain.gain":0.25,"ring.gone":2.0}}}"#;
        let p: Presets = serde_json::from_str(text).unwrap();
        let bank = ParamBank::new();
        let report = p.apply(&bank, "ring", "odd").unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(report.unknown.len(), 3);
        assert_eq!(bank.get_by_id("ring.on"), Some(0.0));
        assert_eq!(bank.get_by_id("grain.gain"), Some(1.0));
    }
}
