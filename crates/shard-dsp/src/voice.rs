//! A voice: one track playing one patch.
//!
//! Everything that belongs to the patch and to the track that plays it lives
//! here: the two materials and how each is read, the player, the cloud, FM,
//! the patch's own chain, the envelopes, the fades, the smoothers, the
//! modulation, the note clock, and the track's steps. The engine
//! (`engine.rs`) holds the arrangement around it: the transport, the tape,
//! the drums tracks' kits, the arrangement's chain and the limiter.
//!
//! Epic 28 (several tracks): the engine holds a fixed pool of these, each
//! reading its own bank, modulation, materials and steps, all on the one
//! transport. The tape's speed is the engine's too (brake and rewind slow
//! everything), handed to every voice each frame. A voice is switched in and
//! out with `set_active`, which fades it; once faded out it is idle and the
//! engine skips it outright.
//!
//! Everything here runs on the audio thread. No allocation, no locks, no
//! logging, no file access.

use std::sync::Arc;

use crate::chain::{Chain, ChainParams, ChainSlots, BYPASS_MS};
use crate::envelope::EnvParams;
use crate::fm::{Fm, FmParams, FmType};
use crate::granular::{GrainParams, Granular, Window};
use crate::inspect::{GrainLog, GrainSpawn};
use crate::ladder::{self, Ladder, LadderParams, LadderSlots};
use crate::modulation::{Globals, ModSet};
use crate::noise::{Noise, NoiseParams};
use crate::note::{Alone, Length};
use crate::params::{index_of, ParamBank};
use crate::player::Player;
use crate::scrub::Scrub;
use crate::smooth::{OnePole, Ramp};
use crate::sources::{Generator, Reading};
use crate::steps::{Beat, StepClock, StepParams, Transport};

/// How long a change of velocity takes to arrive, in ms.
const VELOCITY_MS: f32 = 3.0;

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
    grain_gain: usize,
    material_on: usize,
    material_gain: usize,
    scrub: usize,
    scrub_position: usize,
    grain_on: usize,
    env_on: usize,
    env_mix: usize,
    env_attack: usize,
    env_decay: usize,
    env_sustain: usize,
    env_release: usize,
    tape_time: usize,
    tape_flick: usize,
    patch_length: usize,
    patch_hold: usize,
    fm_on: usize,
    fm_gain: usize,
    fm_freq: usize,
    fm_ratio: usize,
    fm_index: usize,
    fm_feedback: usize,
    fm_drift: usize,
    fm_type: usize,
    noise_on: usize,
    noise_gain: usize,
    noise_color: usize,
    noise_band: usize,
    noise_freq: usize,
    noise_width: usize,
    /// The patch's drive, crush, ring, filter and output.
    fx: ChainSlots,
    /// The patch filter's key tracking. Not in `ChainSlots`: the arrangement
    /// shares that struct and has no note to follow.
    filter_tracking: usize,
    /// The ladder after the filter: patch only, like the tracking row.
    ladder: LadderSlots,
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
            grain_gain: at("grain.gain"),
            material_on: at("material.on"),
            material_gain: at("material.gain"),
            scrub: at("material.scrub"),
            scrub_position: at("material.position"),
            grain_on: at("grain.on"),
            env_on: at("env.on"),
            env_mix: at("env.mix"),
            env_attack: at("env.attack"),
            env_decay: at("env.decay"),
            env_sustain: at("env.sustain"),
            env_release: at("env.release"),
            tape_time: at("tape.time"),
            tape_flick: at("tape.flick"),
            patch_length: at("patch.length"),
            patch_hold: at("patch.hold"),
            fm_on: at("fm.on"),
            fm_gain: at("fm.gain"),
            fm_freq: at("fm.freq"),
            fm_ratio: at("fm.ratio"),
            fm_index: at("fm.index"),
            fm_feedback: at("fm.feedback"),
            fm_drift: at("fm.drift"),
            fm_type: at("fm.type"),
            noise_on: at("noise.on"),
            noise_gain: at("noise.gain"),
            noise_color: at("noise.color"),
            noise_band: at("noise.band"),
            noise_freq: at("noise.freq"),
            noise_width: at("noise.width"),
            fx: ChainSlots::resolve(at),
            filter_tracking: at("filter.tracking"),
            ladder: LadderSlots::resolve(at),
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
    fm_gain: OnePole,
    noise_gain: OnePole,
}

/// One ramp per switchable section. Each multiplies its section's mix and
/// lands on an exact zero, so a section that is off is bit-exact with its mix
/// at zero, and the cloud can be skipped outright.
struct Fades {
    material: Ramp,
    grain: Ramp,
    fm: Ramp,
    noise: Ramp,
    /// FM and noise have no pass of their own, so under the steps they
    /// follow the player's: this opens while one sounds and closes between
    /// them. One gate for both.
    note_gate: Ramp,
    env: Ramp,
    /// Scrub's switch: the plain sample's passes fade out as the head fades in.
    scrub: Ramp,
}

impl Fades {
    fn new(sample_rate: f32, slots: &Slots) -> Self {
        let defs = &crate::params::PARAMS[..];
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
            fm: ramp(slots.fm_on),
            noise: ramp(slots.noise_on),
            note_gate: {
                let mut r = Ramp::new(1.0);
                r.set_time(BYPASS_MS, sample_rate);
                r
            },
            env: ramp(slots.env_on),
            scrub: ramp(slots.scrub),
        }
    }
}

