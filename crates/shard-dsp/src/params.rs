//! The parameter table.
//!
//! One flat list of definitions with stable string ids. Everything addresses
//! this: the audio thread reads it, and any UI is generated from it rather
//! than hand-wired, so adding a parameter is adding a row and nothing else.
//!
//! Two rules carried from the design and worth keeping:
//!
//! - **Ids are a wire format.** Renaming one breaks every saved patch.
//! - **The taper belongs to the parameter, not the caller.** It says how a
//!   0-to-1 control position maps to a value, and it also says what a control
//!   should look like: bipolar wants a centre detent, stepped wants a
//!   selector rather than a slider.

use std::sync::atomic::{AtomicU32, Ordering};

/// How a normalised 0-to-1 control position maps onto the real value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taper {
    /// Even. Halfway along is halfway up.
    Linear,
    /// Perceptual. Use for anything measured in time, frequency, size or
    /// density, because hearing is roughly logarithmic. Linear on grain size
    /// gives you a control whose useful range is the bottom few millimetres.
    Exponential,
    /// Symmetric around zero. Pitch, pan, feedback.
    Bipolar,
    /// Discrete choices. Never smoothed, because interpolating between two
    /// enum values produces a number that means nothing.
    Stepped(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    None,
    Ms,
    Hz,
    Semitones,
    Percent,
}

#[derive(Debug, Clone, Copy)]
pub struct ParamDef {
    pub id: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub taper: Taper,
    pub unit: Unit,
    /// Per-parameter smoothing time. The table decides, not the caller.
    pub smooth_ms: f32,
}

impl ParamDef {
    /// Control position to real value.
    pub fn denormalise(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self.taper {
            Taper::Linear | Taper::Bipolar => self.min + (self.max - self.min) * t,
            Taper::Exponential => {
                // Geometric interpolation needs a positive floor; fall back to
                // linear for ranges that touch or cross zero.
                if self.min > 0.0 && self.max > 0.0 {
                    self.min * (self.max / self.min).powf(t)
                } else {
                    self.min + (self.max - self.min) * t
                }
            }
            Taper::Stepped(n) => {
                let n = n.max(1);
                let step = (t * n as f32).floor().min((n - 1) as f32);
                self.min + (self.max - self.min) * (step / (n - 1).max(1) as f32)
            }
        }
    }

    /// Real value back to control position. The inverse of `denormalise`.
    pub fn normalise(&self, v: f32) -> f32 {
        let v = v.clamp(self.min.min(self.max), self.max.max(self.min));
        match self.taper {
            Taper::Linear | Taper::Bipolar | Taper::Stepped(_) => {
                if (self.max - self.min).abs() < f32::EPSILON {
                    0.0
                } else {
                    (v - self.min) / (self.max - self.min)
                }
            }
            Taper::Exponential => {
                if self.min > 0.0 && self.max > 0.0 {
                    (v / self.min).ln() / (self.max / self.min).ln()
                } else if (self.max - self.min).abs() < f32::EPSILON {
                    0.0
                } else {
                    (v - self.min) / (self.max - self.min)
                }
            }
        }
        .clamp(0.0, 1.0)
    }
}

