//! One pass of the patch, rendered offline, for drawing.
//!
//! Georg, 2026-09-27: *"just want to see how the patch looks like, how the
//! different things affect it."* So the preview is the patch's own output,
//! not a sketch of any one node: a fresh engine, the same values, materials,
//! readings and modulation as the live one, heard alone as sound scaping hears
//! it, with the steps off.
//!
//! A fresh engine starts every smoother and fade at the table's defaults, so a
//! capture from its first sample would glide in from values the patch does
//! not have, over exactly the attack you are looking at. It settles first,
//! then `Engine::restart_pass` starts a clean pass, and that pass is captured.
//!
//! Deterministic: every random source is seeded, so the same patch draws the
//! same picture and the preview does not flicker while nothing changes.
//!
//! Allocates freely. It is never called on the audio thread.

use crate::chain::BYPASS_MS;
use crate::modulation::ModSet;
use crate::note::Length;
use crate::params::{index_of, ParamBank};
use crate::sources::{Generator, Reading};
use crate::steps::StepParams;
use crate::Engine;

/// How long the patch settles before the captured pass. Six of the longest
/// usual smoothing times, which is 40 ms.
const SETTLE_S: f32 = 0.25;

/// When nothing is wired there is no pass to measure, so draw this long.
const UNWIRED_S: f32 = 1.0;

/// A Loop patch runs for ever, so draw this much of it: the attack and
/// enough of what it settles into.
const LOOP_S: f32 = 2.0;

/// The gestures a hand makes while playing. They are not part of the patch,
/// and a brake held while a control moves would flatten the picture.
const GESTURES: [&str; 2] = ["tape.brake", "tape.reverse"];

pub struct PreviewInput<'a> {
    pub sample_rate: f32,
    /// As the live engine, or the cloud steals voices differently.
    pub max_grains: usize,
    /// The hand's values. Copied; the gestures are left out.
    pub bank: &'a ParamBank,
    /// The arrangement's values. Read as sound scaping reads them: the patch
    /// heard alone.
    pub arrangement: &'a ParamBank,
    /// Each generator's whole material, and how it reads it.
    pub player: &'a [f32],
    pub player_reading: Reading,
    pub grain: &'a [f32],
    pub grain_reading: Reading,
    pub mods: ModSet,
    /// The longest capture. A pass past it is cut, and says so.
    pub max_seconds: f32,
}

pub struct Preview {
    /// One pass, stereo, interleaved.
    pub samples: Vec<f32>,
    /// The pass ran past `max_seconds` and was cut.
    pub capped: bool,
}

/// A generator's trimmed window as its own buffer, and a reading of the
/// whole of it at the same octave. The engine then sees exactly the window.
fn windowed(material: &[f32], reading: Reading) -> (Vec<f32>, Reading) {
    if material.len() < 2 {
        return (Vec::new(), Reading::default());
    }
    let (lo, hi) = reading.window(material.len());
    (
        material[lo..hi].to_vec(),
        Reading {
            octave: reading.octave,
            ..Reading::default()
        },
    )
}

