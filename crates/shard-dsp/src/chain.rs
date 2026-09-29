//! The effect chain: drive, crusher, ring modulator, chorus, then filter and gain.
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
use crate::drive::{Drive, DriveParams, DriveType};
use crate::filter::{Filter, FilterParams, FilterType};
use crate::params::ParamDef;
use crate::ringmod::{RingMod, RingModParams};
use crate::smooth::{OnePole, Ramp};

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
    filter: Ramp,
}

pub(crate) struct Chain {
    drive: Drive,
    crush: Crush,
    ringmod: RingMod,
    chorus: Chorus,
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
            filter: Filter::new(sample_rate),
            fades: ChainFades {
                drive: ramp(s.drive_on),
                crush: ramp(s.crush_on),
                ring: ramp(s.ring_on),
                chorus: ramp(s.chorus_on),
                filter: ramp(s.filter_on),
            },
            crush_mix: smoother(s.crush_mix),
            gain: smoother(s.gain),
        }
    }

    /// Drive, crush, ring, chorus, for one frame. `crush_env` scales the crusher's
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
        self.chorus.process(l, r, &chorus)
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
