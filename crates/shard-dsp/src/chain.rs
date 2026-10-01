//! The effect chain: drive, crusher, ring modulator, chorus, delay, then filter and gain.
//!
//! One type, so the patch and the level above it can run the same effects
//! (`design/arrangement-layer.md`). It comes in two halves because the patch
//! puts the tape's level and its envelope between them: `front` is drive,
//! crush, ring and chorus, `back` is the filter and the output gain.
//!
//! Each effect has a switch that fades its mix over the same 10 ms as every
//! other section, landing on an exact zero, so an effect that is off is
//! bit-exact with its mix at zero.

use crate::chorus::{Chorus, ChorusParams, ChorusType};
use crate::crush::{Crush, CrushParams};
use crate::delay::{Delay, DelayParams};
use crate::drive::{Drive, DriveParams, DriveType};
use crate::echo::{Echo, EchoParams};
use crate::filter::{Filter, FilterParams, FilterType};
use crate::flanger::{Flanger, FlangerParams};
use crate::params::ParamDef;
use crate::ringmod::{RingMod, RingModParams};
use crate::rise::{Rise, RiseParams};
use crate::smooth::{OnePole, Ramp};
use crate::wear::{Wear, WearParams};

/// How long a section switch takes. Short enough to feel instant, long enough
/// that cutting a loud section in or out never clicks.
pub(crate) const BYPASS_MS: f32 = 10.0;

/// Where a chain's rows sit in its table, resolved once at construction so
/// the audio thread never compares a string.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChainSlots {
    pub drive_on: usize,
    pub drive_mix: usize,
    pub drive_amount: usize,
    pub drive_tone: usize,
    pub drive_type: usize,
    pub crush_on: usize,
    pub crush_mix: usize,
    pub crush_bits: usize,
    pub crush_rate: usize,
    pub ring_on: usize,
    pub ring_mix: usize,
    pub ring_freq: usize,
    pub chorus_on: usize,
    pub chorus_mix: usize,
    pub chorus_type: usize,
    pub chorus_rate: usize,
    pub chorus_depth: usize,
    pub chorus_voices: usize,
    pub chorus_spread: usize,
    pub chorus_lowcut: usize,
    pub chorus_eq: usize,
    pub flanger_on: usize,
    pub flanger_mix: usize,
    pub flanger_manual: usize,
    pub flanger_rate: usize,
    pub flanger_depth: usize,
    pub flanger_feedback: usize,
    pub wear_on: usize,
    pub wear_mix: usize,
    pub wear_wow: usize,
    pub wear_flutter: usize,
    pub wear_unstable: usize,
    pub wear_dropouts: usize,
    pub wear_dull: usize,
    pub delay_on: usize,
    pub delay_mix: usize,
    pub delay_time: usize,
    pub delay_feedback: usize,
    pub delay_tone: usize,
    pub delay_pingpong: usize,
    pub echo_on: usize,
    pub echo_mix: usize,
    pub echo_time: usize,
    pub echo_feedback: usize,
    pub echo_tone: usize,
    pub echo_wobble: usize,
    pub echo_grit: usize,
    pub rise_on: usize,
    pub rise_mix: usize,
    pub rise_time: usize,
    pub rise_feedback: usize,
    pub rise_shift: usize,
    pub rise_wobble: usize,
    pub rise_tone: usize,
    pub filter_on: usize,
    pub filter_mix: usize,
    pub filter_cutoff: usize,
    pub filter_resonance: usize,
    pub filter_type: usize,
    pub gain: usize,
}

