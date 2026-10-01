//! Maps: which control reaches which target, and pickup.
//!
//! Story 2 of `controller-mapping.md`. A knob is a hand: when it catches the
//! value it writes the base value, exactly as dragging the slider does, and
//! whatever LFO is linked keeps moving on top. Maps are app-wide and named,
//! and a patch names the one it wants.
//!
//! Nothing here touches a device or a lock. `Map` is data the registry saves;
//! `Pickup` is the runtime state the MIDI thread keeps, never saved.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shard_dsp::params::{ParamDef, Taper};

const MAX_NAME: usize = 40;

/// What a control does when it moves. Only parameters today; LFO fields,
/// link depths and commands are story 4 and story 3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Param(String),
}

impl Target {
    pub fn describe(&self) -> String {
        match self {
            Target::Param(id) => id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Map {
    pub id: u64,
    pub name: String,
    /// By control id. One control reaches one target, and `bind` keeps one
    /// target on one control, because two knobs with pickup would fight.
    pub mappings: BTreeMap<u64, Target>,
}

impl Map {
    /// The control that reaches `target`, if any.
    pub fn control_for(&self, target: &Target) -> Option<u64> {
        self.mappings
            .iter()
            .find(|(_, t)| *t == target)
            .map(|(c, _)| *c)
    }

    /// Point `control` at `target`, taking it off any control that had it.
    /// Returns the control it was taken from.
    pub fn bind(&mut self, control: u64, target: Target) -> Option<u64> {
        let previous = self.control_for(&target).filter(|c| *c != control);
        if let Some(p) = previous {
            self.mappings.remove(&p);
        }
        self.mappings.insert(control, target);
        previous
    }

    /// Take `target` off whichever control had it. True if one did.
    pub fn unbind(&mut self, target: &Target) -> bool {
        match self.control_for(target) {
            Some(c) => {
                self.mappings.remove(&c);
                true
            }
            None => false,
        }
    }
}

pub fn valid_map_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a map needs a name".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("a name is at most {MAX_NAME} characters"));
    }
    Ok(name.to_string())
}

/// What a patch keeps: the map it wants, by stable id. The name is for the
/// load report when the id is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapRef {
    pub id: u64,
    pub name: String,
}

/// One 7-bit step, in control position.
const STEP: f32 = 1.0 / 127.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    /// Where the knob physically is, 0 to 1, once it has said.
    knob: Option<f32>,
    /// The base value this slot last wrote, as a control position. `None`
    /// while armed. If the bank no longer holds it, someone else moved the
    /// value and the knob is armed again.
    written: Option<f32>,
}

/// Pickup state per control. Only knobs on continuous targets ever arm;
/// stepped targets and switches write at once.
#[derive(Debug, Default)]
pub struct Pickup {
    slots: BTreeMap<u64, Slot>,
}

/// What a knob movement did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Turn {
    /// Write this real value to the parameter.
    Write(f32),
    /// Still armed: the knob has not reached the value yet.
    Waiting,
}

/// One detent of an endless encoder: one 7-bit step of a continuous
/// parameter, one step of a stepped one.
///
/// **Clamped, never wrapped.** An encoder that rolls the cutoff off the top
/// and back to 20 Hz will do it during a take.
fn nudge(def: &ParamDef, base: f32, delta: i8) -> f32 {
    let d = f32::from(delta);
    let per = match def.taper {
        // Stepped ranges are short, so a detent is a step, not 1/127 of one.
        Taper::Stepped(n) => 1.0 / (n.max(2) - 1) as f32,
        _ => STEP,
    };
    def.denormalise((def.normalise(base) + d * per).clamp(0.0, 1.0))
}

