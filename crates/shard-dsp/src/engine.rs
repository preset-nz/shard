//! The engine: source buffer, granular cloud, crusher, ring modulator, filter, out.
//!
//! Everything here runs on the audio thread. No allocation, no locks, no
//! logging, no file access. The only thing crossing in from outside is the
//! atomic parameter bank, which is read once per block.

use std::sync::Arc;

use crate::crush::{Crush, CrushParams};
use crate::envelope::EnvParams;
use crate::filter::{Filter, FilterParams, FilterType};
use crate::granular::{GrainParams, Granular, Window};
use crate::inspect::{GrainLog, GrainSpawn};
use crate::modulation::ModSet;
use crate::params::{index_of, ParamBank};
use crate::player::Player;
use crate::ringmod::{RingMod, RingModParams};
use crate::smooth::{OnePole, Ramp};

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
    octave: usize,
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
    grain_gain: usize,
    material_on: usize,
    material_gain: usize,
    grain_on: usize,
    crush_on: usize,
    ring_on: usize,
    filter_on: usize,
    filter_mix: usize,
    filter_cutoff: usize,
    filter_resonance: usize,
    filter_type: usize,
    env_on: usize,
    trim_start: usize,
    trim_end: usize,
    env_mix: usize,
    env_attack: usize,
    env_decay: usize,
    env_sustain: usize,
    env_release: usize,
    tape_brake: usize,
    tape_time: usize,
    tape_reverse: usize,
    tape_flick: usize,
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
            octave: at("material.octave"),
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
            grain_gain: at("grain.gain"),
            material_on: at("material.on"),
            material_gain: at("material.gain"),
            grain_on: at("grain.on"),
            crush_on: at("crush.on"),
            ring_on: at("ring.on"),
            filter_on: at("filter.on"),
            filter_mix: at("filter.mix"),
            filter_cutoff: at("filter.cutoff"),
            filter_resonance: at("filter.resonance"),
            filter_type: at("filter.type"),
            env_on: at("env.on"),
            trim_start: at("trim.start"),
            trim_end: at("trim.end"),
            env_mix: at("env.mix"),
            env_attack: at("env.attack"),
            env_decay: at("env.decay"),
            env_sustain: at("env.sustain"),
            env_release: at("env.release"),
            tape_brake: at("tape.brake"),
            tape_time: at("tape.time"),
            tape_reverse: at("tape.reverse"),
            tape_flick: at("tape.flick"),
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
    grain_gain: OnePole,
    material_gain: OnePole,
    crush_mix: OnePole,
    gain: OnePole,
    /// The tape's own inertia. Its time constant is `tape.time`, reset per
    /// block, so the brake's feel is a control rather than a constant.
    speed: OnePole,
}

/// How long a section switch takes. Short enough to feel instant, long enough
/// that cutting a loud section in or out never clicks.
const BYPASS_MS: f32 = 10.0;

/// One ramp per switchable section. Each multiplies its section's mix and
/// lands on an exact zero, so a section that is off is bit-exact with its mix
/// at zero, and the cloud can be skipped outright.
struct Fades {
    material: Ramp,
    grain: Ramp,
    crush: Ramp,
    ring: Ramp,
    env: Ramp,
    filter: Ramp,
}

impl Fades {
    fn new(sample_rate: f32, slots: &Slots) -> Self {
        let defs = crate::params::PARAMS;
        // Settled at each switch's own default, so nothing fades in or out
        // on launch. Effects start off and the envelope starts on.
        let ramp = |slot: usize| {
            let mut r = Ramp::new(defs[slot].default);
            r.set_time(BYPASS_MS, sample_rate);
            r
        };
        Self {
            material: ramp(slots.material_on),
            grain: ramp(slots.grain_on),
            crush: ramp(slots.crush_on),
            ring: ramp(slots.ring_on),
            env: ramp(slots.env_on),
            filter: ramp(slots.filter_on),
        }
    }
}

pub struct Engine {
    source: Vec<f32>,
    player: Player,
    /// Transport. Stopped means silence out and no grains left hanging.
    playing: bool,
    granular: Granular,
    /// Every spawn the cloud makes, for the inspector. Shared out by
    /// `grain_log`; the audio thread only ever pushes to it.
    log: Arc<GrainLog>,
    /// Playing one logged grain on its own, with the transport stopped.
    auditioning: bool,
    /// Reverse, resolved from the gate. See `resolve_reverse`.
    reversing: bool,
    /// Samples since reverse engaged, for the flick's minimum on-time.
    reverse_age: f32,
    crush: Crush,
    ringmod: RingMod,
    filter: Filter,
    slots: Slots,
    smooth: Smoothers,
    fades: Fades,
    /// LFOs and links. Swapped in whole by `set_modulation`; read through by
    /// every parameter the engine uses.
    mods: ModSet,
    sample_rate: f32,
    peak: f32,
    trimmed_play_position: f32,
}

impl Engine {
    pub fn new(sample_rate: f32, max_grains: usize) -> Self {
        let slots = Slots::resolve();
        let fades = Fades::new(sample_rate, &slots);
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
            grain_gain: mk(defs[slots.grain_gain].smooth_ms),
            material_gain: mk(defs[slots.material_gain].smooth_ms),
            crush_mix: mk(defs[slots.crush_mix].smooth_ms),
            gain: mk(defs[slots.gain].smooth_ms),
            speed: mk(defs[slots.tape_time].default),
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
        smooth.grain_gain.reset(defs[slots.grain_gain].default);
        smooth
            .material_gain
            .reset(defs[slots.material_gain].default);
        smooth.crush_mix.reset(defs[slots.crush_mix].default);
        smooth.gain.reset(defs[slots.gain].default);
        // Unity, not a parameter default. Copying the line above would reset
        // the tape to `tape.brake`'s default of zero and pitch the instrument
        // up from a standstill on every launch.
        smooth.speed.reset(1.0);

        let log = Arc::new(GrainLog::new());
        let mut granular = Granular::new(sample_rate, max_grains);
        granular.set_log(Arc::clone(&log));

        Self {
            source: Vec::new(),
            player: Player::new(sample_rate),
            playing: false,
            granular,
            log,
            auditioning: false,
            reversing: false,
            reverse_age: 0.0,
            crush: Crush::new(sample_rate),
            ringmod: RingMod::new(sample_rate),
            filter: Filter::new(sample_rate),
            slots,
            smooth,
            fades,
            mods: ModSet::empty(),
            sample_rate,
            peak: 0.0,
            trimmed_play_position: 0.0,
        }
    }