/// Batch one's set. Deliberately small. Do not grow this past what the
/// granular voice and the ring modulator actually read.
pub const PARAMS: &[ParamDef] = &[
    ParamDef {
        id: "grain.position",
        name: "Position",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    ParamDef {
        id: "grain.jitter",
        name: "Jitter",
        min: 0.0,
        max: 1.0,
        default: 0.05,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    ParamDef {
        id: "grain.size",
        name: "Size",
        min: 5.0,
        max: 500.0,
        default: 120.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.density",
        name: "Density",
        min: 0.5,
        max: 200.0,
        default: 18.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.pitch",
        name: "Pitch",
        min: -24.0,
        max: 24.0,
        default: 0.0,
        taper: Taper::Bipolar,
        unit: Unit::Semitones,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.spread",
        name: "Pitch spread",
        min: 0.0,
        max: 24.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Semitones,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.pan",
        name: "Pan spread",
        min: 0.0,
        max: 1.0,
        default: 0.6,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.reverse",
        name: "Reverse",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "grain.window",
        name: "Window",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        taper: Taper::Stepped(4),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // Defaults fully dry on purpose. Pressing play should give you the sample
    // as it is, so every granular control has an audible before and after.
    ParamDef {
        id: "mix.dry",
        name: "Dry / Granular",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    ParamDef {
        id: "ring.freq",
        name: "Ring freq",
        min: 1.0,
        max: 5000.0,
        default: 140.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "ring.mix",
        name: "Ring mix",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "amp.gain",
        name: "Gain",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
];

pub fn index_of(id: &str) -> Option<usize> {
    PARAMS.iter().position(|p| p.id == id)
}

/// Lock-free parameter storage, shared between whatever edits values and the
/// audio thread. Values are stored as real values, not normalised, so the
/// audio thread never has to consult the table.
///
/// Relaxed ordering is correct here: each parameter is independent, a reader
/// that sees a value one block late is inaudible, and there is no other state
/// whose visibility depends on these writes.
pub struct ParamBank {
    values: Vec<AtomicU32>,
}

impl ParamBank {
    pub fn new() -> Self {
        Self {
            values: PARAMS
                .iter()
                .map(|p| AtomicU32::new(p.default.to_bits()))
                .collect(),
        }
    }

    #[inline]
    pub fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set(&self, index: usize, value: f32) {
        let d = &PARAMS[index];
        let lo = d.min.min(d.max);
        let hi = d.max.max(d.min);
        self.values[index].store(value.clamp(lo, hi).to_bits(), Ordering::Relaxed);
    }

    pub fn get_by_id(&self, id: &str) -> Option<f32> {
        index_of(id).map(|i| self.get(i))
    }

    pub fn set_by_id(&self, id: &str, value: f32) -> bool {
        match index_of(id) {
            Some(i) => {
                self.set(i, value);
                true
            }
            None => false,
        }
    }

    /// Set from a 0-to-1 control position, applying the parameter's taper.
    pub fn set_normalised(&self, id: &str, t: f32) -> bool {
        match index_of(id) {
            Some(i) => {
                self.set(i, PARAMS[i].denormalise(t));
                true
            }
            None => false,
        }
    }
}

impl Default for ParamBank {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in PARAMS {
            assert!(seen.insert(p.id), "duplicate id: {}", p.id);
        }
    }

    #[test]
    fn defaults_sit_inside_their_range() {
        for p in PARAMS {
            assert!(
                p.default >= p.min && p.default <= p.max,
                "{} default {} outside {}..{}",
                p.id,
                p.default,
                p.min,
                p.max
            );
        }
    }

    #[test]
    fn exponential_params_have_a_positive_floor() {
        // Geometric interpolation is undefined through zero. A parameter
        // tagged exponential with a zero minimum would silently fall back to
        // linear, which is the bug this test exists to catch.
        for p in PARAMS {
            if p.taper == Taper::Exponential {
                assert!(
                    p.min > 0.0,
                    "{} is exponential but starts at {}",
                    p.id,
                    p.min
                );
            }
        }
    }

    #[test]
    fn stepped_params_never_smooth() {
        for p in PARAMS {
            if matches!(p.taper, Taper::Stepped(_)) {
                assert_eq!(p.smooth_ms, 0.0, "{} is stepped but smooths", p.id);
            }
        }
    }

    #[test]
    fn denormalise_hits_both_ends() {
        for p in PARAMS {
            assert!((p.denormalise(0.0) - p.min).abs() < 1e-3, "{} at 0", p.id);
            assert!((p.denormalise(1.0) - p.max).abs() < 1e-3, "{} at 1", p.id);
        }
    }

    #[test]
    fn normalise_round_trips() {
        for p in PARAMS {
            for step in 0..=20 {
                let t = step as f32 / 20.0;
                let v = p.denormalise(t);
                let back = p.normalise(v);
                let tolerance = if matches!(p.taper, Taper::Stepped(_)) {
                    0.3
                } else {
                    1e-3
                };
                assert!(
                    (back - t).abs() < tolerance,
                    "{} round trip {t} -> {v} -> {back}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn exponential_midpoint_is_geometric() {
        let size = &PARAMS[index_of("grain.size").unwrap()];
        let mid = size.denormalise(0.5);
        let geometric = (size.min * size.max).sqrt();
        assert!((mid - geometric).abs() < 1.0, "{mid} vs {geometric}");
        // And it is well below the arithmetic midpoint, which is the point.
        assert!(mid < (size.min + size.max) / 2.0);
    }

    #[test]
    fn stepped_lands_on_exact_steps() {
        let w = &PARAMS[index_of("grain.window").unwrap()];
        let seen: Vec<f32> = (0..=20).map(|i| w.denormalise(i as f32 / 20.0)).collect();
        for v in &seen {
            assert!((v - v.round()).abs() < 1e-4, "{v} is not a whole step");
            assert!(*v >= 0.0 && *v <= 3.0);
        }
        assert!(seen.contains(&0.0) && seen.contains(&3.0));
    }

    #[test]
    fn bank_starts_at_defaults() {
        let bank = ParamBank::new();
        for (i, p) in PARAMS.iter().enumerate() {
            assert_eq!(bank.get(i), p.default, "{}", p.id);
        }
    }

    #[test]
    fn bank_clamps_out_of_range_writes() {
        let bank = ParamBank::new();
        bank.set_by_id("grain.position", 99.0);
        assert_eq!(bank.get_by_id("grain.position"), Some(1.0));
        bank.set_by_id("grain.position", -99.0);
        assert_eq!(bank.get_by_id("grain.position"), Some(0.0));
    }

    #[test]
    fn unknown_ids_are_rejected_not_ignored() {
        let bank = ParamBank::new();
        assert!(!bank.set_by_id("grain.nope", 0.5));
        assert!(bank.get_by_id("grain.nope").is_none());
        assert!(bank.set_by_id("grain.size", 200.0));
    }

    #[test]
    fn normalised_writes_apply_the_taper() {
        let bank = ParamBank::new();
        bank.set_normalised("grain.size", 0.5);
        let v = bank.get_by_id("grain.size").unwrap();
        let d = &PARAMS[index_of("grain.size").unwrap()];
        assert!((v - (d.min * d.max).sqrt()).abs() < 1.0);
    }
}