impl Pickup {
    /// A knob on `def` sent `value`, and the bank holds `base`. Decides whether
    /// the knob has caught the value.
    ///
    /// `delta` is `Some` for an endless encoder, which never arms: it has no
    /// position to disagree with the value, so every message is a change from
    /// wherever the value already is.
    pub fn turn(
        &mut self,
        control: u64,
        value: u8,
        delta: Option<i8>,
        def: &ParamDef,
        base: f32,
    ) -> Turn {
        if let Some(d) = delta {
            // Nothing to remember: there is no knob position to track.
            self.slots.remove(&control);
            return Turn::Write(nudge(def, base, d));
        }
        let t = f32::from(value.min(127)) / 127.0;
        if matches!(def.taper, Taper::Stepped(_)) {
            return Turn::Write(def.denormalise(t));
        }
        let slot = self.slots.entry(control).or_insert(Slot {
            knob: None,
            written: None,
        });
        let base_t = def.normalise(base);
        // Someone else moved it: the slider, a patch, a preset. Arm again.
        if let Some(w) = slot.written {
            if (w - base_t).abs() > 1e-4 {
                slot.written = None;
            }
        }
        let caught = slot.written.is_some()
            || (t - base_t).abs() <= STEP + 1e-6
            || slot
                .knob
                .is_some_and(|prev| (prev - base_t) * (t - base_t) < 0.0);
        slot.knob = Some(t);
        if caught {
            slot.written = Some(t);
            Turn::Write(def.denormalise(t))
        } else {
            Turn::Waiting
        }
    }

    /// Whether `control` is armed, and where its knob is if known. An
    /// endless encoder is never armed and has no mark to draw.
    pub fn state(
        &self,
        control: u64,
        relative: bool,
        def: &ParamDef,
        base: f32,
    ) -> (bool, Option<f32>) {
        if relative || matches!(def.taper, Taper::Stepped(_)) {
            return (false, None);
        }
        match self.slots.get(&control) {
            Some(s) => {
                let armed = match s.written {
                    Some(w) => (w - def.normalise(base)).abs() > 1e-4,
                    None => true,
                };
                (armed, s.knob)
            }
            None => (true, None),
        }
    }

    /// Forget every position. On a map switch or a device change, every
    /// knob is a stranger again.
    pub fn rearm_all(&mut self) {
        self.slots.clear();
    }

    pub fn forget(&mut self, control: u64) {
        self.slots.remove(&control);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shard_dsp::params::{index_of, PARAMS};

    fn def(id: &str) -> &'static ParamDef {
        &PARAMS[index_of(id).unwrap()]
    }

    #[test]
    fn bind_keeps_one_control_per_target() {
        let mut m = Map {
            id: 1,
            name: "live".into(),
            mappings: BTreeMap::new(),
        };
        let size = Target::Param("grain.size".into());
        assert_eq!(m.bind(1, size.clone()), None);
        assert_eq!(m.bind(2, size.clone()), Some(1));
        assert_eq!(m.mappings.len(), 1);
        assert_eq!(m.control_for(&size), Some(2));
        assert_eq!(m.bind(2, size.clone()), None);
        assert!(m.unbind(&size));
        assert!(!m.unbind(&size));
    }

    #[test]
    fn an_armed_knob_writes_nothing_until_it_catches() {
        let d = def("grain.size");
        let base = d.denormalise(0.5);
        let mut p = Pickup::default();
        assert_eq!(p.turn(1, 10, None, d, base), Turn::Waiting);
        assert_eq!(p.turn(1, 30, None, d, base), Turn::Waiting);
        assert_eq!(p.state(1, false, d, base), (true, Some(30.0 / 127.0)));
        // A jump straight across the value catches, and the bank now holds
        // what the knob wrote.
        let Turn::Write(v) = p.turn(1, 90, None, d, base) else {
            panic!("a jump across the value catches");
        };
        assert!(!p.state(1, false, d, v).0);
    }

    #[test]
    fn catching_works_in_both_directions_and_within_one_step() {
        let d = def("grain.size");
        let base = d.denormalise(0.5);
        let mut p = Pickup::default();
        assert_eq!(p.turn(1, 120, None, d, base), Turn::Waiting);
        assert!(matches!(p.turn(1, 20, None, d, base), Turn::Write(_)));

        let mut p = Pickup::default();
        // 64/127 is within a step of 0.5, so a first message can catch.
        assert!(matches!(p.turn(2, 64, None, d, base), Turn::Write(_)));
    }