    /// Replace the source material and hand back the old buffer.
    ///
    /// Safe on the audio thread at a block boundary, because nothing here
    /// touches the allocator: the new buffer arrives already built, and the
    /// old one is returned rather than dropped. Freeing it takes the
    /// allocator's lock just as building it did, so the caller passes it to a
    /// thread that is allowed to block. Never from inside `process_block`.
    pub fn set_source(&mut self, samples: Vec<f32>) -> Vec<f32> {
        let old = std::mem::replace(&mut self.source, samples);
        self.granular.clear();
        self.auditioning = false;
        self.player.rewind();
        old
    }

    /// Swap in a new set of LFOs and links, and hand back the old one.
    ///
    /// The same contract as `set_source`: built on the command thread, swapped
    /// at a block boundary, and the old set dropped anywhere but here. LFOs in
    /// both sets keep running, matched by id, so adding an LFO or changing a
    /// rate never restarts the others.
    pub fn set_modulation(&mut self, mut next: ModSet) -> ModSet {
        next.inherit(&self.mods);
        std::mem::replace(&mut self.mods, next)
    }

    /// Every parameter as the engine hears it, LFOs included, written into
    /// `heard`, so the UI can draw where a linked control has been moved to.
    /// The hand's `bank` is only read. Atomic stores only, so it is safe to
    /// call from the audio callback once a block.
    pub fn publish_heard(&self, bank: &ParamBank, heard: &ParamBank) {
        for slot in 0..crate::params::PARAMS.len() {
            heard.set(slot, read(&self.mods, bank, slot));
        }
    }

    /// The spawn log, for whatever wants to draw it. Cloning the handle is the
    /// only way out of the audio thread; nothing here hands out a `&mut`.
    pub fn grain_log(&self) -> Arc<GrainLog> {
        Arc::clone(&self.log)
    }

    /// Play one logged grain on its own.
    ///
    /// Only while stopped, which is the point — a grain auditioned into a live
    /// cloud is a grain you cannot hear. It reads the **whole** source rather
    /// than the trimmed window, because the logged position is absolute and
    /// the grain should sound as it did when it happened.
    ///
    /// Returns false if the transport is running or the pool is full.
    pub fn audition(&mut self, spawn: &GrainSpawn) -> bool {
        if self.playing {
            return false;
        }
        self.granular.clear();
        self.auditioning = self.granular.trigger(spawn, self.source.len());
        self.auditioning
    }

    pub fn auditioning(&self) -> bool {
        self.auditioning
    }

    /// Whether the tape is running backwards, which is not the same as whether
    /// the reverse gate is held. For the UI, so the button can stay lit for as
    /// long as the reel is actually turning the other way.
    pub fn reversing(&self) -> bool {
        self.reversing
    }

    /// Turn the reverse *gate* into a direction, once per block.
    ///
    /// A tap and a hold are the same gesture held for different lengths, so
    /// one rule covers both: reverse engages when the gate rises, and stays
    /// engaged until the gate is low **and** it has run for at least `flick`.
    /// Tap and you get a flick of a set length whatever your finger did; hold
    /// and it stays until you let go.
    ///
    /// This lives here rather than in the button so that a MIDI note, a pad or
    /// a footswitch produces exactly the same gesture with no code of its own.
    fn resolve_reverse(&mut self, gate: bool, flick_samples: f32, frames: f32) {
        if gate && !self.reversing {
            self.reversing = true;
            self.reverse_age = 0.0;
        }
        if self.reversing {
            self.reverse_age += frames;
            if !gate && self.reverse_age >= flick_samples {
                self.reversing = false;
            }
        }
    }

    /// One grain, windowed and panned as logged, with no cloud around it and
    /// no overlap compensation. Deliberately bypasses the crusher, the ring
    /// modulator and the amplitude envelope: the question an audition answers
    /// is what the *grain* sounded like, not what the patch did to it.
    fn render_audition(&mut self, out: &mut [f32], bank: &ParamBank) {
        // The grain as it was: the cloud's level and the master gain as the
        // hand set them, unmodulated.
        let gain = bank.get(self.slots.grain_gain) * bank.get(self.slots.gain);
        // Split the borrows by field so the grain pool can be advanced while
        // the source is read.
        let granular = &mut self.granular;
        let source = &self.source;
        let mut peak = self.peak;

        for frame in out.chunks_mut(2) {
            let (l, r) = granular.render_solo(source);
            let l = (l * gain).clamp(-1.0, 1.0);
            let r = (r * gain).clamp(-1.0, 1.0);
            peak = peak.max(l.abs()).max(r.abs());
            if let [lo, ro] = frame {
                *lo = l;
                *ro = r;
            }
        }

        self.peak = peak;
        if granular.active_grains() == 0 {
            self.auditioning = false;
        }
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
        // Either direction cancels an audition: starting drowns it, stopping
        // has just cleared the pool out from under it.
        self.auditioning = false;
        // A flick left mid-flight when the transport stopped would still be
        // engaged when it started again, which is a surprise rather than a
        // gesture.
        self.reversing = false;
        self.reverse_age = 0.0;
        self.playing = playing;
    }

