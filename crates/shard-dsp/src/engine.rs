//! The engine: source buffer, granular cloud, ring modulator, out.
//!
//! Everything here runs on the audio thread. No allocation, no locks, no
//! logging, no file access. The only thing crossing in from outside is the
//! atomic parameter bank, which is read once per block.

use crate::granular::{GrainParams, Granular, Window};
use crate::params::{index_of, ParamBank};
use crate::ringmod::{RingMod, RingModParams};
use crate::smooth::OnePole;

/// Resolved indices into the parameter table, looked up once at construction
/// so the audio thread never does a string comparison.
struct Slots {
    position: usize,
    jitter: usize,
    size: usize,
    density: usize,
    pitch: usize,
    spread: usize,
    pan: usize,
    reverse: usize,
    window: usize,
    ring_freq: usize,
    ring_mix: usize,
    gain: usize,
}

impl Slots {
    /// Panics on a missing id. That is correct: the table is a compile-time
    /// constant, so a miss here is a programming error, not a runtime one,
    /// and it fails on the first construction rather than silently going
    /// quiet on the audio thread.
    fn resolve() -> Self {
        let at = |id: &str| index_of(id).unwrap_or_else(|| panic!("missing parameter: {id}"));
        Self {
            position: at("grain.position"),
            jitter: at("grain.jitter"),
            size: at("grain.size"),
            density: at("grain.density"),
            pitch: at("grain.pitch"),
            spread: at("grain.spread"),
            pan: at("grain.pan"),
            reverse: at("grain.reverse"),
            window: at("grain.window"),
            ring_freq: at("ring.freq"),
            ring_mix: at("ring.mix"),
            gain: at("amp.gain"),
        }
    }
}

/// Smoothers for the continuous parameters. Stepped ones are read raw.
struct Smoothers {
    position: OnePole,
    jitter: OnePole,
    size: OnePole,
    density: OnePole,
    pitch: OnePole,
    spread: OnePole,
    pan: OnePole,
    gain: OnePole,
}

pub struct Engine {
    source: Vec<f32>,
    granular: Granular,
    ringmod: RingMod,
    slots: Slots,
    smooth: Smoothers,
    sample_rate: f32,
    peak: f32,
}

impl Engine {
    pub fn new(sample_rate: f32, max_grains: usize) -> Self {
        let slots = Slots::resolve();
        let mk = |ms: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p
        };
        let defs = crate::params::PARAMS;
        let mut smooth = Smoothers {
            position: mk(defs[slots.position].smooth_ms),
            jitter: mk(defs[slots.jitter].smooth_ms),
            size: mk(defs[slots.size].smooth_ms),
            density: mk(defs[slots.density].smooth_ms),
            pitch: mk(defs[slots.pitch].smooth_ms),
            spread: mk(defs[slots.spread].smooth_ms),
            pan: mk(defs[slots.pan].smooth_ms),
            gain: mk(defs[slots.gain].smooth_ms),
        };
        // Start settled at the defaults, otherwise every parameter glides up
        // from zero for the first few milliseconds after load.
        smooth.position.reset(defs[slots.position].default);
        smooth.jitter.reset(defs[slots.jitter].default);
        smooth.size.reset(defs[slots.size].default);
        smooth.density.reset(defs[slots.density].default);
        smooth.pitch.reset(defs[slots.pitch].default);
        smooth.spread.reset(defs[slots.spread].default);
        smooth.pan.reset(defs[slots.pan].default);
        smooth.gain.reset(defs[slots.gain].default);

        Self {
            source: Vec::new(),
            granular: Granular::new(sample_rate, max_grains),
            ringmod: RingMod::new(sample_rate),
            slots,
            smooth,
            sample_rate,
            peak: 0.0,
        }
    }

    /// Replace the source material. Allocates, so call it from the loader
    /// thread before the stream starts, or hand the buffer across a queue.
    /// Never from inside `process_block`.
    pub fn set_source(&mut self, samples: Vec<f32>) {
        self.source = samples;
        self.granular.clear();
    }

    pub fn source_len(&self) -> usize {
        self.source.len()
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn active_grains(&self) -> usize {
        self.granular.active_grains()
    }

    /// Peak since the last call, then reset. Cheap enough to poll at 30 Hz
    /// for a meter.
    pub fn take_peak(&mut self) -> f32 {
        core::mem::take(&mut self.peak)
    }

    /// Render interleaved stereo into `out`, which must have an even length.
    pub fn process_block(&mut self, out: &mut [f32], bank: &ParamBank) {
        // Read the bank once per block, not once per sample. The smoothers
        // handle the step between blocks.
        let target = GrainParams {
            position: bank.get(self.slots.position),
            jitter: bank.get(self.slots.jitter),
            size_ms: bank.get(self.slots.size),
            density: bank.get(self.slots.density),
            pitch: bank.get(self.slots.pitch),
            pitch_spread: bank.get(self.slots.spread),
            pan_spread: bank.get(self.slots.pan),
            reverse: bank.get(self.slots.reverse),
            window: window_from(bank.get(self.slots.window)),
            gain: bank.get(self.slots.gain),
        };
        let ring = RingModParams {
            freq: bank.get(self.slots.ring_freq),
            mix: bank.get(self.slots.ring_mix),
        };

        let mut p = target;
        for frame in out.chunks_mut(2) {
            p.position = self.smooth.position.process(target.position);
            p.jitter = self.smooth.jitter.process(target.jitter);
            p.size_ms = self.smooth.size.process(target.size_ms);
            p.density = self.smooth.density.process(target.density);
            p.pitch = self.smooth.pitch.process(target.pitch);
            p.pitch_spread = self.smooth.spread.process(target.pitch_spread);
            p.pan_spread = self.smooth.pan.process(target.pan_spread);
            p.gain = self.smooth.gain.process(target.gain);

            let (l, r) = self.granular.process(&self.source, &p);
            let (l, r) = self.ringmod.process(l, r, &ring);

            // A safety clip, not a limiter. Dense clouds sum above unity and
            // a hard clip is preferable to handing the device something that
            // wraps. If this engages often, lower the gain.
            let l = l.clamp(-1.0, 1.0);
            let r = r.clamp(-1.0, 1.0);

            self.peak = self.peak.max(l.abs()).max(r.abs());

            if let [lo, ro] = frame {
                *lo = l;
                *ro = r;
            }
        }
    }
}

