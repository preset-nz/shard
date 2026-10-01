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
    Row::Borrowed("arrangement.drive.on"),
    Row::Borrowed("arrangement.drive.mix"),
    Row::Borrowed("arrangement.drive.amount"),
    Row::Borrowed("arrangement.drive.tone"),
    Row::Borrowed("arrangement.drive.type"),
    Row::Borrowed("arrangement.crush.on"),
    Row::Borrowed("arrangement.crush.mix"),
    Row::Borrowed("arrangement.crush.bits"),
    Row::Borrowed("arrangement.crush.rate"),
    Row::Borrowed("arrangement.ring.on"),
    Row::Borrowed("arrangement.ring.mix"),
    Row::Borrowed("arrangement.ring.freq"),
    Row::Borrowed("arrangement.overtone.on"),
    Row::Borrowed("arrangement.overtone.mix"),
    Row::Borrowed("arrangement.overtone.sub"),
    Row::Borrowed("arrangement.overtone.octave"),
    Row::Borrowed("arrangement.overtone.fifth"),
    Row::Borrowed("arrangement.overtone.tone"),
    Row::Borrowed("arrangement.chorus.on"),
    Row::Borrowed("arrangement.chorus.mix"),
    Row::Borrowed("arrangement.chorus.type"),
    Row::Borrowed("arrangement.chorus.rate"),
    Row::Borrowed("arrangement.chorus.depth"),
    Row::Borrowed("arrangement.chorus.voices"),
    Row::Borrowed("arrangement.chorus.spread"),
    Row::Borrowed("arrangement.chorus.lowcut"),
    Row::Borrowed("arrangement.chorus.eq"),
    Row::Borrowed("arrangement.flanger.on"),
    Row::Borrowed("arrangement.flanger.mix"),
    Row::Borrowed("arrangement.flanger.manual"),
    Row::Borrowed("arrangement.flanger.rate"),
    Row::Borrowed("arrangement.flanger.depth"),
    Row::Borrowed("arrangement.flanger.feedback"),
    Row::Borrowed("arrangement.wear.on"),
    Row::Borrowed("arrangement.wear.mix"),
    Row::Borrowed("arrangement.wear.wow"),
    Row::Borrowed("arrangement.wear.flutter"),
    Row::Borrowed("arrangement.wear.unstable"),
    Row::Borrowed("arrangement.wear.dropouts"),
    Row::Borrowed("arrangement.wear.dull"),
    Row::Borrowed("arrangement.delay.on"),
    Row::Borrowed("arrangement.delay.mix"),
    Row::Borrowed("arrangement.delay.time"),
    Row::Borrowed("arrangement.delay.feedback"),
    Row::Borrowed("arrangement.delay.tone"),
    Row::Borrowed("arrangement.delay.pingpong"),
    Row::Borrowed("arrangement.echo.on"),
    Row::Borrowed("arrangement.echo.mix"),
    Row::Borrowed("arrangement.echo.time"),
    Row::Borrowed("arrangement.echo.feedback"),
    Row::Borrowed("arrangement.echo.tone"),
    Row::Borrowed("arrangement.echo.wobble"),
    Row::Borrowed("arrangement.echo.grit"),
    Row::Borrowed("arrangement.rise.on"),
    Row::Borrowed("arrangement.rise.mix"),
    Row::Borrowed("arrangement.rise.time"),
    Row::Borrowed("arrangement.rise.feedback"),
    Row::Borrowed("arrangement.rise.shift"),
    Row::Borrowed("arrangement.rise.wobble"),
    Row::Borrowed("arrangement.rise.tone"),
    Row::Borrowed("arrangement.reverb.on"),
    Row::Borrowed("arrangement.reverb.mix"),
    Row::Borrowed("arrangement.reverb.type"),
    Row::Borrowed("arrangement.reverb.decay"),
    Row::Borrowed("arrangement.reverb.size"),
    Row::Borrowed("arrangement.reverb.tone"),
    Row::Borrowed("arrangement.reverb.predelay"),
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
        ROWS.iter()
            .map(|row| match row {
                Row::Borrowed(id) => ParamDef {
                    id,
                    ..*source_of(id)
                },
                Row::Own(def) => *def,
            })
            .collect()
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