    /// The trimmed window, as indices into the source. Always at least two
    /// samples wide, and always ordered, however the two controls are set.
    fn trim_range(&self, bank: &ParamBank) -> (usize, usize) {
        let n = self.source.len();
        if n < 2 {
            return (0, n);
        }
        let a = read(&self.mods, bank, self.slots.trim_start).clamp(0.0, 1.0);
        let b = read(&self.mods, bank, self.slots.trim_end).clamp(0.0, 1.0);
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
        // LFOs move with the transport, and once per block, before anything
        // below reads a parameter through them.
        if self.playing {
            self.mods.advance(out.len() / 2, self.sample_rate);
        }
        // Read the bank once per block, not once per sample. The smoothers
        // handle the step between blocks.
        let target = GrainParams {
            position: read(&self.mods, bank, self.slots.position),
            jitter: read(&self.mods, bank, self.slots.jitter),
            size_ms: read(&self.mods, bank, self.slots.size),
            density: read(&self.mods, bank, self.slots.density),
            pitch: read(&self.mods, bank, self.slots.pitch),
            pitch_spread: read(&self.mods, bank, self.slots.spread),
            pan_spread: read(&self.mods, bank, self.slots.pan),
            reverse: read(&self.mods, bank, self.slots.reverse),
            window: window_from(read(&self.mods, bank, self.slots.window)),
            // The cloud's own level; the master gain comes after everything.
            gain: read(&self.mods, bank, self.slots.grain_gain),
            // Set per sample below; this is only the struct's starting shape.
            speed: 1.0,
        };
        let ring = RingModParams {
            freq: read(&self.mods, bank, self.slots.ring_freq),
            mix: read(&self.mods, bank, self.slots.ring_mix),
        };
        // Bits and rate are read once per block; the mix is rebuilt per sample
        // below, because the crush envelope moves it.
        let crush_bits = read(&self.mods, bank, self.slots.crush_bits);
        let crush_rate = read(&self.mods, bank, self.slots.crush_rate);

        if !self.playing {
            if self.auditioning {
                self.render_audition(out, bank);
            } else {
                out.fill(0.0);
            }
            return;
        }

        let material_gain_target = read(&self.mods, bank, self.slots.material_gain);
        let master_gain_target = read(&self.mods, bank, self.slots.gain);
        // Section switches, read as gates. Each one fades its section's mix
        // rather than cutting it.
        let gate = |slot: usize| if bank.get(slot) >= 0.5 { 1.0 } else { 0.0 };
        let grain_on_target = gate(self.slots.grain_on);
        let material_on_target = gate(self.slots.material_on);
        let crush_on_target = gate(self.slots.crush_on);
        let ring_on_target = gate(self.slots.ring_on);
        let env_on_target = gate(self.slots.env_on);
        let filter_on_target = gate(self.slots.filter_on);
        let filter = FilterParams {
            cutoff_hz: read(&self.mods, bank, self.slots.filter_cutoff),
            resonance: read(&self.mods, bank, self.slots.filter_resonance),
            kind: FilterType::from_value(bank.get(self.slots.filter_type)),
            mix: read(&self.mods, bank, self.slots.filter_mix),
        };
        let env = EnvParams {
            amount: read(&self.mods, bank, self.slots.env_mix),
            attack_ms: read(&self.mods, bank, self.slots.env_attack),
            decay_ms: read(&self.mods, bank, self.slots.env_decay),
            sustain: read(&self.mods, bank, self.slots.env_sustain),
            release_ms: read(&self.mods, bank, self.slots.env_release),
        };

        // Trim is applied by slicing the source, so nothing downstream knows
        // it exists. `position` and the player both address the window, not
        // the file, which is what makes trimming a long sample feel like
        // loading a short one.
        let crush_mix_target = read(&self.mods, bank, self.slots.crush_mix);
        // The crush envelope reads the same clock as the amplitude one, so the
        // two stay in step, but it means something different. The amplitude
        // envelope multiplies the *signal*, where one is transparent; this one
        // multiplies the *mix knob*, where one is "as set" and zero is clean.
        // An attack is therefore "starts clean, then crushes", and a release
        // is the same gesture backwards.
        let crush_env = EnvParams {
            amount: read(&self.mods, bank, self.slots.crush_env_amount),
            attack_ms: read(&self.mods, bank, self.slots.crush_env_attack),
            decay_ms: read(&self.mods, bank, self.slots.crush_env_decay),
            sustain: read(&self.mods, bank, self.slots.crush_env_sustain),
            release_ms: read(&self.mods, bank, self.slots.crush_env_release),
        };

        // The tape. Brake and reverse are two ways of asking for a speed, and
        // they share one slew — so flipping direction slows to a stop and
        // climbs back the other way, exactly as a reel does. An instant flip
        // would be one branch here, and would not sound like tape.
        self.resolve_reverse(
            read(&self.mods, bank, self.slots.tape_reverse) >= 0.5,
            read(&self.mods, bank, self.slots.tape_flick).max(0.0) * 0.001 * self.sample_rate,
            (out.len() / 2) as f32,
        );
        let direction = if self.reversing { -1.0 } else { 1.0 };
        let brake = read(&self.mods, bank, self.slots.tape_brake).clamp(0.0, 1.0);
        let speed_target = direction * (1.0 - brake);
        self.smooth.speed.set_time(
            read(&self.mods, bank, self.slots.tape_time).max(0.0),
            self.sample_rate,
        );

        // Octave is a pre-condition on the material, not a property of the
        // tape: it changes how fast the source is *read*, and nothing else.
        // The player reads 2ⁿ faster and each grain is transposed by n octaves
        // at spawn, but the grain scheduler and grain ageing stay on tape time
        // — so density stays in hertz and a grain keeps its length, where a
        // tape at double speed would throw twice as many half-length grains.
        //
        // Varispeed, so an octave up also halves the pass. The envelopes run
        // on the player's position, so they follow the sample, not the clock:
        // an attack drawn over the first bar of the waveform stays there.
        let octave = read(&self.mods, bank, self.slots.octave)
            .round()
            .clamp(-2.0, 2.0);
        let read_ratio = octave.exp2();
        let octave_semis = 12.0 * octave;

        let (lo, hi) = self.trim_range(bank);
        // The cloud only sees the slice, so it has to be told where the slice
        // is before it logs a spawn — otherwise every drawn mark would sit at
        // the wrong place the moment the trim moved off zero.
        self.granular.set_source_window(lo, self.source.len());
        let source = &self.source[lo..hi];
        let span = self.source.len().max(1) as f32;
        let window_len = source.len() as f32;

        let mut p = target;
        for frame in out.chunks_mut(2) {
            p.position = self.smooth.position.process(target.position);
            p.jitter = self.smooth.jitter.process(target.jitter);
            p.size_ms = self.smooth.size.process(target.size_ms);
            p.density = self.smooth.density.process(target.density);
            // Added after smoothing: the octave is stepped and lands at once.
            p.pitch = self.smooth.pitch.process(target.pitch) + octave_semis;
            p.pitch_spread = self.smooth.spread.process(target.pitch_spread);
            p.pan_spread = self.smooth.pan.process(target.pan_spread);
            p.speed = self.smooth.speed.process(speed_target);

            // Two generators, summed (Georg, 2026-09-13). Each has a switch
            // that fades to an exact zero and a gain; on at unity is bit-exact.
            let grain_on = self.fades.grain.process(grain_on_target);
            // The cloud's level is its gain times its switch, applied inside
            // the cloud together with its overlap compensation.
            p.gain = self.smooth.grain_gain.process(target.gain) * grain_on;
            let material_level = self.smooth.material_gain.process(material_gain_target)
                * self.fades.material.process(material_on_target);
            // The player runs whether the material is switched on or not: the
            // playhead and both envelopes are clocked by it. The octave scales
            // the read, never `p.speed` itself: the tape's level fade below
            // reads speed alone, and an octave down must not sound like a reel
            // slowing to a stop.
            let material = self.player.process(source, p.speed * read_ratio) * material_level;
            // Off means off. Once the fade has landed the cloud's contribution
            // is already an exact zero, so it is not run at all.
            let (gl, gr) = if grain_on > 0.0 {
                self.granular.process(source, &p)
            } else {
                (0.0, 0.0)
            };
            let l = material + gl;
            let r = material + gr;

            // Crushed before the ring modulator, so the modulator has the
            // extra partials the crusher just generated to fold against.
            // Order stops being fixed the day the modifier stack lands.
            let elapsed = self.player.elapsed();
            let crush = CrushParams {
                bits: crush_bits,
                rate: crush_rate,
                // Switched off, the mix lands on an exact zero, which the
                // crusher already treats as a true bypass.
                mix: self.smooth.crush_mix.process(crush_mix_target)
                    * crush_env.gain_at(elapsed, window_len, self.sample_rate)
                    * self.fades.crush.process(crush_on_target),
            };
            let (l, r) = self.crush.process(l, r, &crush);

            let ring_on = self.fades.ring.process(ring_on_target);
            let faded_ring = RingModParams {
                mix: ring.mix * ring_on,
                ..ring
            };
            let (l, r) = self.ringmod.process(l, r, &faded_ring);

            // A stopping reel loses level as well as pitch, because the head
            // stops seeing tape. Without this the last of the brake is a cloud
            // of grains each reading one frozen sample — a thud per grain, and
            // a buzz at the spawn rate rather than a fade into silence.
            let t = tape_gain(p.speed);
            let (l, r) = (l * t, r * t);

            // One pass through the trimmed window is one envelope. The
            // player's position is the clock, so the release always lands on
            // the loop point whatever the sample's length.
            // Switched off, the amount lands on zero, and a zero-amount
            // envelope is neutral: exactly one, not nearly.
            let faded_env = EnvParams {
                amount: env.amount * self.fades.env.process(env_on_target),
                ..env
            };
            let e = faded_env.gain_at(elapsed, window_len, self.sample_rate);
            let (l, r) = (l * e, r * e);

            // The filter, on the master: after every generator and effect,
            // before the gain. Switched off, its mix lands on an exact zero.
            let filter_on = self.fades.filter.process(filter_on_target);
            let faded_filter = FilterParams {
                mix: filter.mix * filter_on,
                ..filter
            };
            let (l, r) = self.filter.process(l, r, &faded_filter);

            // The master gain, after every generator and effect. Unity is
            // exact, since the smoother starts and rests on it.
            let g = self.smooth.gain.process(master_gain_target);
            let (l, r) = (l * g, r * g);

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

        // A cloud switched off is cleared once its fade has landed, so turning
        // it back on starts fresh rather than resuming grains frozen mid-window.
        if self.fades.grain.value() == 0.0 {
            self.granular.clear();
        }
    }
}

/// How far into a stop the level starts following the speed down.
///
/// Only the last sliver, so the brake reads as a slowing reel for almost all
/// of its travel and only fades at the very end.
const TAPE_KNEE: f32 = 0.12;

/// Output level for a tape running at `speed`.
///
/// Returns exactly one above the knee — not a curve that merely approaches it.
/// Every other node here is bit-exact when neutral, and a gain of 0.9997 at
/// rest would break that quietly, in a way no existing test would catch.
fn tape_gain(speed: f32) -> f32 {
    let s = speed.abs();
    if s >= TAPE_KNEE {
        1.0
    } else {
        s / TAPE_KNEE
    }
}

/// A parameter as the engine should hear it: the hand's value from the bank,
/// bent by its link to an LFO if it has one. The bank itself is never written.
#[inline]
fn read(mods: &ModSet, bank: &ParamBank, slot: usize) -> f32 {
    mods.apply(slot, bank.get(slot))
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
        // Every effect in, or the range check is only checking the dry path.
        bank.set_by_id("ring.on", 1.0);
        bank.set_by_id("crush.on", 1.0);
        bank.set_by_id("grain.on", 1.0);
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
            bank.set_by_id("material.on", 0.0);
            bank.set_by_id("grain.on", 1.0);
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
        // Grains audible, or jerking their position moves nothing you hear.
        bank.set_by_id("grain.on", 1.0);
        bank.set_by_id("material.on", 0.0);
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
        set(&bank, "grain.on", 1.0);
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
        bank.set_by_id("grain.on", 1.0);
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
        bank.set_by_id("grain.on", 0.0);
        bank.set_by_id("amp.gain", 1.0);

        let mut player = crate::player::Player::new(48_000.0);
        let mut out = vec![0.0; 512];
        let mut worst = 0.0f32;
        for _ in 0..90 {
            e.process_block(&mut out, &bank);
            for frame in out.chunks(2) {
                let expected = player.process(&src, 1.0);
                worst = worst.max((frame[0] - expected).abs());
            }
        }
        assert!(worst < 0.02, "dry path diverged from playback by {worst}");
    }

