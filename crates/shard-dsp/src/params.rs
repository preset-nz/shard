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
use std::sync::LazyLock;

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
    Seconds,
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
    /// One sentence for the user, shown as the row's tooltip. A switch row
    /// (`<node>.on`) describes the node itself.
    pub description: &'static str,
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
/// **Every switchable node follows one pattern, by role** (Georg, 2026-09-13).
/// Its switch is `<node>.on`. A **generator** makes sound, and its first row
/// after the switch is `<node>.gain`, named "Gain". An **effect** shapes
/// sound, and its first row is `<node>.mix`, named "Mix". No row's name
/// repeats its node's name, because the section header already says it:
/// "Frequency", not "Ring freq". The panel draws rows in table order, so order
/// here is layout. The tests at the bottom hold all of this.
const BASE: &[ParamDef] = &[
    // The patch's own settings, before any node (Georg, 2026-09-27): what
    // sets a note's length. See `note.rs`. Sample by default, which is how
    // every patch played before the setting existed.
    ParamDef {
        id: "patch.length",
        name: "Length",
        description: "How long a note lasts: Loop runs for ever, Sample plays one pass of the sample, and Hold lasts as long as Hold says.",
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
        description: "How long a note is held before it is released, when Length is set to Hold.",
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
        description: "The loaded sample played as it is, ahead of the effects; the other generators sound alongside it.",
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
        description: "How loud the sample is going into the effects, from silence to double.",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // Scrubbing (roadmap row 45), a mode of the plain sample: with it on, the
    // sample plays no passes and its head chases Position instead. The head
    // moves through a smoother in the voice (`scrub.rs`), so these two rows
    // are read raw. Octave, steps and Length do not apply while it is on.
    ParamDef {
        id: "material.scrub",
        name: "Scrub",
        description: "Swaps normal playback for a head that glides to Position, so you can drag through the sample like a record.",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "material.position",
        name: "Position",
        description: "Where in the sample the scrub head sits, from the start at the left to the end at the right.",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 0.0,
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
        description: "Cuts the sample into tiny grains and scatters them into a cloud, anywhere from a shimmer to a stuttering texture.",
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
        description: "How loud the grain cloud is, from silence to double.",
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
        description: "Where in the sample the grains are taken from, from the start to the end.",
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
        description: "How far each grain strays from Position at random: none keeps them together, more smears the cloud across the sample.",
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
        description: "How long each grain is: short grains buzz and blur, long ones keep the sample recognisable.",
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
        description: "How many grains start each second: sparse grains are heard one by one, dense ones fuse into a continuous sound.",
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
        description: "Shifts every grain up or down in pitch, in semitones.",
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
        description: "Gives each grain a random pitch within this many semitones, from a plain cloud to a detuned swarm.",
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
        description: "How widely the grains scatter between left and right; none keeps every grain in the centre.",
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
        description: "The share of grains that play backwards.",
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
        description: "The fade shape of each grain: Hann is smooth, Triangle is slightly brighter, Expodec clicks like a percussive hit, and Rexpodec swells then cuts off.",
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
        description: "A two-operator FM voice: a pitched tone that one oscillator bends with another, from soft bell to harsh metal.",
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
        description: "How loud the FM tone is, from silence to double.",
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
        description: "The pitch of the FM tone; a step's pitch moves it.",
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
        description: "How the second oscillator's pitch relates to the first: whole numbers sound harmonic, anything else turns clangy and bell-like.",
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
        description: "How hard one oscillator bends the other: low is a soft tone, high is bright and buzzy.",
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
        description: "Feeds the modulating oscillator back into itself, thickening the tone to a rasp and, at the top, towards noise.",
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
        description: "Lets each oscillator wander slightly off its pitch, from clinically exact at zero to loosely analogue.",
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
        description: "How the oscillators bend each other: Phase holds its pitch at any Index, Linear drifts in pitch once Feedback is up, and Exponential goes sharp as Index rises, as an analogue VCO does.",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Stepped(3),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // Noise, the fourth generator (epic 48). See `noise.rs`. It reads no
    // material and, for now, has no pitch. Off by default, like FM.
    ParamDef {
        id: "noise.on",
        name: "Noise",
        description: "A continuous noise generator, from low rumble to bright hiss, with an optional pitched band for wind and whistles.",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "noise.gain",
        name: "Gain",
        description: "How loud the noise is, from silence to double.",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    // The kind of noise, as a slider so an LFO can sweep it: -1 brown, -0.5
    // pink, 0 white, 0.5 blue, 1 violet, and between two a crossfade of the
    // pair at the same level. The id keeps the standard spelling.
    ParamDef {
        id: "noise.color",
        name: "Colour",
        description: "The kind of noise: brown rumble at the far left, then pink, white at the centre, blue, and violet hiss at the far right.",
        min: -1.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Bipolar,
        unit: Unit::None,
        smooth_ms: 20.0,
    },
    // A resonant band-pass of the noise at Frequency, crossfaded in at the
    // noise's own level (see `noise.rs`): wind, whistle, a pitched hiss. Zero
    // is the plain noise, bit for bit.
    ParamDef {
        id: "noise.band",
        name: "Band",
        description: "How much of the noise is squeezed into a narrow band at Frequency, for wind, whistles and pitched hiss; zero leaves the plain noise.",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // The band's centre. A step's pitch moves it, as it moves fm.freq.
    ParamDef {
        id: "noise.freq",
        name: "Frequency",
        description: "The centre of the noise band; a step's pitch moves it.",
        min: 20.0,
        max: 8000.0,
        default: 440.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    // 0 a whistle (Q 80), 1 a blow two octaves wide.
    ParamDef {
        id: "noise.width",
        name: "Width",
        description: "How wide the noise band is, from a thin whistle to a broad blow two octaves wide.",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // 0 Notes: gated by the notes and shaped by the amplitude envelope like
    // every generator. 1 Free: sounds for as long as the transport plays,
    // past the envelope and Hold's fall, into the filter. See `voice.rs`.
    ParamDef {
        id: "noise.run",
        name: "Run",
        description: "Whether the noise follows the notes (Notes) or sounds for as long as the transport plays, past the envelope (Free).",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // Dust, the fifth generator (row 59): sparse random clicks, a tick now
    // and then to dense crackle. See `dust.rs`. Reads no material and has no
    // pitch. Off by default.
    ParamDef {
        id: "dust.on",
        name: "Dust",
        description: "Sparse random clicks, from the odd tick to a dense crackle, like dust on a record.",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "dust.gain",
        name: "Gain",
        description: "How loud the clicks are, from silence to double.",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 40.0,
    },
    // Clicks a second, on average; where they fall is random, never a grid.
    ParamDef {
        id: "dust.density",
        name: "Density",
        description: "How many clicks fall each second on average, from a sparse tick to a dense crackle; where they land is random.",
        min: 0.1,
        max: 2000.0,
        default: 8.0,
        taper: Taper::Exponential,
        unit: Unit::None,
        smooth_ms: 20.0,
    },
    // Each click's brightness: dark thuds at 0, bright ticks at 1.
    ParamDef {
        id: "dust.tone",
        name: "Tone",
        description: "How bright each click is, from dark thuds to bright ticks.",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // Each click's length: a needle tick to a short pop.
    ParamDef {
        id: "dust.decay",
        name: "Decay",
        description: "How long each click lasts, from a needle tick to a short pop.",
        min: 0.1,
        max: 50.0,
        default: 1.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 20.0,
    },
    // 0 every click in the centre; 1 each at its own random place.
    ParamDef {
        id: "dust.spread",
        name: "Spread",
        description: "How widely the clicks scatter across the stereo field; none puts every click in the centre.",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "env.on",
        name: "Envelope",
        description: "Shapes how each note swells in and dies away; at its neutral setting it passes the sound untouched.",
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
        description: "How much of the shaped sound replaces the unshaped one; zero leaves the sound untouched.",
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
        description: "How long the sound takes to rise from silence to full level.",
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
        description: "How long the sound takes to fall from full level to Sustain.",
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
        description: "The level held after the decay, until the note is released.",
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
        description: "How long the sound takes to fade to silence once the note ends.",
        min: 0.0,
        max: 4000.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    // The tape's feel. One mechanism: `speed` is a multiplier on tape time,
    // and brake and reverse are two ways of asking for a different one. Both
    // arrive through the same slew, which is what makes a stop sound like a
    // finger on the reel rather than a mute. Brake and reverse themselves are
    // played, and slow everything, so they are the arrangement's
    // (`arrangement.tape.brake`, `arrangement.tape.reverse`); the slew and the
    // flick are still the patch's, and the engine reads the first voice's.
    ParamDef {
        id: "tape.time",
        name: "Time",
        description: "How quickly the tape slows or speeds up for a brake or reverse: short is a snap, long is a slow wind.",
        min: 20.0,
        max: 3000.0,
        default: 400.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "tape.flick",
        name: "Flick",
        description: "How long a quick tap on reverse plays backwards; hold it longer and the reverse lasts until you let go.",
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
        description: "A resonant filter after all the effects, taking away the highs, the lows or everything outside a band.",
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
        description: "How much filtered sound is blended with the unfiltered; full is all filtered.",
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
        description: "The frequency where the filter starts to cut.",
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
        description: "Emphasis around the cutoff: a gentle bump at low settings, a ringing whistle at high ones.",
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
        description: "Which part of the sound is kept: Low-pass lets the lows through, High-pass the highs, Band-pass a band around the cutoff.",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Stepped(3),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // 12 or 24 dB per octave. At 24 the resonance sits on the second of two
    // stages; the arrangement's filter borrows this row too.
    ParamDef {
        id: "filter.slope",
        name: "Slope",
        description: "How steeply the filter cuts past the cutoff: 12 dB is gentle, 24 dB is steep.",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // Off, half, full: how far the cutoff follows the step's pitch, an octave
    // per octave at full. The patch's filter only; the arrangement has no note.
    ParamDef {
        id: "filter.tracking",
        name: "Tracking",
        description: "How far the cutoff follows each step's pitch: off, half, or a full octave for every octave.",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Stepped(3),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    // The ladder (epic 36, story 3): a driven 4-pole low-pass after the
    // filter, patch only. Resonance one self-oscillates at the cutoff.
    ParamDef {
        id: "ladder.on",
        name: "Ladder",
        description: "A driven four-pole low-pass after the patch filter, modelled on the Moog ladder: growls when pushed, whistles at full resonance.",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Stepped(2),
        unit: Unit::None,
        smooth_ms: 0.0,
    },
    ParamDef {
        id: "ladder.mix",
        name: "Mix",
        description: "How much of the ladder's sound is blended with what goes into it; full is all ladder.",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "ladder.drive",
        name: "Drive",
        description: "Pushes the signal into the filter so it gets dirtier rather than louder.",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "ladder.cutoff",
        name: "Cutoff",
        description: "The frequency above which the ladder cuts, at a steep 24 dB per octave.",
        min: 20.0,
        max: 20_000.0,
        default: 2_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        smooth_ms: 20.0,
    },
    ParamDef {
        id: "ladder.resonance",
        name: "Resonance",
        description: "Emphasis at the cutoff: it growls as it rises and whistles on its own at full.",
        min: 0.0,
        max: 1.0,
        default: 0.2,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
    // Off, half, full, as the filter's: the ladder's own, so the clean filter
    // can stay put while this follows the note.
    ParamDef {
        id: "ladder.tracking",
        name: "Tracking",
        description: "How far the ladder's cutoff follows each step's pitch: off, half, or full, separate from the filter's own.",
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
        description: "The master level, after every generator and effect; 100% leaves the sound as it is.",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        smooth_ms: 20.0,
    },
];

/// Every row: the fixed ones, with the palette's instance rows and order rows
/// placed where the process lane begins.
pub static PARAMS: LazyLock<Vec<ParamDef>> = LazyLock::new(|| {
    let env = BASE
        .iter()
        .position(|p| p.id == "env.on")
        .expect("the envelope's switch is in the base table");
    let mut all = Vec::with_capacity(BASE.len() + crate::fx::rows().len());
    all.extend_from_slice(&BASE[..env]);
    all.extend_from_slice(crate::fx::rows());
    all.extend_from_slice(&BASE[env..]);
    all
});

/// The switchable nodes that make sound rather than shape it. They lead with
/// Gain; every other switchable node is an effect and leads with Mix. A
/// drums track's kit is a generator too, but a track's, not a row here
/// (`kit.rs`).
pub const GENERATORS: &[&str] = &["material", "grain", "fm", "noise", "dust"];

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
        Self::for_table(&PARAMS[..])
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
        // `fx.3.chorus.rate` belongs to the chorus, not to "fx".
        let node = match crate::fx::parse_row(p.id) {
            Some((_, template)) => template.split('.').next().unwrap(),
            None => p.id.trim_start_matches(prefix).split('.').next().unwrap(),
        };
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
        for p in PARAMS.iter() {
            assert!(seen.insert(p.id), "duplicate id: {}", p.id);
        }
    }

    #[test]
    fn defaults_sit_inside_their_range() {
        for p in PARAMS.iter() {
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
        for p in PARAMS.iter() {
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
        for p in PARAMS.iter() {
            if matches!(p.taper, Taper::Stepped(_)) {
                assert_eq!(p.smooth_ms, 0.0, "{} is stepped but smooths", p.id);
            }
        }
    }

    #[test]
    fn denormalise_hits_both_ends() {
        for p in PARAMS.iter() {
            assert!((p.denormalise(0.0) - p.min).abs() < 1e-3, "{} at 0", p.id);
            assert!((p.denormalise(1.0) - p.max).abs() < 1e-3, "{} at 1", p.id);
        }
    }

    #[test]
    fn normalise_round_trips() {
        for p in PARAMS.iter() {
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
        leads_with_its_level(&PARAMS, "");
    }

    #[test]
    fn no_parameter_repeats_its_node_name() {
        repeats_no_node_name(&PARAMS, "");
    }

    /// An added effect is silent until its Mix is raised (Georg, 2026-10-10),
    /// in the patch and in the arrangement.
    #[test]
    fn every_effect_comes_in_with_its_mix_at_zero() {
        let rows = PARAMS.iter().chain(crate::arrangement::params());
        let mixes: Vec<_> = rows
            .filter(|p| p.id.contains("fx.") && p.id.ends_with(".mix"))
            .collect();
        assert!(!mixes.is_empty());
        for p in mixes {
            assert_eq!(p.default, 0.0, "{}", p.id);
        }
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

    fn assert_described(p: &ParamDef) {
        let d = p.description;
        assert!(!d.is_empty(), "{} has no description", p.id);
        assert!(
            d.ends_with('.'),
            "{}'s description must end with a full stop",
            p.id
        );
        assert_eq!(
            d.matches(". ").count(),
            0,
            "{}'s description is more than one sentence",
            p.id
        );
        let first = d
            .split_whitespace()
            .next()
            .unwrap()
            .trim_matches(|c: char| !c.is_alphanumeric());
        let name_first = p.name.split_whitespace().next().unwrap();
        assert!(
            !first.eq_ignore_ascii_case(name_first),
            "{}'s description opens with its own name",
            p.id
        );
    }

    #[test]
    fn every_row_and_template_has_a_description() {
        for p in PARAMS.iter() {
            assert_described(p);
        }
        for (kind, _) in crate::fx::POOL.iter() {
            for row in kind.rows() {
                assert_described(row);
            }
        }
        for p in crate::arrangement::params() {
            assert_described(p);
        }
    }
}
