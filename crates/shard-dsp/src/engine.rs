//! The engine: the arrangement around the voices. A voice (`voice.rs`) is one
//! track playing one patch; the engine holds a fixed pool of them, the
//! transport they all step on, the tape they all run on, the drums tracks'
//! kits beside them, the arrangement's chain over them and the limiter.
//!
//! **The pool (epic 28).** `VOICES` voices, every buffer sized here, in
//! `new`. Voice `i` reads its own bank (`process_voices`), modulation
//! (`set_voice_modulation`), materials (`swap_voice_source`), readings
//! (`set_voice_readings`) and steps (`set_voice_steps`), and is switched in
//! and out with `set_voice_active`, which fades it, and has its own fader,
//! its track's gain (`set_voice_gain`). Voice 0 starts active and
//! the rest idle; an idle voice is skipped whole and costs nothing. The calls
//! without `voice` in their name (`process_block`, `set_steps`,
//! `swap_source` and the rest) are the single-track shorthand: they address
//! voice 0, and `process_block` hands every voice the same bank.
//!
//! **One transport** (`steps::Transport`): tempo, swing and whether the steps
//! run are the arrangement's, set by `set_transport`. Every voice and kit
//! reads the same position each frame, so their step ones land on the same
//! sample and "the next bar" means one thing (`bar`).
//!
//! **Mute and solo** (`mix.rs`): each voice has its mix switches and a gate
//! after its fader. The host says what it wants (`set_voice_mix`); while the
//! transport runs a change lands on the first sample of the next bar, and
//! while it does not, at once. The gate ramps over `mix::GATE_MS`, so a
//! landing never clicks: silencing fades over the gate's length *before* the
//! bar line and is silent from it, so the downbeat's note never sounds
//! through the fade; bringing back fades in from the bar line. A kit has
//! the same switches as a voice, and solos count across both pools.
//!
//! **Drums tracks** (story d): `KITS` kits, each a drums track's, with its
//! own grid and sound (`set_kit`), fader (`set_kit_gain`), mix
//! (`set_kit_mix`) and switch (`set_kit_active`). Kit `k`'s noise is salted
//! by `k`, so kit 0 keeps the seed the single kit had. A kit switched out
//! rings out and then costs nothing. Kit 0 starts active, its switch off,
//! as the single kit did; the rest idle.
//!
//! **One tape.** Brake and rewind are the arrangement's rows
//! (`arrangement.tape.brake`, `arrangement.tape.reverse`), and slow every
//! voice, every kit and the transport together (Georg, 2026-10-12). The slew
//! and the flick length are still a patch's rows, read from voice 0.
//!
//! Everything here runs on the audio thread. No allocation, no locks, no
//! logging, no file access. The only things crossing in from outside are the
//! atomic parameter banks, each read once per block.

use std::sync::Arc;

use crate::arrangement::{self, index_of_arrangement};
use crate::chain::{Chain, ChainParams, ChainSlots};
use crate::inspect::{GrainLog, GrainSpawn};
use crate::kit::{Kit, KitClock, KitParams, KitPattern, VOICES as KIT_VOICES};
use crate::limiter::{Limiter, LimiterParams};
use crate::mix::{gate_frames, Gate, Mix};
use crate::modulation::ModSet;
use crate::params::ParamBank;
use crate::rng::salt;
use crate::smooth::OnePole;
use crate::sources::{Generator, Reading};
use crate::steps::{StepParams, Transport, BAR};
use crate::voice::{tape_gain, Tape, Voice};

/// How many voices the engine holds: the most melody tracks that can play at
/// once (epic 28). Fixed, so every buffer is sized when the engine is built.
pub const VOICES: usize = 8;

/// How many kits the engine holds: the most drums tracks that can play at
/// once (epic 28, story d). Fixed, like `VOICES`.
pub const KITS: usize = 4;

/// Every switchable channel: the voices, then the kits. The mix arrays are
/// indexed this way, so mute and solo are one rule across both pools.
const CHANNELS: usize = VOICES + KITS;

/// Where the arrangement's rows sit in its own table.
struct ArrangementSlots {
    fx: ChainSlots,
    limit: usize,
    ceiling: usize,
    tape_brake: usize,
    tape_reverse: usize,
}

impl ArrangementSlots {
    fn resolve() -> Self {
        let at = |id: &str| {
            let full = format!("{}{id}", arrangement::PREFIX);
            index_of_arrangement(&full).unwrap_or_else(|| panic!("missing arrangement row: {full}"))
        };
        Self {
            fx: ChainSlots::resolve(at),
            limit: at("amp.limit"),
            ceiling: at("amp.ceiling"),
            tape_brake: at("tape.brake"),
            tape_reverse: at("tape.reverse"),
        }
    }
}

/// The arrangement's values for one block, as `set_arrangement` last read
/// them. Copied in whole, so the audio thread never holds the host's bank.
#[derive(Clone, Copy)]
struct ArrangementBlock {
    fx: ChainParams,
    /// The patch alone: every track's fader at unity and the drums silent.
    /// See `read`.
    patch_alone: bool,
    limiter: LimiterParams,
    /// The tape's gestures: how far the brake is down, 0 to 1, and whether
    /// the reverse gate is held.
    brake: f32,
    reverse: bool,
}

impl ArrangementBlock {
    /// Read once a block. No LFO moves an arrangement row yet, so every row is
    /// read as the hand set it.
    fn read(s: &ArrangementSlots, value: impl Fn(usize) -> f32 + Copy, patch_alone: bool) -> Self {
        let mut fx = ChainParams::read(&s.fx, value, value);
        // The patch alone, as sound scaping hears it (Georg, 2026-09-26). Not
        // a separate path: the arrangement's chain is emptied, the tracks'
        // faders go to unity and the drums' to zero, so the ramps fade the change
        // in 10 ms, and once they
        // land the chain is bit-exact with no arrangement at all. The filter
        // keeps running, so one switched back in has no stale state. The
        // tape is everyone's, so the patch alone still brakes.
        if patch_alone {
            fx = fx.without_effects();
        }
        Self {
            fx,
            patch_alone,
            limiter: LimiterParams {
                ceiling: value(s.ceiling),
                on: value(s.limit) >= 0.5,
            },
            brake: value(s.tape_brake).clamp(0.0, 1.0),
            reverse: value(s.tape_reverse) >= 0.5,
        }
    }
}

/// The tape every voice, every kit and the transport run on: one speed, a
/// multiplier on tape time. Brake and reverse are two ways of asking for a
/// different one, and they share one slew, so flipping direction slows to a
/// stop and climbs back the other way, exactly as a reel does.
struct Reel {
    /// The tape's inertia. Its time constant is the patch's `tape.time`,
    /// reset per block, so the brake's feel is a control, not a constant.
    speed: OnePole,
    /// Reverse, resolved from the gate. See `resolve`.
    reversing: bool,
    /// Samples since reverse engaged, for the flick's minimum on-time.
    reverse_age: f32,
}