    #[test]
    fn master_gain_scales_the_plain_sample_too() {
        // The bug this fixed: Output Gain used to scale only the grain cloud,
        // so with granular off, which is how Shard starts, it did nothing.
        let render = |gain: f32| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "amp.gain", gain);
            let mut out = vec![0.0; 512];
            let mut tail = Vec::new();
            for block in 0..200 {
                e.process_block(&mut out, &bank);
                if block >= 100 {
                    tail.extend_from_slice(&out);
                }
            }
            tail
        };
        let unity = render(1.0);
        let half = render(0.5);
        assert!(
            unity.iter().any(|s| s.abs() > 0.5),
            "the sample was not heard"
        );
        for (h, u) in half.iter().zip(&unity) {
            assert!(
                (h - u * 0.5).abs() < 1e-3,
                "master gain 0.5 gave {h} for {u}"
            );
        }
    }

    #[test]
    fn the_generators_sum_and_either_can_stand_alone() {
        // Material alone, the cloud alone, and both: both is the other two
        // added, sample for sample, because nothing between them mixes.
        let render = |material: bool, grains: bool| {
            let mut e = Engine::new(48_000.0, 128);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "material.on", if material { 1.0 } else { 0.0 });
            set(&bank, "grain.on", if grains { 1.0 } else { 0.0 });
            set(&bank, "grain.gain", 0.5);
            let mut out = vec![0.0; 512];
            let mut tail = Vec::new();
            for block in 0..160 {
                e.process_block(&mut out, &bank);
                if block >= 60 {
                    tail.extend_from_slice(&out);
                }
            }
            tail
        };
        let (material, cloud, both) =
            (render(true, false), render(false, true), render(true, true));
        assert!(material.iter().any(|s| s.abs() > 0.1), "no material");
        assert!(cloud.iter().any(|s| s.abs() > 0.01), "no cloud");
        for ((b, m), c) in both.iter().zip(&material).zip(&cloud) {
            assert!(
                (b - (m + c).clamp(-1.0, 1.0)).abs() < 1e-5,
                "both gave {b}, the parts {m} + {c}"
            );
        }
    }

    /// A second of the tone: `configure` before the first block, `later` about
    /// a tenth of a second in, and everything after 0.64 s returned. Long
    /// enough for the ring modulator's own mix smoothing to settle.
    fn render_tail(configure: impl Fn(&ParamBank), later: impl Fn(&ParamBank)) -> Vec<f32> {
        let mut e = Engine::new(48_000.0, 128);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        configure(&bank);
        let mut out = vec![0.0; 512];
        let mut tail = Vec::new();
        for block in 0..188 {
            if block == 20 {
                later(&bank);
            }
            e.process_block(&mut out, &bank);
            if block >= 120 {
                tail.extend_from_slice(&out);
            }
        }
        tail
    }

    #[test]
    fn a_section_switched_off_is_exactly_that_section_at_zero() {
        // The null test for every switch. Off must not merely sound off: once
        // the fade has landed it has to be bit-exact with the section at rest,
        // or "off" is quietly leaving something behind.
        //
        // Each section starts *on* and audible, then is switched off partway
        // in, so the fade actually runs. What it must then equal depends on the
        // role: an effect, its own mix at zero, which proves it does not colour
        // the signal; a generator, the same generator never switched on, which
        // proves nothing of it lingers.
        type Settings<'a> = &'a [(&'a str, f32)];
        let cases: [(&str, &str, Settings, Settings, bool); 5] = [
            ("material", "material.on", &[], &[], false),
            ("granular", "grain.on", &[], &[], false),
            (
                "crush",
                "crush.on",
                &[("crush.mix", 1.0), ("crush.bits", 4.0)],
                &[("crush.mix", 0.0), ("crush.bits", 4.0)],
                true,
            ),
            (
                "ring",
                "ring.on",
                &[("ring.mix", 1.0)],
                &[("ring.mix", 0.0)],
                true,
            ),
            (
                "envelope",
                "env.on",
                &[("env.attack", 300.0), ("env.sustain", 0.3)],
                &[
                    ("env.attack", 300.0),
                    ("env.sustain", 0.3),
                    ("env.mix", 0.0),
                ],
                true,
            ),
        ];
        for (name, switch, active, rest, rest_is_on) in cases {
            let switched = render_tail(
                |b| {
                    set(b, switch, 1.0);
                    for (id, v) in active {
                        set(b, id, *v);
                    }
                },
                |b| set(b, switch, 0.0),
            );
            let at_zero = render_tail(
                |b| {
                    set(b, switch, if rest_is_on { 1.0 } else { 0.0 });
                    for (id, v) in rest {
                        set(b, id, *v);
                    }
                },
                |_| {},
            );
            assert!(
                switched == at_zero,
                "{name} switched off is not bit-exact with its mix at zero"
            );
        }
    }

    #[test]
    fn switching_a_section_does_not_click() {
        // A hard bypass jumps from the processed signal to the plain one in a
        // single sample. The fade has to keep every switch inside the ordinary
        // movement of the signal itself.
        let worst_step = |toggle: bool| {
            let mut e = Engine::new(48_000.0, 128);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "grain.gain", 0.5);
            set(&bank, "ring.mix", 1.0);
            // The steady baseline is both sections on and audible.
            set(&bank, "grain.on", 1.0);
            set(&bank, "ring.on", 1.0);
            let mut out = vec![0.0; 256];
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            for block in 0..400 {
                if toggle {
                    let on = if (block / 25) % 2 == 0 { 1.0 } else { 0.0 };
                    set(&bank, "grain.on", on);
                    set(&bank, "ring.on", on);
                }
                e.process_block(&mut out, &bank);
                for s in out.iter().step_by(2) {
                    if block > 20 {
                        worst = worst.max((s - prev).abs());
                    }
                    prev = *s;
                }
            }
            worst
        };
        let steady = worst_step(false);
        let switched = worst_step(true);
        assert!(
            switched < steady * 2.0 + 0.02,
            "switching jumped by {switched} against a steady {steady}"
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
            bank.set_by_id("grain.on", 0.0);
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
        set(&bank, "grain.on", 0.0);
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
            set(&bank, "grain.on", 0.0);
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
        set(&bank, "grain.on", 0.0);
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
        // Switched on, so this tests the mix at zero rather than the bypass.
        let destructive = render_crushed(&[
            ("crush.on", 1.0),
            ("crush.bits", 1.0),
            ("crush.rate", 200.0),
        ]);
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
            ("crush.on", 1.0),
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
            ("crush.on", 1.0),
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
    fn logged_grains_land_inside_the_trim_window() {
        // The trap this offset exists for. The cloud only ever sees the
        // trimmed slice, so a logged position is relative to the window unless
        // the offset is added back. Get it wrong and the marks look perfect at
        // the default trim of zero and are wrong everywhere else — which is a
        // bug that ships.
        let mut e = Engine::new(48_000.0, 128);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "trim.start", 0.5);
        set(&bank, "trim.end", 0.6);
        set(&bank, "grain.position", 0.5);
        set(&bank, "grain.jitter", 0.0);
        set(&bank, "grain.gain", 1.0);
        set(&bank, "grain.on", 1.0);

        let log = e.grain_log();
        let mut out = vec![0.0; 512];
        for _ in 0..400 {
            e.process_block(&mut out, &bank);
        }

        let mut spawns = Vec::new();
        log.drain(&mut spawns);
        assert!(!spawns.is_empty(), "nothing was logged");
        for g in &spawns {
            assert!(
                (0.5..=0.6).contains(&g.position),
                "logged position {} is outside the 0.5..0.6 trim window",
                g.position
            );
        }
    }

    #[test]
    fn the_log_records_what_the_controls_asked_for() {
        let mut e = Engine::new(48_000.0, 128);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "grain.on", 1.0);
        set(&bank, "grain.size", 100.0);
        set(&bank, "grain.reverse", 0.0);
        set(&bank, "grain.pan", 0.0);
        set(&bank, "grain.pitch", 0.0);
        set(&bank, "grain.spread", 0.0);

        let log = e.grain_log();
        let mut out = vec![0.0; 512];
        for _ in 0..400 {
            e.process_block(&mut out, &bank);
        }
        let mut spawns = Vec::new();
        log.drain(&mut spawns);
        assert!(!spawns.is_empty());

        // The last few, once the size smoother has settled on 100 ms.
        for g in spawns.iter().rev().take(5) {
            assert!(
                (g.len - 4_800.0).abs() < 200.0,
                "100 ms should log as ~4800 samples, got {}",
                g.len
            );
            assert!(
                g.rate > 0.0,
                "no reverse was asked for, got rate {}",
                g.rate
            );
            assert!(
                (g.pan - 0.5).abs() < 1e-3,
                "no pan spread should log as centred, got {}",
                g.pan
            );
        }
    }

    #[test]
    fn a_logged_grain_can_be_played_back_on_its_own_while_stopped() {
        // The inspector's whole promise: stop, pick a grain, hear it. A grain
        // is six numbers, so this is reconstruction, not a recording.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "grain.gain", 1.0);
        set(&bank, "grain.on", 1.0);

        let log = e.grain_log();
        let mut out = vec![0.0; 512];
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
        let mut spawns = Vec::new();
        log.drain(&mut spawns);
        let pick = *spawns.last().expect("nothing logged");

        e.set_playing(false);
        // Stopped and not auditioning is still silence.
        e.process_block(&mut out, &bank);
        assert!(out.iter().all(|s| *s == 0.0), "silence expected before");

        assert!(e.audition(&pick), "the audition was refused");
        let mut peak = 0.0f32;
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
            peak = peak.max(out.iter().fold(0.0f32, |a, s| a.max(s.abs())));
        }
        assert!(peak > 0.01, "the auditioned grain was inaudible: {peak}");
    }

    #[test]
    fn an_audition_ends_by_itself_and_leaves_silence() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        let g = GrainSpawn {
            position: 0.25,
            rate: 1.0,
            // 100 ms, so a second of rendering outlasts it many times over.
            len: 4_800.0,
            pan: 0.5,
            window: 0,
            seq: 1,
        };
        assert!(e.audition(&g));
        assert!(e.auditioning());

        let mut out = vec![0.0; 512];
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
        assert!(!e.auditioning(), "the audition never finished");
        e.process_block(&mut out, &bank);
        assert!(
            out.iter().all(|s| *s == 0.0),
            "left ringing after finishing"
        );
    }

    #[test]
    fn an_audition_is_refused_while_the_transport_runs() {
        // Not a limitation to work around: a single grain dropped into a live
        // cloud is inaudible, so accepting the call would be a lie.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let g = GrainSpawn {
            position: 0.25,
            rate: 1.0,
            len: 4_800.0,
            pan: 0.5,
            window: 0,
            seq: 1,
        };
        assert!(!e.audition(&g));
        assert!(!e.auditioning());
    }

    #[test]
    fn starting_playback_cancels_an_audition() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let g = GrainSpawn {
            position: 0.25,
            rate: 1.0,
            len: 48_000.0,
            pan: 0.5,
            window: 0,
            seq: 1,
        };
        assert!(e.audition(&g));
        e.set_playing(true);
        assert!(!e.auditioning());
    }

    #[test]
    fn an_unbraked_tape_is_exactly_transparent() {
        // The whole transport must vanish at rest. `tape_gain` returns a
        // literal one above the knee for this reason: a curve that merely
        // approached unity would put a silent 0.03 dB on everything.
        assert_eq!(tape_gain(1.0), 1.0);
        assert_eq!(tape_gain(-1.0), 1.0);
        assert_eq!(tape_gain(TAPE_KNEE), 1.0);
        assert_eq!(tape_gain(0.0), 0.0);
        assert!(tape_gain(TAPE_KNEE * 0.5) < 1.0);
    }

    #[test]
    fn the_brake_slows_the_tape_rather_than_muting_it() {
        // The OP-1 gesture: a finger on the reel. Level has to survive well
        // into the brake — if it did not, this would be a fade with extra
        // steps — and the pitch has to fall, which shows up as the playhead
        // covering less ground per block.
        let mut e = Engine::new(48_000.0, 128);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "grain.on", 0.0);
        set(&bank, "amp.gain", 1.0);
        set(&bank, "tape.time", 200.0);

        let mut out = vec![0.0; 512];
        for _ in 0..40 {
            e.process_block(&mut out, &bank);
        }
        let free_start = e.play_position();
        e.process_block(&mut out, &bank);
        let free_step = e.play_position() - free_start;
        let free_level = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));

        // Half a brake: running slow, still clearly audible.
        set(&bank, "tape.brake", 0.5);
        for _ in 0..80 {
            e.process_block(&mut out, &bank);
        }
        let half_start = e.play_position();
        e.process_block(&mut out, &bank);
        let half_step = e.play_position() - half_start;
        let half_level = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));

        assert!(
            half_step < free_step * 0.75,
            "half brake should slow the tape: {half_step} vs {free_step}"
        );
        assert!(
            half_level > free_level * 0.5,
            "half brake should not be a fade: {half_level} vs {free_level}"
        );

        // Full brake: the reel comes to rest and the level goes with it.
        set(&bank, "tape.brake", 1.0);
        for _ in 0..400 {
            e.process_block(&mut out, &bank);
        }
        let level = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(
            level < free_level * 0.05,
            "a stopped tape should be quiet: {level}"
        );
    }

    #[test]
    fn the_brake_takes_the_time_it_is_given() {
        // `tape.time` is the gesture. A long brake must still be audibly
        // moving at a point where a short one has already stopped, or the
        // control is decorative.
        let render = |time: f32, blocks: usize| {
            let mut e = Engine::new(48_000.0, 128);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "grain.on", 0.0);
            set(&bank, "amp.gain", 1.0);
            set(&bank, "tape.time", time);
            let mut out = vec![0.0; 512];
            for _ in 0..20 {
                e.process_block(&mut out, &bank);
            }
            set(&bank, "tape.brake", 1.0);
            let mut level = 0.0f32;
            for _ in 0..blocks {
                e.process_block(&mut out, &bank);
                level = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            }
            level
        };
        // ~0.5 s of audio after the brake goes on.
        let quick = render(50.0, 90);
        let slow = render(3000.0, 90);
        assert!(quick < 0.05, "a 50 ms brake should be done by now: {quick}");
        assert!(
            slow > quick * 4.0,
            "a 3 s brake should still be running: {slow}"
        );
    }

    #[test]
    fn an_octave_reads_the_material_at_twice_or_half_the_speed() {
        // Varispeed: an octave up covers twice the ground per block, so the
        // pass through the sample halves. Down is the reverse.
        let step = |octave: f32| {
            let mut e = Engine::new(48_000.0, 128);
            e.set_source(tone(96_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "grain.on", 0.0);
            set(&bank, "material.octave", octave);
            let mut out = vec![0.0; 512];
            e.process_block(&mut out, &bank);
            let start = e.play_position();
            e.process_block(&mut out, &bank);
            e.play_position() - start
        };
        let unity = step(0.0);
        for (octave, ratio) in [(-2.0, 0.25), (-1.0, 0.5), (1.0, 2.0), (2.0, 4.0)] {
            let s = step(octave);
            assert!(
                (s / unity - ratio).abs() < 1e-3,
                "octave {octave}: step {s} against {unity}, wanted ×{ratio}"
            );
        }
    }

    #[test]
    fn an_octave_transposes_grains_without_changing_density() {
        // The difference from the tape. An octave is read rate only: grains
        // read 2ⁿ faster, but the scheduler throws exactly as many per second,
        // so density stays in hertz whatever the octave.
        let run = |octave: f32| {
            let mut e = Engine::new(48_000.0, 256);
            e.set_source(tone(96_000));
            let log = e.grain_log();
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "grain.gain", 1.0);
            set(&bank, "grain.on", 1.0);
            set(&bank, "grain.density", 40.0);
            set(&bank, "material.octave", octave);
            let mut out = vec![0.0; 512];
            // About a second.
            for _ in 0..94 {
                e.process_block(&mut out, &bank);
            }
            let mut spawns = Vec::new();
            log.drain(&mut spawns);
            spawns
        };
        let unity = run(0.0);
        assert!(
            unity.len() > 20,
            "expected a cloud, got {} grains",
            unity.len()
        );
        for octave in [-2.0f32, 2.0] {
            let shifted = run(octave);
            assert_eq!(
                shifted.len(),
                unity.len(),
                "octave {octave} changed how many grains were thrown"
            );
            let want = octave.exp2();
            assert!(
                shifted.iter().all(|g| (g.rate - want).abs() < 1e-4),
                "octave {octave}: every grain should read at ×{want}"
            );
        }
    }

    fn lfo(id: u64, rate_hz: f32, shape: crate::modulation::Shape) -> crate::modulation::LfoSpec {
        crate::modulation::LfoSpec {
            id,
            rate_hz,
            shape,
            phase: 0.0,
        }
    }

    #[test]
    fn a_linked_parameter_moves_and_the_bank_keeps_the_hands_value() {
        use crate::modulation::Shape;
        // Watched through the grain log, which records each grain's length as
        // it was asked for. Unlinked, every grain is the same length; linked to
        // a square LFO, the lengths split between two sizes.
        let lengths = |linked: bool| {
            let mut e = Engine::new(48_000.0, 256);
            e.set_source(tone(48_000));
            let log = e.grain_log();
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "grain.on", 1.0);
            set(&bank, "grain.gain", 1.0);
            set(&bank, "grain.size", 100.0);
            if linked {
                let mut mods = ModSet::new(&[lfo(1, 2.0, Shape::Square)]);
                mods.link("grain.size", 1, 0.2, 0.8).unwrap();
                drop(e.set_modulation(mods));
            }
            let mut out = vec![0.0; 512];
            let mut spawns = Vec::new();
            for block in 0..400 {
                e.process_block(&mut out, &bank);
                // Let the size smoother settle from its default before counting.
                if block == 40 {
                    log.drain(&mut spawns);
                    spawns.clear();
                }
            }
            log.drain(&mut spawns);
            assert_eq!(
                bank.get_by_id("grain.size"),
                Some(100.0),
                "modulation wrote the bank"
            );
            assert!(spawns.len() > 20, "expected a cloud, got {}", spawns.len());
            let lo = spawns.iter().map(|g| g.len).fold(f32::MAX, f32::min);
            let hi = spawns.iter().map(|g| g.len).fold(0.0f32, f32::max);
            (lo, hi)
        };
        let (steady_lo, steady_hi) = lengths(false);
        let (lo, hi) = lengths(true);
        assert!(
            // The smoother is still creeping towards 100 ms by a sample or so;
            // one percent is "did not move" next to a fourfold split.
            steady_hi - steady_lo < steady_lo * 0.01,
            "unlinked grain size moved: {steady_lo}..{steady_hi} samples"
        );
        assert!(
            hi > lo * 4.0,
            "the link did not move grain size: {lo}..{hi} samples"
        );
    }

    #[test]
    fn an_unlinked_modulation_set_is_bit_exact_with_none() {
        // LFOs that nothing follows must not change a single sample.
        let render = |with_lfos: bool| {
            let mut e = Engine::new(48_000.0, 128);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            for (id, v) in [
                ("grain.on", 1.0),
                ("grain.gain", 0.5),
                ("ring.on", 1.0),
                ("ring.mix", 0.3),
            ] {
                set(&bank, id, v);
            }
            if with_lfos {
                let specs: Vec<_> = crate::modulation::Shape::ALL
                    .iter()
                    .enumerate()
                    .map(|(i, s)| lfo(i as u64 + 1, 3.0, *s))
                    .collect();
                drop(e.set_modulation(ModSet::new(&specs)));
            }
            let mut out = vec![0.0; 512];
            let mut all = Vec::new();
            for _ in 0..200 {
                e.process_block(&mut out, &bank);
                all.extend_from_slice(&out);
            }
            all
        };
        assert!(render(false) == render(true));
    }

    #[test]
    fn a_square_lfo_does_not_step_a_smoothed_parameter() {
        // Modulation lands before smoothing, so even a square wave on the ring
        // mix arrives as a glide. A mix that jumped would move the output by up
        // to twice the signal each time the square flips.
        let worst_step = |linked: bool| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "ring.on", 1.0);
            set(&bank, "ring.mix", 0.5);
            if linked {
                let mut mods = ModSet::new(&[lfo(1, 5.0, crate::modulation::Shape::Square)]);
                mods.link("ring.mix", 1, 0.0, 0.5).unwrap();
                drop(e.set_modulation(mods));
            }
            let mut out = vec![0.0; 256];
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            for block in 0..400 {
                e.process_block(&mut out, &bank);
                for s in out.iter().step_by(2) {
                    if block > 20 {
                        worst = worst.max((s - prev).abs());
                    }
                    prev = *s;
                }
            }
            worst
        };
        let steady = worst_step(false);
        let moving = worst_step(true);
        assert!(
            moving < steady * 2.0 + 0.02,
            "a square LFO stepped the ring mix: {moving} against a steady {steady}"
        );
    }

    #[test]
    fn reverse_runs_the_tape_backwards() {
        // A ramp makes direction audible: forwards the playhead climbs, and
        // backwards it falls.
        let n = 48_000;
        let src: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(src);
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "grain.on", 0.0);
        set(&bank, "tape.time", 20.0);

        let mut out = vec![0.0; 512];
        for _ in 0..40 {
            e.process_block(&mut out, &bank);
        }
        let before = e.play_position();
        e.process_block(&mut out, &bank);
        assert!(e.play_position() > before, "forwards should advance");

        set(&bank, "tape.reverse", 1.0);
        // Long enough for the slew to cross zero and settle the other way.
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
        let a = e.play_position();
        e.process_block(&mut out, &bank);
        assert!(e.play_position() < a, "reverse should run the tape back");
    }

    #[test]
    fn a_tap_on_reverse_flicks_and_releases_itself() {
        // Tap and hold are one gesture at two lengths. A tap must outlast the
        // finger — that is what makes it a flick rather than a glitch — and it
        // must end on its own.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "tape.flick", 150.0);
        let mut out = vec![0.0; 512];

        // A tap: gate high for one block, about 5 ms.
        set(&bank, "tape.reverse", 1.0);
        e.process_block(&mut out, &bank);
        set(&bank, "tape.reverse", 0.0);
        assert!(e.reversing(), "a tap should engage reverse");

        // Well inside the flick, the finger long gone.
        for _ in 0..10 {
            e.process_block(&mut out, &bank);
        }
        assert!(
            e.reversing(),
            "the flick ended with the tap instead of outlasting it"
        );

        // Past 150 ms it releases without being told.
        for _ in 0..40 {
            e.process_block(&mut out, &bank);
        }
        assert!(!e.reversing(), "the flick never released itself");
    }

    #[test]
    fn holding_reverse_outlasts_the_flick_and_releases_on_let_go() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "tape.flick", 150.0);
        let mut out = vec![0.0; 512];

        set(&bank, "tape.reverse", 1.0);
        // Roughly two seconds, far past the flick.
        for _ in 0..400 {
            e.process_block(&mut out, &bank);
        }
        assert!(e.reversing(), "a hold should stay engaged");

        set(&bank, "tape.reverse", 0.0);
        e.process_block(&mut out, &bank);
        assert!(
            !e.reversing(),
            "a hold past the flick should release as soon as the gate does"
        );
    }

    #[test]
    fn the_flick_length_is_the_control() {
        // The knob has to do something, or it is decoration.
        let engaged_after = |flick: f32, blocks: usize| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "tape.flick", flick);
            let mut out = vec![0.0; 512];
            set(&bank, "tape.reverse", 1.0);
            e.process_block(&mut out, &bank);
            set(&bank, "tape.reverse", 0.0);
            for _ in 0..blocks {
                e.process_block(&mut out, &bank);
            }
            e.reversing()
        };
        // ~160 ms after the tap: a short flick is done, a long one is not.
        assert!(!engaged_after(50.0, 30), "a 50 ms flick should be over");
        assert!(engaged_after(500.0, 30), "a 500 ms flick should still run");
    }

    #[test]
    fn stopping_clears_a_flick_in_flight() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        set(&bank, "tape.flick", 500.0);
        let mut out = vec![0.0; 512];
        set(&bank, "tape.reverse", 1.0);
        e.process_block(&mut out, &bank);
        set(&bank, "tape.reverse", 0.0);
        assert!(e.reversing());
        e.set_playing(false);
        assert!(
            !e.reversing(),
            "a flick survived a stop and would fire on play"
        );
    }

    #[test]
    fn the_tape_stays_in_range_however_it_is_driven() {
        // Brake and reverse are performance controls and will be thrown about.
        let mut e = Engine::new(48_000.0, 128);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = ParamBank::new();
        let mut out = vec![0.0; 512];
        for (brake, reverse, time) in [
            (1.0, 1.0, 20.0),
            (0.0, 1.0, 3000.0),
            (0.5, 0.0, 20.0),
            (1.0, 0.0, 20.0),
            (0.0, 0.0, 20.0),
        ] {
            set(&bank, "tape.brake", brake);
            set(&bank, "tape.reverse", reverse);
            set(&bank, "tape.time", time);
            for _ in 0..80 {
                e.process_block(&mut out, &bank);
                for s in &out {
                    assert!(s.is_finite(), "non-finite at brake {brake} rev {reverse}");
                    assert!((-1.0..=1.0).contains(s), "out of range: {s}");
                }
                assert!(
                    (0.0..=1.0).contains(&e.play_position()),
                    "playhead left the sample: {}",
                    e.play_position()
                );
            }
        }
    }

    #[test]
    fn an_audition_ignores_the_tape() {
        // A latched reverse must not make the inspector play grains backwards.
        // The audition answers what the grain sounded like, not what the
        // transport is currently doing to everything else.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        set(&bank, "tape.reverse", 1.0);
        set(&bank, "tape.brake", 1.0);
        let g = GrainSpawn {
            position: 0.25,
            rate: 1.0,
            len: 4_800.0,
            pan: 0.5,
            window: 0,
            seq: 1,
        };
        assert!(e.audition(&g));
        let mut out = vec![0.0; 512];
        let mut peak = 0.0f32;
        for _ in 0..40 {
            e.process_block(&mut out, &bank);
            peak = peak.max(out.iter().fold(0.0f32, |a, s| a.max(s.abs())));
        }
        assert!(peak > 0.01, "a braked tape silenced the audition: {peak}");
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
