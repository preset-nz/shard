//! The materials: WAV files on disk and the built-in drone, gathered into a
//! pool and wired into the generators that read them.
//!
//! Georg, 2026-09-15, first: *"can't we add wavs/materials there? so I can load
//! one or more wav files, set their octave and trim."* Then, having tried
//! switching between them: *"I also want to be able to wire the material into
//! the player or the granular node."* So each generator reads its own
//! material, and Sample and Granular can sound two different files at once.
//! And: *"the built-in drone should be a material as well"*, because something
//! playing that the pool does not list is inconsistent. A generator with
//! nothing wired is silent.
//!
//! **Each material keeps its own octave and trim,** and every generator wired
//! to it reads through them. They belong to the material rather than to the
//! patch, so they are not rows in the parameter table and no LFO moves them
//! (Georg: not needed). They reach the engine as one `Reading` per generator.
//!
//! The pool is saved in the document beside the tracker, and the patch saves
//! its wires. Material is still WAV on disk, never copied in.

use serde::{Deserialize, Serialize};
use shard_dsp::{Generator, Reading};

/// What the built-in drone is called in the tree.
pub const DRONE_NAME: &str = "Built-in drone";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialRecord {
    /// Stable for the life of the material and never reused, so a wire cannot
    /// come to mean a different file.
    pub id: u64,
    /// The file name, for the tree.
    pub name: String,
    /// Where the WAV is, absolute: samples live wherever they live. None for
    /// the built-in drone, which has no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub octave: f32,
    #[serde(default)]
    pub trim_start: f32,
    #[serde(default = "one")]
    pub trim_end: f32,
    /// The note the material sounds at, as a MIDI number (60 is C4), when it
    /// has one. Step pitches are semitones from the sample whatever it is; a
    /// root lets the tracker show them as note names. None means unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<u8>,
}

fn one() -> f32 {
    1.0
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl MaterialRecord {
    /// How a generator wired to this material reads it.
    pub fn reading(&self) -> Reading {
        Reading {
            octave: self.octave,
            trim_start: self.trim_start,
            trim_end: self.trim_end,
        }
    }
}

/// Which material each generator reads, by id. None is silent. Named by the
/// generators' node ids in the table, so a document reads
/// `"wires": { "material": 3, "grain": 5 }`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wires {
    /// Sample, plain playback.
    #[serde(default)]
    pub material: Option<u64>,
    /// Granular, the cloud.
    #[serde(default)]
    pub grain: Option<u64>,
}

impl Wires {
    pub fn get(&self, g: Generator) -> Option<u64> {
        match g {
            Generator::Player => self.material,
            Generator::Grain => self.grain,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.material.is_none() && self.grain.is_none()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pool {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<MaterialRecord>,
    /// The id the next new material takes. Saved, so a removed material's id
    /// is never handed out again.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub next_material_id: u64,
    /// What each generator reads. Saved in the patch as `wires`, not here.
    #[serde(skip)]
    pub wires: Wires,
}

impl Pool {
    /// A new document's pool: the built-in drone, wired into both generators,
    /// so what plays at launch is listed like any other material. The seed of
    /// a new session (`object_model`).
    pub fn with_drone() -> Self {
        Self {
            materials: vec![MaterialRecord {
                id: 1,
                name: DRONE_NAME.to_string(),
                path: None,
                octave: 0.0,
                trim_start: 0.0,
                trim_end: 1.0,
                root: None,
            }],
            next_material_id: 2,
            wires: Wires {
                material: Some(1),
                grain: Some(1),
            },
        }
    }

    pub fn get(&self, id: u64) -> Option<&MaterialRecord> {
        self.materials.iter().find(|m| m.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_pool_lists_the_drone_it_plays() {
        let pool = Pool::with_drone();
        assert_eq!(pool.materials.len(), 1);
        let drone = &pool.materials[0];
        assert_eq!(drone.name, DRONE_NAME);
        assert!(drone.path.is_none());
        assert_eq!(pool.wires.material, Some(drone.id));
        assert_eq!(pool.wires.grain, Some(drone.id));
    }
}
