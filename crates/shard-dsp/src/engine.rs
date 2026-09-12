//! The engine: source buffer, granular cloud, crusher, ring modulator, out.
//!
//! Everything here runs on the audio thread. No allocation, no locks, no
//! logging, no file access. The only thing crossing in from outside is the
//! atomic parameter bank, which is read once per block.

use crate::crush::{Crush, CrushParams};
use crate::envelope::EnvParams;
use crate::granular::{GrainParams, Granular, Window};
use crate::params::{index_of, ParamBank};
use crate::player::Player;
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
    crush_bits: usize,
    crush_rate: usize,
    crush_mix: usize,
    crush_env_amount: usize,
    crush_env_attack: usize,
    crush_env_decay: usize,
    crush_env_sustain: usize,
    crush_env_release: usize,
    dry: usize,
    trim_start: usize,
    trim_end: usize,
    env_amount: usize,
    env_attack: usize,
    env_decay: usize,
    env_sustain: usize,
    env_release: usize,
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
            crush_bits: at("crush.bits"),
            crush_rate: at("crush.rate"),
            crush_mix: at("crush.mix"),
            crush_env_amount: at("crush.env.amount"),
            crush_env_attack: at("crush.env.attack"),
            crush_env_decay: at("crush.env.decay"),
            crush_env_sustain: at("crush.env.sustain"),
            crush_env_release: at("crush.env.release"),
            dry: at("mix.dry"),
            trim_start: at("trim.start"),
            trim_end: at("trim.end"),
            env_amount: at("env.amount"),
            env_attack: at("env.attack"),
            env_decay: at("env.decay"),
            env_sustain: at("env.sustain"),
            env_release: at("env.release"),
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
    dry: OnePole,
    crush_mix: OnePole,
    gain: OnePole,
}

pub struct Engine {
    source: Vec<f32>,
    player: Player,
    /// Transport. Stopped means silence out and no grains left hanging.
    playing: bool,
    granular: Granular,
    crush: Crush,
    ringmod: RingMod,
    slots: Slots,
    smooth: Smoothers,
    sample_rate: f32,
    peak: f32,
    trimmed_play_position: f32,
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
            dry: mk(defs[slots.dry].smooth_ms),
            crush_mix: mk(defs[slots.crush_mix].smooth_ms),
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
        smooth.dry.reset(defs[slots.dry].default);
        smooth.crush_mix.reset(defs[slots.crush_mix].default);
        smooth.gain.reset(defs[slots.gain].default);

