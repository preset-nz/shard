//! Undo and redo (`design/undo.md`).
//!
//! A snapshot is everything an edit can change that Shard owns: the patch's
//! values, the arrangement's, the tracker, the LFOs and links, and the node
//! presets. An entry holds the snapshot from *before* an edit, with a name for
//! it. Undo swaps the current state for the newest entry's and keeps the state
//! it replaced for redo; a new edit clears redo.
//!
//! **A drag is one step** (`native-apps.md` rule 3). An edit that carries the
//! same key as the one before it, within `GESTURE`, is the same gesture, so it
//! records nothing: the entry already holds the state from before it began.
//! The key is the control being moved (`param:chorus.rate`); a structural edit,
//! such as adding an effect, has no key and is always its own step.
//!
//! **Only what a person does is recorded.** The engine writes nothing here:
//! LFOs move the heard value and never the hand's, and a MIDI knob writes the
//! bank directly without passing through a command (not undoable yet). Materials
//! and the files behind them are outside a snapshot.

use std::time::{Duration, Instant};

use crate::modulation::Modulation;
use crate::presets::Presets;
use crate::tracker::Tracker;

/// How long a pause ends a gesture. Long enough that a slow drag stays one
/// step, short enough that two separate tweaks of one control are two.
pub const GESTURE: Duration = Duration::from_millis(700);

/// The most steps kept (Oblique keeps fifty; a snapshot here is a few
/// kilobytes).
pub const DEPTH: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// The patch bank's values, by table index.
    pub patch: Vec<f32>,
    /// The arrangement bank's values, by table index.
    pub arrangement: Vec<f32>,
    pub tracker: Tracker,
    pub modulation: Modulation,
    pub presets: Presets,
}

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    state: Snapshot,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    last: Option<(String, Instant)>,
}

/// What the UI needs to know: the name of what Undo and Redo would do.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct HistoryView {
    pub undo: Option<String>,
    pub redo: Option<String>,
}

impl History {
    /// An edit is about to happen. `capture` is only called if it starts a new
    /// step, so a drag does not copy the state sixty times a second.
    ///
    /// Answers whether it began a step. When the edit is then refused, `cancel`
    /// takes that step back out.
    pub fn record(
        &mut self,
        key: Option<&str>,
        label: &str,
        now: Instant,
        capture: impl FnOnce() -> Snapshot,
    ) -> bool {
        if let (Some(key), Some((last, at))) = (key, &self.last) {
            if last == key && now.duration_since(*at) < GESTURE {
                self.last = Some((key.to_string(), now));
                return false;
            }
        }
        self.redo.clear();
        self.undo.push(Entry {
            label: label.to_string(),
            state: capture(),
        });
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
        self.last = key.map(|k| (k.to_string(), now));
        true
    }

    /// The edit that began the newest step was refused and changed nothing, so
    /// there is nothing to undo.
    pub fn cancel(&mut self) {
        self.undo.pop();
        self.last = None;
    }

    /// Step back. `current` is the state now, kept for redo. Answers with the
    /// state to restore and the name of what was undone.
    pub fn undo(&mut self, current: Snapshot) -> Option<(Snapshot, String)> {
        let entry = self.undo.pop()?;
        self.redo.push(Entry {
            label: entry.label.clone(),
            state: current,
        });
        self.last = None;
        Some((entry.state, entry.label))
    }

    /// Step forward again.
    pub fn redo(&mut self, current: Snapshot) -> Option<(Snapshot, String)> {
        let entry = self.redo.pop()?;
        self.undo.push(Entry {
            label: entry.label.clone(),
            state: current,
        });
        self.last = None;
        Some((entry.state, entry.label))
    }

