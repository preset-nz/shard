//! Granular playback.
//!
//! A grain is a windowed slice of the sample buffer, read at its own rate from
//! its own position. A cloud of them, scheduled at a rate you control and
//! smeared by jitter, is the core of the instrument.
//!
//! The grain pool is fixed and preallocated. Nothing here allocates, locks or
//! logs, because all of it runs on the audio thread.

use std::sync::Arc;

use crate::inspect::{GrainLog, GrainSpawn};
use crate::rng::Rng;

/// How a grain fades in and out. A grain with no window clicks at both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Window {
    /// Raised cosine. The safe default, and the quietest.
    #[default]
    Hann,
    /// Linear up and down. Slightly brighter at the joins.
    Triangle,
    /// Fast attack, long decay. Reads as a percussive click per grain.
    Expodec,
    /// Long attack, fast decay. Expodec played backwards, sucks inward.
    Rexpodec,
}

impl Window {
    /// `phase` runs 0 to 1 across the grain's life.
    #[inline]
    pub fn gain(self, phase: f32) -> f32 {
        match self {
            Window::Hann => 0.5 - 0.5 * (core::f32::consts::TAU * phase).cos(),
            Window::Triangle => 1.0 - (2.0 * phase - 1.0).abs(),
            // Both exponential shapes still have to reach zero at each end or
            // they click, so they are an exponential scaled by a short fade.
            Window::Expodec => {
                let fade = (phase * 40.0).min(1.0) * ((1.0 - phase) * 8.0).min(1.0);
                (-6.0 * phase).exp() * fade
            }
            Window::Rexpodec => {
                let fade = (phase * 8.0).min(1.0) * ((1.0 - phase) * 40.0).min(1.0);
                (-6.0 * (1.0 - phase)).exp() * fade
            }
        }
    }

    pub const ALL: [Window; 4] = [
        Window::Hann,
        Window::Triangle,
        Window::Expodec,
        Window::Rexpodec,
    ];
}

/// Everything the caller can change while it plays. All plain values; the
/// smoothing and denormalising happen above this.
#[derive(Debug, Clone, Copy)]
pub struct GrainParams {
    /// Read position in the source, 0 to 1.
    pub position: f32,
    /// Random spread around `position`, 0 to 1 of the whole buffer.
    pub jitter: f32,
    /// Grain length in milliseconds. Short warbles, long smears.
    pub size_ms: f32,
    /// Grains spawned per second.
    pub density: f32,
    /// Transposition in semitones.
    pub pitch: f32,
    /// Random transposition spread, in semitones, per grain.
    pub pitch_spread: f32,
    /// Stereo spread, 0 centred to 1 fully wide.
    pub pan_spread: f32,
    /// Probability, 0 to 1, that a grain plays backwards. Not a switch.
    pub reverse: f32,
    pub window: Window,
    pub gain: f32,
    /// Tape speed: 1 normal, 0 stopped, negative backwards.
    ///
    /// It scales *tape time*, not just pitch — the scheduler, each grain's
    /// ageing and each grain's reading all run on it. That is what makes a
    /// brake sound like a reel slowing rather than a pitch knob being turned:
    /// grains stretch and thin out as they drop in pitch, and at a standstill
    /// nothing new is thrown at all.
    pub speed: f32,
}

