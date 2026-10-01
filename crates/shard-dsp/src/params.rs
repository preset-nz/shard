//! The parameter table.
//!
//! One flat list of definitions with stable string ids. Everything addresses
//! this: the audio thread reads it, and any UI is generated from it rather
//! than hand-wired, so adding a parameter is adding a row and nothing else.
//!
//! Two rules carried from the design and worth keeping:
//!
//! - **Ids are a wire format.** Renaming one breaks every saved patch.
//! - **The taper belongs to the parameter, not the caller.** It says how a
//!   0-to-1 control position maps to a value, and it also says what a control
//!   should look like: bipolar wants a centre detent, stepped wants a
//!   selector rather than a slider.

use std::sync::atomic::{AtomicU32, Ordering};

/// How a normalised 0-to-1 control position maps onto the real value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taper {
    /// Even. Halfway along is halfway up.
    Linear,
    /// Perceptual. Use for anything measured in time, frequency, size or
    /// density, because hearing is roughly logarithmic. Linear on grain size
    /// gives you a control whose useful range is the bottom few millimetres.
    Exponential,
    /// Symmetric around zero. Pitch, pan, feedback.
    Bipolar,
    /// Discrete choices. Never smoothed, because interpolating between two
    /// enum values produces a number that means nothing.
    Stepped(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    None,
    Ms,
    Hz,
    Semitones,
    Percent,
    Octaves,
    Db,
}

#[derive(Debug, Clone, Copy)]
pub struct ParamDef {
    pub id: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub taper: Taper,
    pub unit: Unit,
    /// Per-parameter smoothing time. The table decides, not the caller.
    pub smooth_ms: f32,
}

impl ParamDef {
    /// Control position to real value.
    pub fn denormalise(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self.taper {
            Taper::Linear | Taper::Bipolar => self.min + (self.max - self.min) * t,
            Taper::Exponential => {
                // Geometric interpolation needs a positive floor; fall back to
                // linear for ranges that touch or cross zero.
                if self.min > 0.0 && self.max > 0.0 {
                    self.min * (self.max / self.min).powf(t)
                } else {
                    self.min + (self.max - self.min) * t
                }
            }
            Taper::Stepped(n) => {
                let n = n.max(1);
                let step = (t * n as f32).floor().min((n - 1) as f32);
                self.min + (self.max - self.min) * (step / (n - 1).max(1) as f32)
            }
        }
    }

    /// Real value back to control position. The inverse of `denormalise`.
    pub fn normalise(&self, v: f32) -> f32 {
        let v = v.clamp(self.min.min(self.max), self.max.max(self.min));
        match self.taper {
            Taper::Linear | Taper::Bipolar | Taper::Stepped(_) => {
                if (self.max - self.min).abs() < f32::EPSILON {
                    0.0
                } else {
                    (v - self.min) / (self.max - self.min)
                }
            }
            Taper::Exponential => {
                if self.min > 0.0 && self.max > 0.0 {
                    (v / self.min).ln() / (self.max / self.min).ln()
                } else if (self.max - self.min).abs() < f32::EPSILON {
                    0.0
                } else {
                    (v - self.min) / (self.max - self.min)
                }
            }
        }
        .clamp(0.0, 1.0)
    }
}