/// The note clock (`note.rs`), which always runs: samples of tape time since
/// the last trigger, reset by the transport and by every step.
#[derive(Default)]
struct NoteClock {
    /// f64, because an f32 count stops moving after about six minutes and a
    /// Loop drone runs longer than that.
    age: f64,
    /// Where this note's gate closes, where its release has finished and the
    /// generators start to fade, and where it ends once they have. Latched
    /// when it started, so nothing moving `patch.hold` moves a note mid-note.
    gate: f64,
    released: f64,
    end: f64,
    /// Counts triggers, so the modulation envelopes can tell a new note.
    id: u32,
    /// A trigger waiting for the next block, which has the bank to latch
    /// from: the transport starting, or a pass restarted offline.
    pending: bool,
    /// Each envelope's shape as it last was, and where a new note's attack
    /// climbs from, so a retrigger mid-release does not click.
    amp_shape: f32,
    amp_from: f32,
}

impl NoteClock {
    /// Start a note: the clock from zero, this note's gate and end latched,
    /// and every envelope's attack climbing from where it had got to.
    fn start(&mut self, gate: f64, released: f64, end: f64) {
        self.age = 0.0;
        self.gate = gate;
        self.released = released;
        self.end = end;
        self.id = self.id.wrapping_add(1);
        self.amp_from = self.amp_shape;
    }
}

/// What a block works out once, before its frames: the bank read through the
/// modulation, the switches as gates, the windows into the materials, and the
/// note a trigger would latch. Held on the voice between `begin_block` and
/// `end_block`, so `frame` can be called once per frame by the engine.
#[derive(Clone, Copy)]
struct Block {
    length: Length,
    target: GrainParams,
    /// The cloud's parameters as this frame left them.
    p: GrainParams,
    fx: ChainParams,
    /// How many octaves the filter's cutoff moves per octave of the step's
    /// pitch: 0 (off), 0.5 (half) or 1 (full).
    filter_tracking: f32,
    /// The ladder's values, and its own tracking in the same units.
    ladder: LadderParams,
    ladder_tracking: f32,
    material_gain_target: f32,
    fm_gain_target: f32,
    fm_params: FmParams,
    noise_gain_target: f32,
    noise_params: NoiseParams,
    grain_on_target: f32,
    fm_on_target: f32,
    noise_on_target: f32,
    material_on_target: f32,
    /// Whether the plain sample is scrubbed, and where its head is asked to be.
    scrub_target: f32,
    scrub_position: f32,
    env_on_target: f32,
    steps_off: bool,
    /// Alone, one pass: nothing loops, nothing is retriggered, and the
    /// engine stops when `once_done`.
    once: bool,
    looping: bool,
    env: EnvParams,
    grain_semis: f32,
    /// The player's window into its material, and the cloud's into its own.
    lo: usize,
    hi: usize,
    grain_lo: usize,
    grain_hi: usize,
    /// Whether anything is wired into Sample; if not, the pass runs through
    /// the cloud's window instead.
    player_wired: bool,
    read_ratio: f32,
    window_len: f32,
    note_gate: f64,
    note_release: f64,
    note_released: f64,
    note_end: f64,
}

/// The tape on one frame, as the engine runs it for every voice: this frame's
/// speed (negative is backwards) and the last frame's, which decides which
/// end a step starts its pass from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tape {
    pub speed: f32,
    pub was: f32,
}

pub(crate) struct Voice {
    /// What plain playback reads, and what the cloud reads. Each generator
    /// has its own material (Georg, 2026-09-15); they may be the same file.
    player_source: Vec<f32>,
    grain_source: Vec<f32>,
    /// How each reads it: which part, at what octave. See `set_readings`.
    player_reading: Reading,
    grain_reading: Reading,
    player: Player,
    scrub: Scrub,
    granular: Granular,
    /// The third generator, which reads no material.
    fm: Fm,
    /// The fourth, which reads none either.
    noise: Noise,
    /// Every spawn the cloud makes, for the inspector. Shared out by
    /// `grain_log`; the audio thread only ever pushes to it.
    log: Arc<GrainLog>,
    /// Playing one logged grain on its own, with the transport stopped.
    auditioning: bool,
    /// Whether the host wants this voice playing, and its level fading it in
    /// and out. Once faded out it is idle: the engine skips it, and it costs
    /// nothing. `fresh` marks one just woken, which settles at its next
    /// block. See `set_active`.
    active: bool,
    level: Ramp,
    idle: bool,
    fresh: bool,
    /// This track's step clock: its pattern, length and triggers.
    steps: StepClock,
    /// What the steps play, from the tracker above the patch. See `set_steps`.
    step_params: StepParams,
    /// The pass a step cut short, fading out under the new one.
    tail: Player,
    /// Samples of that fade still to run; zero when there is no tail.
    tail_left: f32,
    /// The pass's read ratio from its step's pitch, and the same pitch in
    /// semitones for the grains. Unity and zero while the steps are off.
    pass_ratio: f32,
    pass_semis: f32,
    /// The ratio the tail was read at, so a pitch change fades out at the
    /// old pitch rather than jumping to the new one.
    tail_ratio: f32,
    /// The step's velocity as a gain, glided over a few milliseconds so a
    /// step that is quieter than the last does not click, and what it is
    /// gliding to. Unity while the steps are off.
    vel: Ramp,
    vel_target: f32,
    /// The velocity of a step that began on the last frame, 0 to 1, or zero
    /// for none: what a track this one keys dips by (`duck.rs`).
    fired: f32,
    /// The patch's own effects and output.
    fx: Chain,
    /// The driven 4-pole between the patch's filter and its output gain.
    ladder: Ladder,
    slots: Slots,
    smooth: Smoothers,
    fades: Fades,
    /// LFOs and links. Swapped in whole by `set_modulation`; read through by
    /// every parameter the voice uses.
    mods: ModSet,
    sample_rate: f32,
    trimmed_play_position: f32,
    /// The pass's window length in samples, as last block found it, for the
    /// modulation envelopes' clock.
    pass_len: f32,
    /// The note clock; see `NoteClock`.
    note: NoteClock,
    /// Under Hold, the generators fall to an exact zero once the note has
    /// ended, whether or not the Envelope node is on.
    note_fade: Ramp,
    /// Whether the patch plays on its own, and how; see `Alone`.
    alone: Alone,
    /// This block's values; see `Block`.
    block: Block,
    /// What this voice mixes into its seeds; see `new`.
    salt: u32,
}

