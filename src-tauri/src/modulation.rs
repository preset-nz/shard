//! LFOs, modulation envelopes and links, as the document holds them.
//!
//! The patch keeps a list of LFOs, a list of envelopes, and a map from
//! parameter ids to the modulator each one follows. The two lists share one id
//! counter, because a link names its source by id alone. `build` turns that into a `ModSet` on the command thread, and
//! the audio thread swaps it in whole. See
//! `guidance/projects/shard/design/modulation.md`.
//!
//! **Nothing refused is dropped from the document.** A link to an LFO that has
//! since been removed stays in the file, leaves its parameter at the hand's
//! value, and is reported every time the set is built, the way a patch load
//! reports an unknown id.
//!
//! **Edits are checked before anything changes.** The commands that add,
//! change and link LFOs refuse a bad edit with a reason and leave the document
//! as it was, because the UI only ever offers valid choices and a bad one is a
//! bug to surface, not a quirk to keep.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shard_dsp::modulation::{MAX_RATE_HZ, MIN_RATE_HZ};
use shard_dsp::params::index_of;
use shard_dsp::{EnvSpec, LfoSpec, LinkError, ModSet, Shape, Taper, PARAMS};

/// Long enough for anything typed into the tree, short enough to stay a label.
const MAX_NAME: usize = 60;

/// A new LFO wanders slowly: the movement drift used to give.
const NEW_RATE_HZ: f32 = 0.1;
const NEW_SHAPE: &str = "smooth-random";

/// The longest envelope stage, in ms. The engine fits stages to the pass
/// anyway; this only keeps a typed number sane.
pub const MAX_STAGE_MS: f32 = 10_000.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LfoRecord {
    /// Stable for the life of the LFO and never reused, so a link survives
    /// renaming and reordering.
    pub id: u64,
    #[serde(default)]
    pub name: String,
    /// In Hz. The engine clamps it to its range.
    pub rate: f32,
    /// One of `Shape::NAMES`, such as `smooth-random`.
    pub shape: String,
    /// 0 to 1.
    #[serde(default)]
    pub phase: f32,
}

/// A modulation envelope: ADSR on the pass's clock, so a step starts it.
/// Named "Mod envelope" in the app, so it is not mistaken for the Envelope
/// node, which shapes the amplitude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvelopeRecord {
    /// Shares the id space with the LFOs.
    pub id: u64,
    #[serde(default)]
    pub name: String,
    /// In ms.
    pub attack: f32,
    pub decay: f32,
    /// 0 to 1.
    pub sustain: f32,
    pub release: f32,
}