    /// A different document opened: nothing before it can be undone.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.last = None;
    }

    pub fn view(&self) -> HistoryView {
        HistoryView {
            undo: self.undo.last().map(|e| e.label.clone()),
            redo: self.redo.last().map(|e| e.label.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot that is only a number, carried in the first value.
    fn at(n: f32) -> Snapshot {
        Snapshot {
            patch: vec![n],
            arrangement: Vec::new(),
            tracker: Tracker::default(),
            modulation: Modulation::default(),
            presets: Presets::default(),
        }
    }

    fn value(s: &Snapshot) -> f32 {
        s.patch[0]
    }

    #[test]
    fn undo_gives_back_the_state_before_the_edit_and_redo_the_one_after() {
        let mut h = History::default();
        let t = Instant::now();
        h.record(None, "Add Delay", t, || at(0.0));
        // The edit made the state 1. Undo hands back 0 and keeps 1.
        let (back, label) = h.undo(at(1.0)).unwrap();
        assert_eq!((value(&back), label.as_str()), (0.0, "Add Delay"));
        let (again, label) = h.redo(at(0.0)).unwrap();
        assert_eq!((value(&again), label.as_str()), (1.0, "Add Delay"));
    }

    #[test]
    fn nothing_to_undo_or_redo_answers_none() {
        let mut h = History::default();
        assert!(h.undo(at(0.0)).is_none());
        assert!(h.redo(at(0.0)).is_none());
        assert_eq!(h.view(), HistoryView::default());
    }

    #[test]
    fn a_drag_is_one_step_and_remembers_where_it_began() {
        let mut h = History::default();
        let t = Instant::now();
        let mut captures = 0;
        for i in 0..50 {
            h.record(
                Some("param:chorus.rate"),
                "Change Rate",
                t + Duration::from_millis(i * 10),
                || {
                    captures += 1;
                    at(i as f32)
                },
            );
        }
        assert_eq!(captures, 1, "only the first move copies the state");
        let (back, _) = h.undo(at(49.0)).unwrap();
        assert_eq!(value(&back), 0.0, "undo returns to before the drag");
        assert!(h.undo(at(0.0)).is_none(), "and the drag was one step");
    }

    #[test]
    fn a_pause_ends_the_gesture() {
        let mut h = History::default();
        let t = Instant::now();
        h.record(Some("param:a"), "Change A", t, || at(0.0));
        h.record(Some("param:a"), "Change A", t + GESTURE * 2, || at(5.0));
        assert_eq!(value(&h.undo(at(9.0)).unwrap().0), 5.0);
        assert_eq!(value(&h.undo(at(5.0)).unwrap().0), 0.0);
    }

    #[test]
    fn a_different_control_is_a_different_step() {
        let mut h = History::default();
        let t = Instant::now();
        h.record(Some("param:a"), "Change A", t, || at(0.0));
        h.record(
            Some("param:b"),
            "Change B",
            t + Duration::from_millis(5),
            || at(1.0),
        );
        assert_eq!(h.view().undo.as_deref(), Some("Change B"));
        h.undo(at(2.0));
        assert_eq!(h.view().undo.as_deref(), Some("Change A"));
    }

    #[test]
    fn structural_edits_are_never_merged() {
        let mut h = History::default();
        let t = Instant::now();
        h.record(None, "Add Delay", t, || at(0.0));
        h.record(None, "Add Echo", t, || at(1.0));
        assert_eq!(h.view().undo.as_deref(), Some("Add Echo"));
        h.undo(at(2.0));
        assert_eq!(h.view().undo.as_deref(), Some("Add Delay"));
    }

    #[test]
    fn a_gesture_does_not_continue_across_an_undo() {
        let mut h = History::default();
        let t = Instant::now();
        h.record(Some("param:a"), "Change A", t, || at(0.0));
        h.undo(at(1.0));
        // Moving the same control again right after is a new step.
        h.record(
            Some("param:a"),
            "Change A",
            t + Duration::from_millis(10),
            || at(0.0),
        );
        assert_eq!(h.view().undo.as_deref(), Some("Change A"));
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut h = History::default();
        let t = Instant::now();
        h.record(None, "Add Delay", t, || at(0.0));
        h.undo(at(1.0));
        assert!(h.view().redo.is_some());
        h.record(None, "Add Echo", t, || at(0.0));
        assert!(h.view().redo.is_none());
    }

    #[test]
    fn the_stack_is_bounded_and_forgets_the_oldest() {
        let mut h = History::default();
        let t = Instant::now();
        for i in 0..DEPTH + 20 {
            h.record(None, "Edit", t, || at(i as f32));
        }
        let mut n = 0;
        let mut oldest = f32::MAX;
        while let Some((s, _)) = h.undo(at(0.0)) {
            n += 1;
            oldest = oldest.min(value(&s));
        }
        assert_eq!(n, DEPTH);
        assert_eq!(oldest, 20.0, "the first twenty were forgotten");
    }

    #[test]
    fn a_refused_edit_leaves_no_step() {
        let mut h = History::default();
        let began = h.record(None, "Link", Instant::now(), || at(0.0));
        assert!(began);
        h.cancel();
        assert_eq!(h.view(), HistoryView::default());
    }

    #[test]
    fn record_says_whether_it_began_a_step() {
        let mut h = History::default();
        let t = Instant::now();
        assert!(h.record(Some("k"), "Edit", t, || at(0.0)));
        assert!(!h.record(Some("k"), "Edit", t + Duration::from_millis(1), || at(1.0)));
    }

    #[test]
    fn opening_a_document_forgets_everything() {
        let mut h = History::default();
        h.record(None, "Add Delay", Instant::now(), || at(0.0));
        h.clear();
        assert_eq!(h.view(), HistoryView::default());
    }
}