impl Voice {
    /// Voice `index` of the pool. Its random sources are salted by it
    /// (`rng::salt`), so voice 0 keeps the seeds the engine always had and
    /// two voices on one patch differ. Voice 0 starts active; the rest idle.
    pub(crate) fn new(sample_rate: f32, max_grains: usize, index: usize) -> Self {
        let salt = crate::rng::salt(index);
        let slots = Slots::resolve();
        let fades = Fades::new(sample_rate, &slots);
        let mk = |ms: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p
        };
        let defs = &crate::params::PARAMS[..];
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
            fm_gain: mk(defs[slots.fm_gain].smooth_ms),
            noise_gain: mk(defs[slots.noise_gain].smooth_ms),
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
        smooth.fm_gain.reset(defs[slots.fm_gain].default);
        smooth.noise_gain.reset(defs[slots.noise_gain].default);

        let log = Arc::new(GrainLog::new());
        let mut granular = Granular::seeded(sample_rate, max_grains, salt);
        granular.set_log(Arc::clone(&log));

        // Only a shape until the first `begin_block` fills it in.
        let grain = GrainParams {
            position: 0.0,
            jitter: 0.0,
            size_ms: 0.0,
            density: 0.0,
            pitch: 0.0,
            pitch_spread: 0.0,
            pan_spread: 0.0,
            reverse: 0.0,
            window: Window::ALL[0],
            gain: 0.0,
            speed: 1.0,
            spawning: true,
        };
        let block = Block {
            length: Length::Sample,
            target: grain,
            p: grain,
            fx: ChainParams::read(
                &slots.fx,
                |slot| defs[slot].default,
                |slot| defs[slot].default,
            ),
            filter_tracking: 0.0,
            ladder: slots
                .ladder
                .read(|slot| defs[slot].default, |slot| defs[slot].default),
            ladder_tracking: 0.0,
            material_gain_target: 0.0,
            fm_gain_target: 0.0,
            fm_params: FmParams {
                freq: 0.0,
                ratio: 0.0,
                index: 0.0,
                feedback: 0.0,
                drift: 0.0,
                ty: FmType::from_value(0.0),
            },
            noise_gain_target: 0.0,
            noise_params: NoiseParams::default(),
            grain_on_target: 0.0,
            fm_on_target: 0.0,
            noise_on_target: 0.0,
            material_on_target: 0.0,
            scrub_target: 0.0,
            scrub_position: 0.0,
            env_on_target: 0.0,
            steps_off: true,
            once: false,
            looping: true,
            env: EnvParams {
                amount: 0.0,
                attack_ms: 0.0,
                decay_ms: 0.0,
                sustain: 0.0,
                release_ms: 0.0,
            },
            grain_semis: 0.0,
            lo: 0,
            hi: 0,
            grain_lo: 0,
            grain_hi: 0,
            player_wired: false,
            read_ratio: 1.0,
            window_len: 0.0,
            note_gate: 0.0,
            note_release: 0.0,
            note_released: 0.0,
            note_end: 0.0,
        };