fn window_from(v: f32) -> Window {
    Window::ALL[(v.round().max(0.0) as usize).min(Window::ALL.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (core::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn silent_with_no_source_loaded() {
        let mut e = Engine::new(48_000.0, 64);
        let bank = ParamBank::new();
        let mut out = vec![0.0; 512];
        for _ in 0..100 {
            e.process_block(&mut out, &bank);
            assert!(out.iter().all(|s| *s == 0.0));
        }
    }

    #[test]
    fn makes_sound_once_a_source_is_loaded() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        let mut out = vec![0.0; 512];
        let mut peak: f32 = 0.0;
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
            peak = peak.max(out.iter().fold(0.0f32, |a, s| a.max(s.abs())));
        }
        assert!(peak > 0.01, "peak was {peak}");
    }

    #[test]
    fn output_never_leaves_the_valid_range() {
        let mut e = Engine::new(48_000.0, 256);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        bank.set_by_id("grain.density", 200.0);
        bank.set_by_id("grain.size", 500.0);
        bank.set_by_id("amp.gain", 1.5);
        bank.set_by_id("ring.mix", 1.0);
        let mut out = vec![0.0; 512];
        for _ in 0..500 {
            e.process_block(&mut out, &bank);
            for s in &out {
                assert!(s.is_finite(), "non-finite sample");
                assert!((-1.0..=1.0).contains(s), "sample out of range: {s}");
            }
        }
    }

    #[test]
    fn every_window_setting_renders() {
        for step in 0..4 {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(48_000));
            let bank = ParamBank::new();
            bank.set_by_id("grain.window", step as f32);
            let mut out = vec![0.0; 512];
            let mut peak: f32 = 0.0;
            for _ in 0..200 {
                e.process_block(&mut out, &bank);
                peak = peak.max(out.iter().fold(0.0f32, |a, s| a.max(s.abs())));
            }
            assert!(peak > 0.001, "window {step} produced nothing");
        }
    }

    #[test]
    fn a_parameter_sweep_produces_no_discontinuity() {
        // The anti-zipper contract. Jerking position across its whole range
        // must not put a step into the output.
        let mut e = Engine::new(48_000.0, 128);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        bank.set_by_id("grain.density", 60.0);
        let mut out = vec![0.0; 256];
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for block in 0..200 {
            bank.set_by_id("grain.position", if block % 2 == 0 { 0.0 } else { 1.0 });
            e.process_block(&mut out, &bank);
            for s in out.iter().step_by(2) {
                worst = worst.max((s - prev).abs());
                prev = *s;
            }
        }
        // Grain onsets are legitimate transients, so this is a sanity ceiling
        // rather than a tight bound. Without smoothing it exceeds 1.0.
        assert!(worst < 0.9, "worst sample-to-sample jump was {worst}");
    }

    #[test]
    fn peak_meter_reports_then_resets() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        let mut out = vec![0.0; 512];
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
        assert!(e.take_peak() > 0.0);
        assert_eq!(e.take_peak(), 0.0, "peak should reset after reading");
    }

    #[test]
    fn swapping_the_source_does_not_leave_stale_grains() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        let mut out = vec![0.0; 512];
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
        assert!(e.active_grains() > 0);
        e.set_source(tone(1_000));
        assert_eq!(e.active_grains(), 0);
        // And it keeps running against the shorter buffer without panicking.
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
    }

    #[test]
    fn handles_an_odd_length_output_buffer() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        let mut out = vec![0.0; 511];
        e.process_block(&mut out, &bank);
    }
}
