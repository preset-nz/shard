//! Node presets.
//!
//! A preset is one node's state under a name: every parameter whose id starts
//! with `<node>.`, except the node's own `.on` switch, and which LFO each of
//! those parameters follows. Applying a preset changes how a section sounds
//! without switching it in or out.
//!
//! **Presets live in the patch** (Georg, 2026-09-13). They are document data:
//! saved with the `.shard` file, replaced when another patch loads, and gone if
//! a patch is closed unsaved. Keyed by node, then by name, with values by id,
//! for the same reason the patch keys by id: table order is not a wire format.
//!
//! **Links are part of a preset** (modulation decision 6). A link names its
//! LFO by id, so applying a preset follows whatever that LFO is set to in the
//! patch it is applied in.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shard_dsp::params::PARAMS;
use shard_dsp::ParamBank;

use crate::modulation::{Links, Refused};

/// One preset's values, by parameter id.
pub type Values = BTreeMap<String, f32>;

/// Long enough for anything a person types into a menu, short enough that a
/// menu stays a menu.
const MAX_NAME: usize = 60;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub values: Values,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: Links,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Presets(BTreeMap<String, BTreeMap<String, Preset>>);

/// What an apply did. Surfaced rather than swallowed, like a patch load: a
/// preset saved before a rename must not look like it applied cleanly.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ApplyReport {
    pub applied: usize,
    /// Ids in the preset that were not applied: renamed since it was saved, or
    /// hand-edited to reach outside its node. Values and links alike.
    pub unknown: Vec<String>,
    /// Links the preset restored that the engine could not use, such as one to
    /// an LFO this patch does not have.
    pub refused: Vec<Refused>,
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

    /// Store the node's current values and links under a new name.
    pub fn save(
        &mut self,
        bank: &ParamBank,
        links: &Links,
        node: &str,
        name: &str,
    ) -> Result<(), String> {
        let name = valid_name(name)?;
        let preset = capture(bank, links, node)?;
        let names = self.0.entry(node.to_string()).or_default();
        if names.contains_key(&name) {
            return Err(format!(
                "a preset called “{name}” already exists here; update it instead"
            ));
        }
        names.insert(name, preset);
        Ok(())
    }

    /// Overwrite an existing preset with the node's current values and links.
    pub fn update(
        &mut self,
        bank: &ParamBank,
        links: &Links,
        node: &str,
        name: &str,
    ) -> Result<(), String> {
        let captured = capture(bank, links, node)?;
        let preset = self
            .0
            .get_mut(node)
            .and_then(|names| names.get_mut(name))
            .ok_or_else(|| missing(name))?;
        *preset = captured;
        Ok(())
    }

    /// Write a preset's values back into the bank and its links into the
    /// document. The node's links are replaced as a whole, so a value that
    /// was unlinked when the preset was saved is unlinked again. The bank
    /// clamps, so a hand-edited preset cannot put a nonsense value on the
    /// audio thread.
    pub fn apply(
        &self,
        bank: &ParamBank,
        links: &mut Links,
        node: &str,
        name: &str,
    ) -> Result<ApplyReport, String> {
        let preset = self
            .0
            .get(node)
            .and_then(|names| names.get(name))
            .ok_or_else(|| missing(name))?;
        let mut report = ApplyReport::default();
        for (id, value) in &preset.values {
            // A preset only ever writes its own node, and never its switch,
            // however the file was edited.
            if belongs(node, id) && bank.set_by_id(id, *value) {
                report.applied += 1;
            } else {
                report.unknown.push(id.clone());
            }
        }

        links.retain(|id, _| !belongs(node, id));
        for (id, link) in &preset.links {
            if belongs(node, id) {
                links.insert(id.clone(), *link);
            } else {
                report.unknown.push(id.clone());
            }
        }
        Ok(report)
    }
}

/// Whether `id` is one of `node`'s own values: under its prefix, not its switch.
pub fn belongs(node: &str, id: &str) -> bool {
    id.strip_prefix(node)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|rest| rest != "on")
}

