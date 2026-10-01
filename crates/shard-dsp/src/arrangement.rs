//! The arrangement's table: the level above the patch.
//!
//! A patch keeps its own effects and output. The arrangement puts a second
//! chain of the same kinds over it, with a fader for the patch between them
//! and the limiter after everything (`design/arrangement-layer.md`, Georg,
//! 2026-09-26: *"all the other processes and master controls are a layer on
//! top. they control the overall sound, not the patch."*).
//!
//! **These values are not patch parameters.** Loading a different patch keeps
//! them, so they are not in `params::PARAMS`. The host holds them in a
//! `ParamBank::for_table(arrangement::params())` and hands them to the engine
//! once a block with `Engine::set_arrangement`.
//!
//! **Borrowed rows** are the patch's own rows under `arrangement.`, with the
//! same range, taper and smoothing, so the two chains cannot drift apart. The
//! crusher's envelope is not borrowed: it follows a pass through one patch,
//! and the arrangement has no pass (Georg: *"no envelope"*). **Own rows** are
//! the patch's fader and the limiter.
//!
//! Ids are a wire format, as in the patch table.

use std::sync::OnceLock;

use crate::params::{index_of, ParamDef, Taper, Unit, PARAMS};

/// Every arrangement id starts with this.
pub const PREFIX: &str = "arrangement.";

enum Row {
    /// A patch row under `PREFIX`. The string is the arrangement id.
    Borrowed(&'static str),
    Own(ParamDef),
    /// The palette's rows, expanded where this sits.
    Fx,
}

/// In panel order: the patch's fader, the process lane, then the master.
const ROWS: &[Row] = &[
    // The patch's level in the arrangement, a mixer's channel fader. Not the
    // patch's own Output, so one patch can sit at different levels in
    // different arrangements. One today; one per track with roadmap row 10.
    Row::Own(ParamDef {
        id: "arrangement.track.gain",
        name: "Gain",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    }),
    // The palette: every instance's rows and the order rows, borrowed from
    // the patch's by `fx::rows`. Which effects run, and in what order, is the
    // arrangement's own: the patch's order is not heard here.
    Row::Fx,
    Row::Borrowed("arrangement.filter.on"),
    Row::Borrowed("arrangement.filter.mix"),
    Row::Borrowed("arrangement.filter.cutoff"),
    Row::Borrowed("arrangement.filter.resonance"),
    Row::Borrowed("arrangement.filter.type"),
    Row::Borrowed("arrangement.amp.gain"),
    // The limiter, last of all (Georg, 2026-09-14), and since 2026-09-26
    // after the arrangement rather than inside the patch. It runs in sound
    // scaping too, where the arrangement's chain is not heard. On by default;
    // off leaves the hard clip as the only guard, for whoever wants that edge.
    Row::Own(ParamDef {
        id: "arrangement.amp.limit",
        name: "Limit",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    }),
    Row::Own(ParamDef {
        id: "arrangement.amp.ceiling",
        name: "Ceiling",
        min: 0.5,
        max: 1.0,
        default: 0.95,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 0.0,
    }),
];

/// The patch row an arrangement id borrows.
fn source_of(id: &str) -> &'static ParamDef {
    let bare = id
        .strip_prefix(PREFIX)
        .unwrap_or_else(|| panic!("{id} does not start with {PREFIX}"));
    let i = index_of(bare).unwrap_or_else(|| panic!("{id} borrows {bare}, which the patch lacks"));
    &PARAMS[i]
}

/// The arrangement's table, built once. Built rather than written out so a
/// borrowed row always matches the patch row it came from.
pub fn params() -> &'static [ParamDef] {
    static TABLE: OnceLock<Vec<ParamDef>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = Vec::new();
        for row in ROWS {
            match row {
                Row::Borrowed(id) => out.push(ParamDef {
                    id,
                    ..*source_of(id)
                }),
                Row::Own(def) => out.push(*def),
                Row::Fx => out.extend(
                    crate::fx::rows()
                        .iter()
                        .filter(|def| !def.id.contains(".env."))
                        .map(|def| ParamDef {
                            id: Box::leak(format!("{PREFIX}{}", def.id).into_boxed_str()),
                            ..*def
                        }),
                ),
            }
        }
        out
    })
}

pub fn index_of_arrangement(id: &str) -> Option<usize> {
    params().iter().position(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{leads_with_its_level, repeats_no_node_name};

    #[test]
    fn every_id_is_unique_and_prefixed() {
        let mut seen = std::collections::HashSet::new();
        for p in params() {
            assert!(p.id.starts_with(PREFIX), "{}", p.id);
            assert!(seen.insert(p.id), "duplicate id: {}", p.id);
            assert!(p.default >= p.min && p.default <= p.max, "{}", p.id);
        }
    }

    #[test]
    fn a_borrowed_row_matches_its_patch_row_in_all_but_the_id() {
        for row in ROWS {
            let Row::Borrowed(id) = row else { continue };
            let ours = &params()[index_of_arrangement(id).unwrap()];
            let theirs = source_of(id);
            assert_eq!(ours.name, theirs.name, "{id}");
            assert_eq!(ours.min, theirs.min, "{id}");
            assert_eq!(ours.max, theirs.max, "{id}");
            assert_eq!(ours.default, theirs.default, "{id}");
            assert_eq!(ours.taper, theirs.taper, "{id}");
            assert_eq!(ours.unit, theirs.unit, "{id}");
            assert_eq!(ours.smooth_ms, theirs.smooth_ms, "{id}");
        }
    }

    #[test]
    fn every_palette_row_is_mirrored_in_the_same_order() {
        let ours: Vec<_> = params()
            .iter()
            .filter(|p| p.id.starts_with("arrangement.fx."))
            .collect();
        // The crusher's envelope follows a pass, and the arrangement has none.
        let theirs: Vec<_> = crate::fx::rows()
            .iter()
            .filter(|d| !d.id.contains(".env."))
            .collect();
        assert_eq!(ours.len(), theirs.len());
        for (a, b) in ours.iter().zip(theirs.iter()) {
            assert_eq!(a.id, format!("{PREFIX}{}", b.id));
            assert_eq!(
                (a.name, a.min, a.max, a.default),
                (b.name, b.min, b.max, b.default)
            );
            assert_eq!(
                (a.taper, a.unit, a.smooth_ms),
                (b.taper, b.unit, b.smooth_ms)
            );
        }
    }

    #[test]
    fn the_crusher_has_no_envelope_here() {
        assert!(params().iter().all(|p| !p.id.contains(".env.")));
    }

    #[test]
    fn the_limiter_is_the_arrangements_not_the_patchs() {
        assert!(index_of_arrangement("arrangement.amp.limit").is_some());
        assert!(index_of("amp.limit").is_none());
        assert!(index_of("amp.ceiling").is_none());
    }

    #[test]
    fn follows_the_node_pattern() {
        leads_with_its_level(params(), PREFIX);
        repeats_no_node_name(params(), PREFIX);
    }
}