/// One pass of the patch as sound scaping plays it.
pub fn render_pass(input: PreviewInput) -> Preview {
    let sr = input.sample_rate;
    let mut e = Engine::new(sr, input.max_grains);
    let (player, player_reading) = windowed(input.player, input.player_reading);
    let (grain, grain_reading) = windowed(input.grain, input.grain_reading);

    // The pass is the player's window, or the cloud's when nothing is wired
    // into Sample, as the engine clocks it. Varispeed, so an octave up halves
    // it.
    let (clock_len, clock_reading) = if player.len() >= 2 {
        (player.len(), player_reading)
    } else {
        (grain.len(), grain_reading)
    };
    // What one note is depends on the patch's Length (`note.rs`): a pass
    // through the sample, a held note and its release, or a stretch of a
    // patch that never ends.
    let value = |id: &str| index_of(id).map_or(0.0, |i| input.bank.get(i));
    let pass = match Length::from_value(value("patch.length")) {
        Length::Sample if clock_len >= 2 => {
            (clock_len as f32 / clock_reading.octaves().exp2()).ceil() as usize
        }
        Length::Sample => (UNWIRED_S * sr) as usize,
        Length::Hold => {
            let release = if value("env.on") >= 0.5 {
                value("env.release").max(0.0)
            } else {
                0.0
            };
            ((value("patch.hold").max(0.0) + release + BYPASS_MS) * 0.001 * sr).ceil() as usize
        }
        Length::Loop => (LOOP_S * sr) as usize,
    };
    let max = (input.max_seconds.max(0.0) * sr) as usize;
    let frames = pass.min(max.max(1));

    drop(e.swap_source(Generator::Player, player));
    drop(e.swap_source(Generator::Grain, grain));
    e.set_readings(player_reading, grain_reading);
    drop(e.set_modulation(input.mods));
    e.set_steps(StepParams::default());
    e.set_arrangement(input.arrangement, true);

    let bank = ParamBank::new();
    for i in 0..bank.defs().len() {
        bank.set(i, input.bank.get(i));
    }
    for id in GESTURES {
        if let Some(i) = index_of(id) {
            bank.set(i, bank.defs()[i].default);
        }
    }

    e.set_playing(true);
    let run = |e: &mut Engine, frames: usize, keep: &mut Vec<f32>| {
        let mut block = vec![0.0f32; 512];
        let mut left = frames;
        while left > 0 {
            let n = left.min(256);
            let out = &mut block[..n * 2];
            e.process_block(out, &bank);
            keep.extend_from_slice(out);
            left -= n;
        }
    };
    let mut settle = Vec::new();
    run(&mut e, (SETTLE_S * sr) as usize, &mut settle);
    e.restart_pass();
    let mut samples = Vec::with_capacity(frames * 2);
    run(&mut e, frames, &mut samples);

    Preview {
        samples,
        capped: pass > frames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn tone(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (core::f32::consts::TAU * 220.0 * i as f32 / SR).sin())
            .collect()
    }

    fn render(bank: &ParamBank, material: &[f32], octave: f32) -> Preview {
        let arr = ParamBank::for_table(crate::arrangement::params());
        let reading = Reading {
            octave,
            ..Reading::default()
        };
        render_pass(PreviewInput {
            sample_rate: SR,
            max_grains: 256,
            bank,
            arrangement: &arr,
            player: material,
            player_reading: reading,
            grain: material,
            grain_reading: reading,
            mods: ModSet::empty(),
            max_seconds: 8.0,
        })
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn the_same_patch_draws_the_same_picture() {
        let bank = ParamBank::new();
        bank.set_by_id("grain.on", 1.0);
        let m = tone(24_000);
        assert!(render(&bank, &m, 0.0).samples == render(&bank, &m, 0.0).samples);
    }

    #[test]
    fn it_is_one_pass_at_the_materials_octave() {
        let bank = ParamBank::new();
        let m = tone(12_000);
        assert_eq!(render(&bank, &m, 0.0).samples.len(), 24_000);
        assert_eq!(render(&bank, &m, 1.0).samples.len(), 12_000);
        assert_eq!(render(&bank, &m, -1.0).samples.len(), 48_000);
    }

    #[test]
    fn a_long_pass_is_cut_and_says_so() {
        let bank = ParamBank::new();
        let p = render(&bank, &tone(48_000 * 10), 0.0);
        assert!(p.capped);
        assert_eq!(p.samples.len(), 48_000 * 8 * 2);
    }

    #[test]
    fn the_capture_does_not_glide_in_from_the_defaults() {
        // A quiet sample: from a fresh engine the gain would glide down from
        // unity over its first 40 ms, which is the attack being drawn.
        let bank = ParamBank::new();
        bank.set_by_id("material.gain", 0.2);
        let x = render(&bank, &tone(24_000), 0.0).samples;
        let early = peak(&x[2 * 480..2 * 1_200]); // 10 to 25 ms
        let later = peak(&x[2 * 4_800..2 * 5_520]); // 100 to 115 ms
        assert!(
            (early - later).abs() < later * 0.05,
            "glided in: {early} early against {later} later"
        );
    }

    #[test]
    fn a_generator_changes_the_picture() {
        let bank = ParamBank::new();
        let m = tone(24_000);
        let plain = render(&bank, &m, 0.0).samples;
        bank.set_by_id("fm.on", 1.0);
        assert!(plain != render(&bank, &m, 0.0).samples);
    }

    #[test]
    fn a_held_brake_is_not_drawn() {
        let bank = ParamBank::new();
        let m = tone(24_000);
        let plain = render(&bank, &m, 0.0).samples;
        bank.set_by_id("tape.brake", 1.0);
        assert!(plain == render(&bank, &m, 0.0).samples);
    }

    #[test]
    fn a_held_note_is_drawn_to_the_end_of_its_release() {
        let bank = ParamBank::new();
        bank.set_by_id("patch.length", 2.0);
        bank.set_by_id("patch.hold", 200.0);
        bank.set_by_id("env.release", 100.0);
        let m = tone(96_000);
        // 200 held, 100 released, 10 to fade: 310 ms, whatever the sample.
        assert_eq!(render(&bank, &m, 0.0).samples.len(), 2 * 14_880);
        assert_eq!(render(&bank, &m, 2.0).samples.len(), 2 * 14_880);
        let x = render(&bank, &m, 0.0).samples;
        assert_eq!(peak(&x[x.len() - 200..]), 0.0, "it should end in silence");
    }

    #[test]
    fn a_loop_is_drawn_for_two_seconds() {
        let bank = ParamBank::new();
        bank.set_by_id("patch.length", 0.0);
        assert_eq!(render(&bank, &tone(12_000), 0.0).samples.len(), 2 * 96_000);
    }
}
