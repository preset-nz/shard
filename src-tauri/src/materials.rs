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
use shard_dsp::sources::OCTAVE_RANGE;
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

    fn slot(&mut self, g: Generator) -> &mut Option<u64> {
        match g {
            Generator::Player => &mut self.material,
            Generator::Grain => &mut self.grain,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.material.is_none() && self.grain.is_none()
    }
}

/// The generator a node id names: `material` is Sample and `grain` is
/// Granular. None for a node that reads no material.
pub fn generator_of(node: &str) -> Option<Generator> {
    match node {
        "material" => Some(Generator::Player),
        "grain" => Some(Generator::Grain),
        _ => None,
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
    /// so what plays at launch is listed like any other material.
    pub fn with_drone() -> Self {
        let mut pool = Self::default();
        let id = pool.add_drone();
        pool.wire_unwired(id);
        pool
    }

    pub fn get(&self, id: u64) -> Option<&MaterialRecord> {
        self.materials.iter().find(|m| m.id == id)
    }

    /// Add a WAV at its defaults, the whole file at its own pitch, and return
    /// its id. Nothing reads it until it is wired.
    pub fn add(&mut self, name: &str, path: &str) -> u64 {
        self.push(name, Some(path.to_string()))
    }

    /// Add the built-in drone and return its id.
    pub fn add_drone(&mut self) -> u64 {
        self.push(DRONE_NAME, None)
    }

    fn push(&mut self, name: &str, path: Option<String>) -> u64 {
        // Never below an id already in the list, which a hand-edited document
        // can carry past its counter.
        let floor = self
            .materials
            .iter()
            .map(|m| m.id.saturating_add(1))
            .max()
            .unwrap_or(1);
        let id = self.next_material_id.max(floor);
        self.next_material_id = id.saturating_add(1);
        self.materials.push(MaterialRecord {
            id,
            name: name.to_string(),
            path,
            octave: 0.0,
            trim_start: 0.0,
            trim_end: 1.0,
        });
        id
    }

    /// Wire a material into a generator, or unwire it with none. An id the
    /// pool does not have changes nothing.
    pub fn wire(&mut self, g: Generator, material: Option<u64>) -> Result<(), String> {
        if let Some(id) = material {
            if self.get(id).is_none() {
                return Err(format!("there is no material {id}"));
            }
        }
        *self.wires.slot(g) = material;
        Ok(())
    }

    /// Wire `id` into every generator that reads nothing yet, so a material
    /// added to a silent generator is heard at once, and one added later
    /// leaves the wiring alone.
    pub fn wire_unwired(&mut self, id: u64) {
        for g in Generator::ALL {
            if self.wires.get(g).is_none() {
                *self.wires.slot(g) = Some(id);
            }
        }
    }

    /// Take a saved patch's wires, dropping any to a material the pool does
    /// not have.
    pub fn set_wires(&mut self, wires: Wires) {
        for g in Generator::ALL {
            let id = wires.get(g).filter(|id| self.get(*id).is_some());
            *self.wires.slot(g) = id;
        }
    }

    /// The materials wired into a generator, the player's first.
    pub fn wired(&self) -> impl Iterator<Item = &MaterialRecord> + '_ {
        Generator::ALL
            .into_iter()
            .filter_map(|g| self.wires.get(g))
            .filter_map(|id| self.get(id))
    }

    /// Set a material's octave and trim. The octave lands on a whole octave
    /// inside the range and the ends inside the file; a non-number is refused
    /// and changes nothing.
    pub fn set_values(
        &mut self,
        id: u64,
        octave: f32,
        trim_start: f32,
        trim_end: f32,
    ) -> Result<(), String> {
        if !(octave.is_finite() && trim_start.is_finite() && trim_end.is_finite()) {
            return Err("a material's octave and trim must be numbers".into());
        }
        let m = self
            .materials
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| format!("there is no material {id}"))?;
        m.octave = octave.round().clamp(-OCTAVE_RANGE, OCTAVE_RANGE);
        m.trim_start = trim_start.clamp(0.0, 1.0);
        m.trim_end = trim_end.clamp(0.0, 1.0);
        Ok(())
    }

    /// Remove a material. A generator that read it goes silent.
    pub fn remove(&mut self, id: u64) -> Result<(), String> {
        let before = self.materials.len();
        self.materials.retain(|m| m.id != id);
        if self.materials.len() == before {
            return Err(format!("there is no material {id}"));
        }
        for g in Generator::ALL {
            if self.wires.get(g) == Some(id) {
                *self.wires.slot(g) = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_pool_lists_the_drone_it_plays() {
        let pool = Pool::with_drone();
        let [drone] = pool.materials.as_slice() else {
            panic!("expected only the drone, got {:?}", pool.materials);
        };
        assert!(drone.path.is_none(), "the drone has no file");
        assert_eq!(drone.name, DRONE_NAME);
        assert_eq!(
            pool.wires,
            Wires {
                material: Some(drone.id),
                grain: Some(drone.id)
            }
        );
        // Saved without a path, which is what marks it as built in.
        let json = serde_json::to_value(drone).unwrap();
        assert!(json.get("path").is_none(), "{json}");
        let back: MaterialRecord = serde_json::from_value(json).unwrap();
        assert!(back.path.is_none());
    }

    #[test]
    fn removing_the_drone_silences_what_read_it() {
        let mut pool = Pool::with_drone();
        let drone = pool.materials[0].id;
        pool.remove(drone).unwrap();
        assert!(pool.wires.is_empty());
        assert!(pool.materials.is_empty());
        let again = pool.add_drone();
        assert!(again > drone, "the drone comes back with a fresh id");
    }

    #[test]
    fn a_material_added_to_a_silent_generator_is_wired_in() {
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        pool.wire_unwired(a);
        assert_eq!(
            pool.wires,
            Wires {
                material: Some(a),
                grain: Some(a)
            }
        );

        // A later one leaves the wiring as it is.
        let b = pool.add("b.wav", "/b.wav");
        pool.wire_unwired(b);
        assert_eq!(pool.wires.get(Generator::Grain), Some(a));

        // Unless a generator is free.
        pool.wire(Generator::Grain, None).unwrap();
        pool.wire_unwired(b);
        assert_eq!(pool.wires.get(Generator::Grain), Some(b));
        assert_eq!(pool.wires.get(Generator::Player), Some(a));
    }

    #[test]
    fn each_generator_reads_its_own_material() {
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        let b = pool.add("b.wav", "/b.wav");
        pool.wire(Generator::Player, Some(a)).unwrap();
        pool.wire(Generator::Grain, Some(b)).unwrap();
        pool.set_values(a, 1.0, 0.2, 0.6).unwrap();
        pool.set_values(b, -2.0, 0.5, 0.9).unwrap();
        let reading = |g| {
            pool.wires
                .get(g)
                .and_then(|id| pool.get(id))
                .unwrap()
                .reading()
        };
        assert_eq!(
            reading(Generator::Player),
            Reading {
                octave: 1.0,
                trim_start: 0.2,
                trim_end: 0.6
            }
        );
        assert_eq!(
            reading(Generator::Grain),
            Reading {
                octave: -2.0,
                trim_start: 0.5,
                trim_end: 0.9
            }
        );
    }

    #[test]
    fn wiring_an_unknown_material_changes_nothing() {
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        pool.wire(Generator::Player, Some(a)).unwrap();
        assert!(pool.wire(Generator::Player, Some(99)).is_err());
        assert_eq!(pool.wires.get(Generator::Player), Some(a));
    }

    #[test]
    fn removing_a_material_unwires_it() {
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        let b = pool.add("b.wav", "/b.wav");
        pool.wire(Generator::Player, Some(a)).unwrap();
        pool.wire(Generator::Grain, Some(b)).unwrap();
        pool.remove(a).unwrap();
        assert_eq!(pool.wires.get(Generator::Player), None);
        assert_eq!(pool.wires.get(Generator::Grain), Some(b));
        assert!(pool.remove(a).is_err());
    }

    #[test]
    fn saved_wires_to_a_missing_material_are_dropped() {
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        pool.set_wires(Wires {
            material: Some(a),
            grain: Some(42),
        });
        assert_eq!(
            pool.wires,
            Wires {
                material: Some(a),
                grain: None
            }
        );
        assert_eq!(pool.wired().map(|m| m.id).collect::<Vec<_>>(), vec![a]);
    }

    #[test]
    fn values_are_brought_into_range_and_non_numbers_refused() {
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        pool.set_values(a, 1.4, -0.5, 2.0).unwrap();
        let m = pool.get(a).unwrap();
        assert_eq!((m.octave, m.trim_start, m.trim_end), (1.0, 0.0, 1.0));
        pool.set_values(a, 7.0, 0.3, 0.4).unwrap();
        assert_eq!(pool.get(a).unwrap().octave, OCTAVE_RANGE);

        assert!(pool.set_values(a, f32::NAN, 0.1, 0.2).is_err());
        let m = pool.get(a).unwrap();
        assert_eq!((m.octave, m.trim_start, m.trim_end), (2.0, 0.3, 0.4));
    }

    #[test]
    fn wires_are_named_by_node() {
        let json = serde_json::to_value(Wires {
            material: Some(3),
            grain: None,
        })
        .unwrap();
        assert_eq!(json["material"], 3);
        assert!(json["grain"].is_null());
        assert_eq!(generator_of("material"), Some(Generator::Player));
        assert_eq!(generator_of("grain"), Some(Generator::Grain));
        assert_eq!(generator_of("crush"), None);
    }

    #[test]
    fn a_removed_id_is_never_handed_out_again() {
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        let b = pool.add("b.wav", "/b.wav");
        pool.remove(b).unwrap();
        let c = pool.add("c.wav", "/c.wav");
        assert!(c > b && b > a);

        // Also after a save, since the counter is saved.
        let mut back: Pool = serde_json::from_str(&serde_json::to_string(&pool).unwrap()).unwrap();
        assert!(back.add("d.wav", "/d.wav") > c);
    }
}