    #[test]
    fn a_caught_knob_rearms_when_something_else_moves_the_value() {
        let d = def("grain.size");
        let mut p = Pickup::default();
        let base = d.denormalise(0.5);
        let Turn::Write(v) = p.turn(1, 64, None, d, base) else {
            panic!("should catch");
        };
        // Base is now what the knob wrote; the next message applies.
        assert!(matches!(p.turn(1, 70, None, d, v), Turn::Write(_)));
        // The slider moved it far away: armed again.
        let elsewhere = d.denormalise(0.1);
        assert_eq!(p.turn(1, 72, None, d, elsewhere), Turn::Waiting);
        assert!(p.state(1, false, d, elsewhere).0);
    }

    #[test]
    fn the_ends_land_exactly_on_min_and_max_through_every_taper() {
        for d in PARAMS.iter() {
            let mut p = Pickup::default();
            // Caught by construction: base sits where the knob is.
            let at_min = p.turn(1, 0, None, d, d.min);
            assert_eq!(at_min, Turn::Write(d.min), "{}", d.id);
            let mut p = Pickup::default();
            let at_max = p.turn(1, 127, None, d, d.max);
            assert_eq!(at_max, Turn::Write(d.max), "{}", d.id);
        }
    }

    #[test]
    fn stepped_targets_never_arm_and_land_on_a_step() {
        let d = def("grain.window");
        let mut p = Pickup::default();
        for v in [3u8, 40, 90, 127] {
            let Turn::Write(x) = p.turn(1, v, None, d, 2.0) else {
                panic!("stepped writes at once");
            };
            assert_eq!(x, x.round());
        }
        assert_eq!(p.state(1, false, d, 2.0), (false, None));
        let sw = def("grain.on");
        assert_eq!(p.turn(2, 63, None, sw, 0.0), Turn::Write(0.0));
        assert_eq!(p.turn(2, 64, None, sw, 0.0), Turn::Write(1.0));
    }

    #[test]
    fn an_endless_encoder_never_arms_and_nudges_from_the_value() {
        let d = def("grain.size");
        let mut p = Pickup::default();
        let base = d.denormalise(0.5);
        // A pot at 10 would be nowhere near the value and would wait. An
        // encoder has no position, so the same message is one step up.
        let Turn::Write(up) = p.turn(1, 10, Some(1), d, base) else {
            panic!("an encoder writes at once");
        };
        assert!(up > base, "one click up raises it");
        assert_eq!(p.state(1, true, d, base), (false, None));

        // Down by the same amount comes back to where it started.
        let Turn::Write(back) = p.turn(1, 127, Some(-1), d, up) else {
            panic!("an encoder writes at once");
        };
        assert!((back - base).abs() < 1e-3, "{back} should be {base}");
    }

    #[test]
    fn an_encoder_clamps_at_the_ends_rather_than_wrapping() {
        for d in PARAMS.iter() {
            let mut p = Pickup::default();
            let Turn::Write(v) = p.turn(1, 1, Some(-127), d, d.min) else {
                panic!("writes");
            };
            assert_eq!(v, d.min, "{} fell off the bottom", d.id);
            let Turn::Write(v) = p.turn(1, 1, Some(127), d, d.max) else {
                panic!("writes");
            };
            assert_eq!(v, d.max, "{} rolled over the top", d.id);
        }
    }

    #[test]
    fn one_detent_moves_a_stepped_parameter_exactly_one_step() {
        let d = def("grain.window");
        let mut p = Pickup::default();
        let Turn::Write(v) = p.turn(1, 1, Some(1), d, d.min) else {
            panic!("writes");
        };
        assert_eq!(v, d.min + 1.0);
        // And a switch flips with one click rather than needing 127.
        let sw = def("grain.on");
        let Turn::Write(on) = p.turn(2, 1, Some(1), sw, 0.0) else {
            panic!("writes");
        };
        assert_eq!(on, 1.0);
    }

    #[test]
    fn rearm_all_forgets_positions() {
        let d = def("grain.size");
        let base = d.denormalise(0.5);
        let mut p = Pickup::default();
        p.turn(1, 64, None, d, base);
        p.rearm_all();
        assert_eq!(p.state(1, false, d, base), (true, None));
    }
}
