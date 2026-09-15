//! The materials: WAV files on disk, gathered into a pool you switch between.
//!
//! Georg, 2026-09-15: *"can't we add wavs/materials there? so I can load one
//! or more wav files, set their octave and trim."* A pool, not a mixer: one
//! material plays at a time, and both generators read it. Several sounding at
//! once is roadmap row 10.
//!
//! **Each material keeps its own octave and trim.** They stay rows in the
//! parameter table, because the engine reads them there and an LFO can move
//! trim. The record is where they rest while the material is not playing:
//! switching writes the rows onto the material being left, then the arriving
//! material's values into the rows. So the rows are live for the active
//! material only, and the record is only ever stale for that one; `settle`
//! brings it up to date before anything reads the pool.
//!
//! The pool is saved in the document beside the tracker, and the patch names
//! the material it plays. Material is still WAV on disk, never copied in.

use serde::{Deserialize, Serialize};
use shard_dsp::ParamBank;

/// The rows a material owns. Values here belong to whichever material is
/// playing, so the patch does not save them.
pub const MATERIAL_PARAMS: [&str; 3] = ["material.octave", "trim.start", "trim.end"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialRecord {
    /// Stable for the life of the material and never reused, so the patch's
    /// reference cannot come to mean a different file.
    pub id: u64,
    /// The file name, for the tree.
    pub name: String,
    /// Absolute, as the patch's sample path was: samples live wherever they live.
    pub path: String,
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

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pool {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<MaterialRecord>,
    /// The id the next new material takes. Saved, so a removed material's id
    /// is never handed out again.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub next_material_id: u64,
    /// The material playing now. Saved as the patch's `material`, not here.
    #[serde(skip)]
    pub active: Option<u64>,
}

impl MaterialRecord {
    fn values(&self) -> [f32; 3] {
        [self.octave, self.trim_start, self.trim_end]
    }
}

impl Pool {
    pub fn get(&self, id: u64) -> Option<&MaterialRecord> {
        self.materials.iter().find(|m| m.id == id)
    }

    /// Add a material at its defaults, the whole file at its own pitch, and
    /// return its id. It does not start playing: `activate` does that.
    pub fn add(&mut self, name: &str, path: &str) -> u64 {
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
            path: path.to_string(),
            octave: 0.0,
            trim_start: 0.0,
            trim_end: 1.0,
        });
        id
    }

    /// Copy the rows onto the playing material, so its record is current.
    /// The hand's values, never what an LFO has moved them to.
    pub fn settle(&mut self, bank: &ParamBank) {
        let Some(id) = self.active else { return };
        let read = |key: &str, fallback: f32| bank.get_by_id(key).unwrap_or(fallback);
        if let Some(m) = self.materials.iter_mut().find(|m| m.id == id) {
            m.octave = read("material.octave", m.octave);
            m.trim_start = read("trim.start", m.trim_start);
            m.trim_end = read("trim.end", m.trim_end);
        }
    }

    /// Make `id` the playing material: settle the one being left, then write
    /// the new one's octave and trim into the rows. Loading its audio is the
    /// caller's, and happens first, so a file that fails to load changes
    /// nothing here.
    pub fn activate(&mut self, id: u64, bank: &ParamBank) -> Result<(), String> {
        let values = self
            .get(id)
            .ok_or_else(|| format!("there is no material {id}"))?
            .values();
        self.settle(bank);
        for (key, v) in MATERIAL_PARAMS.iter().zip(values) {
            bank.set_by_id(key, v);
        }
        self.active = Some(id);
        Ok(())
    }

    /// Stop playing any material, and put the rows back at their defaults.
    pub fn deactivate(&mut self, bank: &ParamBank) {
        self.settle(bank);
        self.active = None;
        for (key, v) in MATERIAL_PARAMS.iter().zip([0.0, 0.0, 1.0]) {
            bank.set_by_id(key, v);
        }
    }

    /// Remove a material from the pool. Removing the playing one leaves
    /// nothing active; the caller decides what plays instead.
    pub fn remove(&mut self, id: u64) -> Result<(), String> {
        let before = self.materials.len();
        self.materials.retain(|m| m.id != id);
        if self.materials.len() == before {
            return Err(format!("there is no material {id}"));
        }
        if self.active == Some(id) {
            self.active = None;
        }
        Ok(())
    }

    /// The material to play after `id` is removed: the one below it in the
    /// tree, or above it when it was last. Asked before removing.
    pub fn neighbour(&self, id: u64) -> Option<u64> {
        let at = self.materials.iter().position(|m| m.id == id)?;
        self.materials
            .get(at + 1)
            .or_else(|| at.checked_sub(1).and_then(|i| self.materials.get(i)))
            .map(|m| m.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(bank: &ParamBank, octave: f32, start: f32, end: f32) {
        bank.set_by_id("material.octave", octave);
        bank.set_by_id("trim.start", start);
        bank.set_by_id("trim.end", end);
    }

    fn rows(bank: &ParamBank) -> [f32; 3] {
        MATERIAL_PARAMS.map(|k| bank.get_by_id(k).unwrap())
    }

    #[test]
    fn every_material_param_is_a_row() {
        for key in MATERIAL_PARAMS {
            assert!(
                shard_dsp::params::index_of(key).is_some(),
                "{key} is not in the table"
            );
        }
    }

    #[test]
    fn each_material_keeps_its_own_octave_and_trim() {
        let bank = ParamBank::new();
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        let b = pool.add("b.wav", "/b.wav");

        pool.activate(a, &bank).unwrap();
        set(&bank, 1.0, 0.2, 0.6);
        pool.activate(b, &bank).unwrap();
        assert_eq!(rows(&bank), [0.0, 0.0, 1.0], "a new material starts whole");
        set(&bank, -2.0, 0.5, 0.9);

        pool.activate(a, &bank).unwrap();
        assert_eq!(rows(&bank), [1.0, 0.2, 0.6]);
        pool.activate(b, &bank).unwrap();
        assert_eq!(rows(&bank), [-2.0, 0.5, 0.9]);
    }

    #[test]
    fn an_unknown_material_changes_nothing() {
        let bank = ParamBank::new();
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        pool.activate(a, &bank).unwrap();
        set(&bank, 1.0, 0.2, 0.6);
        assert!(pool.activate(99, &bank).is_err());
        assert_eq!(pool.active, Some(a));
        assert_eq!(rows(&bank), [1.0, 0.2, 0.6]);
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
        let back: Pool = serde_json::from_str(&serde_json::to_string(&pool).unwrap()).unwrap();
        let mut back = back;
        assert!(back.add("d.wav", "/d.wav") > c);
    }

    #[test]
    fn the_neighbour_is_below_then_above() {
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        let b = pool.add("b.wav", "/b.wav");
        let c = pool.add("c.wav", "/c.wav");
        assert_eq!(pool.neighbour(b), Some(c));
        assert_eq!(pool.neighbour(c), Some(b));
        pool.remove(b).unwrap();
        pool.remove(c).unwrap();
        assert_eq!(pool.neighbour(a), None);
    }

    #[test]
    fn deactivating_keeps_the_record_and_resets_the_rows() {
        let bank = ParamBank::new();
        let mut pool = Pool::default();
        let a = pool.add("a.wav", "/a.wav");
        pool.activate(a, &bank).unwrap();
        set(&bank, 1.0, 0.2, 0.6);
        pool.deactivate(&bank);
        assert_eq!(pool.active, None);
        assert_eq!(rows(&bank), [0.0, 0.0, 1.0]);
        assert_eq!(pool.get(a).unwrap().values(), [1.0, 0.2, 0.6]);
    }
}