impl ChainSlots {
    /// `at` maps an id without its prefix, such as `crush.mix`, to a slot.
    /// It panics on a miss, since the tables are compile-time constants.
    pub fn resolve(at: impl Fn(&str) -> usize) -> Self {
        Self {
            drive_on: at("drive.on"),
            drive_mix: at("drive.mix"),
            drive_amount: at("drive.amount"),
            drive_tone: at("drive.tone"),
            drive_type: at("drive.type"),
            crush_on: at("crush.on"),
            crush_mix: at("crush.mix"),
            crush_bits: at("crush.bits"),
            crush_rate: at("crush.rate"),
            ring_on: at("ring.on"),
            ring_mix: at("ring.mix"),
            ring_freq: at("ring.freq"),
            chorus_on: at("chorus.on"),
            chorus_mix: at("chorus.mix"),
            chorus_type: at("chorus.type"),
            chorus_rate: at("chorus.rate"),
            chorus_depth: at("chorus.depth"),
            chorus_voices: at("chorus.voices"),
            chorus_spread: at("chorus.spread"),
            chorus_lowcut: at("chorus.lowcut"),
            chorus_eq: at("chorus.eq"),
            flanger_on: at("flanger.on"),
            flanger_mix: at("flanger.mix"),
            flanger_manual: at("flanger.manual"),
            flanger_rate: at("flanger.rate"),
            flanger_depth: at("flanger.depth"),
            flanger_feedback: at("flanger.feedback"),
            wear_on: at("wear.on"),
            wear_mix: at("wear.mix"),
            wear_wow: at("wear.wow"),
            wear_flutter: at("wear.flutter"),
            wear_unstable: at("wear.unstable"),
            wear_dropouts: at("wear.dropouts"),
            wear_dull: at("wear.dull"),
            delay_on: at("delay.on"),
            delay_mix: at("delay.mix"),
            delay_time: at("delay.time"),
            delay_feedback: at("delay.feedback"),
            delay_tone: at("delay.tone"),
            delay_pingpong: at("delay.pingpong"),
            echo_on: at("echo.on"),
            echo_mix: at("echo.mix"),
            echo_time: at("echo.time"),
            echo_feedback: at("echo.feedback"),
            echo_tone: at("echo.tone"),
            echo_wobble: at("echo.wobble"),
            echo_grit: at("echo.grit"),
            rise_on: at("rise.on"),
            rise_mix: at("rise.mix"),
            rise_time: at("rise.time"),
            rise_feedback: at("rise.feedback"),
            rise_shift: at("rise.shift"),
            rise_wobble: at("rise.wobble"),
            rise_tone: at("rise.tone"),
            filter_on: at("filter.on"),
            filter_mix: at("filter.mix"),
            filter_cutoff: at("filter.cutoff"),
            filter_resonance: at("filter.resonance"),
            filter_type: at("filter.type"),
            gain: at("amp.gain"),
        }
    }
}

/// One block's worth of targets for a chain. The switches are gates, zero or
/// one; the ramps turn them into fades.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChainParams {
    pub drive: DriveParams,
    pub drive_on: f32,
    pub crush_bits: f32,
    pub crush_rate: f32,
    pub crush_mix: f32,
    pub crush_on: f32,
    pub ring: RingModParams,
    pub ring_on: f32,
    pub chorus: ChorusParams,
    pub chorus_on: f32,
    pub flanger: FlangerParams,
    pub flanger_on: f32,
    pub wear: WearParams,
    pub wear_on: f32,
    pub delay: DelayParams,
    pub delay_on: f32,
    pub echo: EchoParams,
    pub echo_on: f32,
    pub rise: RiseParams,
    pub rise_on: f32,
    pub filter: FilterParams,
    pub filter_on: f32,
    pub gain: f32,
}