impl Reel {
    fn new() -> Self {
        // Unity, not a parameter default: a brake's default of zero would
        // pitch the instrument up from a standstill on every launch.
        let mut speed = OnePole::new();
        speed.reset(1.0);
        Self {
            speed,
            reversing: false,
            reverse_age: 0.0,
        }
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
    fn resolve(&mut self, gate: bool, flick_samples: f32, frames: f32) {
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
}

/// One drums track's kit in the pool (story d): its grid, its sound, its
/// place in its own loop and its fader. Its hits, step by step, are what a
/// sidechain keyed by a drums track would follow (roadmap row 41): not built.
struct Drums {
    kit: Kit,
    pattern: KitPattern,
    params: KitParams,
    clock: KitClock,
    /// Its track's fader as the host last set it, and the glide.
    gain_target: f32,
    gain: OnePole,
    /// Switched in: it hits. Switched out, it rings out and goes idle.
    active: bool,
    /// Woken while the transport runs: the next tick only finds its place,
    /// so a kit switched in never fires in the middle of a step.
    rejoin: bool,
}

impl Drums {
    fn new(sample_rate: f32, index: usize) -> Self {
        let mut gain = OnePole::new();
        gain.set_time(arrangement::TRACK_GAIN_MS, sample_rate);
        // Silent until its switch is on, as the single kit started.
        gain.reset(0.0);
        Self {
            kit: Kit::seeded(sample_rate, salt(index)),
            pattern: KitPattern::default(),
            params: KitParams::default(),
            clock: KitClock::default(),
            gain_target: arrangement::TRACK_GAIN_DEFAULT,
            gain,
            active: false,
            rejoin: false,
        }
    }

    /// Skipped whole: switched out and nothing left ringing.
    #[inline]
    fn idle(&self) -> bool {
        !self.active && !self.kit.sounding()
    }

    /// Where its fader is headed: its gain while it is on, else silence.
    #[inline]
    fn level(&self, patch_alone: bool) -> f32 {
        if self.params.on && !patch_alone {
            self.gain_target
        } else {
            0.0
        }
    }
}

pub struct Engine {
    /// The pool. Boxed, so the engine itself stays small to move.
    voices: Box<[Voice]>,
    /// Transport. Stopped means silence out and no grains left hanging.
    playing: bool,
    /// The steps' clock, shared by every voice and kit.
    transport: Transport,
    /// The tape's speed, shared the same way.
    reel: Reel,
    /// The arrangement's chain, over the voices.
    arr_slots: ArrangementSlots,
    arr_fx: Chain,
    arr: ArrangementBlock,
    /// Each voice's fader into it, its track's gain: what the host last set
    /// (`set_voice_gain`) and where the glide has got to.
    gain_targets: [f32; VOICES],
    gains: [OnePole; VOICES],
    /// Each channel's mix switches, the voices' then the kits'
    /// (`CHANNELS`): what the host wants, what is applied (the next bar line
    /// copies one onto the other), whether that is heard, and the gate
    /// carrying it.
    mix_wanted: [Mix; CHANNELS],
    mix: [Mix; CHANNELS],
    audible: [bool; CHANNELS],
    /// Who will be heard once the wanted mix lands: the gates' targets for
    /// the last stretch before the bar.
    wanted_audible: [bool; CHANNELS],
    /// The gate's length in frames: how far before the bar silencing starts.
    gate_frames: f64,
    gates: [Gate; CHANNELS],
    /// The drums tracks' kits. See `kit.rs`.
    drums: [Drums; KITS],
    /// After everything, the arrangement included.
    limiter: Limiter,
    sample_rate: f32,
    peak: f32,
}

impl Engine {
    pub fn new(sample_rate: f32, max_grains: usize) -> Self {
        let arr_defs = arrangement::params();
        let arr_slots = ArrangementSlots::resolve();
        let arr = ArrangementBlock::read(&arr_slots, |slot| arr_defs[slot].default, false);
        let gains = std::array::from_fn(|_| {
            let mut g = OnePole::new();
            g.set_time(arrangement::TRACK_GAIN_MS, sample_rate);
            g.reset(arrangement::TRACK_GAIN_DEFAULT);
            g
        });
        let mut drums: [Drums; KITS] = std::array::from_fn(|k| Drums::new(sample_rate, k));
        // Kit 0 starts switched in, its switch off, as the single kit did.
        drums[0].active = true;
        let steps = StepParams::default();
        let mut transport = Transport::new(sample_rate);
        transport.set(&steps);

        Self {
            voices: (0..VOICES)
                .map(|i| Voice::new(sample_rate, max_grains, i))
                .collect(),
            playing: false,
            transport,
            reel: Reel::new(),
            arr_fx: Chain::new(sample_rate, arr_defs, &arr_slots.fx, 0),
            arr_slots,
            arr,
            gain_targets: [arrangement::TRACK_GAIN_DEFAULT; VOICES],
            gains,
            mix_wanted: [Mix::default(); CHANNELS],
            mix: [Mix::default(); CHANNELS],
            audible: [true; CHANNELS],
            wanted_audible: [true; CHANNELS],
            gate_frames: gate_frames(sample_rate) as f64,
            gates: [Gate::new(sample_rate); CHANNELS],
            drums,
            limiter: Limiter::new(sample_rate),
            sample_rate,
            peak: 0.0,
        }
    }

    // The voices. `i` is below `VOICES`; anything else is a programming
    // error and panics.

    /// Switch voice `i` in or out. In, it starts clean (nothing of what it
    /// played before) and fades in; out, it fades out and then goes idle,
    /// where it is skipped whole. At a block boundary, like every hand-off.
    pub fn set_voice_active(&mut self, i: usize, on: bool) {
        if on && self.voices[i].idle() {
            // Woken clean, its fader and its gate already where its track
            // has them.
            self.gains[i].reset(self.gain_targets[i]);
            self.gates[i].reset(self.audible[i]);
        }
        self.voices[i].set_active(on);
    }

    /// Voice `i`'s fader: its track's gain, 0 to `TRACK_GAIN_MAX`, glided
    /// over `TRACK_GAIN_MS`. Once a block, like the steps.
    pub fn set_voice_gain(&mut self, i: usize, gain: f32) {
        self.gain_targets[i] = if gain.is_finite() {
            gain.clamp(0.0, arrangement::TRACK_GAIN_MAX)
        } else {
            arrangement::TRACK_GAIN_DEFAULT
        };
    }

    /// Voice `i`'s mute and solo, as the host wants them. Once a block is
    /// enough. While the transport runs, the change lands on the first sample
    /// of the next bar, for every voice at once, and wanting the applied
    /// state again before then cancels it. While it does not run (stopped,
    /// or the steps off) it applies at once.
    pub fn set_voice_mix(&mut self, i: usize, mix: Mix) {
        assert!(i < VOICES);
        self.want_mix(i, mix);
    }

    /// `set_voice_mix` on channel `c`, a voice's or a kit's.
    fn want_mix(&mut self, c: usize, mix: Mix) {
        self.mix_wanted[c] = mix;
        if !self.transport.running() && self.mix[c] != mix {
            self.mix[c] = mix;
            self.resolve_mix();
        }
    }

    /// `place_voice_mix` on channel `c`.
    fn place_mix(&mut self, c: usize, mix: Mix) {
        self.mix_wanted[c] = mix;
        if self.mix[c] != mix {
            self.mix[c] = mix;
            self.resolve_mix();
        }
    }

    /// Voice `i`'s mix, applied at once rather than on the bar: for a track
    /// arriving on the voice (a track removed before it, an undo, a file
    /// opened), whose switches go with it. The gate still ramps.
    pub fn place_voice_mix(&mut self, i: usize, mix: Mix) {
        assert!(i < VOICES);
        self.place_mix(i, mix);
    }

    /// Voice `i`'s mix as applied: what is heard, or fading towards it.
    pub fn voice_mix(&self, i: usize) -> Mix {
        self.mix[i]
    }

    /// Whether voice `i` is heard under the applied mutes and solos.
    pub fn voice_audible(&self, i: usize) -> bool {
        self.audible[i]
    }

    // The kits. `k` is below `KITS`.

    /// Switch kit `k` in or out. In, it starts from its place in the loop
    /// (never mid-step) with its fader and gate where its track has them;
    /// out, no new hit starts and what rings rings out, then it is skipped.
    pub fn set_kit_active(&mut self, k: usize, on: bool) {
        let c = VOICES + k;
        let level = self.drums[k].level(self.arr.patch_alone);
        let running = self.transport.running();
        let audible = self.audible[c];
        let d = &mut self.drums[k];
        if on && d.idle() {
            d.clock.reset();
            d.rejoin = running;
            d.gain.reset(level);
            self.gates[c].reset(audible);
        }
        d.active = on;
    }

    pub fn kit_active(&self, k: usize) -> bool {
        self.drums[k].active
    }

    /// Kit `k`'s grid and sound, once a block, as a track's steps are.
    pub fn set_kit(&mut self, k: usize, pattern: KitPattern, params: KitParams) {
        let d = &mut self.drums[k];
        d.pattern = pattern;
        d.params = params;
    }

    /// Kit `k`'s fader, as `set_voice_gain`.
    pub fn set_kit_gain(&mut self, k: usize, gain: f32) {
        self.drums[k].gain_target = if gain.is_finite() {
            gain.clamp(0.0, arrangement::TRACK_GAIN_MAX)
        } else {
            arrangement::TRACK_GAIN_DEFAULT
        };
    }

    /// Kit `k`'s mute and solo, landing on the bar as `set_voice_mix`.
    pub fn set_kit_mix(&mut self, k: usize, mix: Mix) {
        self.want_mix(VOICES + k, mix);
    }

    /// Kit `k`'s mix at once, as `place_voice_mix`.
    pub fn place_kit_mix(&mut self, k: usize, mix: Mix) {
        self.place_mix(VOICES + k, mix);
    }

    pub fn kit_mix(&self, k: usize) -> Mix {
        self.mix[VOICES + k]
    }

    /// Whether kit `k` is heard under the applied mutes and solos.
    pub fn kit_audible(&self, k: usize) -> bool {
        self.audible[VOICES + k]
    }

    /// The step kit `k` is in, or none while the steps are off or it is idle.
    pub fn kit_step(&self, k: usize) -> Option<u32> {
        if self.transport.running() && !self.drums[k].idle() {
            self.drums[k].clock.current()
        } else {
            None
        }
    }

    /// Every wanted mix applied: on a bar line.
    #[inline]
    fn land_mix(&mut self) {
        if self.mix != self.mix_wanted {
            self.mix = self.mix_wanted;
            self.resolve_mix();
        }
    }

    /// Who is heard under the applied mix. While stopped the gates jump
    /// there, since nothing is sounding to click.
    fn resolve_mix(&mut self) {
        let any_solo = self.mix.iter().any(|m| m.solo);
        for (a, m) in self.audible.iter_mut().zip(&self.mix) {
            *a = m.audible(any_solo);
        }
        if !self.playing {
            for (g, a) in self.gates.iter_mut().zip(&self.audible) {
                g.reset(*a);
            }
        }
    }

    /// Whether voice `i` is switched in. A voice switched out may still be
    /// fading.
    pub fn voice_active(&self, i: usize) -> bool {
        self.voices[i].active()
    }

    /// Whether voice `i` has faded out and is skipped: silent, so its
    /// materials can be swapped under it without a cut.
    pub fn voice_idle(&self, i: usize) -> bool {
        self.voices[i].idle()
    }

    /// Replace one of voice `i`'s materials and hand back the old buffer.
    ///
    /// Safe on the audio thread at a block boundary, because nothing here
    /// touches the allocator: the new buffer arrives already built, and the
    /// old one is returned rather than dropped. Freeing it takes the
    /// allocator's lock just as building it did, so the caller passes it to a
    /// thread that is allowed to block. Never from inside `process_block`.
    ///
    /// Each generator reads its own material (Georg, 2026-09-15), so swapping
    /// one leaves the other playing: a new source for the player restarts its
    /// pass, and one for the cloud clears its grains.
    pub fn swap_voice_source(&mut self, i: usize, which: Generator, samples: Vec<f32>) -> Vec<f32> {
        self.voices[i].swap_source(which, samples)
    }

    /// Both of voice `i`'s generators read `samples`. For tests, the CLI and
    /// startup: it copies the buffer and frees the old ones, so never on the
    /// audio thread.
    pub fn set_voice_source(&mut self, i: usize, samples: Vec<f32>) {
        drop(self.swap_voice_source(i, Generator::Grain, samples.clone()));
        drop(self.swap_voice_source(i, Generator::Player, samples));
    }

    /// How voice `i`'s generators read their materials: which part, at what
    /// octave. Handed in once a block, as the steps are.
    pub fn set_voice_readings(&mut self, i: usize, player: Reading, grain: Reading) {
        self.voices[i].set_readings(player, grain);
    }

    /// Swap in a new set of LFOs and links for voice `i`, and hand back the
    /// old one: built on the command thread, swapped at a block boundary, and
    /// the old set dropped anywhere but here. LFOs in both sets keep running,
    /// matched by id, so adding an LFO or changing a rate never restarts the
    /// others.
    pub fn set_voice_modulation(&mut self, i: usize, next: ModSet) -> ModSet {
        self.voices[i].set_modulation(next)
    }

    /// What voice `i`'s track plays: whether it follows the steps, its
    /// pattern, length, pitches, holds, velocities and nudges. Its tempo
    /// and swing are ignored: those are the transport's
    /// (`set_transport`).
    pub fn set_voice_steps(&mut self, i: usize, p: StepParams) {
        self.voices[i].set_steps(p);
    }

    /// The arrangement's half of the steps, once a block: whether they run,
    /// the tempo and the swing.
    pub fn set_transport(&mut self, p: &StepParams) {
        self.transport.set(p);
    }

    // The single-track shorthand: voice 0.

    /// `swap_voice_source` on voice 0.
    pub fn swap_source(&mut self, which: Generator, samples: Vec<f32>) -> Vec<f32> {
        self.swap_voice_source(0, which, samples)
    }

    /// `set_voice_source` on voice 0.
    pub fn set_source(&mut self, samples: Vec<f32>) {
        self.set_voice_source(0, samples);
    }

    /// `set_voice_readings` on voice 0.
    pub fn set_readings(&mut self, player: Reading, grain: Reading) {
        self.set_voice_readings(0, player, grain);
    }

    /// `set_voice_modulation` on voice 0.
    pub fn set_modulation(&mut self, next: ModSet) -> ModSet {
        self.set_voice_modulation(0, next)
    }

    /// The tracker as one track: the transport from `p`, and `p` as voice
    /// 0's steps.
    pub fn set_steps(&mut self, p: StepParams) {
        self.set_transport(&p);
        self.set_voice_steps(0, p);
    }

    /// Every parameter as voice 0 hears it, LFOs included, written into
    /// `heard`, so the UI can draw where a linked control has been moved to.
    /// The hand's `bank` is only read. Atomic stores only, so it is safe to
    /// call from the audio callback once a block.
    pub fn publish_heard(&self, bank: &ParamBank, heard: &ParamBank) {
        self.publish_voice_heard(0, bank, heard);
    }

    /// `publish_heard` for voice `i`, which reads `bank`.
    pub fn publish_voice_heard(&self, i: usize, bank: &ParamBank, heard: &ParamBank) {
        self.voices[i].publish_heard(bank, heard);
    }

    /// Voice 0's spawn log, for whatever wants to draw it. Cloning the handle
    /// is the only way out of the audio thread; nothing here hands out a
    /// `&mut`.
    pub fn grain_log(&self) -> Arc<GrainLog> {
        self.voices[0].grain_log()
    }

    /// Play one logged grain on its own, on voice 0.
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
        self.voices[0].audition(spawn)
    }

    pub fn auditioning(&self) -> bool {
        self.voices[0].auditioning()
    }

    /// Whether the tape is running backwards, which is not the same as whether
    /// the reverse gate is held. For the UI, so the button can stay lit for as
    /// long as the reel is actually turning the other way.
    pub fn reversing(&self) -> bool {
        self.reel.reversing
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Start or stop. Stopping clears every grain pool rather than letting
    /// the clouds ring out, so stop means stop.
    pub fn set_playing(&mut self, playing: bool) {
        for v in self.voices.iter_mut() {
            v.set_playing(playing, self.playing);
        }
        // A flick left mid-flight when the transport stopped would still be
        // engaged when it started again, which is a surprise rather than a
        // gesture.
        self.reel.reversing = false;
        self.reel.reverse_age = 0.0;
        // The steps stop with the tape, and the next block starts them again
        // from step one.
        self.transport.stop();
        // The kits too: no drum rings on into the next play.
        for d in &mut self.drums {
            d.kit.reset();
            d.clock.reset();
            d.rejoin = false;
        }
        self.playing = playing;
        // A mute still waiting for its bar lands now: stopped, there is none.
        self.mix = self.mix_wanted;
        self.resolve_mix();
    }

    /// Start voice 0's pass again from the top, with nothing of the last one
    /// left: no tail fading out and no grains in flight. Smoothers, fades,
    /// LFOs and filter state carry on. For an offline render, which settles
    /// the patch first and then captures one clean pass; see `preview`.
    pub fn restart_pass(&mut self) {
        self.voices[0].restart_pass();
    }

    /// Where voice 0's plain playback has reached, 0 to 1 across the *whole*
    /// source, so the drawn playhead lines up with the drawn waveform rather
    /// than with the trimmed window.
    pub fn play_position(&self) -> f32 {
        self.voice_play_position(0)
    }

    /// `play_position` for voice `i`.
    pub fn voice_play_position(&self, i: usize) -> f32 {
        self.voices[i].play_position()
    }

    /// How long one of voice 0's materials is, in samples.
    pub fn source_len(&self, which: Generator) -> usize {
        self.voices[0].source_len(which)
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Grains sounding in voice 0's cloud.
    pub fn active_grains(&self) -> usize {
        self.voice_active_grains(0)
    }

    /// Grains sounding in voice `i`'s cloud.
    pub fn voice_active_grains(&self, i: usize) -> usize {
        self.voices[i].active_grains()
    }

    /// The step voice 0's track is in, or none while it is off. For the UI.
    pub fn current_step(&self) -> Option<u32> {
        self.voice_step(0)
    }

    /// The step voice `i`'s track is in, or none while it is off.
    pub fn voice_step(&self, i: usize) -> Option<u32> {
        self.voices[i].clock().current()
    }

    /// Bars of the transport completed since the steps started, or none
    /// while they are stopped. A bar is sixteen sixteenths of the shared
    /// clock, whatever any track's length, so "the next bar" is the same
    /// sample for every track: the one this count changes on.
    pub fn bar(&self) -> Option<u64> {
        self.transport
            .running()
            .then(|| self.transport.beat().bar())
    }

    /// The arrangement's values, read from its bank once a block, like the
    /// steps. `bank` holds `arrangement::params()`. With `patch_alone` the
    /// arrangement's chain and the fader step aside and only the limiter
    /// remains, which is how sound scaping hears a patch.
    pub fn set_arrangement(&mut self, bank: &ParamBank, patch_alone: bool) {
        self.arr = ArrangementBlock::read(&self.arr_slots, |slot| bank.get(slot), patch_alone);
    }

    /// Peak since the last call, then reset. Cheap enough to poll at 30 Hz
    /// for a meter.
    pub fn take_peak(&mut self) -> f32 {
        core::mem::take(&mut self.peak)
    }

    /// How late the output is, in frames: the limiter's look-ahead.
    pub fn latency(&self) -> usize {
        self.limiter.latency()
    }

    /// The lowest gain the limiter applied since the last call, one meaning
    /// none. For the meter.
    pub fn take_reduction(&mut self) -> f32 {
        self.limiter.take_reduction()
    }

    /// Render interleaved stereo into `out`, which must have an even length,
    /// with every voice reading `bank`: the single-track shorthand, and two
    /// voices on one patch.
    pub fn process_block(&mut self, out: &mut [f32], bank: &ParamBank) {
        self.process_voices(out, &[bank; VOICES]);
    }

    /// Render interleaved stereo into `out`, which must have an even length.
    /// Voice `i` reads `banks[i]`; an idle voice's bank is not read at all.
    pub fn process_voices(&mut self, out: &mut [f32], banks: &[&ParamBank; VOICES]) {
        if !self.playing {
            if self.voices[0].auditioning() {
                self.voices[0].render_audition(out, banks[0], &mut self.peak);
            } else {
                out.fill(0.0);
            }
            return;
        }

        // The steps starting: the kits' loops start with them, before any
        // voice starts its own clock.
        if self.transport.begin_block() {
            for d in &mut self.drums {
                d.clock.reset();
                d.rejoin = false;
            }
        }
        // No bar to wait for while the steps are off.
        if !self.transport.running() {
            self.land_mix();
        }
        // A change waiting for its bar: who will be heard once it lands.
        let waiting = self.mix != self.mix_wanted;
        if waiting {
            let any_solo = self.mix_wanted.iter().any(|m| m.solo);
            for (w, m) in self.wanted_audible.iter_mut().zip(&self.mix_wanted) {
                *w = m.audible(any_solo);
            }
        }
        let frames = out.len() / 2;
        // The voices that play this block. One that goes idle does so at
        // the end of a block, so this holds for every frame of it.
        let mut live = [0usize; VOICES];
        let mut n = 0;
        for (i, v) in self.voices.iter_mut().enumerate() {
            if !v.idle() {
                v.begin_block(banks[i], frames, &self.transport);
                live[n] = i;
                n += 1;
            }
        }
        let live = &live[..n];
        // The kits that play this block, the same way: one switched out
        // still rings out, and only then is skipped, its clock with it.
        let mut kits = [0usize; KITS];
        let mut kn = 0;
        for (k, d) in self.drums.iter().enumerate() {
            if !d.idle() {
                kits[kn] = k;
                kn += 1;
            }
        }
        let kits = &kits[..kn];

        let arr = self.arr;
        // The tape: its gestures from the arrangement, its feel from voice 0.
        let (time_ms, flick_ms) = self.voices[0].tape_feel(banks[0]);
        self.reel.resolve(
            arr.reverse,
            flick_ms * 0.001 * self.sample_rate,
            frames as f32,
        );
        let direction = if self.reel.reversing { -1.0 } else { 1.0 };
        let speed_target = direction * (1.0 - arr.brake);
        self.reel.speed.set_time(time_ms, self.sample_rate);

        // Who the arrangement's order names this block. Reorders, resets and
        // fades start here.
        self.arr_fx.prepare(&arr.fx);
        let swing = self.transport.swing();

        for frame in out.chunks_mut(2) {
            let was = self.reel.speed.value();
            let speed = self.reel.speed.process(speed_target);
            let tape = Tape { speed, was };
            // This frame's place, read by every voice and kit before the
            // transport moves on.
            let beat = *self.transport.beat();
            // The last gate's length before a bar line a change waits for:
            // whatever will be silent from the bar fades out now, so it is
            // silent on the downbeat. Only bringing back waits for the bar.
            let closing = waiting
                && self.mix != self.mix_wanted
                && self.transport.running()
                && beat.step % BAR == BAR - 1
                && f64::from(beat.len) - beat.offset <= self.gate_frames * f64::from(speed.abs());
            // Each voice through its track's fader. The patch alone hears
            // every voice at unity, and the glide carries the change.
            let (mut l, mut r) = (0.0, 0.0);
            for &i in live {
                let (vl, vr) = self.voices[i].frame(tape, &beat, swing);
                let want = if arr.patch_alone {
                    1.0
                } else {
                    self.gain_targets[i]
                };
                let heard = self.audible[i] && (!closing || self.wanted_audible[i]);
                let g = self.gains[i].process(want) * self.gates[i].process(heard);
                l += vl * g;
                r += vr * g;
            }
            // The kits, on the same clock: each reads how far the steps have
            // run, so it stays on their grid at any length of its own. Its
            // clock runs while its switch is off, so switching it on lands in
            // step.
            if self.transport.running() {
                for &k in kits {
                    let d = &mut self.drums[k];
                    let pattern = &d.pattern;
                    let at = beat.elapsed(pattern.length.clamp(1, crate::steps::STEPS as u32));
                    if let Some(step) = d.clock.tick(at, pattern, swing) {
                        if d.rejoin {
                            d.rejoin = false;
                        } else if d.active && d.params.on {
                            for v in 0..KIT_VOICES {
                                d.kit.hit(v, pattern.hit(v, step), &d.params);
                            }
                        }
                    }
                }
            }
            // A bar began: the next frame is its first, and every waiting
            // mute and solo lands on it.
            if self.transport.advance(speed) {
                self.land_mix();
            }

            // Into the arrangement's own drive, crush, ring, chorus, filter
            // and output. It has no pass, so its crusher has no envelope.
            // The kits join here, beside the voices past their faders: past
            // no patch, into the arrangement's chain. On the tape like them,
            // so a brake slows and fades them too. Mono, into both sides,
            // each through its track's fader and gate.
            let mut kit = 0.0;
            for &k in kits {
                let c = VOICES + k;
                let d = &mut self.drums[k];
                let level = d.level(arr.patch_alone);
                let heard = self.audible[c] && (!closing || self.wanted_audible[c]);
                kit += d.kit.process_at(speed.abs())
                    * tape_gain(speed)
                    * d.gain.process(level)
                    * self.gates[c].process(heard);
            }
            let (l, r) = self.arr_fx.front(l + kit, r + kit, &arr.fx);
            let (l, r) = self.arr_fx.back(l, r, &arr.fx);

            // The limiter holds peaks under the ceiling, a millisecond late.
            // The clip after it is a safety net for when it is switched off:
            // a hard clip beats handing the device something that wraps.
            let (l, r) = self.limiter.process(l, r, &arr.limiter);
            let l = l.clamp(-1.0, 1.0);
            let r = r.clamp(-1.0, 1.0);

            self.peak = self.peak.max(l.abs()).max(r.abs());

            if let [lo, ro] = frame {
                *lo = l;
                *ro = r;
            }
        }

        for &i in live {
            self.voices[i].end_block();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::index_of;
    use crate::voice::{tape_gain, TAPE_KNEE};

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
    fn each_generator_reads_its_own_material() {
        // Sample and Granular wired to different materials (Georg,
        // 2026-09-15). Silence in one and a tone in the other shows which
        // generator reads which.
        let peak = |player: Vec<f32>, grain: Vec<f32>, material_on: f32, grain_on: f32| {
            let mut e = Engine::new(48_000.0, 128);
            drop(e.swap_source(Generator::Player, player));
            drop(e.swap_source(Generator::Grain, grain));
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "material.on", material_on);
            set(&bank, "grain.on", grain_on);
            set(&bank, "grain.gain", 1.0);
            let mut out = vec![0.0; 512];
            let mut peak = 0.0f32;
            for block in 0..200 {
                e.process_block(&mut out, &bank);
                // Measured once the switches have landed: Sample starts on and
                // takes its 10 ms to fade out, which is not a routing fault.
                if block >= 20 {
                    peak = peak.max(out.iter().fold(0.0f32, |a, s| a.max(s.abs())));
                }
            }
            peak
        };
        let silence = || vec![0.0; 48_000];
        assert!(
            peak(tone(48_000), silence(), 1.0, 0.0) > 0.01,
            "the player should read its own material"
        );
        assert!(
            peak(silence(), tone(48_000), 0.0, 1.0) > 0.01,
            "the cloud should read its own material"
        );
        assert_eq!(
            peak(tone(48_000), silence(), 0.0, 1.0),
            0.0,
            "the cloud heard the player's material"
        );
        assert_eq!(
            peak(silence(), tone(48_000), 1.0, 0.0),
            0.0,
            "the player heard the cloud's material"
        );
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
        bank.set_by_id("fx.2.ring.mix", 1.0);
        // Every effect in, or the range check is only checking the dry path.
        set(&bank, "fx.2.ring.on", 1.0);
        set(&bank, "fx.1.crush.on", 1.0);
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
        // A full-scale tone sits above the limiter's ceiling; this test is
        // about the path, not the limiter, so hold it off.
        e.set_arrangement(&limiter_off(), false);

        let mut player = crate::player::Player::new(48_000.0);
        let mut out = vec![0.0; 512];
        let mut worst = 0.0f32;
        // The output runs a look-ahead late; the expectation waits with it.
        let mut expected = std::collections::VecDeque::from(vec![0.0; e.latency()]);
        for _ in 0..90 {
            e.process_block(&mut out, &bank);
            for frame in out.chunks(2) {
                expected.push_back(player.process(&src, 1.0, true));
                let want = expected.pop_front().unwrap();
                worst = worst.max((frame[0] - want).abs());
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
            // Linearity is the point here, and the limiter is not linear.
            e.set_arrangement(&limiter_off(), false);
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
            e.set_arrangement(&limiter_off(), false);
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
        let cases: [(&str, &str, Settings, Settings, bool); 14] = [
            ("material", "material.on", &[], &[], false),
            ("granular", "grain.on", &[], &[], false),
            (
                "fm",
                "fm.on",
                &[("fm.index", 4.0), ("fm.feedback", 0.5)],
                &[],
                false,
            ),
            (
                "crush",
                "fx.1.crush.on",
                &[("fx.1.crush.mix", 1.0), ("fx.1.crush.bits", 4.0)],
                &[("fx.1.crush.mix", 0.0), ("fx.1.crush.bits", 4.0)],
                true,
            ),
            (
                "ring",
                "fx.2.ring.on",
                &[("fx.2.ring.mix", 1.0)],
                &[("fx.2.ring.mix", 0.0)],
                true,
            ),
            (
                "chorus",
                "fx.3.chorus.on",
                &[("fx.3.chorus.mix", 1.0)],
                &[("fx.3.chorus.mix", 0.0)],
                true,
            ),
            (
                "delay",
                "fx.7.delay.on",
                &[("fx.7.delay.mix", 1.0)],
                &[("fx.7.delay.mix", 0.0)],
                true,
            ),
            (
                "flanger",
                "fx.5.flanger.on",
                &[("fx.5.flanger.mix", 1.0)],
                &[("fx.5.flanger.mix", 0.0)],
                true,
            ),
            (
                "wear",
                "fx.6.wear.on",
                &[("fx.6.wear.mix", 1.0)],
                &[("fx.6.wear.mix", 0.0)],
                true,
            ),
            (
                "echo",
                "fx.8.echo.on",
                &[("fx.8.echo.mix", 1.0)],
                &[("fx.8.echo.mix", 0.0)],
                true,
            ),
            (
                "rise",
                "fx.9.rise.on",
                &[("fx.9.rise.mix", 1.0)],
                &[("fx.9.rise.mix", 0.0)],
                true,
            ),
            (
                "overtone",
                "fx.4.overtone.on",
                &[("fx.4.overtone.mix", 1.0)],
                &[("fx.4.overtone.mix", 0.0)],
                true,
            ),
            (
                "reverb",
                "fx.10.reverb.on",
                &[("fx.10.reverb.mix", 1.0)],
                &[("fx.10.reverb.mix", 0.0)],
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
            set(&bank, "fx.2.ring.mix", 1.0);
            // The steady baseline is both sections on and audible.
            set(&bank, "grain.on", 1.0);
            set(&bank, "fx.2.ring.on", 1.0);
            let mut out = vec![0.0; 256];
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            for block in 0..400 {
                if toggle {
                    let on = if (block / 25) % 2 == 0 { 1.0 } else { 0.0 };
                    set(&bank, "grain.on", on);
                    set(&bank, "fx.2.ring.on", on);
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
    fn a_step_that_is_on_rewinds_the_pass() {
        let run = |on: bool| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(96_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            // Four sixteenths at 120 is half a second; step one only.
            e.set_steps(StepParams {
                on,
                length: 4,
                pattern: 1,
                ..Default::default()
            });
            let mut out = vec![0.0; 128];
            for _ in 0..(24_000 / 64 + 2) {
                e.process_block(&mut out, &bank);
            }
            e.play_position()
        };
        let stepped = run(true);
        let free = run(false);
        assert!(stepped < 0.01, "the second pass began at {stepped}");
        assert!(free > 0.24, "without steps the pass ran on to {free}");
    }

    #[test]
    fn steps_start_from_one_with_the_transport() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        e.set_steps(StepParams {
            on: true,
            ..Default::default()
        });
        e.set_playing(true);
        let mut out = vec![0.0; 128];
        // 12,800 frames is into the third sixteenth at 120.
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
        assert_eq!(e.current_step(), Some(2));
        e.set_playing(false);
        assert_eq!(e.current_step(), None, "a stopped transport lights no step");
        e.set_playing(true);
        e.process_block(&mut out, &bank);
        assert_eq!(
            e.current_step(),
            Some(0),
            "play starts the pattern from the top"
        );
    }

    #[test]
    fn retriggering_does_not_click() {
        // A step rewinds the pass wherever the waveform happens to be. Cut
        // hard, that is a jump to the start of the sample on every step.
        let worst_step = |seq: bool| {
            let mut e = Engine::new(48_000.0, 128);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = ParamBank::new();
            // Every step, at a tempo that lands the rewinds all over the sine.
            e.set_steps(StepParams {
                on: seq,
                tempo_bpm: 137.0,
                pattern: 0xFFFF,
                ..Default::default()
            });
            let mut out = vec![0.0; 256];
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            // Long enough that the baseline crosses its own loop point too.
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
        let stepped = worst_step(true);
        assert!(
            stepped < steady * 2.0 + 0.02,
            "retriggering jumped by {stepped} against a steady {steady}"
        );
    }

    #[test]
    fn a_step_an_octave_up_plays_its_pass_twice_as_fast() {
        // Varispeed, as the octave is: a quarter of a second into a pass,
        // the step an octave up has read twice as far.
        let run = |semis: i8| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(96_000));
            let mut pitches = [0; crate::steps::STEPS];
            pitches[0] = semis;
            e.set_steps(StepParams {
                on: true,
                pattern: 1,
                pitches,
                ..Default::default()
            });
            e.set_playing(true);
            let bank = ParamBank::new();
            let mut out = vec![0.0; 480];
            for _ in 0..50 {
                e.process_block(&mut out, &bank);
            }
            e.play_position()
        };
        let (flat, up) = (run(0), run(12));
        let ratio = up / flat;
        assert!(
            (1.95..2.05).contains(&ratio),
            "an octave up read {ratio}x as far"
        );
        // A quarter of a second of 96,000 samples is an eighth, unpitched.
        assert!(flat > 0.12, "the flat pass read {flat}");
    }

    #[test]
    fn steps_with_none_set_are_silent() {
        // Georg, 2026-09-14: the steps on with no step set played the loop,
        // where he expected silence. Both generators on, so the cloud has to
        // hold its peace as well as the material.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        bank.set_by_id("grain.on", 1.0);
        e.set_steps(StepParams {
            on: true,
            pattern: 0,
            ..Default::default()
        });
        e.set_playing(true);
        let mut out = vec![0.0; 512];
        for block in 0..200 {
            e.process_block(&mut out, &bank);
            assert!(
                out.iter().all(|s| *s == 0.0),
                "block {block} made sound with no step set"
            );
        }
        assert_eq!(e.active_grains(), 0);
    }

    #[test]
    fn a_step_plays_the_pass_once() {
        // A quarter-second sample under one step in a two-second bar: it
        // sounds, the cloud trails off after it, then nothing until the bar
        // comes round.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(12_000));
        let bank = ParamBank::new();
        bank.set_by_id("grain.on", 1.0);
        e.set_steps(StepParams {
            on: true,
            length: 16,
            pattern: 1,
            ..Default::default()
        });
        e.set_playing(true);
        // 240 frames a block is 5 ms, so 400 blocks is the bar.
        let mut out = vec![0.0; 480];
        let peaks: Vec<f32> = (0..480)
            .map(|_| {
                e.process_block(&mut out, &bank);
                out.iter().fold(0.0f32, |m, s| m.max(s.abs()))
            })
            .collect();
        let loudest = |r: core::ops::Range<usize>| peaks[r].iter().copied().fold(0.0, f32::max);
        assert!(loudest(0..40) > 0.1, "the step never sounded");
        // The pass ends at 0.25 s and the last grain 180 ms after it.
        assert_eq!(loudest(120..395), 0.0, "sound between the steps");
        assert!(loudest(400..440) > 0.1, "the bar came round in silence");
    }

    #[test]
    fn fm_stands_alone_as_a_generator() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(48_000));
        let bank = ParamBank::new();
        bank.set_by_id("material.on", 0.0);
        bank.set_by_id("fm.on", 1.0);
        e.set_playing(true);
        let mut out = vec![0.0; 512];
        let mut peak = 0.0f32;
        for _ in 0..100 {
            e.process_block(&mut out, &bank);
            peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
        }
        assert!(peak > 0.5, "FM alone should be heard, peaked at {peak}");
    }

    #[test]
    fn under_the_steps_fm_sounds_with_the_pass_and_not_between() {
        // The same bar as `a_step_plays_the_pass_once`, with FM in place of
        // the material: it has no pass of its own, so it follows the player's.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(12_000));
        let bank = ParamBank::new();
        bank.set_by_id("material.on", 0.0);
        bank.set_by_id("fm.on", 1.0);
        e.set_steps(StepParams {
            on: true,
            length: 16,
            pattern: 1,
            ..Default::default()
        });
        e.set_playing(true);
        let mut out = vec![0.0; 480];
        let peaks: Vec<f32> = (0..480)
            .map(|_| {
                e.process_block(&mut out, &bank);
                out.iter().fold(0.0f32, |m, s| m.max(s.abs()))
            })
            .collect();
        let loudest = |r: core::ops::Range<usize>| peaks[r].iter().copied().fold(0.0, f32::max);
        assert!(loudest(0..40) > 0.3, "the step never sounded");
        assert_eq!(loudest(60..395), 0.0, "FM between the steps");
        assert!(loudest(400..440) > 0.3, "the bar came round in silence");
    }

    #[test]
    fn with_nothing_wired_into_sample_a_step_plays_the_cloud_once() {
        // An empty player never reached the edge of a window, so its pass
        // never ended and the cloud threw grains between the steps. The
        // cloud's window is the clock instead: one pass a step, and without
        // the steps it keeps going rather than falling silent.
        let peaks = |on: bool| {
            let mut e = Engine::new(48_000.0, 64);
            drop(e.swap_source(Generator::Grain, tone(12_000)));
            let bank = ParamBank::new();
            bank.set_by_id("grain.on", 1.0);
            e.set_steps(StepParams {
                on,
                length: 16,
                pattern: 1,
                ..Default::default()
            });
            e.set_playing(true);
            let mut out = vec![0.0; 480];
            (0..480)
                .map(|_| {
                    e.process_block(&mut out, &bank);
                    out.iter().fold(0.0f32, |m, s| m.max(s.abs()))
                })
                .collect::<Vec<f32>>()
        };
        let loudest =
            |p: &[f32], r: core::ops::Range<usize>| p[r].iter().copied().fold(0.0, f32::max);
        let stepped = peaks(true);
        assert!(loudest(&stepped, 0..40) > 0.1, "the step never sounded");
        assert_eq!(loudest(&stepped, 120..395), 0.0, "sound between the steps");
        assert!(
            loudest(&stepped, 400..440) > 0.1,
            "the bar came round in silence"
        );
        let free = peaks(false);
        assert!(
            (120..395)
                .step_by(25)
                .all(|b| loudest(&free, b..b + 25) > 0.1),
            "the cloud alone fell silent without the steps"
        );
    }

    #[test]
    fn a_step_on_a_reversed_reel_plays_from_the_far_end() {
        // Rewound to the top, a backwards pass leaves the window on its first
        // sample and a reversed reel under the steps is silent.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(12_000));
        let bank = ParamBank::new();
        let tape = ParamBank::for_table(arrangement::params());
        set(&tape, "arrangement.tape.reverse", 1.0);
        e.set_arrangement(&tape, false);
        e.set_playing(true);
        let mut out = vec![0.0; 480];
        // A second for the reel to come round to backwards.
        for _ in 0..200 {
            e.process_block(&mut out, &bank);
        }
        assert!(e.reversing());
        e.set_steps(StepParams {
            on: true,
            length: 16,
            pattern: 1,
            ..Default::default()
        });
        let mut peak = 0.0f32;
        for block in 0..40 {
            e.process_block(&mut out, &bank);
            // Past the first 25 ms, so the loop the steps took over has
            // faded out as a tail and only the new pass is left to measure.
            if block >= 5 {
                peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
            }
        }
        assert!(peak > 0.1, "the reversed pass was {peak}");
    }

    #[test]
    fn switching_the_steps_on_mid_pass_fades_the_loop_out() {
        // The steps take over from a loop that is sounding. With no step set
        // it has to go quiet, and over the tail's fade rather than at once.
        let worst_step = |switch: bool| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(48_000));
            let bank = ParamBank::new();
            e.set_playing(true);
            let mut out = vec![0.0; 256];
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            for block in 0..300 {
                if block == 100 {
                    e.set_steps(StepParams {
                        on: switch,
                        pattern: 0,
                        ..Default::default()
                    });
                }
                e.process_block(&mut out, &bank);
                for s in out.iter().step_by(2) {
                    if block > 20 {
                        worst = worst.max((s - prev).abs());
                    }
                    prev = *s;
                }
                if switch && block > 110 {
                    assert!(out.iter().all(|s| *s == 0.0), "block {block} still sounded");
                }
            }
            worst
        };
        let steady = worst_step(false);
        let switched = worst_step(true);
        assert!(
            switched < steady * 2.0 + 0.02,
            "switching the steps on jumped by {switched} against a steady {steady}"
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
            let window = Reading {
                trim_start: lo,
                trim_end: hi,
                ..Reading::default()
            };
            e.set_readings(window, window);
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
            let window = Reading {
                trim_start: lo,
                trim_end: hi,
                ..Reading::default()
            };
            e.set_readings(window, window);
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
    /// An arrangement at its defaults with the limiter off, for tests about
    /// the path rather than the limiter.
    fn limiter_off() -> ParamBank {
        let arr = ParamBank::for_table(arrangement::params());
        set(&arr, "arrangement.amp.limit", 0.0);
        arr
    }

    fn set(bank: &ParamBank, id: &str, v: f32) {
        assert!(bank.set_by_id(id, v), "no such parameter: {id}");
        // Switching an effect on in a test means having it in the chain.
        if v >= 0.5 && id.ends_with(".on") {
            if let Some((n, _)) = crate::fx::parse_row(id) {
                let mut order = crate::fx::order_of(bank);
                if order.push(n) {
                    crate::fx::set_order(bank, &order);
                }
            }
        }
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
            ("fx.1.crush.on", 1.0),
            ("fx.1.crush.bits", 1.0),
            ("fx.1.crush.rate", 200.0),
        ]);
        assert_eq!(neutral.len(), destructive.len());
        for (i, (a, b)) in neutral.iter().zip(destructive.iter()).enumerate() {
            assert_eq!(a, b, "frame {i} differs with the crush mix shut");
        }
    }

    #[test]
    fn a_mod_envelope_on_the_crush_mix_starts_clean_and_then_crushes() {
        // What crush's own envelope was for, now done by a modulation
        // envelope linked to the mix (Georg, 2026-10-04). An attack must
        // leave the beginning of the pass untouched and have the effect fully
        // in by the end of it.
        let render = |swell: bool| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(48_000));
            if swell {
                // A ramp across the whole one-second pass.
                let point = |x: f32, y: f32| preset_math::CurvePoint {
                    x,
                    y,
                    basis: preset_math::Basis::Linear,
                    tension: 0.0,
                };
                let rise = crate::modulation::EnvSpec {
                    id: 3,
                    curve: preset_math::Curve {
                        points: vec![point(0.0, 0.0), point(1.0, 1.0)],
                        sustain: None,
                    },
                };
                let mut set = ModSet::with_envelopes(&[], &[rise]);
                set.link("fx.1.crush.mix", 3, 0.0, 1.0).unwrap();
                drop(e.set_modulation(set));
            }
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "grain.on", 0.0);
            set(&bank, "amp.gain", 1.0);
            if swell {
                set(&bank, "fx.1.crush.on", 1.0);
                set(&bank, "fx.1.crush.bits", 1.0);
                set(&bank, "fx.1.crush.rate", 400.0);
                set(&bank, "fx.1.crush.mix", 1.0);
            }
            let mut out = vec![0.0; 512];
            let mut all = Vec::new();
            for _ in 0..180 {
                e.process_block(&mut out, &bank);
                all.extend_from_slice(&out);
            }
            all
        };
        let clean = render(false);
        let swelling = render(true);

        // The first 50 ms: the envelope has barely opened, so this is still
        // the material. Not bit-exact, because the mix is smoothed and read a
        // block at a time, but audibly the same sound.
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
        // Only the cloud's window moves, which is the one its marks must honour.
        e.set_readings(
            Reading::default(),
            Reading {
                trim_start: 0.5,
                trim_end: 0.6,
                ..Reading::default()
            },
        );
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
        let tape = ParamBank::for_table(arrangement::params());
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
        set(&tape, "arrangement.tape.brake", 0.5);
        e.set_arrangement(&tape, false);
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
        set(&tape, "arrangement.tape.brake", 1.0);
        e.set_arrangement(&tape, false);
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
            let tape = ParamBank::for_table(arrangement::params());
            set(&bank, "grain.on", 0.0);
            set(&bank, "amp.gain", 1.0);
            set(&bank, "tape.time", time);
            let mut out = vec![0.0; 512];
            for _ in 0..20 {
                e.process_block(&mut out, &bank);
            }
            set(&tape, "arrangement.tape.brake", 1.0);
            e.set_arrangement(&tape, false);
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
            e.set_readings(
                Reading {
                    octave,
                    ..Reading::default()
                },
                Reading::default(),
            );
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
            e.set_readings(
                Reading::default(),
                Reading {
                    octave,
                    ..Reading::default()
                },
            );
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
                ("fx.2.ring.on", 1.0),
                ("fx.2.ring.mix", 0.3),
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

    /// A pluck: fast up, down to half by the middle of the pass, and away to
    /// nothing at its end.
    fn pluck(id: u64) -> crate::modulation::EnvSpec {
        crate::modulation::EnvSpec {
            id,
            curve: crate::modulation::adsr_curve(0.04, 0.46, 0.5, 0.5),
        }
    }

    /// Filter cutoff as the engine heard it, block by block, normalised, with
    /// a pluck on it from 0.1 to 0.9 of its range. A quarter-second sample.
    fn cutoff_under_a_pluck(steps_on: bool, octave: f32, blocks: usize) -> Vec<f32> {
        cutoff_under_a_pluck_at(1.0, steps_on, octave, blocks)
    }

    /// The same under any `patch.length`.
    fn cutoff_under_a_pluck_at(
        length: f32,
        steps_on: bool,
        octave: f32,
        blocks: usize,
    ) -> Vec<f32> {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(12_000));
        e.set_readings(
            Reading {
                octave,
                ..Reading::default()
            },
            Reading::default(),
        );
        let mut set = ModSet::with_envelopes(&[], &[pluck(7)]);
        set.link("filter.cutoff", 7, 0.1, 0.9).unwrap();
        drop(e.set_modulation(set));
        e.set_steps(StepParams {
            on: steps_on,
            length: 16,
            pattern: 1,
            ..Default::default()
        });
        e.set_playing(true);
        let bank = ParamBank::new();
        bank.set_by_id("patch.length", length);
        bank.set_by_id("patch.hold", 100.0);
        let heard = ParamBank::new();
        let slot = index_of("filter.cutoff").unwrap();
        let def = &crate::params::PARAMS[slot];
        // 240 frames a block is 5 ms, so 400 blocks is the two-second bar.
        let mut out = vec![0.0; 480];
        (0..blocks)
            .map(|_| {
                e.process_block(&mut out, &bank);
                e.publish_heard(&bank, &heard);
                def.normalise(heard.get(slot))
            })
            .collect()
    }

    #[test]
    fn a_mod_envelope_peaks_with_each_step_and_rests_between() {
        let v = cutoff_under_a_pluck(true, 0.0, 480);
        let most = |r: core::ops::Range<usize>| v[r].iter().copied().fold(0.0, f32::max);
        let rest = |r: core::ops::Range<usize>| v[r].iter().all(|x| (x - 0.1).abs() < 1e-4);
        assert!(
            most(0..10) > 0.8,
            "no peak on the first step: {:?}",
            &v[..10]
        );
        assert!(
            (0.45..0.55).contains(&v[25]),
            "not half way down mid-pass: {}",
            v[25]
        );
        assert!(rest(60..398), "moved between the steps");
        assert!(most(400..410) > 0.8, "no peak on the next bar");
    }

    #[test]
    fn with_the_steps_off_a_mod_envelope_retriggers_every_loop() {
        // A loop is a quarter second, fifty blocks.
        let v = cutoff_under_a_pluck(false, 0.0, 200);
        for start in [0, 50, 100, 150] {
            let peak = v[start..start + 10].iter().copied().fold(0.0, f32::max);
            assert!(peak > 0.8, "no peak on the loop at block {start}");
        }
    }

    #[test]
    fn a_mod_envelope_runs_on_the_pass_so_an_octave_up_halves_it() {
        // Decided, not incidental: it reads the pass's clock, as the
        // amplitude envelope does, so it follows the material's octave.
        let down = cutoff_under_a_pluck(true, 0.0, 60);
        let up = cutoff_under_a_pluck(true, 1.0, 60);
        assert!(down[25] > 0.4, "the pass should still be sounding");
        assert!((up[30] - 0.1).abs() < 1e-4, "an octave up should be over");
    }

    #[test]
    fn under_hold_a_mod_envelope_follows_the_note_not_the_sample() {
        // The whole curve spans the 100 ms hold: at rest by 150 ms, at any
        // octave, where under Sample it would ride the quarter-second pass.
        for octave in [0.0, 2.0] {
            let v = cutoff_under_a_pluck_at(2.0, true, octave, 480);
            let most = v[0..10].iter().copied().fold(0.0, f32::max);
            assert!(most > 0.8, "no peak at octave {octave}");
            assert!(
                v[30..398].iter().all(|x| (x - 0.1).abs() < 1e-4),
                "not at rest after the note at octave {octave}"
            );
        }
    }

    #[test]
    fn an_unlinked_mod_envelope_is_bit_exact_with_none() {
        let render = |with_env: bool| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(12_000));
            if with_env {
                drop(e.set_modulation(ModSet::with_envelopes(&[], &[pluck(1)])));
            }
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "filter.on", 1.0);
            set(&bank, "filter.mix", 1.0);
            let mut out = vec![0.0; 512];
            let mut all = Vec::new();
            for _ in 0..100 {
                e.process_block(&mut out, &bank);
                all.extend_from_slice(&out);
            }
            all
        };
        assert!(render(false) == render(true));
    }

    /// FM alone, over a two-second material that only sets the pass, with
    /// the patch's length and any extra rows set. Peak per 5 ms block.
    fn fm_blocks(
        settings: &[(&str, f32)],
        octave: f32,
        steps: StepParams,
        blocks: usize,
    ) -> Vec<f32> {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(96_000));
        e.set_readings(
            Reading {
                octave,
                ..Reading::default()
            },
            Reading::default(),
        );
        e.set_steps(steps);
        let bank = ParamBank::new();
        set(&bank, "material.on", 0.0);
        set(&bank, "fm.on", 1.0);
        set(&bank, "fm.index", 0.0);
        for (id, v) in settings {
            set(&bank, id, *v);
        }
        e.set_playing(true);
        let mut out = vec![0.0; 480];
        (0..blocks)
            .map(|_| {
                e.process_block(&mut out, &bank);
                out.iter().fold(0.0f32, |m, s| m.max(s.abs()))
            })
            .collect()
    }

    const HOLD: f32 = 2.0;
    const LOOP: f32 = 0.0;

    #[test]
    fn under_hold_a_note_lasts_its_hold_and_then_starts_again() {
        // 100 ms held, no release, the Envelope node off: sound, then the
        // 5 ms fade, then the next note. The two-second material has no say.
        let v = fm_blocks(
            &[
                ("patch.length", HOLD),
                ("patch.hold", 100.0),
                ("env.on", 0.0),
            ],
            0.0,
            StepParams::default(),
            60,
        );
        assert!(v[2..19].iter().all(|p| *p > 0.5), "the note should sound");
        // The fade takes 10 ms, so a 5 ms block's peak never shows the zero;
        // it shows as the quietest blocks, followed by the next note.
        assert!(
            v[20..23].iter().any(|p| *p < 0.35),
            "and end: {:?}",
            &v[18..26]
        );
        assert!(v[25..40].iter().all(|p| *p > 0.5), "then start again");
    }

    #[test]
    fn under_hold_a_note_ends_in_silence_even_with_the_envelope_off() {
        // One step a bar: 100 ms held, the fade, then nothing at all until
        // the bar comes round. Without the fade FM would drone on and Sample
        // would play to the end of its material.
        let one_a_bar = StepParams {
            on: true,
            length: 16,
            pattern: 1,
            ..Default::default()
        };
        for material_on in [0.0, 1.0] {
            let v = fm_blocks(
                &[
                    ("patch.length", HOLD),
                    ("patch.hold", 100.0),
                    ("env.on", 0.0),
                    ("material.on", material_on),
                ],
                0.0,
                one_a_bar,
                420,
            );
            assert!(v[5] > 0.5, "the note never sounded");
            assert!(
                v[24..399].iter().all(|p| *p == 0.0),
                "sound between the notes with material {material_on}"
            );
            assert!(v[401] > 0.5, "the bar came round in silence");
        }
    }

    /// Peaks per 5 ms block for one step a bar at 120 bpm (a sixteenth is
    /// 125 ms, so a bar is 400 blocks), the patch held 100 ms.
    fn held_steps(length: f32, pattern: u64, holds: &[(usize, u8)]) -> Vec<f32> {
        let mut h = [0u8; crate::steps::STEPS];
        for (i, v) in holds {
            h[*i] = *v;
        }
        fm_blocks(
            &[
                ("patch.length", length),
                ("patch.hold", 100.0),
                ("env.on", 0.0),
            ],
            0.0,
            StepParams {
                on: true,
                length: 16,
                pattern,
                holds: h,
                ..Default::default()
            },
            420,
        )
    }

    #[test]
    fn a_steps_hold_replaces_the_patchs_hold_for_that_step() {
        // Four sixteenths is half a second: 100 blocks of sound, not the
        // patch's 20, then silence until the bar comes round.
        let v = held_steps(HOLD, 1, &[(0, 4)]);
        assert!(v[5..95].iter().all(|p| *p > 0.5), "the held note broke up");
        assert!(
            v[105..399].iter().all(|p| *p == 0.0),
            "sound after the hold ended"
        );
        assert!(v[401] > 0.5, "the bar came round in silence");
    }

    #[test]
    fn a_step_with_no_hold_keeps_the_patchs() {
        let v = held_steps(HOLD, 1, &[]);
        assert!(v[5] > 0.5);
        assert!(v[30..399].iter().all(|p| *p == 0.0));
    }

    #[test]
    fn each_step_holds_for_its_own_length() {
        // Steps one and nine: a quarter second, then three quarters.
        let v = held_steps(HOLD, 0b1_0000_0001, &[(0, 2), (8, 6)]);
        assert!(v[5..45].iter().all(|p| *p > 0.5));
        assert!(v[60..195].iter().all(|p| *p == 0.0), "the gap");
        assert!(v[205..345].iter().all(|p| *p > 0.5), "the long note");
        assert!(v[360..399].iter().all(|p| *p == 0.0));
    }

    /// Peaks per 5 ms block of two held steps, a half bar apart, with the
    /// given velocities (steps one and nine).
    fn two_velocities(first: u8, second: u8) -> Vec<f32> {
        let mut v = [crate::steps::VELOCITY_MAX; crate::steps::STEPS];
        v[0] = first;
        v[8] = second;
        fm_blocks(
            &[
                ("patch.length", HOLD),
                ("patch.hold", 150.0),
                ("env.on", 0.0),
            ],
            0.0,
            StepParams {
                on: true,
                length: 16,
                pattern: 0b1_0000_0001,
                velocities: v,
                ..Default::default()
            },
            420,
        )
    }

    #[test]
    fn a_steps_velocity_scales_its_note() {
        let v = two_velocities(127, 64);
        let loud = v[10..25].iter().fold(0.0f32, |m, p| m.max(*p));
        let soft = v[210..225].iter().fold(0.0f32, |m, p| m.max(*p));
        assert!(loud > 0.5, "the full step is silent: {loud}");
        assert!(
            (soft / loud - 64.0 / 127.0).abs() < 0.03,
            "half velocity should be about half: {soft} against {loud}"
        );
    }

    #[test]
    fn a_step_at_velocity_zero_is_silent_and_full_is_unity() {
        let silent = two_velocities(127, 0);
        assert!(silent[210..225].iter().all(|p| *p < 1e-4));
        // Full is exactly the old behaviour: bit-exact with steps that never
        // heard of velocity (the default of every other test here).
        let full = two_velocities(127, 127);
        let default = fm_blocks(
            &[
                ("patch.length", HOLD),
                ("patch.hold", 150.0),
                ("env.on", 0.0),
            ],
            0.0,
            StepParams {
                on: true,
                length: 16,
                pattern: 0b1_0000_0001,
                ..Default::default()
            },
            420,
        );
        assert_eq!(full, default);
    }

    #[test]
    fn a_soft_step_does_not_turn_down_the_tails_of_the_loud_one_before_it() {
        // A delay with feedback holds the loud note's repeats for a while. The
        // soft step that follows must scale its own note, not those repeats.
        // Loud at step one (0 ms), a very soft one at step five (500 ms). The
        // loud note's third repeat rings from 450 to 550 ms, so 510 to 540 ms
        // is that repeat with the soft note's quiet start on top. The same
        // window with no second step is the repeat alone.
        let run = |pattern: u64, soft: u8| {
            let mut v = [crate::steps::VELOCITY_MAX; crate::steps::STEPS];
            v[4] = soft;
            fm_blocks(
                &[
                    ("patch.length", HOLD),
                    ("patch.hold", 100.0),
                    ("env.on", 0.0),
                    ("fx.7.delay.on", 1.0),
                    ("fx.7.delay.mix", 0.5),
                    ("fx.7.delay.time", 150.0),
                    ("fx.7.delay.feedback", 0.6),
                ],
                0.0,
                StepParams {
                    on: true,
                    length: 16,
                    pattern,
                    velocities: v,
                    ..Default::default()
                },
                160,
            )
        };
        let window = |v: &[f32]| v[102..108].iter().fold(0.0f32, |m, p| m.max(*p));
        let alone = window(&run(0b1, 127));
        let with_soft = window(&run(0b1_0001, 8));
        assert!(alone > 0.05, "no repeat to measure: {alone}");
        assert!(
            // Turned down by the soft step's velocity it would be about 6 % of
            // this (8 of 127); the soft note's own quiet start adds a little.
            with_soft > 0.7 * alone && with_soft < 1.3 * alone,
            "the soft step changed the loud note's repeat: {with_soft} against {alone}"
        );
    }

    /// Peak per 5 ms block of one 100 ms note through a delay with
    /// feedback, the patch held, the amplitude envelope on or off (no
    /// release when on).
    fn note_through_a_delay(envelope: f32) -> Vec<f32> {
        fm_blocks(
            &[
                ("patch.length", HOLD),
                ("patch.hold", 100.0),
                ("env.on", envelope),
                ("env.release", 0.0),
                ("fx.7.delay.on", 1.0),
                ("fx.7.delay.mix", 0.5),
                ("fx.7.delay.time", 150.0),
                ("fx.7.delay.feedback", 0.6),
            ],
            0.0,
            StepParams {
                on: true,
                length: 16,
                pattern: 0b1,
                ..Default::default()
            },
            160,
        )
    }

    #[test]
    fn under_hold_the_amplitude_envelope_decides_whether_the_tails_of_effects_are_heard() {
        // KNOWN, AND OPEN FOR GEORG (handover-effects.md): the envelope sits
        // after the effect chain, as it did before the time-based effects, so
        // under Hold it closes on the whole signal when the note ends, tails
        // included. With the envelope off, the delay's repeats ring on.
        let tail = |v: &[f32]| v[40..80].iter().fold(0.0f32, |m, p| m.max(*p));
        let without = tail(&note_through_a_delay(0.0));
        let with = tail(&note_through_a_delay(1.0));
        assert!(without > 0.05, "no repeats to measure: {without}");
        assert!(
            with < 0.1 * without,
            "the envelope no longer cuts the tails ({with} against {without}): \
             if that is on purpose, update this test and the handover"
        );
    }

    #[test]
    fn velocity_is_forgotten_when_the_steps_stop() {
        // A quiet step then the steps switched off: the patch plays at unity.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(96_000));
        let bank = ParamBank::new();
        set(&bank, "material.on", 0.0);
        set(&bank, "fm.on", 1.0);
        set(&bank, "fm.index", 0.0);
        e.set_playing(true);
        let mut v = [crate::steps::VELOCITY_MAX; crate::steps::STEPS];
        v[0] = 20;
        e.set_steps(StepParams {
            on: true,
            length: 16,
            pattern: 1,
            velocities: v,
            ..Default::default()
        });
        let mut out = vec![0.0; 480];
        for _ in 0..40 {
            e.process_block(&mut out, &bank);
        }
        e.set_steps(StepParams::default());
        let mut peak = 0.0f32;
        for _ in 0..120 {
            e.process_block(&mut out, &bank);
            peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
        }
        assert!(
            peak > 0.5,
            "the steps ended and left the patch quiet: {peak}"
        );
    }

    #[test]
    fn a_hold_is_ignored_unless_the_patch_is_gated() {
        // Under Loop nothing is gated, so a step's hold changes nothing.
        let with = held_steps(LOOP, 1, &[(0, 2)]);
        let without = held_steps(LOOP, 1, &[]);
        assert_eq!(with, without);
    }

    #[test]
    fn under_hold_the_materials_octave_does_not_change_a_note() {
        // Under Sample an octave up halves the pass and both envelopes with
        // it. Under Hold a note is real time, so it must not.
        let settings = [
            ("patch.length", HOLD),
            ("patch.hold", 100.0),
            ("env.on", 0.0),
        ];
        let down = fm_blocks(&settings, 0.0, StepParams::default(), 30);
        let up = fm_blocks(&settings, 2.0, StepParams::default(), 30);
        let quiet = |v: &[f32]| v.iter().position(|p| *p < 0.05);
        assert_eq!(quiet(&down), quiet(&up));
    }

    #[test]
    fn under_hold_the_envelope_releases_after_the_gate_in_real_time() {
        // Held 100 ms, released over 100 ms: half level halfway down.
        let v = fm_blocks(
            &[
                ("patch.length", HOLD),
                ("patch.hold", 100.0),
                ("env.release", 100.0),
            ],
            0.0,
            StepParams::default(),
            50,
        );
        assert!(v[15] > 0.9, "held at full: {}", v[15]);
        assert!(
            (0.4..0.6).contains(&v[30]),
            "halfway down the release: {}",
            v[30]
        );
        assert!(v[41] < 0.05, "released: {}", v[41]);
    }

    #[test]
    fn under_loop_the_envelope_attacks_once_and_holds() {
        // The pass is two seconds and would restart the envelope each time
        // under Sample; under Loop it holds at sustain through the loop.
        let v = fm_blocks(
            &[
                ("patch.length", LOOP),
                ("env.attack", 20.0),
                ("env.decay", 50.0),
                ("env.sustain", 0.4),
            ],
            0.0,
            StepParams::default(),
            1_000,
        );
        let peak = v[2..8].iter().copied().fold(0.0, f32::max);
        assert!(peak > 0.8, "the peak: {peak}");
        for at in [100, 399, 401, 600, 999] {
            assert!((v[at] - 0.4).abs() < 0.03, "not held at {at}: {}", v[at]);
        }
    }

    #[test]
    fn a_retrigger_mid_note_does_not_click() {
        // Steps every 62 ms land mid-release of a longer note. Each attack
        // climbs from where the last had got to, so the largest step between
        // samples stays near a 110 Hz sine's own.
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(96_000));
        e.set_steps(StepParams {
            on: true,
            tempo_bpm: 240.0,
            pattern: 0xFFFF,
            ..Default::default()
        });
        let bank = ParamBank::new();
        for (id, v) in [
            ("material.on", 0.0),
            ("fm.on", 1.0),
            ("fm.index", 0.0),
            ("patch.length", HOLD),
            ("patch.hold", 30.0),
            ("env.attack", 5.0),
            ("env.release", 200.0),
        ] {
            set(&bank, id, v);
        }
        e.set_playing(true);
        let mut out = vec![0.0; 256];
        let (mut last, mut worst) = (0.0f32, 0.0f32);
        for _ in 0..400 {
            e.process_block(&mut out, &bank);
            for s in out.iter().step_by(2) {
                worst = worst.max((s - last).abs());
                last = *s;
            }
        }
        assert!(worst < 0.05, "a jump of {worst}");
    }

    #[test]
    fn switching_length_while_playing_keeps_sounding() {
        let mut e = Engine::new(48_000.0, 64);
        e.set_source(tone(96_000));
        let bank = ParamBank::new();
        set(&bank, "material.on", 0.0);
        set(&bank, "fm.on", 1.0);
        set(&bank, "patch.hold", 400.0);
        e.set_playing(true);
        let mut out = vec![0.0; 480];
        for (i, length) in [LOOP, HOLD, 1.0, LOOP, 1.0, HOLD, LOOP].iter().enumerate() {
            set(&bank, "patch.length", *length);
            let mut peak = 0.0f32;
            for _ in 0..20 {
                e.process_block(&mut out, &bank);
                peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
                assert!(out.iter().all(|s| s.is_finite()));
            }
            assert!(peak > 0.3, "went quiet after switch {i} to {length}");
        }
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
            set(&bank, "fx.2.ring.on", 1.0);
            set(&bank, "fx.2.ring.mix", 0.5);
            if linked {
                let mut mods = ModSet::new(&[lfo(1, 5.0, crate::modulation::Shape::Square)]);
                mods.link("fx.2.ring.mix", 1, 0.0, 0.5).unwrap();
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
        let tape = ParamBank::for_table(arrangement::params());
        set(&bank, "grain.on", 0.0);
        set(&bank, "tape.time", 20.0);

        let mut out = vec![0.0; 512];
        for _ in 0..40 {
            e.process_block(&mut out, &bank);
        }
        let before = e.play_position();
        e.process_block(&mut out, &bank);
        assert!(e.play_position() > before, "forwards should advance");

        set(&tape, "arrangement.tape.reverse", 1.0);
        e.set_arrangement(&tape, false);
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
        let tape = ParamBank::for_table(arrangement::params());
        set(&bank, "tape.flick", 150.0);
        let mut out = vec![0.0; 512];

        // A tap: gate high for one block, about 5 ms.
        set(&tape, "arrangement.tape.reverse", 1.0);
        e.set_arrangement(&tape, false);
        e.process_block(&mut out, &bank);
        set(&tape, "arrangement.tape.reverse", 0.0);
        e.set_arrangement(&tape, false);
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
        let tape = ParamBank::for_table(arrangement::params());
        set(&bank, "tape.flick", 150.0);
        let mut out = vec![0.0; 512];

        set(&tape, "arrangement.tape.reverse", 1.0);
        e.set_arrangement(&tape, false);
        // Roughly two seconds, far past the flick.
        for _ in 0..400 {
            e.process_block(&mut out, &bank);
        }
        assert!(e.reversing(), "a hold should stay engaged");

        set(&tape, "arrangement.tape.reverse", 0.0);
        e.set_arrangement(&tape, false);
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
            let tape = ParamBank::for_table(arrangement::params());
            set(&bank, "tape.flick", flick);
            let mut out = vec![0.0; 512];
            set(&tape, "arrangement.tape.reverse", 1.0);
            e.set_arrangement(&tape, false);
            e.process_block(&mut out, &bank);
            set(&tape, "arrangement.tape.reverse", 0.0);
            e.set_arrangement(&tape, false);
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
        let tape = ParamBank::for_table(arrangement::params());
        set(&bank, "tape.flick", 500.0);
        let mut out = vec![0.0; 512];
        set(&tape, "arrangement.tape.reverse", 1.0);
        e.set_arrangement(&tape, false);
        e.process_block(&mut out, &bank);
        set(&tape, "arrangement.tape.reverse", 0.0);
        e.set_arrangement(&tape, false);
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
        let tape = ParamBank::for_table(arrangement::params());
        let mut out = vec![0.0; 512];
        for (brake, reverse, time) in [
            (1.0, 1.0, 20.0),
            (0.0, 1.0, 3000.0),
            (0.5, 0.0, 20.0),
            (1.0, 0.0, 20.0),
            (0.0, 0.0, 20.0),
        ] {
            set(&tape, "arrangement.tape.brake", brake);
            e.set_arrangement(&tape, false);
            set(&tape, "arrangement.tape.reverse", reverse);
            e.set_arrangement(&tape, false);
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
        let tape = ParamBank::for_table(arrangement::params());
        set(&tape, "arrangement.tape.reverse", 1.0);
        e.set_arrangement(&tape, false);
        set(&tape, "arrangement.tape.brake", 1.0);
        e.set_arrangement(&tape, false);
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

    /// A patch with every effect on and loud. Arrangement tests need the
    /// patch's own chain doing something, or they prove nothing.
    fn busy_patch() -> ParamBank {
        let bank = ParamBank::new();
        for (id, v) in [
            ("fx.0.drive.on", 1.0),
            ("fx.0.drive.mix", 1.0),
            ("fx.0.drive.amount", 18.0),
            ("fx.1.crush.on", 1.0),
            ("fx.1.crush.mix", 0.7),
            ("fx.1.crush.bits", 6.0),
            ("fx.2.ring.on", 1.0),
            ("fx.2.ring.mix", 0.5),
            ("filter.on", 1.0),
            ("filter.cutoff", 900.0),
            ("grain.on", 1.0),
            ("amp.gain", 1.3),
        ] {
            set(&bank, id, v);
        }
        bank
    }

    /// An arrangement whose every node is on and audible.
    fn busy_arrangement() -> ParamBank {
        let arr = ParamBank::for_table(arrangement::params());
        for (id, v) in [
            ("arrangement.fx.1.crush.on", 1.0),
            ("arrangement.fx.1.crush.mix", 1.0),
            ("arrangement.fx.1.crush.bits", 4.0),
            ("arrangement.filter.on", 1.0),
            ("arrangement.filter.cutoff", 300.0),
            ("arrangement.amp.gain", 0.8),
        ] {
            set(&arr, id, v);
        }
        arr
    }

    /// The busy arrangement's track fader, set beside it.
    const BUSY_GAIN: f32 = 0.4;

    fn render_arranged(arr: &ParamBank, gain: f32, patch_alone: bool, blocks: usize) -> Vec<f32> {
        let mut e = Engine::new(48_000.0, 64);
        e.set_voice_gain(0, gain);
        e.set_source(tone(48_000));
        e.set_playing(true);
        let bank = busy_patch();
        let mut out = vec![0.0; 256];
        let mut all = Vec::new();
        for _ in 0..blocks {
            e.set_arrangement(arr, patch_alone);
            e.process_block(&mut out, &bank);
            all.extend_from_slice(&out);
        }
        all
    }

    #[test]
    fn the_patch_alone_is_bit_exact_with_an_arrangement_at_its_defaults() {
        // Sound scaping hears the patch alone. However busy the arrangement
        // is, stepping it aside must leave exactly the patch, not nearly.
        let defaults = render_arranged(
            &ParamBank::for_table(arrangement::params()),
            1.0,
            false,
            100,
        );
        let alone = render_arranged(&busy_arrangement(), BUSY_GAIN, true, 100);
        assert!(
            defaults.iter().any(|s| s.abs() > 0.1),
            "the patch was not heard"
        );
        assert!(
            defaults == alone,
            "the patch alone differs from no arrangement"
        );
    }

    #[test]
    fn an_arrangement_effect_that_is_on_is_heard() {
        let defaults = render_arranged(
            &ParamBank::for_table(arrangement::params()),
            1.0,
            false,
            100,
        );
        let busy = render_arranged(&busy_arrangement(), BUSY_GAIN, false, 100);
        let most = defaults
            .iter()
            .zip(&busy)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            most > 0.05,
            "the arrangement changed the output by only {most}"
        );
    }

    #[test]
    fn the_fader_silences_the_patch_only_when_arranged() {
        let arr = ParamBank::for_table(arrangement::params());
        // The fader glides from unity over 20 ms; half a second later it is
        // far below anything audible.
        let arranged = render_arranged(&arr, 0.0, false, 400);
        let loudest = arranged[arranged.len() / 2..]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            loudest < 1e-6,
            "a fader at zero still let through {loudest}"
        );

        let defaults = render_arranged(&arr, 1.0, true, 100);
        let alone = render_arranged(&arr, 0.0, true, 100);
        assert!(defaults == alone, "the patch alone heard the fader");
    }

    #[test]
    fn switching_between_the_patch_alone_and_arranged_does_not_click() {
        let worst_step = |toggle: bool| {
            let mut e = Engine::new(48_000.0, 128);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = busy_patch();
            let arr = busy_arrangement();
            e.set_voice_gain(0, BUSY_GAIN);
            let mut out = vec![0.0; 256];
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            for block in 0..400 {
                let alone = toggle && (block / 25) % 2 == 1;
                e.set_arrangement(&arr, alone);
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

    // The pool (epic 28).

    /// A patch with every random source in it: a jittered cloud, drifting
    /// FM and a chorus.
    fn restless() -> ParamBank {
        let bank = ParamBank::new();
        set(&bank, "material.on", 0.0);
        set(&bank, "grain.on", 1.0);
        set(&bank, "grain.jitter", 0.5);
        set(&bank, "fm.on", 1.0);
        set(&bank, "fm.drift", 0.8);
        set(&bank, "fm.gain", 0.3);
        set(&bank, "fx.3.chorus.on", 1.0);
        set(&bank, "fx.3.chorus.mix", 0.5);
        bank
    }

    /// `frames` of voice `i` alone playing `bank`, left channel.
    fn alone(i: usize, bank: &ParamBank, frames: usize) -> Vec<f32> {
        let mut e = Engine::new(48_000.0, 128);
        for v in 0..VOICES {
            e.set_voice_source(v, tone(48_000));
            e.set_voice_active(v, v == i);
        }
        e.set_playing(true);
        let mut out = vec![0.0; 512];
        let mut all = Vec::new();
        while all.len() < frames {
            e.process_block(&mut out, bank);
            all.extend(out.iter().step_by(2));
        }
        all
    }

    fn correlation(a: &[f32], b: &[f32]) -> f32 {
        let dot = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>();
        dot(a, b) / (dot(a, a) * dot(b, b)).sqrt()
    }

    #[test]
    fn two_voices_on_one_patch_are_not_the_same_sound() {
        let bank = restless();
        // Past voice 0 fading out of the second render.
        let a = &alone(0, &bank, 96_000)[9_600..];
        let b = &alone(1, &bank, 96_000)[9_600..];
        let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
        assert!(rms(a) > 0.01 && rms(b) > 0.01, "{} {}", rms(a), rms(b));
        let c = correlation(a, b);
        assert!(c < 0.9, "two voices on one patch correlate at {c}");
        // And each is its own every time: seeded, not random.
        assert!(a == &alone(0, &bank, 96_000)[9_600..]);
        assert!(b == &alone(1, &bank, 96_000)[9_600..]);
    }

    #[test]
    fn switching_a_voice_in_and_out_mid_play_does_not_click() {
        let worst = |toggle: bool| {
            let mut e = Engine::new(48_000.0, 128);
            e.set_source(tone(48_000));
            let other: Vec<f32> = (0..48_000)
                .map(|i| (core::f32::consts::TAU * 330.0 * i as f32 / 48_000.0).sin())
                .collect();
            e.set_voice_source(1, other);
            e.set_voice_active(1, true);
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "material.gain", 0.4);
            let mut out = vec![0.0; 256];
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            for block in 0..600 {
                if toggle && block % 37 == 0 {
                    e.set_voice_active(1, (block / 37) % 2 == 0);
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
        let steady = worst(false);
        let toggled = worst(true);
        assert!(
            toggled < steady * 2.0 + 0.02,
            "switching a voice jumped by {toggled} against a steady {steady}"
        );
    }

    #[test]
    fn a_voices_fader_moves_that_voice_only() {
        // Voice 0 at 220 Hz, voice 1 at 330 Hz. Voice 1's fader down leaves
        // exactly voice 0 alone, once the 20 ms glide has landed.
        let render = |gains: [f32; 2], active: [bool; 2]| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(48_000));
            let other: Vec<f32> = (0..48_000)
                .map(|i| (core::f32::consts::TAU * 330.0 * i as f32 / 48_000.0).sin())
                .collect();
            e.set_voice_source(1, other);
            // Set before it wakes, so it wakes there.
            for (i, g) in gains.iter().enumerate() {
                e.set_voice_gain(i, *g);
            }
            e.set_voice_active(1, active[1]);
            e.set_voice_active(0, active[0]);
            e.set_playing(true);
            let bank = ParamBank::new();
            set(&bank, "material.gain", 0.4);
            let mut out = vec![0.0; 256];
            let mut all = Vec::new();
            for _ in 0..200 {
                e.process_block(&mut out, &bank);
                all.extend_from_slice(&out);
            }
            all.split_off(all.len() / 2)
        };
        let both = render([1.0, 0.0], [true, true]);
        let first = render([1.0, 1.0], [true, false]);
        let most = both
            .iter()
            .zip(&first)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(most < 1e-4, "voice 1 at zero still differed by {most}");
        let half = render([0.5, 1.0], [true, false]);
        let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let ratio = peak(&half) / peak(&first);
        assert!((ratio - 0.5).abs() < 0.05, "half the fader gave {ratio}");
    }

    #[test]
    fn every_voice_steps_on_the_one_transport() {
        // Voice 1 asks for its own tempo and swing; the transport's win, so
        // its step ones land on voice 0's samples, and a shorter loop's
        // land on every one of them.
        let mut e = Engine::new(48_000.0, 64);
        for v in 0..3 {
            e.set_voice_source(v, tone(4_800));
            e.set_voice_active(v, true);
        }
        e.set_steps(StepParams {
            on: true,
            tempo_bpm: 240.0,
            swing: 0.3,
            length: 8,
            pattern: 0xFF,
            ..Default::default()
        });
        e.set_voice_steps(
            1,
            StepParams {
                on: true,
                tempo_bpm: 97.0,
                swing: 0.0,
                length: 8,
                pattern: 0xFF,
                ..Default::default()
            },
        );
        e.set_voice_steps(
            2,
            StepParams {
                on: true,
                length: 4,
                pattern: 0xF,
                ..Default::default()
            },
        );
        e.set_playing(true);
        let bank = ParamBank::new();
        let mut out = [0.0f32; 2];
        let mut ones: [Vec<usize>; 3] = Default::default();
        let mut last = [None; 3];
        for frame in 0..60_000 {
            e.process_block(&mut out, &bank);
            for v in 0..3 {
                let now = e.voice_step(v);
                if now == Some(0) && last[v] != Some(0) {
                    ones[v].push(frame);
                }
                last[v] = now;
            }
        }
        // A sixteenth at 240 is 3 000 samples; eight of them, 24 000.
        assert_eq!(ones[0], vec![0, 24_000, 48_000]);
        assert_eq!(ones[1], ones[0], "voice 1 kept a clock of its own");
        assert!(ones[0].iter().all(|f| ones[2].contains(f)), "{:?}", ones[2]);
        assert_eq!(ones[2].len(), 5);
    }

    #[test]
    fn the_brake_slows_every_voice_and_the_kit() {
        // The transport, every track's steps and the kit's, and a voice
        // other than the first: all at half speed under half a brake.
        let steps_in = |brake: f32| {
            let mut e = Engine::new(48_000.0, 64);
            for v in 0..2 {
                e.set_voice_source(v, tone(4_800));
                e.set_voice_active(v, true);
            }
            let arr = ParamBank::for_table(arrangement::params());
            e.set_kit(
                0,
                KitPattern::default(),
                KitParams {
                    on: true,
                    ..KitParams::default()
                },
            );
            set(&arr, "arrangement.tape.brake", brake);
            e.set_arrangement(&arr, false);
            e.set_steps(StepParams {
                on: true,
                ..Default::default()
            });
            e.set_voice_steps(
                1,
                StepParams {
                    on: true,
                    length: 4,
                    ..Default::default()
                },
            );
            let bank = ParamBank::new();
            set(&bank, "tape.time", 20.0);
            e.set_playing(true);
            let mut out = vec![0.0; 128];
            let (mut changes, mut last) = ([0usize; 3], [None; 3]);
            // Two seconds: sixteen sixteenths at 120.
            for _ in 0..1_500 {
                e.process_block(&mut out, &bank);
                let now = [e.voice_step(0), e.voice_step(1), e.kit_step(0)];
                for k in 0..3 {
                    changes[k] += usize::from(now[k] != last[k]);
                }
                last = now;
            }
            (changes, e.bar())
        };
        let (free, free_bar) = steps_in(0.0);
        let (braked, braked_bar) = steps_in(0.5);
        for k in 0..3 {
            let ratio = braked[k] as f32 / free[k] as f32;
            assert!(
                (0.4..0.6).contains(&ratio),
                "clock {k}: {braked:?} against {free:?}"
            );
        }
        assert_eq!(free_bar, Some(1));
        assert_eq!(braked_bar, Some(0));

        // Voice 1's sound slows too: a tone read at half speed is an octave
        // down, half the zero crossings.
        let crossings = |brake: f32| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_voice_source(1, tone(48_000));
            e.set_voice_active(0, false);
            e.set_voice_active(1, true);
            let arr = limiter_off();
            set(&arr, "arrangement.tape.brake", brake);
            e.set_arrangement(&arr, false);
            let bank = ParamBank::new();
            set(&bank, "tape.time", 20.0);
            e.set_playing(true);
            let mut out = vec![0.0; 480];
            let mut all: Vec<f32> = Vec::new();
            for _ in 0..200 {
                e.process_block(&mut out, &bank);
                all.extend(out.iter().step_by(2));
            }
            all[24_000..]
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
        };
        let ratio = crossings(0.5) as f32 / crossings(0.0) as f32;
        assert!(
            (0.45..0.55).contains(&ratio),
            "voice 1 read at {ratio} of its speed"
        );
    }

    #[test]
    fn an_idle_voice_is_not_even_read() {
        // Voices 1 to 7 idle, handed banks at the far end of every row: the
        // output is the single voice's, sample for sample.
        let render = |others: Option<&ParamBank>| {
            let mut e = Engine::new(48_000.0, 64);
            e.set_source(tone(48_000));
            e.set_playing(true);
            let bank = restless();
            let mut out = vec![0.0; 512];
            let mut all = Vec::new();
            for _ in 0..100 {
                match others {
                    Some(o) => {
                        let mut banks = [o; VOICES];
                        banks[0] = &bank;
                        e.process_voices(&mut out, &banks);
                    }
                    None => e.process_block(&mut out, &bank),
                }
                all.extend_from_slice(&out);
            }
            all
        };
        let wild = ParamBank::new();
        for (i, p) in wild.defs().iter().enumerate() {
            wild.set(i, p.max);
        }
        assert!(render(None) == render(Some(&wild)));
    }

    #[test]
    fn a_voice_woken_mid_play_waits_for_its_next_step() {
        // Switched in half way through a step, voice 1 does not fire that
        // step late; it comes in on the next one, with voice 0.
        let mut e = Engine::new(48_000.0, 64);
        e.set_voice_source(1, tone(4_800));
        e.set_steps(StepParams {
            on: true,
            tempo_bpm: 240.0,
            length: 4,
            pattern: 0xF,
            ..Default::default()
        });
        e.set_voice_steps(
            1,
            StepParams {
                on: true,
                length: 4,
                pattern: 0xF,
                ..Default::default()
            },
        );
        e.set_playing(true);
        let bank = ParamBank::new();
        let mut out = vec![0.0; 300];
        // 1 500 frames: half of step one.
        for _ in 0..10 {
            e.process_block(&mut out, &bank);
        }
        e.set_voice_active(1, true);
        e.process_block(&mut out, &bank);
        assert_eq!(e.voice_step(0), Some(0));
        assert_eq!(e.voice_step(1), Some(0));
        let mut out = [0.0f32; 2];
        let mut first = None;
        for frame in 1_650..9_000 {
            e.process_block(&mut out, &bank);
            if first.is_none() && e.voice_step(1) == Some(1) {
                first = Some(frame);
            }
        }
        assert_eq!(first, Some(3_000), "it joins on step two");
    }
}