        Self {
            player_source: Vec::new(),
            grain_source: Vec::new(),
            player_reading: Reading::default(),
            grain_reading: Reading::default(),
            player: Player::new(sample_rate),
            scrub: Scrub::new(sample_rate),
            granular,
            fm: Fm::seeded(sample_rate, salt),
            noise: Noise::seeded(sample_rate, salt),
            log,
            auditioning: false,
            active: index == 0,
            level: {
                let mut r = Ramp::new(if index == 0 { 1.0 } else { 0.0 });
                r.set_time(BYPASS_MS, sample_rate);
                r
            },
            idle: index != 0,
            fresh: false,
            salt,
            steps: StepClock::new(sample_rate),
            step_params: StepParams::default(),
            tail: Player::new(sample_rate),
            tail_left: 0.0,
            pass_ratio: 1.0,
            pass_semis: 0.0,
            tail_ratio: 1.0,
            vel: {
                let mut r = Ramp::new(1.0);
                r.set_time(VELOCITY_MS, sample_rate);
                r
            },
            vel_target: 1.0,
            fired: 0.0,
            fx: Chain::new(sample_rate, defs, &slots.fx, salt),
            ladder: Ladder::new(sample_rate),
            slots,
            smooth,
            fades,
            mods: ModSet::empty(),
            sample_rate,
            trimmed_play_position: 0.0,
            pass_len: 0.0,
            note: NoteClock {
                pending: true,
                ..NoteClock::default()
            },
            note_fade: {
                let mut r = Ramp::new(1.0);
                r.set_time(BYPASS_MS, sample_rate);
                r
            },
            alone: Alone::Song,
            block,
        }
    }

    /// See `Engine::swap_source`.
    pub(crate) fn swap_source(&mut self, which: Generator, samples: Vec<f32>) -> Vec<f32> {
        match which {
            Generator::Player => {
                self.player.rewind();
                self.tail_left = 0.0;
                std::mem::replace(&mut self.player_source, samples)
            }
            Generator::Grain => {
                self.granular.clear();
                self.auditioning = false;
                std::mem::replace(&mut self.grain_source, samples)
            }
        }
    }

    pub(crate) fn set_readings(&mut self, player: Reading, grain: Reading) {
        self.player_reading = player;
        self.grain_reading = grain;
    }

    pub(crate) fn set_modulation(&mut self, mut next: ModSet) -> ModSet {
        next.salt(self.salt);
        next.inherit(&self.mods);
        std::mem::replace(&mut self.mods, next)
    }

    pub(crate) fn publish_heard(&self, bank: &ParamBank, heard: &ParamBank) {
        for slot in 0..crate::params::PARAMS.len() {
            heard.set(slot, read(&self.mods, bank, slot));
        }
    }

    pub(crate) fn grain_log(&self) -> Arc<GrainLog> {
        Arc::clone(&self.log)
    }

    /// One logged grain on its own; the engine has checked the transport is
    /// stopped. See `Engine::audition`.
    pub(crate) fn audition(&mut self, spawn: &GrainSpawn) -> bool {
        self.granular.clear();
        self.auditioning = self.granular.trigger(spawn, self.grain_source.len());
        self.auditioning
    }

    pub(crate) fn auditioning(&self) -> bool {
        self.auditioning
    }

    /// Switch the voice in or out. In, it wakes from idle clean (no grains,
    /// tails or effect tails left from before, smoothers already where the
    /// patch is) and fades in; out, it fades out and then goes idle, so it is
    /// never cut mid-sample.
    pub(crate) fn set_active(&mut self, on: bool) {
        if on && self.idle {
            self.idle = false;
            self.fresh = true;
        }
        self.active = on;
    }

    pub(crate) fn active(&self) -> bool {
        self.active
    }

    /// Faded out: the engine skips it whole.
    pub(crate) fn idle(&self) -> bool {
        self.idle
    }

    /// The tape's feel as this voice's patch sets it, through its LFOs: how
    /// long the brake's slew takes and how long a flick lasts, in ms. The
    /// engine reads voice 0's (the a2 seam: the feel is still a patch's).
    pub(crate) fn tape_feel(&self, bank: &ParamBank) -> (f32, f32) {
        (
            read(&self.mods, bank, self.slots.tape_time).max(0.0),
            read(&self.mods, bank, self.slots.tape_flick).max(0.0),
        )
    }

    /// One grain, windowed and panned as logged, with no cloud around it and
    /// no overlap compensation. Deliberately bypasses the crusher, the ring
    /// modulator and the amplitude envelope: the question an audition answers
    /// is what the *grain* sounded like, not what the patch did to it.
    pub(crate) fn render_audition(&mut self, out: &mut [f32], bank: &ParamBank, peak: &mut f32) {
        // The grain as it was: the cloud's level and the master gain as the
        // hand set them, unmodulated.
        let gain = bank.get(self.slots.grain_gain) * bank.get(self.slots.fx.gain);
        // Split the borrows by field so the grain pool can be advanced while
        // the source is read.
        let granular = &mut self.granular;
        let source = &self.grain_source;
        let mut p = *peak;

        for frame in out.chunks_mut(2) {
            let (l, r) = granular.render_solo(source);
            let l = (l * gain).clamp(-1.0, 1.0);
            let r = (r * gain).clamp(-1.0, 1.0);
            p = p.max(l.abs()).max(r.abs());
            if let [lo, ro] = frame {
                *lo = l;
                *ro = r;
            }
        }

        *peak = p;
        if granular.active_grains() == 0 {
            self.auditioning = false;
        }
    }

    /// The transport starting or stopping; `was_playing` is how it was. See
    /// `Engine::set_playing`.
    pub(crate) fn set_playing(&mut self, playing: bool, was_playing: bool) {
        if !playing {
            self.granular.clear();
        }
        // Either direction cancels an audition: starting drowns it, stopping
        // has just cleared the pool out from under it.
        self.auditioning = false;
        // The steps stop with the tape, so no step stays lit, and the next
        // block starts them again from step one.
        self.steps.set_running(false);
        self.tail_left = 0.0;
        // Under the steps nothing sounds until a step does, so play does not
        // pick up the pass the transport stopped in.
        if playing && !was_playing && self.step_params.on {
            self.player.finish();
        }
        if playing && !was_playing {
            self.note.pending = true;
            // On its own, play starts the patch from the top, every time.
            if self.alone != Alone::Song && !self.step_params.on {
                self.player.rewind();
            }
        }
    }

    /// Whether the patch plays on its own, and how. Takes effect at the next
    /// block; a pass in flight carries on under the new rule.
    pub(crate) fn set_alone(&mut self, alone: Alone) {
        self.alone = alone;
    }

    /// Once, whether the pass has played out: the sample has reached the end
    /// of its window and the cloud's grains have died away, or, under Hold,
    /// the note has ended. Its own envelope's release lands inside the pass
    /// (Sample) or the note (Hold), so nothing of this voice is left to cut.
    /// Always false otherwise, and for a voice that is not playing.
    pub(crate) fn once_done(&self) -> bool {
        if self.alone != Alone::Once || !self.active {
            return false;
        }
        let b = &self.block;
        match b.length {
            Length::Hold => self.note.age >= self.note.end,
            // A window too short to play ends at once; so does a pass that
            // has left it.
            _ => {
                (b.window_len < 2.0 || !self.player.sounding())
                    && self.granular.active_grains() == 0
            }
        }
    }

    /// See `Engine::restart_pass`.
    pub(crate) fn restart_pass(&mut self) {
        self.player.rewind();
        self.tail_left = 0.0;
        self.granular.clear();
        self.note.pending = true;
    }

    pub(crate) fn play_position(&self) -> f32 {
        self.trimmed_play_position
    }

    pub(crate) fn source_len(&self, which: Generator) -> usize {
        match which {
            Generator::Player => self.player_source.len(),
            Generator::Grain => self.grain_source.len(),
        }
    }

    pub(crate) fn active_grains(&self) -> usize {
        self.granular.active_grains()
    }

    /// The velocity of a step that began on the last frame, or zero.
    #[inline]
    pub(crate) fn fired(&self) -> f32 {
        self.fired
    }

    /// This track's step clock, for the step it is in and how far it has run.
    pub(crate) fn clock(&self) -> &StepClock {
        &self.steps
    }

    pub(crate) fn set_steps(&mut self, p: StepParams) {
        self.step_params = p;
    }

    /// Everything a block works out once, before its frames. Only while the
    /// transport runs; a stopped engine renders silence or an audition.
    /// `transport` has begun this block. The voice follows its steps when
    /// its own steps are on and the transport's are running. `globals` have
    /// been advanced this block, once for every voice.
    pub(crate) fn begin_block(
        &mut self,
        bank: &ParamBank,
        frames: usize,
        transport: &Transport,
        globals: &Globals,
    ) {
        let length = self
            .alone
            .length(Length::from_value(bank.get(self.slots.patch_length)));
        // LFOs move with the transport, and once per block, before anything
        // below reads a parameter through them. A global LFO is the engine's,
        // so every voice following it reads the same value.
        self.mods.advance(frames, self.sample_rate);
        self.mods.follow(globals);
        // Envelopes on the pass's clock, or the note's, before any row
        // reads through them. The window's length is last block's, since
        // this block's is worked out below; it changes only when the trim
        // does.
        match length {
            Length::Sample => self.mods.clock_envelopes(
                self.player.elapsed(),
                self.pass_len,
                self.player.sounding(),
            ),
            Length::Loop | Length::Hold => self.mods.clock_envelopes_note(
                self.note.age as f32,
                (length == Length::Hold).then_some(self.note.gate as f32),
                self.note.id,
                self.pass_len,
            ),
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
            // Both set per sample below; this is only the struct's starting
            // shape.
            speed: 1.0,
            spawning: true,
        };
        // The patch's effects and output, heard through its LFOs. The crush
        // mix is rebuilt per sample inside the chain, because the crush
        // envelope moves it.
        let fx = ChainParams::read(
            &self.slots.fx,
            |slot| read(&self.mods, bank, slot),
            |slot| bank.get(slot),
        );

        // Stepped, so raw: no LFO moves it. Off, half, full.
        let filter_tracking = ladder::tracking_from(bank.get(self.slots.filter_tracking));
        let ladder_tracking = ladder::tracking_from(bank.get(self.slots.ladder.tracking));
        let ladder = self
            .slots
            .ladder
            .read(|slot| read(&self.mods, bank, slot), |slot| bank.get(slot));

        let material_gain_target = read(&self.mods, bank, self.slots.material_gain);
        let fm_gain_target = read(&self.mods, bank, self.slots.fm_gain);
        let fm_params = FmParams {
            freq: read(&self.mods, bank, self.slots.fm_freq),
            ratio: read(&self.mods, bank, self.slots.fm_ratio),
            index: read(&self.mods, bank, self.slots.fm_index),
            feedback: read(&self.mods, bank, self.slots.fm_feedback),
            drift: read(&self.mods, bank, self.slots.fm_drift),
            ty: FmType::from_value(bank.get(self.slots.fm_type)),
        };
        let noise_gain_target = read(&self.mods, bank, self.slots.noise_gain);
        let noise_params = NoiseParams {
            color: read(&self.mods, bank, self.slots.noise_color),
            band: read(&self.mods, bank, self.slots.noise_band),
            freq: read(&self.mods, bank, self.slots.noise_freq),
            width: read(&self.mods, bank, self.slots.noise_width),
        };
        // Section switches, read as gates. Each one fades its section's mix
        // rather than cutting it.
        let gate = |slot: usize| if bank.get(slot) >= 0.5 { 1.0 } else { 0.0 };
        let grain_on_target = gate(self.slots.grain_on);
        let fm_on_target = gate(self.slots.fm_on);
        let noise_on_target = gate(self.slots.noise_on);
        let material_on_target = gate(self.slots.material_on);
        let scrub_target = gate(self.slots.scrub);
        let scrub_position = read(&self.mods, bank, self.slots.scrub_position).clamp(0.0, 1.0);
        // Nobody hears the head while Scrub is off, so it waits on the
        // position, and switching on starts from there rather than sweeping
        // in from wherever it was left.
        if scrub_target == 0.0 && self.fades.scrub.value() == 0.0 {
            self.scrub.snap(scrub_position);
        }
        let env_on_target = gate(self.slots.env_on);
        // Whether this voice follows the steps: its own switch, as the
        // tracker last handed it in, and the transport running them.
        let stepped = self.step_params.on && transport.running();
        // Just woken from idle: nothing of before is heard, and nothing
        // glides in from values the patch had then.
        if core::mem::take(&mut self.fresh) {
            let s = &mut self.smooth;
            s.position.reset(target.position);
            s.jitter.reset(target.jitter);
            s.size.reset(target.size_ms);
            s.density.reset(target.density);
            s.pitch.reset(target.pitch);
            s.spread.reset(target.pitch_spread);
            s.pan.reset(target.pan_spread);
            s.grain_gain.reset(target.gain);
            s.material_gain.reset(material_gain_target);
            s.fm_gain.reset(fm_gain_target);
            s.noise_gain.reset(noise_gain_target);
            self.fades.material.set(material_on_target);
            self.fades.scrub.set(scrub_target);
            self.scrub.snap(scrub_position);
            self.fades.grain.set(grain_on_target);
            self.fades.fm.set(fm_on_target);
            self.fades.noise.set(noise_on_target);
            self.fades.env.set(env_on_target);
            self.fades.note_gate.set(if stepped { 0.0 } else { 1.0 });
            self.fx.forget(&fx);
            self.ladder.reset(&ladder);
            self.fm.settle(&fm_params);
            self.noise.settle(&noise_params);
            self.granular.clear();
            self.tail_left = 0.0;
            if stepped {
                self.player.finish();
            } else {
                self.player.rewind();
            }
            self.steps.set_running(false);
            self.pass_ratio = 1.0;
            self.pass_semis = 0.0;
            self.vel.set(1.0);
            self.vel_target = 1.0;
            self.note_fade.set(1.0);
            self.note.pending = true;
        }
        // Switched on mid-pass, the steps take over from the loop: the pass
        // in flight fades out as a tail, and nothing sounds until a step does.
        if stepped && !self.steps.running() && self.player.sounding() {
            self.tail = self.player;
            self.tail_left = self.player.seam_samples();
            self.tail_ratio = self.pass_ratio;
            self.player.finish();
        }
        self.steps.set_running(stepped);
        // Under the steps a pass plays once (Georg, 2026-09-14); without them
        // it loops, and no step's pitch outlives them.
        let steps_off = !stepped;
        if steps_off {
            self.pass_ratio = 1.0;
            self.pass_semis = 0.0;
            self.vel_target = 1.0;
        }
        // Whether the sample loops: always for Loop, never for Hold, and for
        // Sample, whenever the steps are off, as it always has.
        // Alone and once through, none does.
        let once = self.alone == Alone::Once;
        let looping = !once
            && match length {
                Length::Loop => true,
                Length::Sample => steps_off,
                Length::Hold => false,
            };
        // Who the order names this block. Reorders, resets and fades start
        // here.
        self.fx.prepare(&fx);
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
        //
        // Each generator reads its own material, through that material's
        // octave and trim (Georg, 2026-09-15), handed in by `set_readings`.
        let grain_semis = 12.0 * self.grain_reading.octaves();

        let (lo, hi) = self.player_reading.window(self.player_source.len());
        let (grain_lo, grain_hi) = self.grain_reading.window(self.grain_source.len());
        // The cloud only sees its slice, so it has to be told where the slice
        // is before it logs a spawn — otherwise every drawn mark would sit at
        // the wrong place the moment the trim moved off zero.
        self.granular
            .set_source_window(grain_lo, self.grain_source.len());
        // The player is the clock as well as the Sample: a step starts its
        // pass, the pass ends at the edge of its window, the cloud throws
        // grains while it sounds, and both envelopes read it. With nothing
        // wired into Sample there was no edge to reach, so a pass never ended
        // and the cloud kept throwing grains between the steps. The pass runs
        // through the cloud's window instead, at the cloud's octave, and plays
        // nothing of its own.
        let player_wired = self.player_source.len() >= 2;
        let (window_len, read_ratio) = if player_wired {
            ((hi - lo) as f32, self.player_reading.octaves().exp2())
        } else {
            (
                (grain_hi - grain_lo) as f32,
                self.grain_reading.octaves().exp2(),
            )
        };
        self.pass_len = window_len;

        // This block's note, as a trigger now would latch it: held for Hold's
        // time, then the amplitude envelope's release when it is on, then the
        // few milliseconds the generators take to fade to an exact zero.
        let sr = f64::from(self.sample_rate);
        let note_gate =
            f64::from(read(&self.mods, bank, self.slots.patch_hold).max(0.0)) * 0.001 * sr;
        let note_release = if env_on_target > 0.0 {
            f64::from(env.release_ms.max(0.0)) * 0.001 * sr
        } else {
            0.0
        };
        let note_released = note_gate + note_release;
        let note_end = note_released + f64::from(BYPASS_MS) * 0.001 * sr;
        if self.note.pending {
            self.note.pending = false;
            self.note.start(note_gate, note_released, note_end);
        }

        self.block = Block {
            length,
            target,
            p: target,
            fx,
            filter_tracking,
            ladder,
            ladder_tracking,
            material_gain_target,
            fm_gain_target,
            fm_params,
            noise_gain_target,
            noise_params,
            grain_on_target,
            fm_on_target,
            noise_on_target,
            material_on_target,
            scrub_target,
            scrub_position,
            env_on_target,
            steps_off,
            once,
            looping,
            env,
            grain_semis,
            lo,
            hi,
            grain_lo,
            grain_hi,
            player_wired,
            read_ratio,
            window_len,
            note_gate,
            note_release,
            note_released,
            note_end,
        };
    }

    /// One frame of this voice, after the patch's chain and before the
    /// arrangement: the step clock ticked at `beat` on the transport (which
    /// swings by `swing`), the generators summed at the tape's speed, the
    /// patch's effects, envelope, filter and output, and the voice's own
    /// fade in or out.
    #[inline]
    pub(crate) fn frame(&mut self, tape: Tape, beat: &Beat, swing: f32) -> (f32, f32) {
        let b = &mut self.block;
        let length = b.length;
        let target = b.target;
        let fx = &b.fx;
        let steps = &self.step_params;
        let sr = f64::from(self.sample_rate);
        let grain_source = &self.grain_source[b.grain_lo..b.grain_hi];
        let source = if b.player_wired {
            &self.player_source[b.lo..b.hi]
        } else {
            grain_source
        };

        // A step that is on starts a pass, and the envelope with it. A
        // pass it cuts short becomes the tail and fades out over the same
        // few milliseconds the new pass fades in, so the step lands on
        // time without a click. Cut hard, it jumped by 0.49 against a
        // steady 0.03.
        //
        // Only the material is covered. Both envelopes read the new
        // pass's clock from this frame, so a shaped envelope (anything
        // but flat, which is the default) still puts an edge on the tail
        // and on the cloud. Open, roadmap row 4.
        self.fired = 0.0;
        if let Some(k) = self.steps.tick(steps, beat, swing) {
            self.fired = steps.velocity_of(k);
            // A pass that has played out has nothing to fade, and must
            // not replace a tail that is still fading.
            if self.player.sounding() {
                self.tail = self.player;
                self.tail_left = self.player.seam_samples();
                self.tail_ratio = self.pass_ratio;
            }
            (self.pass_ratio, self.pass_semis) = steps.pitch_of(k);
            self.vel_target = steps.velocity_of(k);
            // A step with its own hold gates its note for that many
            // sixteenths, instead of the patch's Hold time.
            let hold = steps.hold_of(k);
            if length == Length::Hold && hold > 0 {
                let gate = f64::from(hold) * f64::from(beat.len);
                let released = gate + b.note_release;
                let end = released + f64::from(BYPASS_MS) * 0.001 * sr;
                self.note.start(gate, released, end);
            } else {
                self.note.start(b.note_gate, b.note_released, b.note_end);
            }
            // A reel running backwards starts its pass from the far end,
            // or it would leave the window on its first sample.
            if tape.was < 0.0 {
                self.player.rewind_to_end(source.len());
            } else {
                self.player.rewind();
            }
        }
        let p = &mut b.p;
        p.position = self.smooth.position.process(target.position);
        p.jitter = self.smooth.jitter.process(target.jitter);
        p.size_ms = self.smooth.size.process(target.size_ms);
        p.density = self.smooth.density.process(target.density);
        // Added after smoothing: the octave is stepped and lands at once.
        // So is the step's pitch, which belongs to the pass.
        p.pitch = self.smooth.pitch.process(target.pitch) + b.grain_semis + self.pass_semis;
        p.pitch_spread = self.smooth.spread.process(target.pitch_spread);
        p.pan_spread = self.smooth.pan.process(target.pan_spread);
        p.speed = tape.speed;

        // Under Hold with nothing to trigger it, a note that has ended
        // starts again, the way a pass loops under Sample, so the patch
        // keeps sounding in sound scaping.
        if length == Length::Hold && b.steps_off && !b.once && self.note.age >= self.note.end {
            self.note.start(b.note_gate, b.note_released, b.note_end);
            if tape.speed < 0.0 {
                self.player.rewind_to_end(source.len());
            } else {
                self.player.rewind();
            }
        }
        let gate = (length == Length::Hold).then_some(self.note.gate as f32);

        // Two generators, summed (Georg, 2026-09-13). Each has a switch
        // that fades to an exact zero and a gain; on at unity is bit-exact.
        let grain_on = self.fades.grain.process(b.grain_on_target);
        // The cloud's level is its gain times its switch, applied inside
        // the cloud together with its overlap compensation.
        p.gain = self.smooth.grain_gain.process(target.gain) * grain_on;
        let material_level = self.smooth.material_gain.process(b.material_gain_target)
            * self.fades.material.process(b.material_on_target);
        // The player runs whether the material is switched on or not: the
        // playhead and both envelopes are clocked by it. The octave scales
        // the read, never `p.speed` itself: the tape's level fade below
        // reads speed alone, and an octave down must not sound like a reel
        // slowing to a stop.
        let read_speed = p.speed * b.read_ratio;
        let mut dry = self
            .player
            .process(source, read_speed * self.pass_ratio, b.looping);
        if self.tail_left > 0.0 {
            let fade = (self.tail_left / self.player.seam_samples()).sqrt();
            dry += self
                .tail
                .process(source, read_speed * self.tail_ratio, b.looping)
                * fade;
            self.tail_left -= 1.0;
        }
        // Scrub takes the sample over from its passes: they fade out as the
        // head fades in. Octave does not reach it, the hand is the speed.
        // Off never gets here once the fade has landed, which keeps the
        // sample bit-exact with a build that has no Scrub.
        let scrub_open = self.fades.scrub.process(b.scrub_target);
        if scrub_open > 0.0 {
            let heard = self.scrub.process(source, b.scrub_position);
            dry = dry * (1.0 - scrub_open) + heard * scrub_open;
        }
        // With nothing wired, the pass was only the clock, and what it
        // read was the cloud's material.
        let material = if b.player_wired {
            dry * material_level
        } else {
            0.0
        };
        // The cloud follows the pass: it throws grains while one sounds,
        // and between steps lets the grains it has thrown play out.
        p.spawning = match length {
            Length::Sample => self.player.sounding(),
            Length::Loop => true,
            Length::Hold => self.note.age < self.note.gate,
        };
        // Off means off. Once the fade has landed the cloud's contribution
        // is already an exact zero, so it is not run at all.
        let (gl, gr) = if grain_on > 0.0 {
            self.granular.process(grain_source, p)
        } else {
            (0.0, 0.0)
        };
        // FM plays on the tape: a step's pitch moves its note, and a brake
        // pulls it down with everything else. Looping, it drones; under
        // the steps it sounds while a pass does. Mono, into both sides.
        let fm_on = self.fades.fm.process(b.fm_on_target);
        let note_sounding = match length {
            Length::Sample => (b.steps_off && !b.once) || self.player.sounding(),
            Length::Loop => true,
            Length::Hold => self.note.age < self.note.end,
        };
        let open = self
            .fades
            .note_gate
            .process(if note_sounding { 1.0 } else { 0.0 });
        let fm_level = self.smooth.fm_gain.process(b.fm_gain_target) * fm_on * open;
        let fm = if fm_on > 0.0 {
            self.fm
                .process(&b.fm_params, self.pass_ratio * p.speed.abs())
                * fm_level
        } else {
            0.0
        };
        // Under Hold the generators fall to an exact zero once the note
        // has ended, with or without the Envelope node. Otherwise this is
        // exactly one, and changes nothing.
        let note_level = self.note_fade.process(
            if length == Length::Hold && self.note.age >= self.note.released {
                0.0
            } else {
                1.0
            },
        );
        // Noise follows the notes as FM does, through the same gate. Added
        // only while it sounds, so off leaves the sum exactly as it was.
        let noise_on = self.fades.noise.process(b.noise_on_target);
        let noise_level = self.smooth.noise_gain.process(b.noise_gain_target) * noise_on * open;
        let (mut l, mut r) = (material + gl + fm, material + gr + fm);
        if noise_on > 0.0 {
            let (nl, nr) = self
                .noise
                .process(&b.noise_params, self.pass_ratio * p.speed.abs());
            l += nl * noise_level;
            r += nr * noise_level;
        }
        let (l, r) = (l * note_level, r * note_level);

        // A step's velocity scales its note where it is made, before the
        // effects, so a soft step does not turn down the tails the loud
        // one before it left in a delay or a reverb.
        let vel = self.vel.process(self.vel_target);
        let (l, r) = (l * vel, r * vel);
        let (l, r) = self.fx.front(l, r, fx);

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
            amount: b.env.amount * self.fades.env.process(b.env_on_target),
            ..b.env
        };
        let elapsed = self.player.elapsed();
        let e = if length == Length::Sample {
            faded_env.gain_at(elapsed, b.window_len, self.sample_rate)
        } else {
            self.note.amp_shape = faded_env.shape_at_note(
                self.note.age as f32,
                gate,
                self.note.amp_from,
                self.sample_rate,
            );
            faded_env.gain_of(self.note.amp_shape)
        };
        let (l, r) = (l * e, r * e);

        // The filter, on the master: after every generator, effect and
        // the envelope, then the master gain.
        // The cutoff follows the step's pitch from the frame it lands on; the
        // filter's own cutoff smoother glides it. Off multiplies by exactly 1.
        let follow = |tracking: f32| {
            if tracking == 0.0 || self.pass_semis == 0.0 {
                1.0
            } else {
                (tracking * self.pass_semis * (1.0 / 12.0)).exp2()
            }
        };
        let (l, r) = self.fx.filter_tracked(l, r, fx, follow(b.filter_tracking));
        // The ladder, after the filter and before the output gain; its own
        // tracking, so the clean filter can stay put while it follows.
        let ladder = LadderParams {
            cutoff_hz: b.ladder.cutoff_hz * follow(b.ladder_tracking),
            ..b.ladder
        };
        let (l, r) = self.ladder.process(l, r, &ladder);
        let (l, r) = self.fx.output(l, r, fx);

        // The note clock runs on tape time, as the grain scheduler does:
        // a brake slows it, and the material's octave does not.
        self.note.age += f64::from(p.speed.abs());

        // The voice's own fade, in and out. Exactly one while it plays.
        let g = self.level.process(if self.active { 1.0 } else { 0.0 });
        (l * g, r * g)
    }

    /// After a block's frames: the playhead, and a cloud or FM switched off
    /// cleared once its fade has landed.
    pub(crate) fn end_block(&mut self) {
        let b = &self.block;
        // A playhead for the Sample's material, so none when nothing is wired.
        self.trimmed_play_position = if b.player_wired {
            let len = b.hi - b.lo;
            let span = self.player_source.len().max(1) as f32;
            let at = if self.fades.scrub.value() > 0.0 {
                self.scrub.position()
            } else {
                self.player.position(len)
            };
            (b.lo as f32 + at * len as f32) / span
        } else {
            0.0
        };

        // A cloud switched off is cleared once its fade has landed, so turning
        // it back on starts fresh rather than resuming grains frozen mid-window.
        if self.fades.grain.value() == 0.0 {
            self.granular.clear();
        }
        // FM likewise starts from the top of a cycle when switched back on.
        if self.fades.fm.value() == 0.0 {
            self.fm.reset();
        }
        // Noise's filters likewise start from rest.
        if self.fades.noise.value() == 0.0 {
            self.noise.reset();
        }

        // Faded out: idle from the next block, with nothing left running.
        if !self.active && self.level.value() == 0.0 {
            self.idle = true;
            self.granular.clear();
            self.tail_left = 0.0;
            self.steps.set_running(false);
        }
    }
}

/// How far into a stop the level starts following the speed down.
///
/// Only the last sliver, so the brake reads as a slowing reel for almost all
/// of its travel and only fades at the very end.
pub(crate) const TAPE_KNEE: f32 = 0.12;

/// Output level for a tape running at `speed`.
///
/// Returns exactly one above the knee — not a curve that merely approaches it.
/// Every other node here is bit-exact when neutral, and a gain of 0.9997 at
/// rest would break that quietly, in a way no existing test would catch.
pub(crate) fn tape_gain(speed: f32) -> f32 {
    let s = speed.abs();
    if s >= TAPE_KNEE {
        1.0
    } else {
        s / TAPE_KNEE
    }
}

/// A parameter as the voice should hear it: the hand's value from the bank,
/// bent by its link to an LFO if it has one. The bank itself is never written.
#[inline]
fn read(mods: &ModSet, bank: &ParamBank, slot: usize) -> f32 {
    mods.apply(slot, bank.get(slot))
}

fn window_from(v: f32) -> Window {
    Window::ALL[(v.round().max(0.0) as usize).min(Window::ALL.len() - 1)]
}