impl ChainParams {
    /// Read once a block. `value` is a continuous row as heard, which for the
    /// patch means through its LFOs; `raw` is a stepped row or a switch,
    /// which no LFO moves.
    pub fn read(s: &ChainSlots, value: impl Fn(usize) -> f32, raw: impl Fn(usize) -> f32) -> Self {
        let gate = |slot: usize| if raw(slot) >= 0.5 { 1.0 } else { 0.0 };
        Self {
            drive: DriveParams {
                amount_db: value(s.drive_amount),
                tone_hz: value(s.drive_tone),
                kind: DriveType::from_value(raw(s.drive_type)),
                mix: value(s.drive_mix),
            },
            drive_on: gate(s.drive_on),
            crush_bits: value(s.crush_bits),
            crush_rate: value(s.crush_rate),
            crush_mix: value(s.crush_mix),
            crush_on: gate(s.crush_on),
            ring: RingModParams {
                freq: value(s.ring_freq),
                mix: value(s.ring_mix),
            },
            ring_on: gate(s.ring_on),
            chorus: ChorusParams {
                kind: ChorusType::from_value(raw(s.chorus_type)),
                rate: value(s.chorus_rate),
                depth: value(s.chorus_depth),
                voices: value(s.chorus_voices),
                spread: value(s.chorus_spread),
                low_cut_hz: value(s.chorus_lowcut),
                eq_db: value(s.chorus_eq),
                mix: value(s.chorus_mix),
            },
            chorus_on: gate(s.chorus_on),
            flanger: FlangerParams {
                manual_ms: value(s.flanger_manual),
                rate_hz: value(s.flanger_rate),
                depth: value(s.flanger_depth),
                feedback: value(s.flanger_feedback),
                mix: value(s.flanger_mix),
            },
            flanger_on: gate(s.flanger_on),
            wear: WearParams {
                wow: value(s.wear_wow),
                flutter: value(s.wear_flutter),
                unstable: value(s.wear_unstable),
                dropouts: value(s.wear_dropouts),
                dull: value(s.wear_dull),
                mix: value(s.wear_mix),
            },
            wear_on: gate(s.wear_on),
            delay: DelayParams {
                time_ms: value(s.delay_time),
                feedback: value(s.delay_feedback),
                tone_hz: value(s.delay_tone),
                cross: value(s.delay_pingpong),
                mix: value(s.delay_mix),
            },
            delay_on: gate(s.delay_on),
            echo: EchoParams {
                time_ms: value(s.echo_time),
                feedback: value(s.echo_feedback),
                tone_hz: value(s.echo_tone),
                wobble: value(s.echo_wobble),
                grit: value(s.echo_grit),
                mix: value(s.echo_mix),
            },
            echo_on: gate(s.echo_on),
            rise: RiseParams {
                time_ms: value(s.rise_time),
                feedback: value(s.rise_feedback),
                shift_st: value(s.rise_shift),
                wobble: value(s.rise_wobble),
                tone_hz: value(s.rise_tone),
                mix: value(s.rise_mix),
            },
            rise_on: gate(s.rise_on),
            filter: FilterParams {
                cutoff_hz: value(s.filter_cutoff),
                resonance: value(s.filter_resonance),
                kind: FilterType::from_value(raw(s.filter_type)),
                mix: value(s.filter_mix),
            },
            filter_on: gate(s.filter_on),
            gain: value(s.gain),
        }
    }
}

struct ChainFades {
    drive: Ramp,
    crush: Ramp,
    ring: Ramp,
    chorus: Ramp,
    flanger: Ramp,
    wear: Ramp,
    delay: Ramp,
    echo: Ramp,
    rise: Ramp,
    filter: Ramp,
}

pub(crate) struct Chain {
    drive: Drive,
    crush: Crush,
    ringmod: RingMod,
    chorus: Chorus,
    flanger: Flanger,
    wear: Wear,
    delay: Delay,
    echo: Echo,
    rise: Rise,
    filter: Filter,
    fades: ChainFades,
    crush_mix: OnePole,
    gain: OnePole,
}

impl Chain {
    /// Settled at each row's default in `defs`, so nothing fades or glides on
    /// launch.
    pub fn new(sample_rate: f32, defs: &[ParamDef], s: &ChainSlots) -> Self {
        let ramp = |slot: usize| {
            let mut r = Ramp::new(defs[slot].default);
            r.set_time(BYPASS_MS, sample_rate);
            r
        };
        let smoother = |slot: usize| {
            let mut p = OnePole::new();
            p.set_time(defs[slot].smooth_ms, sample_rate);
            p.reset(defs[slot].default);
            p
        };
        Self {
            drive: Drive::new(sample_rate),
            crush: Crush::new(sample_rate),
            ringmod: RingMod::new(sample_rate),
            chorus: Chorus::new(sample_rate),
            flanger: Flanger::new(sample_rate),
            wear: Wear::new(sample_rate),
            delay: Delay::new(sample_rate),
            echo: Echo::new(sample_rate),
            rise: Rise::new(sample_rate),
            filter: Filter::new(sample_rate),
            fades: ChainFades {
                drive: ramp(s.drive_on),
                crush: ramp(s.crush_on),
                ring: ramp(s.ring_on),
                chorus: ramp(s.chorus_on),
                flanger: ramp(s.flanger_on),
                wear: ramp(s.wear_on),
                delay: ramp(s.delay_on),
                echo: ramp(s.echo_on),
                rise: ramp(s.rise_on),
                filter: ramp(s.filter_on),
            },
            crush_mix: smoother(s.crush_mix),
            gain: smoother(s.gain),
        }
    }

