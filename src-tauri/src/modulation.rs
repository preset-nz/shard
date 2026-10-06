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
use shard_dsp::{EnvSpec, LfoSpec, ModSet, Shape};

/// Long enough for anything typed into the tree, short enough to stay a label.
const MAX_NAME: usize = 60;

/// A new LFO wanders slowly: the movement drift used to give.
pub(crate) const NEW_RATE_HZ: f32 = 0.1;
pub(crate) const NEW_SHAPE: &str = "smooth-random";

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
pub(crate) fn new_envelope(id: u64) -> EnvelopeRecord {
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
}

/// A trimmed name, or why it will not do. `what` is "an LFO" or similar.
pub(crate) fn valid_name(name: &str, what: &str) -> Result<String, String> {
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
            links: Links::from([("fx.2.ring.freq".into(), link(3, -0.25))]),
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
        doc.links.insert("grain.size".into(), link(1, 1.0));
        assert!(doc.build().1.iter().all(|r| r.id != "grain.size"));
    }

    #[test]
    fn changing_an_lfo_does_not_restart_it() {
        let id = 1;
        let mut doc = Modulation {
            lfos: vec![LfoRecord {
                rate: 1.0,
                ..lfo(id, "sine")
            }],
            ..Default::default()
        };
        let (mut old, _) = doc.build();
        for _ in 0..40 {
            old.advance(BLOCK, SR);
        }

        // A rate and a shape change both rebuild the set.
        doc.lfos[0].rate = 2.0;
        doc.lfos[0].shape = "triangle".into();
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