fn capture(bank: &ParamBank, links: &Links, node: &str) -> Result<Preset, String> {
    let values: Values = PARAMS
        .iter()
        .enumerate()
        .filter(|(_, p)| belongs(node, p.id))
        .map(|(i, p)| (p.id.to_string(), bank.get(i)))
        .collect();
    if values.is_empty() {
        return Err(format!("there is no section called “{node}”"));
    }
    let links = links
        .iter()
        .filter(|(id, _)| belongs(node, id))
        .map(|(id, link)| (id.clone(), *link))
        .collect();
    Ok(Preset { values, links })
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
    use crate::modulation::LinkRecord;

    fn set(bank: &ParamBank, id: &str, v: f32) {
        assert!(bank.set_by_id(id, v), "no parameter {id}");
    }

    fn link(source: u64, depth: f32) -> LinkRecord {
        LinkRecord {
            source,
            lo: 1.0 - depth,
            hi: 1.0,
        }
    }

    #[test]
    fn a_preset_holds_its_node_but_not_its_switch_or_its_neighbours() {
        let bank = ParamBank::new();
        let mut p = Presets::default();
        p.save(&bank, &Links::new(), "crush", "gritty").unwrap();
        let values = &p.0["crush"]["gritty"].values;
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
        let mut links = Links::new();
        set(&bank, "ring.mix", 0.75);
        set(&bank, "ring.freq", 900.0);
        let mut p = Presets::default();
        p.save(&bank, &links, "ring", "bell").unwrap();

        set(&bank, "ring.mix", 0.25);
        set(&bank, "ring.freq", 50.0);
        set(&bank, "ring.on", 1.0);
        let report = p.apply(&bank, &mut links, "ring", "bell").unwrap();

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
    fn a_preset_restores_its_own_links_and_no_one_elses() {
        let bank = ParamBank::new();
        let mut links = Links::from([
            ("grain.size".into(), link(1, 0.5)),
            ("ring.freq".into(), link(2, 0.3)),
        ]);
        let mut p = Presets::default();
        p.save(&bank, &links, "grain", "wobbly").unwrap();
        assert_eq!(
            p.0["grain"]["wobbly"].links,
            Links::from([("grain.size".into(), link(1, 0.5))]),
            "only grain's links are captured"
        );

        // Relink grain.size, link something new in grain, and move ring's.
        links.insert("grain.size".into(), link(3, -1.0));
        links.insert("grain.density".into(), link(1, 0.2));
        links.insert("ring.freq".into(), link(4, 0.9));

        let report = p.apply(&bank, &mut links, "grain", "wobbly").unwrap();
        assert!(report.unknown.is_empty(), "{report:?}");
        assert_eq!(
            links,
            Links::from([
                ("grain.size".into(), link(1, 0.5)),
                ("ring.freq".into(), link(4, 0.9)),
            ]),
            "grain's links come back as saved, density is unlinked again, and ring is left alone"
        );
    }

    #[test]
    fn save_refuses_a_duplicate_and_update_refuses_a_stranger() {
        let bank = ParamBank::new();
        let links = Links::new();
        let mut p = Presets::default();
        p.save(&bank, &links, "grain", "cloud").unwrap();
        assert!(p.save(&bank, &links, "grain", "cloud").is_err());
        assert!(
            p.save(&bank, &links, "grain", "  cloud ").is_err(),
            "names are trimmed before they are compared"
        );
        assert!(p.update(&bank, &links, "grain", "fog").is_err());

        set(&bank, "grain.size", 400.0);
        p.update(&bank, &links, "grain", "cloud").unwrap();
        assert_eq!(p.0["grain"]["cloud"].values["grain.size"], 400.0);
    }

    #[test]
    fn names_must_be_real_and_sections_must_exist() {
        let bank = ParamBank::new();
        let links = Links::new();
        let mut p = Presets::default();
        assert!(p.save(&bank, &links, "grain", "   ").is_err());
        assert!(p
            .save(&bank, &links, "grain", &"x".repeat(MAX_NAME + 1))
            .is_err());
        assert!(p.save(&bank, &links, "nonsense", "x").is_err());
        assert!(p.names("grain").is_empty());
        assert!(p.is_empty());
    }

    #[test]
    fn an_edited_preset_cannot_reach_outside_its_node() {
        // Patches are text. A ring preset that names a grain value, its own
        // switch, a value that no longer exists, or a link on another node's
        // value applies none of those, and says so.
        let text = r#"{"ring":{"odd":{
            "values":{"ring.mix":0.5,"ring.on":1.0,"grain.gain":0.25,"ring.gone":2.0},
            "links":{"grain.size":{"lfo":1,"lo":0.5,"hi":1.0},"ring.freq":{"lfo":1,"lo":0.4,"hi":0.5}}
        }}}"#;
        let p: Presets = serde_json::from_str(text).unwrap();
        let bank = ParamBank::new();
        let mut links = Links::new();
        let report = p.apply(&bank, &mut links, "ring", "odd").unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(report.unknown.len(), 4, "{report:?}");
        assert!(report.unknown.contains(&"grain.size".to_string()));
        assert_eq!(bank.get_by_id("ring.on"), Some(0.0));
        assert_eq!(bank.get_by_id("grain.gain"), Some(1.0));
        assert_eq!(links.keys().collect::<Vec<_>>(), vec!["ring.freq"]);
    }
}