    /// Drive, crush, ring, chorus, delay, for one frame. `crush_env` scales the crusher's
    /// mix: the patch's crush envelope, or exactly one where there is none.
    #[inline]
    pub fn front(&mut self, l: f32, r: f32, p: &ChainParams, crush_env: f32) -> (f32, f32) {
        // Driven first, so the crusher and the ring modulator get the
        // harmonics the curve added. Order stops being fixed the day the
        // modifier stack lands.
        let drive_on = self.fades.drive.process(p.drive_on);
        let drive = DriveParams {
            mix: p.drive.mix * drive_on,
            ..p.drive
        };
        let (l, r) = self.drive.process(l, r, &drive);

        // Crushed before the ring modulator, so the modulator has the extra
        // partials the crusher just generated to fold against. Switched off,
        // the mix lands on an exact zero, which the crusher already treats as
        // a true bypass.
        let crush = CrushParams {
            bits: p.crush_bits,
            rate: p.crush_rate,
            mix: self.crush_mix.process(p.crush_mix)
                * crush_env
                * self.fades.crush.process(p.crush_on),
        };
        let (l, r) = self.crush.process(l, r, &crush);

        let ring_on = self.fades.ring.process(p.ring_on);
        let ring = RingModParams {
            mix: p.ring.mix * ring_on,
            ..p.ring
        };
        let (l, r) = self.ringmod.process(l, r, &ring);

        // Last in the lane (Georg, 2026-09-29), so it thickens whatever the
        // effects before it made.
        let chorus_on = self.fades.chorus.process(p.chorus_on);
        let chorus = ChorusParams {
            mix: p.chorus.mix * chorus_on,
            ..p.chorus
        };
        let (l, r) = self.chorus.process(l, r, &chorus);

        // Right after the chorus, which it is the close cousin of.
        let flanger_on = self.fades.flanger.process(p.flanger_on);
        let flanger = FlangerParams {
            mix: p.flanger.mix * flanger_on,
            ..p.flanger
        };
        let (l, r) = self.flanger.process(l, r, &flanger);

        // Before the delay, so what the delay repeats is already worn.
        let wear_on = self.fades.wear.process(p.wear_on);
        let wear = WearParams {
            mix: p.wear.mix * wear_on,
            ..p.wear
        };
        let (l, r) = self.wear.process(l, r, &wear);

        // After the chorus, so the repeats carry its width. Its line keeps
        // running while it is off, so switching it on never plays stale audio.
        let delay_on = self.fades.delay.process(p.delay_on);
        let delay = DelayParams {
            mix: p.delay.mix * delay_on,
            ..p.delay
        };
        let (l, r) = self.delay.process(l, r, &delay);

        // After the delay, so the two can be stacked.
        let echo_on = self.fades.echo.process(p.echo_on);
        let echo = EchoParams {
            mix: p.echo.mix * echo_on,
            ..p.echo
        };
        let (l, r) = self.echo.process(l, r, &echo);

        // After the echo.
        let rise_on = self.fades.rise.process(p.rise_on);
        let rise = RiseParams {
            mix: p.rise.mix * rise_on,
            ..p.rise
        };
        let (l, r) = self.rise.process(l, r, &rise);

        (l, r)
    }

    /// The filter, then the output gain, for one frame. Switched off, the
    /// filter's mix lands on an exact zero; unity gain is exact, since the
    /// smoother starts and rests on it.
    #[inline]
    pub fn back(&mut self, l: f32, r: f32, p: &ChainParams) -> (f32, f32) {
        let filter_on = self.fades.filter.process(p.filter_on);
        let filter = FilterParams {
            mix: p.filter.mix * filter_on,
            ..p.filter
        };
        let (l, r) = self.filter.process(l, r, &filter);
        let g = self.gain.process(p.gain);
        (l * g, r * g)
    }
}