/// A new envelope is a pluck, the shape a filter envelope most often has.
fn new_envelope(id: u64) -> EnvelopeRecord {
    EnvelopeRecord {
        id,
        name: format!("Mod envelope {id}"),
        attack: 5.0,
        decay: 300.0,
        sustain: 0.2,
        release: 100.0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinkRecord {
    /// The id of the LFO or envelope the parameter follows.
    #[serde(alias = "lfo")]
    pub source: u64,
    /// The ends of the sweep, 0 to 1 of the parameter's range. An LFO's
    /// trough, or an envelope at rest, lands on `lo`; the crest or the peak on
    /// `hi`. `lo` above `hi` runs it the other way.
    pub lo: f32,
    pub hi: f32,
}

/// Links by parameter id. A map, so a parameter has at most one source.
pub type Links = BTreeMap<String, LinkRecord>;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Modulation {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lfos: Vec<LfoRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envelopes: Vec<EnvelopeRecord>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: Links,
    /// The id the next new LFO or envelope takes. Saved, so the id of a
    /// removed one is never handed out again: a preset or a link that still
    /// names it must not start following a different modulator.
    #[serde(default, alias = "next_lfo_id", skip_serializing_if = "is_zero")]
    pub next_modulator_id: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Something in the document the engine could not use.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Refused {
    /// The parameter id for a link, `lfo:<id>` for an LFO, or `envelope:<id>`
    /// for an envelope.
    pub id: String,
    pub reason: String,
}

impl Modulation {
    /// The engine's view of this document, and what it had to leave out.
    /// Allocates, so it belongs on the command thread.
    pub fn build(&self) -> (ModSet, Vec<Refused>) {
        let mut refused = Vec::new();
        let mut specs: Vec<LfoSpec> = Vec::with_capacity(self.lfos.len());
        for lfo in &self.lfos {
            let id = format!("lfo:{}", lfo.id);
            if specs.iter().any(|s| s.id == lfo.id) {
                refused.push(Refused {
                    id,
                    reason: format!("LFO {} appears twice, and only the first is used", lfo.id),
                });
                continue;
            }
            let Some(shape) = Shape::from_name(&lfo.shape) else {
                refused.push(Refused {
                    id,
                    reason: format!(
                        "LFO {} has a shape this build does not know, “{}”",
                        lfo.id, lfo.shape
                    ),
                });
                continue;
            };
            specs.push(LfoSpec {
                id: lfo.id,
                rate_hz: lfo.rate,
                shape,
                phase: lfo.phase,
            });
        }

        // Envelopes after the LFOs, and refused if their id is taken by
        // either: a link could not tell which it meant.
        let mut envs: Vec<EnvSpec> = Vec::with_capacity(self.envelopes.len());
        for env in &self.envelopes {
            if specs.iter().any(|s| s.id == env.id) || envs.iter().any(|e| e.id == env.id) {
                refused.push(Refused {
                    id: format!("envelope:{}", env.id),
                    reason: format!(
                        "envelope {} shares its id with another modulator, and is left out",
                        env.id
                    ),
                });
                continue;
            }
            envs.push(EnvSpec {
                id: env.id,
                attack_ms: env.attack,
                decay_ms: env.decay,
                sustain: env.sustain,
                release_ms: env.release,
            });
        }

        let mut set = ModSet::with_envelopes(&specs, &envs);
        for (param, link) in &self.links {
            if let Err(e) = set.link(param, link.source, link.lo, link.hi) {
                refused.push(Refused {
                    id: param.clone(),
                    reason: e.to_string(),
                });
            }
        }
        (set, refused)
    }

    /// An id no LFO or envelope has had.
    fn fresh_id(&mut self) -> u64 {
        // Never below an id already in either list, which a hand-edited patch
        // can carry past its counter.
        let floor = self
            .lfos
            .iter()
            .map(|l| l.id)
            .chain(self.envelopes.iter().map(|e| e.id))
            .map(|id| id.saturating_add(1))
            .max()
            .unwrap_or(1);
        let id = self.next_modulator_id.max(floor);
        self.next_modulator_id = id.saturating_add(1);
        id
    }

    fn has_source(&self, id: u64) -> bool {
        self.lfos.iter().any(|l| l.id == id) || self.envelopes.iter().any(|e| e.id == id)
    }

    /// Add an envelope with a fresh id and return it.
    pub fn add_envelope(&mut self) -> EnvelopeRecord {
        let env = new_envelope(self.fresh_id());
        self.envelopes.push(env.clone());
        env
    }

    /// Remove an envelope. Its links stay and are reported, as an LFO's do.
    pub fn remove_envelope(&mut self, id: u64) -> Result<(), String> {
        let before = self.envelopes.len();
        self.envelopes.retain(|e| e.id != id);
        if self.envelopes.len() == before {
            return Err(LinkError::UnknownSource(id).to_string());
        }
        Ok(())
    }

    /// Replace an envelope's name and stages, matched by `edit.id`. Stages
    /// are brought into range; the name must already be valid.
    pub fn set_envelope(&mut self, edit: EnvelopeRecord) -> Result<(), String> {
        let stages = [edit.attack, edit.decay, edit.sustain, edit.release];
        if !stages.iter().all(|v| v.is_finite()) {
            return Err("an envelope's stages must be numbers".into());
        }
        let name = valid_name(&edit.name, "an envelope")?;
        let env = self
            .envelopes
            .iter_mut()
            .find(|e| e.id == edit.id)
            .ok_or(LinkError::UnknownSource(edit.id).to_string())?;
        let ms = |v: f32| v.clamp(0.0, MAX_STAGE_MS);
        *env = EnvelopeRecord {
            id: edit.id,
            name,
            attack: ms(edit.attack),
            decay: ms(edit.decay),
            sustain: edit.sustain.clamp(0.0, 1.0),
            release: ms(edit.release),
        };
        Ok(())
    }

    /// Add an LFO with a fresh id and return it.
    pub fn add_lfo(&mut self) -> LfoRecord {
        let id = self.fresh_id();
        let lfo = LfoRecord {
            id,
            name: format!("LFO {id}"),
            rate: NEW_RATE_HZ,
            shape: NEW_SHAPE.into(),
            phase: 0.0,
        };
        self.lfos.push(lfo.clone());
        lfo
    }

    /// Remove an LFO. Links that follow it stay in the document, leave their
    /// parameters at the hand's value, and are reported until they are
    /// unlinked, so nothing linked disappears without a word.
    pub fn remove_lfo(&mut self, id: u64) -> Result<(), String> {
        let before = self.lfos.len();
        self.lfos.retain(|l| l.id != id);
        if self.lfos.len() == before {
            return Err(LinkError::UnknownSource(id).to_string());
        }
        Ok(())
    }

    /// Replace an LFO's name, rate, shape and phase, matched by `edit.id`.
    /// Rate and phase are brought into range; the rest must already be valid.
    pub fn set_lfo(&mut self, edit: LfoRecord) -> Result<(), String> {
        if Shape::from_name(&edit.shape).is_none() {
            return Err(format!("there is no LFO shape called “{}”", edit.shape));
        }
        if !edit.rate.is_finite() || !edit.phase.is_finite() {
            return Err("an LFO's rate and phase must be numbers".into());
        }
        let name = valid_name(&edit.name, "an LFO")?;
        let lfo = self
            .lfos
            .iter_mut()
            .find(|l| l.id == edit.id)
            .ok_or(LinkError::UnknownSource(edit.id).to_string())?;
        *lfo = LfoRecord {
            id: edit.id,
            name,
            rate: edit.rate.clamp(MIN_RATE_HZ, MAX_RATE_HZ),
            shape: edit.shape,
            // Clamped rather than wrapped, so a phase control dragged to its
            // end stays there. The engine treats 1 as 0.
            phase: edit.phase.clamp(0.0, 1.0),
        };
        Ok(())
    }

    /// Make `param` follow the LFO or envelope `source` between `lo` and `hi`,
    /// replacing any link it had. The ends are clamped to 0 to 1, the
    /// parameter's own range.
    pub fn link(&mut self, param: &str, source: u64, lo: f32, hi: f32) -> Result<(), String> {
        let slot =
            index_of(param).ok_or_else(|| LinkError::UnknownParameter(param.into()).to_string())?;
        if matches!(PARAMS[slot].taper, Taper::Stepped(_)) {
            return Err(LinkError::Stepped(PARAMS[slot].id).to_string());
        }
        if !self.has_source(source) {
            return Err(LinkError::UnknownSource(source).to_string());
        }
        if !lo.is_finite() || !hi.is_finite() {
            return Err("a link's ends must be numbers".into());
        }
        self.links.insert(
            param.to_string(),
            LinkRecord {
                source,
                lo: lo.clamp(0.0, 1.0),
                hi: hi.clamp(0.0, 1.0),
            },
        );
        Ok(())
    }

    /// Stop `param` following anything. Unlinking an unlinked parameter is not
    /// an error: the result is what was asked for.
    pub fn unlink(&mut self, param: &str) {
        self.links.remove(param);
    }
}

/// A trimmed name, or why it will not do. `what` is "an LFO" or similar.
fn valid_name(name: &str, what: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("{what} needs a name"));
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("keep names to {MAX_NAME} characters or fewer"));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shard_dsp::params::index_of;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 256;

    fn lfo(id: u64, shape: &str) -> LfoRecord {
        LfoRecord {
            id,
            name: format!("LFO {id}"),
            rate: 5.0,
            shape: shape.into(),
            phase: 0.0,
        }
    }

    /// A sweep over the upper `depth` share of the range.
    fn link(source: u64, depth: f32) -> LinkRecord {
        LinkRecord {
            source,
            lo: 1.0 - depth,
            hi: 1.0,
        }
    }

    #[test]
    fn a_built_link_moves_its_parameter() {
        let doc = Modulation {
            lfos: vec![lfo(1, "square")],
            links: Links::from([("grain.size".into(), link(1, 0.5))]),
            ..Default::default()
        };
        let (mut set, refused) = doc.build();
        assert!(refused.is_empty(), "{refused:?}");

        let slot = index_of("grain.size").unwrap();
        let mut seen = Vec::new();
        for _ in 0..200 {
            set.advance(BLOCK, SR);
            seen.push(set.apply(slot, 100.0));
        }
        assert!(seen.iter().any(|v| *v > 100.0), "never rose: {seen:?}");
        assert!(seen.iter().any(|v| *v < 100.0), "never fell: {seen:?}");
    }

    #[test]
    fn what_cannot_be_used_is_reported_and_left_at_the_base() {
        let doc = Modulation {
            lfos: vec![lfo(1, "sine"), lfo(1, "saw"), lfo(2, "wobble")],
            links: Links::from([
                ("grain.size".into(), link(9, 1.0)),
                ("grain.position".into(), link(2, 1.0)),
                ("grain.window".into(), link(1, 1.0)),
                ("grain.nonsense".into(), link(1, 1.0)),
            ]),
            ..Default::default()
        };
        let (mut set, refused) = doc.build();

        let ids: Vec<&str> = refused.iter().map(|r| r.id.as_str()).collect();
        for expected in [
            "lfo:1",
            "lfo:2",
            "grain.size",
            "grain.position",
            "grain.window",
            "grain.nonsense",
        ] {
            assert!(
                ids.contains(&expected),
                "{expected} not reported: {refused:?}"
            );
        }
        assert_eq!(refused.len(), 6, "{refused:?}");

        // A link to a removed LFO leaves its parameter exactly at the base.
        for id in ["grain.size", "grain.position"] {
            let slot = index_of(id).unwrap();
            for _ in 0..50 {
                set.advance(BLOCK, SR);
                assert_eq!(set.apply(slot, 123.4), 123.4, "{id} moved");
            }
        }
    }

    #[test]
    fn shapes_are_spelled_by_name_in_the_file() {
        let doc = Modulation {
            lfos: vec![lfo(3, "smooth-random"), lfo(4, "elastic-in-out")],
            links: Links::from([("ring.freq".into(), link(3, -0.25))]),
            ..Default::default()
        };
        let text = serde_json::to_string(&doc).unwrap();
        assert!(text.contains(r#""shape":"smooth-random""#), "{text}");
        let back: Modulation = serde_json::from_str(&text).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn an_empty_document_writes_nothing() {
        assert_eq!(serde_json::to_string(&Modulation::default()).unwrap(), "{}");
    }

    #[test]
    fn an_lfo_id_is_never_handed_out_twice() {
        let mut doc = Modulation::default();
        assert_eq!(doc.add_lfo().id, 1);
        assert_eq!(doc.add_lfo().id, 2);
        doc.remove_lfo(2).unwrap();
        assert_eq!(doc.add_lfo().id, 3, "reused the removed id");

        // The counter is saved, so a reload does not forget it either.
        doc.remove_lfo(3).unwrap();
        let mut back: Modulation =
            serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert_eq!(back.add_lfo().id, 4, "reused an id after a reload");

        // A hand-edited patch can carry ids past its counter.
        let mut edited = Modulation {
            lfos: vec![lfo(7, "sine")],
            next_modulator_id: 2,
            ..Default::default()
        };
        assert_eq!(edited.add_lfo().id, 8);
    }

    #[test]
    fn lfos_and_envelopes_never_share_an_id() {
        let mut doc = Modulation::default();
        assert_eq!(doc.add_lfo().id, 1);
        assert_eq!(doc.add_envelope().id, 2);
        assert_eq!(doc.add_lfo().id, 3);
        // A hand-edited envelope past the counter lifts the LFOs' ids too.
        doc.envelopes.push(EnvelopeRecord {
            id: 9,
            ..new_envelope(9)
        });
        assert_eq!(doc.add_lfo().id, 10);
    }

    #[test]
    fn an_envelope_sharing_an_id_is_refused_not_dropped() {
        let mut doc = Modulation {
            lfos: vec![lfo(1, "sine")],
            envelopes: vec![new_envelope(1), new_envelope(2), new_envelope(2)],
            ..Default::default()
        };
        let (_, refused) = doc.build();
        let ids: Vec<_> = refused.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["envelope:1", "envelope:2"]);
        assert_eq!(doc.envelopes.len(), 3, "the document keeps them");
        // A link to the shared id follows the LFO, which is the one that built.
        doc.link("grain.size", 1, 0.0, 1.0).unwrap();
        assert!(doc.build().1.iter().all(|r| r.id != "grain.size"));
    }

    #[test]
    fn removing_an_envelope_keeps_its_links_and_reports_them() {
        let mut doc = Modulation::default();
        let id = doc.add_envelope().id;
        doc.link("filter.cutoff", id, 0.1, 0.9).unwrap();
        assert!(doc.build().1.is_empty());
        doc.remove_envelope(id).unwrap();
        assert!(doc.links.contains_key("filter.cutoff"));
        let (_, refused) = doc.build();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].id, "filter.cutoff");
        assert!(doc.remove_envelope(id).is_err());
    }

    #[test]
    fn an_envelope_edit_brings_its_stages_into_range() {
        let mut doc = Modulation::default();
        let id = doc.add_envelope().id;
        doc.set_envelope(EnvelopeRecord {
            id,
            name: "  filter  ".into(),
            attack: -5.0,
            decay: 1e9,
            sustain: 2.0,
            release: 40.0,
        })
        .unwrap();
        let e = &doc.envelopes[0];
        assert_eq!(e.name, "filter");
        assert_eq!((e.attack, e.decay, e.sustain), (0.0, MAX_STAGE_MS, 1.0));
        let before = doc.clone();
        assert!(doc
            .set_envelope(EnvelopeRecord {
                attack: f32::NAN,
                ..doc.envelopes[0].clone()
            })
            .is_err());
        assert_eq!(doc, before, "a refused edit changed the document");
    }

    #[test]
    fn a_link_written_as_lfo_still_reads() {
        let doc: Modulation = serde_json::from_str(
            r#"{"lfos":[],"links":{"grain.size":{"lfo":3,"lo":0.0,"hi":1.0}},"next_lfo_id":4}"#,
        )
        .unwrap();
        assert_eq!(doc.links["grain.size"].source, 3);
        assert_eq!(doc.next_modulator_id, 4);
    }

    #[test]
    fn a_new_lfo_wanders_slowly_and_is_usable() {
        let mut doc = Modulation::default();
        let new = doc.add_lfo();
        assert_eq!(new.shape, "smooth-random");
        assert_eq!(new.name, "LFO 1");
        let (_, refused) = doc.build();
        assert!(refused.is_empty(), "{refused:?}");
    }

    #[test]
    fn removing_an_lfo_keeps_its_links_and_reports_them() {
        let mut doc = Modulation::default();
        let id = doc.add_lfo().id;
        doc.link("grain.size", id, 0.5, 1.0).unwrap();
        doc.remove_lfo(id).unwrap();

        assert_eq!(doc.links.get("grain.size"), Some(&link(id, 0.5)));
        let (mut set, refused) = doc.build();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].id, "grain.size");
        let slot = index_of("grain.size").unwrap();
        set.advance(BLOCK, SR);
        assert_eq!(set.apply(slot, 100.0), 100.0);

        doc.unlink("grain.size");
        assert!(doc.links.is_empty());
        assert!(doc.build().1.is_empty());
        assert!(doc.remove_lfo(id).is_err(), "removed the same LFO twice");
    }

    #[test]
    fn a_refused_edit_changes_nothing() {
        let mut doc = Modulation::default();
        let id = doc.add_lfo().id;
        doc.link("grain.size", id, 0.0, 0.5).unwrap();
        let before = doc.clone();

        let good = doc.lfos[0].clone();
        for bad in [
            LfoRecord {
                shape: "wobble".into(),
                ..good.clone()
            },
            LfoRecord {
                name: "   ".into(),
                ..good.clone()
            },
            LfoRecord {
                name: "x".repeat(MAX_NAME + 1),
                ..good.clone()
            },
            LfoRecord {
                rate: f32::NAN,
                ..good.clone()
            },
            LfoRecord {
                id: 99,
                ..good.clone()
            },
        ] {
            assert!(doc.set_lfo(bad.clone()).is_err(), "accepted {bad:?}");
        }
        assert!(
            doc.link("grain.window", id, 0.0, 0.5).is_err(),
            "linked a stepped parameter"
        );
        assert!(doc.link("grain.nonsense", id, 0.0, 0.5).is_err());
        assert!(doc.link("grain.position", 99, 0.0, 0.5).is_err());
        assert!(doc.link("grain.position", id, 0.0, f32::INFINITY).is_err());
        assert!(doc.remove_lfo(99).is_err());

        assert_eq!(doc, before);
    }

    #[test]
    fn an_edit_brings_rate_phase_and_ends_into_range() {
        let mut doc = Modulation::default();
        let id = doc.add_lfo().id;
        doc.set_lfo(LfoRecord {
            id,
            name: "  wander  ".into(),
            rate: 1_000.0,
            shape: "sine".into(),
            phase: 1.5,
        })
        .unwrap();
        assert_eq!(
            doc.lfos[0],
            LfoRecord {
                id,
                name: "wander".into(),
                rate: MAX_RATE_HZ,
                shape: "sine".into(),
                phase: 1.0,
            }
        );

        doc.link("grain.position", id, -3.0, 7.0).unwrap();
        assert_eq!(
            doc.links["grain.position"],
            LinkRecord {
                source: id,
                lo: 0.0,
                hi: 1.0
            }
        );
        // Linking again replaces, rather than adding a second source.
        doc.link("grain.position", id, 0.75, 1.0).unwrap();
        assert_eq!(doc.links.len(), 1);
        assert_eq!(doc.links["grain.position"], link(id, 0.25));
    }

    #[test]
    fn changing_an_lfo_does_not_restart_it() {
        let mut doc = Modulation::default();
        let id = doc.add_lfo().id;
        doc.set_lfo(LfoRecord {
            rate: 1.0,
            shape: "sine".into(),
            ..doc.lfos[0].clone()
        })
        .unwrap();
        let (mut old, _) = doc.build();
        for _ in 0..40 {
            old.advance(BLOCK, SR);
        }

        // A rate and a shape change both rebuild the set.
        doc.set_lfo(LfoRecord {
            rate: 2.0,
            shape: "triangle".into(),
            ..doc.lfos[0].clone()
        })
        .unwrap();
        let (mut carried, _) = doc.build();
        carried.inherit(&old);
        let (mut restarted, _) = doc.build();
        carried.advance(BLOCK, SR);
        restarted.advance(BLOCK, SR);

        let (c, r) = (
            carried.lfo_value(id).unwrap(),
            restarted.lfo_value(id).unwrap(),
        );
        assert!(
            (c - r).abs() > 0.1,
            "the edit restarted the LFO: {c} vs {r}"
        );
    }
}