        Self {
            source: Vec::new(),
            player: Player::new(sample_rate),
            playing: false,
            granular: Granular::new(sample_rate, max_grains),
            crush: Crush::new(sample_rate),
            ringmod: RingMod::new(sample_rate),
            slots,
            smooth,
            sample_rate,
            peak: 0.0,
            trimmed_play_position: 0.0,
        }
    }

    /// Replace the source material. Allocates, so call it from the loader
    /// thread before the stream starts, or hand the buffer across a queue.
    /// Never from inside `process_block`.
    pub fn set_source(&mut self, samples: Vec<f32>) {
        self.source = samples;
        self.granular.clear();
        self.player.rewind();
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Start or stop. Stopping clears the grain pool rather than letting the
    /// cloud ring out, so stop means stop.
    pub fn set_playing(&mut self, playing: bool) {
        if !playing {
            self.granular.clear();
        }
        self.playing = playing;
    }

    /// The trimmed window, as indices into the source. Always at least two
    /// samples wide, and always ordered, however the two controls are set.
    fn trim_range(&self, bank: &ParamBank) -> (usize, usize) {
        let n = self.source.len();
        if n < 2 {
            return (0, n);
        }
        let a = bank.get(self.slots.trim_start).clamp(0.0, 1.0);
        let b = bank.get(self.slots.trim_end).clamp(0.0, 1.0);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let lo = ((a * n as f32) as usize).min(n - 2);
        let hi = ((b * n as f32) as usize).clamp(lo + 2, n);
        (lo, hi)
    }

    /// Where plain playback has reached, 0 to 1 across the *whole* source, so
    /// the drawn playhead lines up with the drawn waveform rather than with
    /// the trimmed window.
    pub fn play_position(&self) -> f32 {
        self.trimmed_play_position
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
        // Bits and rate are read once per block; the mix is rebuilt per sample
        // below, because the crush envelope moves it.
        let crush_bits = bank.get(self.slots.crush_bits);
        let crush_rate = bank.get(self.slots.crush_rate);

        if !self.playing {
            out.fill(0.0);
            return;
        }

        let dry_target = bank.get(self.slots.dry);
        let env = EnvParams {
            amount: bank.get(self.slots.env_amount),
            attack_ms: bank.get(self.slots.env_attack),
            decay_ms: bank.get(self.slots.env_decay),
            sustain: bank.get(self.slots.env_sustain),
            release_ms: bank.get(self.slots.env_release),
        };

        // Trim is applied by slicing the source, so nothing downstream knows
        // it exists. `position` and the player both address the window, not
        // the file, which is what makes trimming a long sample feel like
        // loading a short one.
        let crush_mix_target = bank.get(self.slots.crush_mix);
        // The crush envelope reads the same clock as the amplitude one, so the
        // two stay in step, but it means something different. The amplitude
        // envelope multiplies the *signal*, where one is transparent; this one
        // multiplies the *mix knob*, where one is "as set" and zero is clean.
        // An attack is therefore "starts clean, then crushes", and a release
        // is the same gesture backwards.
        let crush_env = EnvParams {
            amount: bank.get(self.slots.crush_env_amount),
            attack_ms: bank.get(self.slots.crush_env_attack),
            decay_ms: bank.get(self.slots.crush_env_decay),
            sustain: bank.get(self.slots.crush_env_sustain),
            release_ms: bank.get(self.slots.crush_env_release),
        };

        let (lo, hi) = self.trim_range(bank);
        let source = &self.source[lo..hi];
        let span = self.source.len().max(1) as f32;
        let window_len = source.len() as f32;

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

            let dry_amount = self.smooth.dry.process(dry_target);
            let dry = self.player.process(source);
            let (gl, gr) = self.granular.process(source, &p);

            // Equal-power crossfade. A linear blend dips by 3 dB in the middle,
            // which reads as the instrument getting quieter as you introduce
            // grains rather than as one sound becoming another.
            let a = (dry_amount.clamp(0.0, 1.0) * core::f32::consts::FRAC_PI_2).sin();
            let b = (dry_amount.clamp(0.0, 1.0) * core::f32::consts::FRAC_PI_2).cos();
            let l = dry * a + gl * b;
            let r = dry * a + gr * b;

            // Crushed before the ring modulator, so the modulator has the
            // extra partials the crusher just generated to fold against.
            // Order stops being fixed the day the modifier stack lands.
            let elapsed = self.player.elapsed();
            let crush = CrushParams {
                bits: crush_bits,
                rate: crush_rate,
                mix: self.smooth.crush_mix.process(crush_mix_target)
                    * crush_env.gain_at(elapsed, window_len, self.sample_rate),
            };
            let (l, r) = self.crush.process(l, r, &crush);

            let (l, r) = self.ringmod.process(l, r, &ring);

            // One pass through the trimmed window is one envelope. The
            // player's position is the clock, so the release always lands on
            // the loop point whatever the sample's length.
            let e = env.gain_at(elapsed, window_len, self.sample_rate);
            let (l, r) = (l * e, r * e);

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

        self.trimmed_play_position =
            (lo as f32 + self.player.position(source.len()) * source.len() as f32) / span;
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
        e.set_playing(true);
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
        e.set_playing(true);
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
        e.set_playing(true);
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
            e.set_playing(true);
            let bank = ParamBank::new();
            // Fully granular, or the window makes no difference to the output.
            bank.set_by_id("mix.dry", 0.0);
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
        e.set_playing(true);
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
        e.set_playing(true);
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
        e.set_playing(true);
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
    fn stopped_means_silence() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        let mut out = vec![0.0; 512];
        assert!(!e.playing(), "should start stopped");
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
            assert!(out.iter().all(|s| *s == 0.0), "output while stopped");
        }
    }

    #[test]
    fn stopping_does_not_leave_the_cloud_ringing() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        bank.set_by_id("mix.dry", 0.0);
        let mut out = vec![0.0; 512];
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
        assert!(e.active_grains() > 0);
        e.set_playing(false);
        assert_eq!(e.active_grains(), 0, "stop must mean stop");
    }

    #[test]
    fn fully_dry_is_the_sample_itself() {
        // The contract the transport exists for: press play and you hear the
        // material, not a texture derived from it. Fully dry output must track
        // plain playback closely enough to be recognisably the same sound.
        let src = tone(48_000);
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(src.clone());
        e.set_playing(true);
        let bank = ParamBank::new();
        bank.set_by_id("mix.dry", 1.0);
        bank.set_by_id("amp.gain", 1.0);

        let mut player = crate::player::Player::new(48_000.0);
        let mut out = vec![0.0; 512];
        let mut worst = 0.0f32;
        for _ in 0..90 {
            e.process_block(&mut out, &bank);
            for frame in out.chunks(2) {
                let expected = player.process(&src);
                worst = worst.max((frame[0] - expected).abs());
            }
        }
        assert!(worst < 0.02, "dry path diverged from playback by {worst}");
    }

    #[test]
    fn the_dry_blend_holds_its_level() {
        // Equal-power, so introducing grains should not read as a volume dip.
        let src = tone(48_000);
        let level = |dry: f32| {
            let mut e = Engine::new(48_000.0, 256);
            e.set_source(src.clone());
            e.set_playing(true);
            let bank = ParamBank::new();
            bank.set_by_id("mix.dry", dry);
            let mut out = vec![0.0; 512];
            let mut sum = 0.0f64;
            let mut n = 0u32;
            for b in 0..200 {
                e.process_block(&mut out, &bank);
                if b > 20 {
                    for s in &out {
                        sum += (*s as f64) * (*s as f64);
                        n += 1;
                    }
                }
            }
            (sum / n as f64).sqrt()
        };
        let (a, mid, b) = (level(1.0), level(0.5), level(0.0));
        let floor = a.min(b) * 0.5;
        assert!(
            mid > floor,
            "midpoint {mid} dipped below {floor} ({a} .. {b})"
        );
    }

    #[test]
    fn trim_reads_only_inside_the_window() {
        // A ramp makes the window audible: reading the last tenth must give
        // values near one, and the first tenth values near zero.
        let n = 48_000;
        let src: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        let level = |lo: f32, hi: f32| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(src.clone());
            e.set_playing(true);
            let bank = ParamBank::new();
            bank.set_by_id("trim.start", lo);
            bank.set_by_id("trim.end", hi);
            bank.set_by_id("mix.dry", 1.0);
            bank.set_by_id("amp.gain", 1.0);
            let mut out = vec![0.0; 512];
            let mut sum = 0.0f64;
            let mut n = 0u32;
            for b in 0..200 {
                e.process_block(&mut out, &bank);
                if b > 20 {
                    for s in out.iter().step_by(2) {
                        sum += *s as f64;
                        n += 1;
                    }
                }
            }
            sum / n as f64
        };
        let early = level(0.0, 0.1);
        let late = level(0.9, 1.0);
        assert!(early < 0.2, "early window averaged {early}");
        assert!(late > 0.8, "late window averaged {late}");
    }

    #[test]
    fn trim_survives_being_set_backwards_or_to_nothing() {
        // The two controls are independent, so a user will cross them. It has
        // to keep playing rather than panic on an empty slice.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        let mut out = vec![0.0; 512];
        for (lo, hi) in [
            (0.9, 0.1),
            (0.5, 0.5),
            (1.0, 1.0),
            (0.0, 0.0),
            (0.3, 0.3001),
        ] {
            bank.set_by_id("trim.start", lo);
            bank.set_by_id("trim.end", hi);
            for _ in 0..50 {
                e.process_block(&mut out, &bank);
                for s in &out {
                    assert!(s.is_finite(), "non-finite with trim {lo}..{hi}");
                    assert!((-1.0..=1.0).contains(s));
                }
            }
        }
    }

    /// `set_by_id` returns false for an unknown id rather than panicking,
    /// which means a renamed parameter makes a test silently assert nothing.
    /// Every test write goes through here.
    fn set(bank: &ParamBank, id: &str, v: f32) {
        assert!(bank.set_by_id(id, v), "no such parameter: {id}");
    }

    #[test]
    fn the_envelope_shapes_each_pass_through_the_sample() {
        // One second of source, so one pass is one second. A 400 ms release
        // must make the last 400 ms of every pass quieter than the middle,
        // and the fade must land on the loop point rather than drift.
        let mut e = Engine::new(48_000.0, 128);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "mix.dry", 1.0);
        set(&bank, "amp.gain", 1.0);
        set(&bank, "env.attack", 0.0);
        set(&bank, "env.decay", 0.0);
        set(&bank, "env.sustain", 1.0);
        set(&bank, "env.release", 400.0);

        let mut out = vec![0.0; 512];
        let frames_per_block = 256.0;
        let mut middle = 0.0f32;
        let mut tail = 1.0f32;
        // Two full passes.
        for b in 0..380 {
            e.process_block(&mut out, &bank);
            let pos = (b as f32 * frames_per_block) % 48_000.0 / 48_000.0;
            let level = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            if (0.2..0.5).contains(&pos) {
                middle = middle.max(level);
            }
            if pos > 0.97 {
                tail = tail.min(level);
            }
        }
        assert!(middle > 0.5, "sustain stage was quiet: {middle}");
        assert!(
            tail < middle * 0.2,
            "release did not close: {tail} vs {middle}"
        );
    }

    #[test]
    fn a_neutral_envelope_changes_nothing() {
        // Defaults are no attack, no decay, full sustain, no release. An
        // untouched envelope must be exactly transparent, not nearly so.
        let src = tone(48_000);
        let render = |release: f32| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(src.clone());
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "mix.dry", 1.0);
            set(&bank, "amp.gain", 1.0);
            set(&bank, "env.release", release);
            let mut out = vec![0.0; 512];
            let mut all = Vec::new();
            for _ in 0..100 {
                e.process_block(&mut out, &bank);
                all.extend_from_slice(&out);
            }
            all
        };
        let neutral = render(0.0);
        // 400 ms release on a one-second pass starts at 600 ms, so the first
        // 200 ms are identical in both. Comparing past that compares the
        // release against nothing, which is the bug this comment prevents.
        let shaped = render(400.0);
        let identical_frames = (0.2 * 48_000.0) as usize;
        for (a, b) in neutral.iter().zip(shaped.iter()).take(identical_frames * 2) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b} before the release begins");
        }
    }

    /// Render a fixed stretch of plain playback with the crusher set however
    /// the caller likes. Dry, so the granular cloud is out of the way and any
    /// difference is the crusher's doing.
    fn render_crushed(settings: &[(&str, f32)]) -> Vec<f32> {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "mix.dry", 1.0);
        set(&bank, "amp.gain", 1.0);
        for (id, v) in settings {
            set(&bank, id, *v);
        }
        let mut out = vec![0.0; 512];
        let mut all = Vec::new();
        for _ in 0..180 {
            e.process_block(&mut out, &bank);
            all.extend_from_slice(&out);
        }
        all
    }

    #[test]
    fn a_shut_crush_mix_is_bit_exact_bypass() {
        // The null test the roadmap asks of every effect. With the mix shut,
        // bits and rate at their most destructive must make no difference at
        // all — not a small one. A near-miss would mean the crusher had put a
        // delay or a gain into the path everything else runs through.
        let neutral = render_crushed(&[]);
        let destructive = render_crushed(&[("crush.bits", 1.0), ("crush.rate", 200.0)]);
        assert_eq!(neutral.len(), destructive.len());
        for (i, (a, b)) in neutral.iter().zip(destructive.iter()).enumerate() {
            assert_eq!(a, b, "frame {i} differs with the crush mix shut");
        }
    }

    #[test]
    fn the_crush_envelope_starts_clean_and_then_crushes() {
        // The contract this feature exists for. An attack on the crush
        // envelope must leave the beginning of the pass untouched and have the
        // effect fully in by the end of it.
        let clean = render_crushed(&[]);
        let swelling = render_crushed(&[
            ("crush.bits", 1.0),
            ("crush.rate", 400.0),
            ("crush.mix", 1.0),
            ("crush.env.amount", 1.0),
            // 900 ms of attack across a one-second pass.
            ("crush.env.attack", 900.0),
        ]);

        // The first 50 ms: the envelope has barely opened, so this is still
        // the material. Not bit-exact, because the mix smoother is already
        // moving, but audibly the same sound.
        let head = (0.05 * 48_000.0) as usize * 2;
        let early = clean
            .iter()
            .zip(swelling.iter())
            .take(head)
            .fold(0.0f32, |w, (a, b)| w.max((a - b).abs()));
        assert!(early < 0.1, "the pass did not start clean: {early}");

        // The last 50 ms before the loop: fully crushed, so it must be a long
        // way from the material.
        let tail = (0.95 * 48_000.0) as usize * 2;
        let late = clean
            .iter()
            .zip(swelling.iter())
            .skip(tail)
            .take(head)
            .fold(0.0f32, |w, (a, b)| w.max((a - b).abs()));
        assert!(late > 0.3, "the crush never arrived: {late}");
    }

    #[test]
    fn the_crush_envelope_runs_independently_of_the_amplitude_one() {
        // Two envelopes, one clock, opposite gestures: the amplitude envelope
        // fades the pass out while the crush envelope fades the effect in.
        // Neither may cancel the other.
        let out = render_crushed(&[
            ("crush.bits", 2.0),
            ("crush.mix", 1.0),
            ("crush.env.attack", 900.0),
            ("env.release", 400.0),
        ]);
        assert!(out.iter().all(|s| s.is_finite()));

        // The amplitude release still closes the pass.
        let tail = (0.99 * 48_000.0) as usize * 2;
        let level = out.iter().skip(tail).fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(level < 0.1, "the amplitude release did not close: {level}");
    }

    #[test]
    fn handles_an_odd_length_output_buffer() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        let mut out = vec![0.0; 511];
        e.process_block(&mut out, &bank);
    }
}