impl Default for GrainParams {
    fn default() -> Self {
        Self {
            position: 0.25,
            jitter: 0.05,
            size_ms: 180.0,
            density: 60.0,
            pitch: 0.0,
            pitch_spread: 0.0,
            pan_spread: 0.6,
            reverse: 0.0,
            window: Window::Hann,
            gain: 1.0,
            speed: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Grain {
    active: bool,
    /// Fractional read head into the source buffer.
    pos: f32,
    /// Samples advanced per output sample. Negative plays backwards.
    rate: f32,
    age: f32,
    /// Grain length in samples. Never zero while active.
    len: f32,
    window: Window,
    left: f32,
    right: f32,
}

/// A fixed pool of grains reading one shared source buffer.
pub struct Granular {
    grains: Vec<Grain>,
    sample_rate: f32,
    /// Counts down to the next spawn, in samples. Fractional so density is
    /// not quantised to whole samples at high rates.
    next_spawn: f32,
    rng: Rng,
    /// Where the slice being read sits inside the whole source. The cloud only
    /// ever sees `source[lo..hi]`, so without this every logged position would
    /// be relative to the trim window and the marks would drift away from the
    /// waveform the moment the trim moved.
    window_offset: usize,
    window_total: usize,
    log: Option<Arc<GrainLog>>,
}

impl Granular {
    /// `max_grains` is the hard ceiling. Allocated once, here, never again.
    pub fn new(sample_rate: f32, max_grains: usize) -> Self {
        Self {
            grains: vec![Grain::default(); max_grains.max(1)],
            sample_rate,
            next_spawn: 0.0,
            rng: Rng::new(0xC0FF_EE01),
            window_offset: 0,
            window_total: 0,
            log: None,
        }
    }

    /// Record every spawn into this log. Optional: the CLI does not set one,
    /// and nothing in the audio path depends on it being there.
    pub fn set_log(&mut self, log: Arc<GrainLog>) {
        self.log = Some(log);
    }

    /// Tell the cloud where its slice sits in the whole source. Called once
    /// per block by the engine, before any grain is scheduled.
    pub fn set_source_window(&mut self, offset: usize, total: usize) {
        self.window_offset = offset;
        self.window_total = total;
    }

    pub fn active_grains(&self) -> usize {
        self.grains.iter().filter(|g| g.active).count()
    }

    /// Stop everything immediately. For patch recall and sample swaps.
    pub fn clear(&mut self) {
        for g in &mut self.grains {
            g.active = false;
        }
        self.next_spawn = 0.0;
    }

    fn spawn(&mut self, p: &GrainParams, source_len: usize) {
        let Some(slot) = self.grains.iter().position(|g| !g.active) else {
            // Pool exhausted. Dropping the grain is correct: stealing a voice
            // mid-window would click, and at this density nobody can hear one
            // missing grain.
            return;
        };

        let len = (p.size_ms * 0.001 * self.sample_rate).max(2.0);
        let semis = p.pitch + self.rng.next_bipolar() * p.pitch_spread;
        let mut rate = (semis / 12.0).exp2();

        let jitter = self.rng.next_bipolar() * p.jitter;
        let mut pos = (p.position + jitter).clamp(0.0, 1.0) * (source_len as f32 - 1.0);

        if self.rng.next_f32() < p.reverse {
            rate = -rate;
            // Start far enough in that the whole grain has buffer behind it.
            pos = (pos + len * rate.abs()).min(source_len as f32 - 1.0);
        }

        // Equal-power pan keeps loudness steady as grains scatter across the
        // field. A linear pan law would dip in the middle.
        let pan = 0.5 + self.rng.next_bipolar() * 0.5 * p.pan_spread;
        let angle = pan.clamp(0.0, 1.0) * core::f32::consts::FRAC_PI_2;

        let g = &mut self.grains[slot];
        g.active = true;
        g.pos = pos;
        g.rate = rate;
        g.age = 0.0;
        g.len = len;
        g.window = p.window;
        g.left = angle.cos();
        g.right = angle.sin();

        if let Some(log) = &self.log {
            let total = self.window_total.max(1) as f32;
            log.push(&GrainSpawn {
                position: ((self.window_offset as f32 + pos) / total).clamp(0.0, 1.0),
                rate,
                len,
                pan,
                window: Window::ALL.iter().position(|w| *w == p.window).unwrap_or(0) as u32,
                // Filled in by the log itself; the caller's value is ignored.
                seq: 0,
            });
        }
    }

    /// Activate one grain directly, bypassing the scheduler, exactly as it was
    /// logged. `position` is normalised against the **whole** source, and so
    /// is the buffer this grain will be rendered against — an audition ignores
    /// the trim, because the point is to hear the grain as it happened.
    ///
    /// Returns false when the pool is full or the grain cannot fit the buffer.
    pub fn trigger(&mut self, g: &GrainSpawn, source_len: usize) -> bool {
        if source_len < 2 {
            return false;
        }
        let Some(slot) = self.grains.iter().position(|x| !x.active) else {
            return false;
        };
        let last = source_len as f32 - 1.0;
        let angle = g.pan.clamp(0.0, 1.0) * core::f32::consts::FRAC_PI_2;

        let grain = &mut self.grains[slot];
        grain.active = true;
        grain.pos = (g.position.clamp(0.0, 1.0) * last).clamp(0.0, last);
        grain.rate = if g.rate.is_finite() && g.rate != 0.0 {
            g.rate
        } else {
            1.0
        };
        grain.age = 0.0;
        grain.len = g.len.max(2.0);
        grain.window = Window::ALL[(g.window as usize).min(Window::ALL.len() - 1)];
        grain.left = angle.cos();
        grain.right = angle.sin();
        true
    }

    /// Render one stereo sample. `source` is mono.
    #[inline]
    pub fn process(&mut self, source: &[f32], p: &GrainParams) -> (f32, f32) {
        if source.len() < 2 {
            return (0.0, 0.0);
        }

        // Schedule on tape time. At a standstill the countdown stops, so a
        // braked reel throws no new grains — the cloud freezes rather than
        // filling with grains that read a single sample and thud.
        let speed = if p.speed.is_finite() { p.speed } else { 1.0 };
        self.next_spawn -= speed.abs();
        if self.next_spawn <= 0.0 {
            let density = p.density.clamp(0.1, 2000.0);
            self.next_spawn += (self.sample_rate / density).max(1.0);
            self.spawn(p, source.len());
        }

        let (l, r) = self.render(source, speed);

        let overlap = (p.density * p.size_ms * 0.001).max(1.0);
        let comp = p.gain / overlap.sqrt();
        (l * comp, r * comp)
    }

    /// Advance every live grain by one sample and sum them. No scheduling and
    /// no overlap compensation — both belong to the cloud, not to a grain.
    #[inline]
    fn render(&mut self, source: &[f32], speed: f32) -> (f32, f32) {
        let mut l = 0.0;
        let mut r = 0.0;
        let last = source.len() - 1;

        for g in &mut self.grains {
            if !g.active {
                continue;
            }

            let phase = g.age / g.len;
            if phase >= 1.0 {
                g.active = false;
                continue;
            }

            // Linear interpolation. Audible on extreme transposition, and
            // inaudible on the drone and texture material this is built for.
            let i = g.pos as usize;
            let s = if i >= last {
                source[last]
            } else {
                let frac = g.pos - i as f32;
                source[i] + (source[i + 1] - source[i]) * frac
            };

            let amp = g.window.gain(phase);
            l += s * amp * g.left;
            r += s * amp * g.right;

            g.pos += g.rate * speed;
            // Ageing runs on tape time too, so a grain stretches as the reel
            // slows instead of ending early at a lower pitch. It also means
            // `size_ms` is tape-milliseconds, not wall-clock ones, once the
            // speed leaves unity.
            g.age += speed.abs();

            // A grain that walks off either end is done, whichever way it ran.
            if g.pos < 0.0 || g.pos > last as f32 {
                g.active = false;
            }
        }

        (l, r)
    }

    /// Render whatever grains are already live, without scheduling new ones
    /// and without overlap compensation.
    ///
    /// This is the audition path. Compensation divides by the square root of
    /// the expected overlap, which at a dense setting is a factor of six or
    /// more — applying it to a single grain would make the thing you asked to
    /// hear almost inaudible, and it would get quieter as you turned density
    /// up, which is exactly backwards for an inspector.
    #[inline]
    pub fn render_solo(&mut self, source: &[f32]) -> (f32, f32) {
        if source.len() < 2 {
            return (0.0, 0.0);
        }
        // Always at unity, whatever the tape is doing. Same reasoning as
        // skipping overlap compensation and the effects: the question is what
        // the grain sounded like, and a latched reverse would answer a
        // different one.
        self.render(source, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32 / n as f32).collect()
    }

    fn tone(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (core::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn silent_without_a_source() {
        let mut g = Granular::new(48_000.0, 16);
        let p = GrainParams::default();
        for _ in 0..1000 {
            assert_eq!(g.process(&[], &p), (0.0, 0.0));
        }
    }

    #[test]
    fn produces_sound_from_a_source() {
        let src = tone(48_000);
        let mut g = Granular::new(48_000.0, 64);
        let p = GrainParams::default();
        let mut peak: f32 = 0.0;
        for _ in 0..48_000 {
            let (l, r) = g.process(&src, &p);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak > 0.01, "expected audible output, peak was {peak}");
    }

    #[test]
    fn output_stays_finite_and_bounded() {
        let src = tone(48_000);
        let mut g = Granular::new(48_000.0, 256);
        // Deliberately brutal: maximum density, maximum spread, long grains.
        let p = GrainParams {
            density: 2000.0,
            size_ms: 500.0,
            jitter: 1.0,
            pitch_spread: 24.0,
            reverse: 0.5,
            ..Default::default()
        };
        for _ in 0..48_000 {
            let (l, r) = g.process(&src, &p);
            assert!(l.is_finite() && r.is_finite(), "non-finite output");
            assert!(l.abs() < 1000.0 && r.abs() < 1000.0, "runaway output");
        }
    }

    #[test]
    fn never_exceeds_the_pool() {
        let src = tone(48_000);
        let mut g = Granular::new(48_000.0, 8);
        let p = GrainParams {
            density: 2000.0,
            size_ms: 500.0,
            ..Default::default()
        };
        for _ in 0..10_000 {
            g.process(&src, &p);
            assert!(g.active_grains() <= 8);
        }
    }

    #[test]
    fn density_controls_grain_count() {
        let src = tone(48_000);
        let p_low = GrainParams {
            density: 5.0,
            size_ms: 200.0,
            ..Default::default()
        };
        let p_high = GrainParams {
            density: 100.0,
            ..p_low
        };

        let mut low = Granular::new(48_000.0, 256);
        let mut high = Granular::new(48_000.0, 256);
        for _ in 0..48_000 {
            low.process(&src, &p_low);
            high.process(&src, &p_high);
        }
        assert!(
            high.active_grains() > low.active_grains(),
            "high {} should exceed low {}",
            high.active_grains(),
            low.active_grains()
        );
    }

    #[test]
    fn density_does_not_double_as_a_volume_control() {
        // Overlap compensation contract: sweeping density across its range
        // must not move the output level much. Without it this is ~20 dB.
        let src = tone(48_000);
        let level = |density: f32| {
            let mut g = Granular::new(48_000.0, 512);
            let p = GrainParams {
                density,
                jitter: 0.3,
                ..Default::default()
            };
            let mut sum = 0.0f64;
            let mut n = 0u32;
            for i in 0..48_000 {
                let (l, r) = g.process(&src, &p);
                if i > 4_800 {
                    sum += (l * l + r * r) as f64;
                    n += 1;
                }
            }
            (sum / n as f64).sqrt()
        };
        let sparse = level(8.0);
        let dense = level(150.0);
        let ratio = dense / sparse;
        assert!(
            (0.4..2.5).contains(&ratio),
            "level moved {ratio:.2}x across a density sweep"
        );
    }

    #[test]
    fn clear_stops_every_grain() {
        let src = tone(48_000);
        let mut g = Granular::new(48_000.0, 64);
        let p = GrainParams::default();
        for _ in 0..24_000 {
            g.process(&src, &p);
        }
        assert!(g.active_grains() > 0);
        g.clear();
        assert_eq!(g.active_grains(), 0);
    }

    #[test]
    fn every_window_starts_and_ends_at_silence() {
        // This is the anti-click contract. A window that does not reach zero
        // at both ends puts a step into the output on every grain boundary.
        for w in Window::ALL {
            assert!(w.gain(0.0).abs() < 1e-3, "{w:?} does not start at zero");
            assert!(w.gain(1.0).abs() < 1e-3, "{w:?} does not end at zero");

            // Where the peak sits is the window's whole character, so this
            // only asserts that there is one. Hann and Triangle peak in the
            // middle; the exponential pair deliberately peak near an edge and
            // are already near-silent by halfway.
            let peak = (0..=100)
                .map(|i| w.gain(i as f32 / 100.0))
                .fold(0.0f32, f32::max);
            assert!(peak > 0.5, "{w:?} never opens up: peak {peak}");

            // And no window may exceed unity, or dense clouds clip early.
            assert!(peak <= 1.0 + 1e-4, "{w:?} exceeds unity: {peak}");
        }
    }

    #[test]
    fn reverse_grains_read_backwards_without_leaving_the_buffer() {
        let src = ramp(48_000);
        let mut g = Granular::new(48_000.0, 64);
        let p = GrainParams {
            reverse: 1.0,
            jitter: 0.0,
            position: 0.5,
            ..Default::default()
        };
        for _ in 0..48_000 {
            let (l, r) = g.process(&src, &p);
            assert!(l.is_finite() && r.is_finite());
        }
    }

    #[test]
    fn pan_spread_of_zero_is_centred() {
        let src = tone(48_000);
        let mut g = Granular::new(48_000.0, 64);
        let p = GrainParams {
            pan_spread: 0.0,
            ..Default::default()
        };
        for _ in 0..24_000 {
            let (l, r) = g.process(&src, &p);
            assert!((l - r).abs() < 1e-6, "expected centred, got {l} vs {r}");
        }
    }
}