/// The set the engine actually reads. Deliberately small: a row exists here
/// because a node reads it, never because a control might be nice to have.
///
/// Two envelopes live in this table and they are not interchangeable. `env.*`
/// shapes amplitude, where neutral means transparent. `crush.env.*` shapes the
/// crush mix, where neutral means "leave the knob alone". Same maths, opposite
/// reading of the same number — see `Engine::process_block`.
///
/// **Every switchable node follows one pattern, by role** (Georg, 2026-09-13).
/// Its switch is `<node>.on`. A **generator** makes sound, and its first row
/// after the switch is `<node>.gain`, named "Gain". An **effect** shapes
/// sound, and its first row is `<node>.mix`, named "Mix". No row's name
/// repeats its node's name, because the section header already says it:
/// "Frequency", not "Ring freq". The panel draws rows in table order, so order
/// here is layout. The tests at the bottom hold all of this.
pub const PARAMS: &[ParamDef] = &[
    // The patch's own settings, before any node (Georg, 2026-09-27): what
    // sets a note's length. See `note.rs`. Sample by default, which is how
    // every patch played before the setting existed.
    ParamDef {
        id: "patch.length",
        name: "Length",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Stepped(3),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // How long a note is held under Hold, before its release. Read once, when
    // the note starts, so nothing moving it can move a note's end mid-note.
    ParamDef {
        id: "patch.hold",
        name: "Hold",
        min: 10.0,
        max: 10_000.0,
        default: 400.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    // Generators make sound, effects shape it (Georg, 2026-09-13). The plain
    // sample is one generator and the grain cloud is another; each has a
    // switch and a Gain, and the two are summed into the effects. The plain
    // sample starts on, so pressing play gives the material.
    ParamDef {
        id: "material.on",
        name: "Material",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "material.gain",
        name: "Gain",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // No octave or trim here: those belong to each material, and reach the
    // engine as a `sources::Reading` per generator (Georg, 2026-09-15).
    // Section switches, one per effect. Stepped, and never smoothed here: the
    // engine fades each bypass itself over a fixed 10 ms, so a switch cannot
    // click, and a section that is off is bit-exact with its mix at zero. The
    // panel draws them in the section header, not as rows.
    //
    // Effects and the grain cloud start off (Georg, 2026-09-13). Pressing play
    // gives the plain sample, and switching a section on is the before and
    // after. The envelope starts on, because its neutral shape is already
    // transparent.
    ParamDef {
        id: "grain.on",
        name: "Granular",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // The cloud's own level. Unity by default, so switching granular on is
    // heard at once: a generator's neutral is unity, where an effect's mix is
    // zero.
    ParamDef {
        id: "grain.gain",
        name: "Gain",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    ParamDef {
        id: "grain.position",
        name: "Position",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    ParamDef {
        id: "grain.jitter",
        name: "Jitter",
        min: 0.0,
        max: 1.0,
        default: 0.05,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    ParamDef {
        id: "grain.size",
        name: "Size",
        min: 5.0,
        max: 500.0,
        default: 180.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.density",
        name: "Density",
        min: 0.5,
        max: 200.0,
        default: 60.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.pitch",
        name: "Pitch",
        min: -24.0,
        max: 24.0,
        default: 0.0,
        taper: Taper::Bipolar,
        unit: Unit::Semitones,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.spread",
        name: "Pitch spread",
        min: 0.0,
        max: 24.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Semitones,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.pan",
        name: "Pan spread",
        min: 0.0,
        max: 1.0,
        default: 0.6,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 30.0,
    },
    ParamDef {
        id: "grain.reverse",
        name: "Reverse",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "grain.window",
        name: "Window",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        taper: Taper::Stepped(4),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // FM, the third generator (Georg, 2026-09-27). Two operators, built as
    // phase modulation; see `fm.rs`. It reads no material: its pitch is the
    // frequency row, moved by a step's pitch. Off by default, like the cloud.
    ParamDef {
        id: "fm.on",
        name: "FM",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "fm.gain",
        name: "Gain",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    ParamDef {
        id: "fm.freq",
        name: "Frequency",
        min: 20.0,
        max: 2000.0,
        default: 110.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    // Exponential, so 1 sits a third of the way along and 2 at the centre.
    ParamDef {
        id: "fm.ratio",
        name: "Ratio",
        min: 0.25,
        max: 16.0,
        default: 2.0,
        taper: Taper::Exponential,
        unit: Unit::None,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "fm.index",
        name: "Index",
        min: 0.0,
        max: 10.0,
        default: 1.5,
        taper: Taper::Linear,
        unit: Unit::None,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "fm.feedback",
        name: "Feedback",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // Towards analogue (Georg, 2026-09-27). Each operator wanders off its
    // pitch on its own; zero is the exact, clinical voice.
    ParamDef {
        id: "fm.drift",
        name: "Drift",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // Phase, linear or exponential: see `FmType`. Names from `FmType::NAMES`.
    ParamDef {
        id: "fm.type",
        name: "Type",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Stepped(3),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "ring.on",
        name: "Ring modulation",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "ring.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "ring.freq",
        name: "Frequency",
        min: 1.0,
        max: 5000.0,
        default: 140.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    // The chorus, last in the process lane (Georg, 2026-09-29): three swept
    // copies a channel, the right's LFOs between the left's, so it widens as
    // well as thickens. Rate and depth stop where it would turn to warble
    // (`chorus::MAX_RATE_HZ`). EQ is the Boss CH-1's: a shelf on the copies.
    // Type picks the voicing: the plain chorus; a string-ensemble one, wider
    // and lusher; or Choir, copies held sharp and flat for voices, where
    // depth is the detune and rate how fast the copies wander. Names from
    // `ChorusType::NAMES`. In the ensemble, rate is the slow swirl and depth
    // scales it and its fast shimmer together.
    ParamDef {
        id: "chorus.on",
        name: "Chorus",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "chorus.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "chorus.type",
        name: "Type",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Stepped(3),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "chorus.rate",
        name: "Rate",
        min: 0.05,
        max: crate::chorus::MAX_RATE_HZ,
        default: 0.6,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "chorus.depth",
        name: "Depth",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // Voices is continuous: the next copy fades in as it turns, so it sweeps
    // and follows an LFO without a click. Few beat, many blur into a wall.
    ParamDef {
        id: "chorus.voices",
        name: "Voices",
        min: 1.0,
        max: crate::chorus::MAX_VOICES as f32,
        default: 4.0,
        taper: Taper::Linear,
        unit: Unit::None,
        smooth_ms: 20.0,
    },
    // Spread scales where every voicing's copies sit, half to twice; low cut
    // is a high-pass on the copies. Both shape every voicing alike.
    ParamDef {
        id: "chorus.spread",
        name: "Spread",
        min: crate::chorus::SPREAD_MIN,
        max: crate::chorus::SPREAD_MAX,
        default: 1.0,
        taper: Taper::Exponential,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "chorus.lowcut",
        name: "Low cut",
        min: crate::chorus::LOW_CUT_MIN_HZ,
        max: crate::chorus::LOW_CUT_MAX_HZ,
        default: 240.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "chorus.eq",
        name: "EQ",
        min: -crate::chorus::EQ_RANGE_DB,
        max: crate::chorus::EQ_RANGE_DB,
        default: 0.0,
        taper: Taper::Bipolar,
        unit: Unit::Db,
        smooth_ms: 20.0,
    },
    // The flanger: a very short swept delay summed with the dry, so comb notches sweep the spectrum. Feedback is bipolar: negative is the hollow comb, positive the ringing one.
    // Right after the chorus, which it is the close cousin of.
    ParamDef {
        id: "flanger.on",
        name: "Flanger",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "flanger.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "flanger.manual",
        name: "Manual",
        min: crate::flanger::MANUAL_MIN_MS,
        max: crate::flanger::MANUAL_MAX_MS,
        default: 1.5,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "flanger.rate",
        name: "Rate",
        min: crate::flanger::MIN_RATE_HZ,
        max: crate::flanger::MAX_RATE_HZ,
        default: 0.25,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "flanger.depth",
        name: "Depth",
        min: 0.0,
        max: 1.0,
        default: 0.7,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "flanger.feedback",
        name: "Feedback",
        min: -crate::flanger::MAX_FEEDBACK,
        max: crate::flanger::MAX_FEEDBACK,
        default: 0.5,
        taper: Taper::Bipolar,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // Wear: a cassette that has been looked after badly. Wow and flutter bend the pitch, Unstable lurches it on a chaotic path that never repeats, Dropouts dip the level, and Dull closes a low-pass that follows the level, so quiet parts go muffled.
    // Before the delay, so what the delay repeats is already worn.
    ParamDef {
        id: "wear.on",
        name: "Wear",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "wear.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 0.7,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "wear.wow",
        name: "Wow",
        min: 0.0,
        max: 1.0,
        default: 0.4,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "wear.flutter",
        name: "Flutter",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "wear.unstable",
        name: "Unstable",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "wear.dropouts",
        name: "Dropouts",
        min: 0.0,
        max: 1.0,
        default: 0.2,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "wear.dull",
        name: "Dull",
        min: 0.0,
        max: 1.0,
        default: 0.4,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // The clean delay: faithful repeats, darkened only by
    // Tone in the loop. Time glides, so turning it bends pitch. Ping-pong
    // sends each repeat to the other side. The degraded one is `echo`.
    ParamDef {
        id: "delay.on",
        name: "Delay",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "delay.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "delay.time",
        name: "Time",
        min: crate::delay::DELAY_MIN_MS,
        max: crate::delay::DELAY_MAX_MS,
        default: 350.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "delay.feedback",
        name: "Feedback",
        min: 0.0,
        max: crate::delay::FEEDBACK_MAX,
        default: 0.4,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "delay.tone",
        name: "Tone",
        min: crate::delay::TONE_MIN_HZ,
        max: crate::delay::TONE_MAX_HZ,
        default: 8000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "delay.pingpong",
        name: "Ping-pong",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "env.on",
        name: "Envelope",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // Formerly `env.amount`. It is the envelope's mix: how much of the shaped
    // signal replaces the unshaped one, transparent at zero.
    ParamDef {
        id: "env.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "env.attack",
        name: "Attack",
        min: 0.0,
        max: 4000.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "env.decay",
        name: "Decay",
        min: 0.0,
        max: 4000.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "env.sustain",
        name: "Sustain",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "env.release",
        name: "Release",
        min: 0.0,
        max: 4000.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    // Drive, first of the processes (Georg, 2026-09-14): shape the material
    // before the crusher and the ring modulator have it. Type names come
    // from `DriveType::NAMES`.
    ParamDef {
        id: "drive.on",
        name: "Drive",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "drive.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "drive.amount",
        name: "Amount",
        min: 0.0,
        max: 48.0,
        default: 24.0,
        taper: Taper::Linear,
        unit: Unit::None,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "drive.tone",
        name: "Tone",
        min: 200.0,
        max: 20_000.0,
        default: 8_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "drive.type",
        name: "Type",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        taper: Taper::Stepped(4),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "crush.on",
        name: "Crush",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // Defaults to zero, like the ring modulator: loading a patch and pressing
    // play gives you the material, not an effect you did not ask for.
    ParamDef {
        id: "crush.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "crush.bits",
        name: "Bits",
        min: 1.0,
        max: 16.0,
        default: 16.0,
        // Bits are already the logarithm of the level count, so a linear
        // control here is the perceptually even one.
        taper: Taper::Linear,
        unit: Unit::None,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "crush.rate",
        name: "Rate",
        min: 200.0,
        max: 48_000.0,
        default: 48_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    // The crush envelope. Multiplies `crush.mix` rather than the signal, so
    // an attack is "starts clean, then crushes" and a release is the reverse.
    // Its neutral shape is a flat one, which leaves the knob untouched.
    ParamDef {
        id: "crush.env.amount",
        name: "Env amount",
        min: 0.0,
        max: 1.0,
        // Full depth, like the amplitude envelope. Harmless as a default
        // because a neutral ADSR is flat whatever the depth is set to.
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "crush.env.attack",
        name: "Env attack",
        min: 0.0,
        max: 4000.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "crush.env.decay",
        name: "Env decay",
        min: 0.0,
        max: 4000.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "crush.env.sustain",
        name: "Env sustain",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "crush.env.release",
        name: "Env release",
        min: 0.0,
        max: 4000.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    // The tape transport. One mechanism: `speed` is a multiplier on tape time,
    // and brake and reverse are two ways of asking for a different one. Both
    // arrive through the same slew, which is what makes a stop sound like a
    // finger on the reel rather than a mute.
    ParamDef {
        id: "tape.brake",
        name: "Brake",
        min: 0.0,
        max: 1.0,
        // Continuous, not a switch, even though the button only sends 0 and 1.
        // A pedal or a MIDI CC lands here unchanged, and half a brake is a
        // real thing: the tape runs slow and flat instead of stopping.
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        // Not smoothed here. `tape.time` is the slew, and it is the control
        // that decides how the gesture sounds.
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "tape.time",
        name: "Time",
        min: 20.0,
        max: 3000.0,
        default: 400.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    // Reverse is a *gate*, not a state: the button holds it high, and later a
    // MIDI note or CC will do the same. What a tap means rather than a hold is
    // decided by the engine, so both sources get the same gesture.
    ParamDef {
        id: "tape.reverse",
        name: "Reverse",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        // Stepped, so it draws as a selector and cannot be linked to an LFO.
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "tape.flick",
        name: "Flick",
        // The shortest reverse a tap can produce. Hold the gate longer than
        // this and it behaves as a plain hold, releasing the moment you do.
        min: 50.0,
        max: 500.0,
        default: 150.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    // The filter, on the master after every effect (Georg, 2026-09-14). What
    // takes the harmonics away that crush and ring add. Type is stepped, and
    // the UI names its steps from `FilterType::NAMES`.
    ParamDef {
        id: "filter.on",
        name: "Filter",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "filter.mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "filter.cutoff",
        name: "Cutoff",
        min: 20.0,
        max: 20_000.0,
        default: 2_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "filter.resonance",
        name: "Resonance",
        min: 0.0,
        max: 1.0,
        default: 0.2,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "filter.type",
        name: "Type",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Stepped(3),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // The master gain, after every generator and effect. Unity is bit-exact.
    ParamDef {
        id: "amp.gain",
        name: "Gain",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
];

/// The switchable nodes that make sound rather than shape it. They lead with
/// Gain; every other switchable node is an effect and leads with Mix.
pub const GENERATORS: &[&str] = &["material", "grain", "fm"];

pub fn index_of(id: &str) -> Option<usize> {
    PARAMS.iter().position(|p| p.id == id)
}

/// Lock-free parameter storage, shared between whatever edits values and the
/// audio thread. Values are stored as real values, not normalised, so the
/// audio thread never has to consult the table.
///
/// Relaxed ordering is correct here: each parameter is independent, a reader
/// that sees a value one block late is inaudible, and there is no other state
/// whose visibility depends on these writes.
pub struct ParamBank {
    values: Vec<AtomicU32>,
    /// The table this bank holds, which decides each slot's range: the
    /// patch's, or the arrangement's (`crate::arrangement`).
    defs: &'static [ParamDef],
}

impl ParamBank {
    /// A bank for the patch table.
    pub fn new() -> Self {
        Self::for_table(PARAMS)
    }

    /// A bank for any table, at its defaults.
    pub fn for_table(defs: &'static [ParamDef]) -> Self {
        Self {
            values: defs
                .iter()
                .map(|p| AtomicU32::new(p.default.to_bits()))
                .collect(),
            defs,
        }
    }

    pub fn defs(&self) -> &'static [ParamDef] {
        self.defs
    }

    /// Where an id sits in this bank's table.
    pub fn index(&self, id: &str) -> Option<usize> {
        self.defs.iter().position(|p| p.id == id)
    }

    #[inline]
    pub fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set(&self, index: usize, value: f32) {
        let d = &self.defs[index];
        let lo = d.min.min(d.max);
        let hi = d.max.max(d.min);
        self.values[index].store(value.clamp(lo, hi).to_bits(), Ordering::Relaxed);
    }

    pub fn get_by_id(&self, id: &str) -> Option<f32> {
        self.index(id).map(|i| self.get(i))
    }

    pub fn set_by_id(&self, id: &str, value: f32) -> bool {
        match self.index(id) {
            Some(i) => {
                self.set(i, value);
                true
            }
            None => false,
        }
    }

    /// Set from a 0-to-1 control position, applying the parameter's taper.
    pub fn set_normalised(&self, id: &str, t: f32) -> bool {
        match self.index(id) {
            Some(i) => {
                self.set(i, self.defs[i].denormalise(t));
                true
            }
            None => false,
        }
    }
}

impl Default for ParamBank {
    fn default() -> Self {
        Self::new()
    }
}

/// One pattern for every switchable node: its switch, then its level as the
/// first row the panel draws. Gain for a generator, Mix for an effect. The
/// panel draws in table order, so this is a test of the table's order as much
/// as of its names. `prefix` is what every id in `table` starts with, such as
/// `arrangement.`, and is not part of the node's name.
#[cfg(test)]
pub(crate) fn leads_with_its_level(table: &[ParamDef], prefix: &str) {
    let switches: Vec<_> = table.iter().filter(|p| p.id.ends_with(".on")).collect();
    assert!(!switches.is_empty());
    for switch in switches {
        let node = switch.id.trim_end_matches(".on");
        let under = format!("{node}.");
        let first = table
            .iter()
            .find(|p| p.id.starts_with(&under) && p.id != switch.id)
            .unwrap_or_else(|| panic!("{node} has a switch and nothing else"));
        let (id, name) = if GENERATORS.contains(&node.trim_start_matches(prefix)) {
            (format!("{node}.gain"), "Gain")
        } else {
            (format!("{node}.mix"), "Mix")
        };
        assert_eq!(first.id, id, "{node} must lead with its {name}");
        assert_eq!(first.name, name, "{node}'s first row must be called {name}");
    }
}

/// Every row sits under a section header that already names its node, so
/// "Ring freq" says "ring" twice. Switches are exempt: their name is the
/// section's own.
#[cfg(test)]
pub(crate) fn repeats_no_node_name(table: &[ParamDef], prefix: &str) {
    for p in table.iter().filter(|p| !p.id.ends_with(".on")) {
        let node = p.id.trim_start_matches(prefix).split('.').next().unwrap();
        assert!(
            !p.name.to_lowercase().starts_with(node),
            "{} repeats its node in the name {:?}",
            p.id,
            p.name
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in PARAMS {
            assert!(seen.insert(p.id), "duplicate id: {}", p.id);
        }
    }

    #[test]
    fn defaults_sit_inside_their_range() {
        for p in PARAMS {
            assert!(
                p.default >= p.min && p.default <= p.max,
                "{} default {} outside {}..{}",
                p.id,
                p.default,
                p.min,
                p.max
            );
        }
    }

    #[test]
    fn exponential_params_have_a_positive_floor() {
        // Geometric interpolation is undefined through zero. A parameter
        // tagged exponential with a zero minimum would silently fall back to
        // linear, which is the bug this test exists to catch.
        for p in PARAMS {
            if p.taper == Taper::Exponential {
                assert!(
                    p.min > 0.0,
                    "{} is exponential but starts at {}",
                    p.id,
                    p.min
                );
            }
        }
    }

    #[test]
    fn stepped_params_never_smooth() {
        for p in PARAMS {
            if matches!(p.taper, Taper::Stepped(_)) {
                assert_eq!(p.smooth_ms, 0.0, "{} is stepped but smooths", p.id);
            }
        }
    }

    #[test]
    fn denormalise_hits_both_ends() {
        for p in PARAMS {
            assert!((p.denormalise(0.0) - p.min).abs() < 1e-3, "{} at 0", p.id);
            assert!((p.denormalise(1.0) - p.max).abs() < 1e-3, "{} at 1", p.id);
        }
    }

    #[test]
    fn normalise_round_trips() {
        for p in PARAMS {
            for step in 0..=20 {
                let t = step as f32 / 20.0;
                let v = p.denormalise(t);
                let back = p.normalise(v);
                // A stepped control cannot round-trip exactly: a position
                // lands in a bucket and comes back as that bucket's centre.
                // The error that allows is the bucket width, so the bound has
                // to come from the step count — a flat 0.3 silently assumed
                // four steps, and a two-step control has buckets twice as
                // wide.
                let tolerance = match p.taper {
                    Taper::Stepped(n) => 1.0 / n.max(1) as f32 + 1e-3,
                    _ => 1e-3,
                };
                assert!(
                    (back - t).abs() < tolerance,
                    "{} round trip {t} -> {v} -> {back}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn exponential_midpoint_is_geometric() {
        let size = &PARAMS[index_of("grain.size").unwrap()];
        let mid = size.denormalise(0.5);
        let geometric = (size.min * size.max).sqrt();
        assert!((mid - geometric).abs() < 1.0, "{mid} vs {geometric}");
        // And it is well below the arithmetic midpoint, which is the point.
        assert!(mid < (size.min + size.max) / 2.0);
    }

    #[test]
    fn stepped_lands_on_exact_steps() {
        let w = &PARAMS[index_of("grain.window").unwrap()];
        let seen: Vec<f32> = (0..=20).map(|i| w.denormalise(i as f32 / 20.0)).collect();
        for v in &seen {
            assert!((v - v.round()).abs() < 1e-4, "{v} is not a whole step");
            assert!(*v >= 0.0 && *v <= 3.0);
        }
        assert!(seen.contains(&0.0) && seen.contains(&3.0));
    }

    #[test]
    fn every_switchable_node_leads_with_its_level() {
        leads_with_its_level(PARAMS, "");
    }

    #[test]
    fn no_parameter_repeats_its_node_name() {
        repeats_no_node_name(PARAMS, "");
    }

    #[test]
    fn bank_starts_at_defaults() {
        let bank = ParamBank::new();
        for (i, p) in PARAMS.iter().enumerate() {
            assert_eq!(bank.get(i), p.default, "{}", p.id);
        }
    }

    #[test]
    fn bank_clamps_out_of_range_writes() {
        let bank = ParamBank::new();
        bank.set_by_id("grain.position", 99.0);
        assert_eq!(bank.get_by_id("grain.position"), Some(1.0));
        bank.set_by_id("grain.position", -99.0);
        assert_eq!(bank.get_by_id("grain.position"), Some(0.0));
    }

    #[test]
    fn unknown_ids_are_rejected_not_ignored() {
        let bank = ParamBank::new();
        assert!(!bank.set_by_id("grain.nope", 0.5));
        assert!(bank.get_by_id("grain.nope").is_none());
        assert!(bank.set_by_id("grain.size", 200.0));
    }

    #[test]
    fn normalised_writes_apply_the_taper() {
        let bank = ParamBank::new();
        bank.set_normalised("grain.size", 0.5);
        let v = bank.get_by_id("grain.size").unwrap();
        let d = &PARAMS[index_of("grain.size").unwrap()];
        assert!((v - (d.min * d.max).sqrt()).abs() < 1.0);
    }
}
