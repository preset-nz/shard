//! A sketch: a document described flat, the way a script or a model writes
//! one, made into a `.shard` by the session's own verbs. `just shard-write`
//! runs it for the `pattern-to-shard` skill, so the skill never writes
//! rhizome's tree itself and cannot drift from the object model.
//!
//! ```json
//! {
//!   "tracker": { "tempo": 87, "swing": 0, "tracks": [ ... ] },
//!   "patch": { "fm.on": 1, "patch.length": 2 },
//!   "effects": [ { "kind": "drive", "amount": 14, "mix": 0.4 } ],
//!   "arrangement": { "arrangement.track.gain": 0.8 }
//! }
//! ```
//!
//! `tracker` is `Tracker`'s own shape. `patch` and `arrangement` are rows by
//! id; an id left out keeps its default. `effects` is the patch's chain in
//! order, each with its kind and its rows by name. Anything the session
//! refuses fails the write, by name.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use shard_dsp::fx;

use crate::session::Session;
use crate::tracker::Tracker;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sketch {
    pub tracker: Option<Tracker>,
    pub patch: BTreeMap<String, f32>,
    pub effects: Vec<Effect>,
    pub arrangement: BTreeMap<String, f32>,
}

#[derive(Debug, Deserialize)]
pub struct Effect {
    pub kind: String,
    #[serde(flatten)]
    pub rows: BTreeMap<String, f32>,
}

/// Writes `sketch` into `session`, which should be fresh.
pub fn apply(sketch: &Sketch, session: &Session) -> Result<(), String> {
    if let Some(tracker) = &sketch.tracker {
        session.set_tracker(tracker.clone())?;
    }
    for (id, &v) in sketch.patch.iter().chain(&sketch.arrangement) {
        if fx::parse_row(id).is_some() || fx::is_order_row(id) {
            return Err(format!("{id}: effects go in \"effects\", by kind"));
        }
        session.set_param(id, v).map_err(|e| format!("{id}: {e}"))?;
    }
    for effect in &sketch.effects {
        let n = session.fx_add("patch", &effect.kind)?;
        for (row, &v) in &effect.rows {
            let id = fx::row_id(n, &format!("{}.{row}", effect.kind));
            session
                .set_param(&id, v)
                .map_err(|e| format!("{id}: {e}"))?;
        }
    }
    Ok(())
}

/// `just shard-write <sketch.json> <out.shard>`.
#[test]
fn write_a_sketch() {
    let (Ok(input), Ok(output)) = (std::env::var("SHARD_SKETCH"), std::env::var("SHARD_OUT"))
    else {
        return;
    };
    let text = std::fs::read_to_string(&input).expect("the sketch reads");
    let sketch: Sketch = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{input}: {e}"));
    let r = crate::session_tests::rig();
    apply(&sketch, &r.session).unwrap_or_else(|e| panic!("{input}: {e}"));
    r.session.save(Path::new(&output)).expect("the file writes");
    println!("wrote {output}");
}

#[test]
fn a_sketch_writes_its_tracker_values_and_chain() {
    let sketch: Sketch = serde_json::from_str(
        r#"{
            "tracker": { "tempo": 87, "tracks": [ { "on": true, "length": 16 } ] },
            "patch": { "fm.on": 1, "filter.cutoff": 420 },
            "effects": [ { "kind": "drive", "amount": 14 }, { "kind": "drive", "mix": 0.5 } ],
            "arrangement": { "arrangement.track.gain": 0.8 }
        }"#,
    )
    .unwrap();
    let r = crate::session_tests::rig();
    apply(&sketch, &r.session).unwrap();
    assert_eq!(r.session.tracker().tempo, 87.0);
    assert_eq!(r.bank.get_by_id("filter.cutoff"), Some(420.0));
    assert_eq!(r.arrangement.get_by_id("arrangement.track.gain"), Some(0.8));
    let order = fx::order_of(&r.bank);
    assert_eq!(order.len(), 2, "two drives, in order");
    let (first, second) = (order.as_slice()[0] as usize, order.as_slice()[1] as usize);
    assert_eq!(
        r.bank.get_by_id(&fx::row_id(first, "drive.amount")),
        Some(14.0)
    );
    assert_eq!(
        r.bank.get_by_id(&fx::row_id(second, "drive.mix")),
        Some(0.5)
    );
}

#[test]
fn a_sketch_with_a_row_that_does_not_exist_fails_by_name() {
    let r = crate::session_tests::rig();
    let bad: Sketch = serde_json::from_str(r#"{ "patch": { "fm.nonsense": 1 } }"#).unwrap();
    let e = apply(&bad, &r.session).unwrap_err();
    assert!(e.contains("fm.nonsense"), "{e}");
    let raw: Sketch = serde_json::from_str(r#"{ "patch": { "fx.0.drive.amount": 1 } }"#).unwrap();
    assert!(apply(&raw, &r.session).unwrap_err().contains("effects"));
    let unknown_kind: Sketch =
        serde_json::from_str(r#"{ "effects": [ { "kind": "wobble" } ] }"#).unwrap();
    assert!(apply(&unknown_kind, &r.session).is_err());
}
