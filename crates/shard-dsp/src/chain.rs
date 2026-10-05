//! The effect chain: the palette's effects in the order you chose, then the
//! filter and the output gain.
//!
//! One type, so the patch and the level above it can run the same effects
//! (`design/arrangement-layer.md`). It comes in two halves because the patch
//! puts the tape's level and its envelope between them: `front` is the
//! palette, `back` is the filter and the output gain.
//!
//! **The palette** (`design/effect-palette.md`). The chain owns a fixed pool
//! of instances, `crate::fx::POOL`, built once. The order rows say which of
//! them run and in what order. An instance the order does not name costs
//! nothing, and an empty order is a bit-exact passthrough.
//!
//! **Changing the chain does not click.**
//! - *Adding* resets the instance, so a tail from its last use cannot replay,
//!   then fades it in over `BYPASS_MS`.
//! - *Removing* fades it out over `BYPASS_MS` and keeps it running until the
//!   fade lands; only then is it dropped.
//! - *Reordering* changes the signal path at once, and effects hold state, so
//!   the chain's output ducks over `DUCK_MS`, swaps while silent, and comes
//!   back. Existing effects keep their state through it.
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
use crate::fx::{self, Kind, Order, INSTANCES, ORDER_LEN, POOL};
use crate::overtone::{Overtone, OvertoneParams};
use crate::params::ParamDef;
use crate::reverb::{Reverb, ReverbParams, ReverbType};
use crate::ringmod::{RingMod, RingModParams};
use crate::rise::{Rise, RiseParams};
use crate::smooth::{OnePole, Ramp};
use crate::wear::{Wear, WearParams};

/// How long a section switch takes. Short enough to feel instant, long enough
/// that cutting a loud section in or out never clicks.
pub(crate) const BYPASS_MS: f32 = 10.0;

/// How long the chain's output takes to go down, and again to come back, when
/// effects already in the chain are reordered.
pub(crate) const DUCK_MS: f32 = 5.0;

/// How long a removed effect keeps running after its fade has landed. Each
/// module smooths its own mix over about 20 ms and snaps to an exact zero only
/// when the smoothing has settled, and an effect dropped before that would cut
/// its own tail off with a step.
pub(crate) const GRACE_MS: f32 = 500.0;

/// The most rows an instance has that the chain reads, counting `on` and
/// `mix`.
const MAX_ROWS: usize = 10;

/// Where a chain's rows sit in its table, resolved once at construction so
/// the audio thread never compares a string.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChainSlots {
    /// Per instance, its rows in template order: `on`, `mix`, then the rest.
    pub inst: [[usize; MAX_ROWS]; INSTANCES],
    pub order: [usize; ORDER_LEN],
    pub filter_on: usize,
    pub filter_mix: usize,
    pub filter_cutoff: usize,
    pub filter_resonance: usize,
    pub filter_type: usize,
    pub gain: usize,
}

impl ChainSlots {
    /// `at` maps an id without its prefix, such as `fx.3.chorus.mix`, to a
    /// slot. It panics on a miss, since the tables are compile-time constants.
    pub fn resolve(at: impl Fn(&str) -> usize) -> Self {
        let mut inst = [[0; MAX_ROWS]; INSTANCES];
        for (n, (kind, _)) in POOL.iter().enumerate() {
            for (j, row) in kind.rows().iter().enumerate() {
                assert!(j < MAX_ROWS, "{} has too many rows", kind.name());
                inst[n][j] = at(&fx::row_id(n, row.id));
            }
        }
        let mut order = [0; ORDER_LEN];
        for (p, slot) in order.iter_mut().enumerate() {
            *slot = at(&fx::order_id(p));
        }
        Self {
            inst,
            order,
            filter_on: at("filter.on"),
            filter_mix: at("filter.mix"),
            filter_cutoff: at("filter.cutoff"),
            filter_resonance: at("filter.resonance"),
            filter_type: at("filter.type"),
            gain: at("amp.gain"),
        }
    }
}

/// One instance's values for a block, in the shape its module takes.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FxParams {
    Drive(DriveParams),
    Crush(CrushParams),
    Ring(RingModParams),
    Chorus(ChorusParams),
    Overtone(OvertoneParams),
    Flanger(FlangerParams),
    Wear(WearParams),
    Delay(DelayParams),
    Echo(EchoParams),
    Rise(RiseParams),
    Reverb(ReverbParams),
}

/// The rows of each kind that `read_fx` takes, after `on` and `mix`, in the
/// order it takes them. A test holds this against the templates, so reordering
/// a template without touching the reader fails.
#[cfg(test)]
pub(crate) fn read_order(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Drive => &["amount", "tone", "type"],
        Kind::Crush => &["bits", "rate"],
        Kind::Ring => &["freq"],
        Kind::Chorus => &["type", "rate", "depth", "voices", "spread", "lowcut", "eq"],
        Kind::Overtone => &["sub", "octave", "fifth", "tone"],
        Kind::Flanger => &["manual", "rate", "depth", "feedback"],
        Kind::Wear => &["wow", "flutter", "unstable", "dropouts", "dull"],
        Kind::Delay => &["time", "feedback", "tone", "pingpong"],
        Kind::Echo => &["time", "feedback", "tone", "wobble", "grit"],
        Kind::Rise => &["time", "feedback", "shift", "wobble", "tone"],
        Kind::Reverb => &["type", "decay", "size", "tone", "predelay"],
    }
}

fn read_fx(
    kind: Kind,
    s: &[usize; MAX_ROWS],
    value: &impl Fn(usize) -> f32,
    raw: &impl Fn(usize) -> f32,
) -> FxParams {
    // Row 0 is `on`, row 1 is `mix`; the rest follow `read_order`.
    let v = |j: usize| value(s[j]);
    let r = |j: usize| raw(s[j]);
    let mix = v(1);
    match kind {
        Kind::Drive => FxParams::Drive(DriveParams {
            amount_db: v(2),
            tone_hz: v(3),
            kind: DriveType::from_value(r(4)),
            mix,
        }),
        Kind::Crush => FxParams::Crush(CrushParams {
            bits: v(2),
            rate: v(3),
            mix,
        }),
        Kind::Ring => FxParams::Ring(RingModParams { freq: v(2), mix }),
        Kind::Chorus => FxParams::Chorus(ChorusParams {
            kind: ChorusType::from_value(r(2)),
            rate: v(3),
            depth: v(4),
            voices: v(5),
            spread: v(6),
            low_cut_hz: v(7),
            eq_db: v(8),
            mix,
        }),
        Kind::Overtone => FxParams::Overtone(OvertoneParams {
            sub: v(2),
            octave: v(3),
            fifth: v(4),
            tone_hz: v(5),
            mix,
        }),
        Kind::Flanger => FxParams::Flanger(FlangerParams {
            manual_ms: v(2),
            rate_hz: v(3),
            depth: v(4),
            feedback: v(5),
            mix,
        }),
        Kind::Wear => FxParams::Wear(WearParams {
            wow: v(2),
            flutter: v(3),
            unstable: v(4),
            dropouts: v(5),
            dull: v(6),
            mix,
        }),
        Kind::Delay => FxParams::Delay(DelayParams {
            time_ms: v(2),
            feedback: v(3),
            tone_hz: v(4),
            cross: v(5),
            mix,
        }),
        Kind::Echo => FxParams::Echo(EchoParams {
            time_ms: v(2),
            feedback: v(3),
            tone_hz: v(4),
            wobble: v(5),
            grit: v(6),
            mix,
        }),
        Kind::Rise => FxParams::Rise(RiseParams {
            time_ms: v(2),
            feedback: v(3),
            shift_st: v(4),
            wobble: v(5),
            tone_hz: v(6),
            mix,
        }),
        Kind::Reverb => FxParams::Reverb(ReverbParams {
            kind: ReverbType::from_value(r(2)),
            decay_s: v(3),
            size: v(4),
            tone_hz: v(5),
            predelay_ms: v(6),
            mix,
        }),
    }
}

/// One instance's block: its switch and its values.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InstParams {
    pub on: f32,
    pub p: FxParams,
}

/// One block's worth of targets for a chain. The switches are gates, zero or
/// one; the ramps turn them into fades.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChainParams {
    pub order: Order,
    pub inst: [InstParams; INSTANCES],
    pub filter: FilterParams,
    pub filter_on: f32,
    pub gain: f32,
}

impl ChainParams {
    /// Read once a block. `value` is a continuous row as heard, which for the
    /// patch means through its LFOs; `raw` is a stepped row or a switch,
    /// which no LFO moves. The order is read raw: no LFO may reorder a chain.
    pub fn read(s: &ChainSlots, value: impl Fn(usize) -> f32, raw: impl Fn(usize) -> f32) -> Self {
        let gate = |slot: usize| if raw(slot) >= 0.5 { 1.0 } else { 0.0 };
        let mut order_values = [0.0; ORDER_LEN];
        for (v, slot) in order_values.iter_mut().zip(s.order) {
            *v = raw(slot);
        }
        let order = Order::sanitise(order_values);
        let inst = std::array::from_fn(|n| InstParams {
            on: gate(s.inst[n][0]),
            p: read_fx(POOL[n].0, &s.inst[n], &value, &raw),
        });
        Self {
            order,
            inst,
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

    /// The same values with every effect taken out of the chain. What sound
    /// scaping hears of the arrangement: the ramps fade the change in 10 ms.
    pub fn without_effects(mut self) -> Self {
        self.order = Order::EMPTY;
        self.filter_on = 0.0;
        self.gain = 1.0;
        self
    }
}

enum Proc {
    Drive(Drive),
    Crush(Crush),
    Ring(RingMod),
    Chorus(Chorus),
    Overtone(Overtone),
    Flanger(Flanger),
    Wear(Wear),
    Delay(Delay),
    Echo(Echo),
    Rise(Rise),
    Reverb(Reverb),
}

impl Proc {
    fn new(kind: Kind, sr: f32) -> Self {
        match kind {
            Kind::Drive => Proc::Drive(Drive::new(sr)),
            Kind::Crush => Proc::Crush(Crush::new(sr)),
            Kind::Ring => Proc::Ring(RingMod::new(sr)),
            Kind::Chorus => Proc::Chorus(Chorus::new(sr)),
            Kind::Overtone => Proc::Overtone(Overtone::new(sr)),
            Kind::Flanger => Proc::Flanger(Flanger::new(sr)),
            Kind::Wear => Proc::Wear(Wear::new(sr)),
            Kind::Delay => Proc::Delay(Delay::new(sr)),
            Kind::Echo => Proc::Echo(Echo::new(sr)),
            Kind::Rise => Proc::Rise(Rise::new(sr)),
            Kind::Reverb => Proc::Reverb(Reverb::new(sr)),
        }
    }

    fn reset(&mut self) {
        match self {
            Proc::Drive(x) => x.reset(),
            Proc::Crush(x) => x.reset(),
            Proc::Ring(x) => x.reset(),
            Proc::Chorus(x) => x.reset(),
            Proc::Overtone(x) => x.reset(),
            Proc::Flanger(x) => x.reset(),
            Proc::Wear(x) => x.reset(),
            Proc::Delay(x) => x.reset(),
            Proc::Echo(x) => x.reset(),
            Proc::Rise(x) => x.reset(),
            Proc::Reverb(x) => x.reset(),
        }
    }
}

struct Instance {
    proc: Proc,
    /// The bypass switch's fade.
    on: Ramp,
    /// 1 while the order names it, 0 while it is leaving or absent.
    presence: Ramp,
    /// The crusher's mix is smoothed here, before the envelope scales it.
    crush_mix: OnePole,
    /// Samples spent fully faded out and not named by the order.
    idle: u32,
}

/// The instances being run, in order. Longer than the order, because an
/// instance that has been taken out stays until its fade lands.
#[derive(Debug, Clone, Copy)]
struct Run {
    items: [u8; INSTANCES],
    len: usize,
}

impl Run {
    const EMPTY: Run = Run {
        items: [0; INSTANCES],
        len: 0,
    };

    fn as_slice(&self) -> &[u8] {
        &self.items[..self.len]
    }

    fn contains(&self, n: u8) -> bool {
        self.as_slice().contains(&n)
    }

    fn from_order(order: &Order) -> Run {
        let mut run = Run::EMPTY;
        for &n in order.as_slice() {
            run.items[run.len] = n;
            run.len += 1;
        }
        run
    }
}

/// Whether the instances `run` and `target` have in common come in the same
/// order. If they do, the target can be reached without moving anything that
/// is already running.
fn same_relative_order(run: &[u8], target: &[u8]) -> bool {
    let a = run.iter().filter(|n| target.contains(n));
    let b = target.iter().filter(|n| run.contains(n));
    a.eq(b)
}

/// The run list for `target` that keeps every leaving instance (one in `run`
/// the target no longer names) just after the nearest earlier instance that
/// stays, or first if there is none. Only valid when `same_relative_order`.
fn merge(run: &[u8], target: &[u8]) -> Run {
    let mut out = Run::EMPTY;
    let push = |out: &mut Run, n: u8| {
        out.items[out.len] = n;
        out.len += 1;
    };
    // Leaving instances ahead of every staying one.
    for &n in run {
        if target.contains(&n) {
            break;
        }
        push(&mut out, n);
    }
    for &t in target {
        push(&mut out, t);
        if let Some(at) = run.iter().position(|&n| n == t) {
            for &n in &run[at + 1..] {
                if target.contains(&n) {
                    break;
                }
                push(&mut out, n);
            }
        }
    }
    out
}

pub(crate) struct Chain {
    inst: Vec<Instance>,
    run: Run,
    /// Who the order names this block, as 0 or 1: the presence targets.
    named: [f32; INSTANCES],
    /// A reordering waiting for the duck to land.
    pending: Option<Order>,
    grace: u32,
    duck: Ramp,
    filter: Filter,
    filter_fade: Ramp,
    gain: OnePole,
}

impl Chain {
    /// Settled at each row's default in `defs`, so nothing fades or glides on
    /// launch.
    pub fn new(sample_rate: f32, defs: &[ParamDef], s: &ChainSlots) -> Self {
        let ramp = |value: f32, ms: f32| {
            let mut r = Ramp::new(value);
            r.set_time(ms, sample_rate);
            r
        };
        let smoother = |slot: usize| {
            let mut p = OnePole::new();
            p.set_time(defs[slot].smooth_ms, sample_rate);
            p.reset(defs[slot].default);
            p
        };
        let inst = POOL
            .iter()
            .enumerate()
            .map(|(n, (kind, _))| Instance {
                proc: Proc::new(*kind, sample_rate),
                on: ramp(defs[s.inst[n][0]].default, BYPASS_MS),
                presence: ramp(0.0, BYPASS_MS),
                crush_mix: smoother(s.inst[n][1]),
                idle: 0,
            })
            .collect();
        Self {
            inst,
            run: Run::EMPTY,
            named: [0.0; INSTANCES],
            pending: None,
            grace: (GRACE_MS * 0.001 * sample_rate) as u32,
            duck: ramp(1.0, DUCK_MS),
            filter: Filter::new(sample_rate),
            filter_fade: ramp(defs[s.filter_on].default, BYPASS_MS),
            gain: smoother(s.gain),
        }
    }

    /// Once a block, before the first frame: take in who the order names.
    pub fn prepare(&mut self, p: &ChainParams) {
        let target = p.order.as_slice();
        for (n, named) in self.named.iter_mut().enumerate() {
            *named = if p.order.contains(n) { 1.0 } else { 0.0 };
        }

        // Instances that finished leaving, and have had time to settle, are
        // dropped.
        let mut kept = Run::EMPTY;
        let mut dropped = false;
        for &n in self.run.as_slice() {
            let gone = self.named[n as usize] == 0.0 && self.inst[n as usize].idle >= self.grace;
            if gone {
                dropped = true;
            } else {
                kept.items[kept.len] = n;
                kept.len += 1;
            }
        }
        if dropped {
            self.run = kept;
        }

        if self.pending.is_some() {
            // A reordering is already ducking; it wants the latest order.
            self.pending = Some(p.order);
            return;
        }
        if !same_relative_order(self.run.as_slice(), target) {
            self.pending = Some(p.order);
            return;
        }
        let merged = merge(self.run.as_slice(), target);
        for &n in merged.as_slice() {
            if !self.run.contains(n) {
                // New to the chain: forget whatever it held last time. It
                // fades in from silence, so the reset is not heard.
                self.inst[n as usize].proc.reset();
                self.inst[n as usize].idle = 0;
            }
        }
        self.run = merged;
    }

    /// The chain has gone quiet: swap to the new order. Effects that were
    /// leaving keep going, after the new order, so their fades finish; being
    /// silent, the swap itself is not heard.
    fn swap(&mut self, to: Order) {
        let old = self.run;
        let mut next = Run::from_order(&to);
        for &n in old.as_slice() {
            if !to.contains(n as usize) && self.inst[n as usize].idle < self.grace {
                next.items[next.len] = n;
                next.len += 1;
            }
        }
        for &n in to.as_slice() {
            if !old.contains(n) {
                // New to the chain: forget what it held, and fade it in.
                self.inst[n as usize].proc.reset();
                self.inst[n as usize].presence.set(0.0);
                self.inst[n as usize].idle = 0;
            }
        }
        self.run = next;
    }

    /// The palette, for one frame.
    #[inline]
    pub fn front(&mut self, l: f32, r: f32, p: &ChainParams) -> (f32, f32) {
        let duck = self
            .duck
            .process(if self.pending.is_some() { 0.0 } else { 1.0 });
        if duck == 0.0 {
            if let Some(to) = self.pending.take() {
                self.swap(to);
            }
        }
        let (mut l, mut r) = (l, r);
        for k in 0..self.run.len {
            let n = self.run.items[k] as usize;
            let inst = &mut self.inst[n];
            let presence = inst.presence.process(self.named[n]);
            if presence == 0.0 && self.named[n] == 0.0 {
                inst.idle = inst.idle.saturating_add(1);
            } else {
                inst.idle = 0;
            }
            let on = inst.on.process(p.inst[n].on);
            let level = on * presence;
            (l, r) = match (&mut inst.proc, &p.inst[n].p) {
                (Proc::Drive(x), FxParams::Drive(q)) => x.process(
                    l,
                    r,
                    &DriveParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Crush(x), FxParams::Crush(q)) => {
                    let mix = inst.crush_mix.process(q.mix) * level;
                    x.process(l, r, &CrushParams { mix, ..*q })
                }
                (Proc::Ring(x), FxParams::Ring(q)) => x.process(
                    l,
                    r,
                    &RingModParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Chorus(x), FxParams::Chorus(q)) => x.process(
                    l,
                    r,
                    &ChorusParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Overtone(x), FxParams::Overtone(q)) => x.process(
                    l,
                    r,
                    &OvertoneParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Flanger(x), FxParams::Flanger(q)) => x.process(
                    l,
                    r,
                    &FlangerParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Wear(x), FxParams::Wear(q)) => x.process(
                    l,
                    r,
                    &WearParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Delay(x), FxParams::Delay(q)) => x.process(
                    l,
                    r,
                    &DelayParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Echo(x), FxParams::Echo(q)) => x.process(
                    l,
                    r,
                    &EchoParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Rise(x), FxParams::Rise(q)) => x.process(
                    l,
                    r,
                    &RiseParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                (Proc::Reverb(x), FxParams::Reverb(q)) => x.process(
                    l,
                    r,
                    &ReverbParams {
                        mix: q.mix * level,
                        ..*q
                    },
                ),
                // The pool and the params are built from the same list, so a
                // mismatch cannot happen; pass the signal on rather than
                // panic on the audio thread.
                _ => (l, r),
            };
        }
        (l * duck, r * duck)
    }

    /// The filter, then the output gain, for one frame. Switched off, the
    /// filter's mix lands on an exact zero; unity gain is exact, since the
    /// smoother starts and rests on it.
    #[inline]
    pub fn back(&mut self, l: f32, r: f32, p: &ChainParams) -> (f32, f32) {
        let filter_on = self.filter_fade.process(p.filter_on);
        let filter = FilterParams {
            mix: p.filter.mix * filter_on,
            ..p.filter
        };
        let (l, r) = self.filter.process(l, r, &filter);
        let g = self.gain.process(p.gain);
        (l * g, r * g)
    }

    /// How many effects are being run, including any still fading out.
    #[cfg(test)]
    pub fn running(&self) -> usize {
        self.run.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reader_takes_the_rows_in_the_order_the_templates_give_them() {
        for kind in Kind::ALL {
            let rows = kind.rows();
            assert!(rows[0].id.ends_with(".on"), "{}", kind.name());
            assert!(rows[1].id.ends_with(".mix"), "{}", kind.name());
            for (j, suffix) in read_order(kind).iter().enumerate() {
                let want = format!("{}.{suffix}", kind.name());
                assert_eq!(rows[2 + j].id, want, "{} row {}", kind.name(), 2 + j);
            }
        }
    }

    #[test]
    fn no_kind_has_more_rows_than_a_slot_array() {
        for kind in Kind::ALL {
            assert!(kind.rows().len() <= MAX_ROWS, "{}", kind.name());
        }
    }

    #[test]
    fn relative_order_ignores_who_came_and_went() {
        assert!(same_relative_order(&[1, 2, 3], &[1, 3]));
        assert!(same_relative_order(&[1, 3], &[1, 2, 3]));
        assert!(same_relative_order(&[], &[4, 5]));
        assert!(!same_relative_order(&[1, 2, 3], &[3, 2]));
        assert!(!same_relative_order(&[1, 2], &[2, 1]));
    }

    #[test]
    fn merge_keeps_leavers_where_they_were() {
        // 2 is leaving between 1 and 3; 9 is new.
        let run = merge(&[1, 2, 3], &[1, 9, 3]);
        assert_eq!(run.as_slice(), &[1, 2, 9, 3]);
        // A leaver at the front stays at the front.
        let run = merge(&[7, 1, 3], &[1, 3]);
        assert_eq!(run.as_slice(), &[7, 1, 3]);
        // Everyone leaving keeps their order.
        let run = merge(&[4, 5, 6], &[]);
        assert_eq!(run.as_slice(), &[4, 5, 6]);
        // From nothing, it is the target.
        let run = merge(&[], &[2, 8]);
        assert_eq!(run.as_slice(), &[2, 8]);
    }

    // The chain, driven by hand: a bank, the slots resolved against its table,
    // and blocks of 64 frames like the engine's.
    use crate::params::{index_of, ParamBank, PARAMS};

    struct Rig {
        chain: Chain,
        slots: ChainSlots,
        bank: ParamBank,
    }

    impl Rig {
        fn new() -> Self {
            let at = |id: &str| index_of(id).unwrap_or_else(|| panic!("no row {id}"));
            let slots = ChainSlots::resolve(at);
            Rig {
                chain: Chain::new(48_000.0, &PARAMS, &slots),
                slots,
                bank: ParamBank::new(),
            }
        }

        fn set(&self, id: &str, v: f32) {
            assert!(self.bank.set_by_id(id, v), "{id}");
        }

        fn order(&self, kinds: &[Kind]) {
            let mut o = Order::EMPTY;
            for k in kinds {
                o.add(*k).unwrap();
            }
            fx::set_order(&self.bank, &o);
        }

        /// One block of `n` frames from `input`, returning the output.
        fn run(&mut self, n: usize, input: impl Fn(usize) -> f32) -> Vec<f32> {
            let p = ChainParams::read(&self.slots, |s| self.bank.get(s), |s| self.bank.get(s));
            self.chain.prepare(&p);
            (0..n)
                .map(|i| {
                    let x = input(i);
                    self.chain.front(x, -x, &p).0
                })
                .collect()
        }

        fn run_for(&mut self, blocks: usize, input: impl Fn(usize) -> f32) -> Vec<f32> {
            let mut all = Vec::new();
            for b in 0..blocks {
                all.extend(self.run(64, |i| input(b * 64 + i)));
            }
            all
        }
    }

    fn noise_at(i: usize) -> f32 {
        let mut x = (i as u32).wrapping_mul(2_654_435_761).wrapping_add(12_345);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    #[test]
    fn an_empty_chain_is_the_input_exactly() {
        let mut rig = Rig::new();
        // Every effect set up to be loud, none of them named in the order.
        for (n, (kind, _)) in POOL.iter().enumerate() {
            rig.set(&format!("fx.{n}.{}.mix", kind.name()), 1.0);
        }
        let out = rig.run_for(100, noise_at);
        for (i, v) in out.iter().enumerate() {
            assert_eq!(*v, noise_at(i), "frame {i}");
        }
    }

    #[test]
    fn an_effect_in_the_chain_is_heard_and_out_of_it_is_not() {
        let mut rig = Rig::new();
        rig.set("fx.2.ring.mix", 1.0);
        rig.set("fx.2.ring.freq", 440.0);
        rig.order(&[Kind::Ring]);
        let with = rig.run_for(200, noise_at);
        assert!(with
            .iter()
            .skip(1000)
            .zip(1000..)
            .any(|(v, i)| *v != noise_at(i)));
        rig.order(&[]);
        rig.run_for(400, noise_at); // the fade lands, it settles, it is dropped
        assert_eq!(rig.chain.running(), 0);
        let without = rig.run_for(20, |i| noise_at(i + 99_999));
        for (i, v) in without.iter().enumerate() {
            assert_eq!(*v, noise_at(i + 99_999));
        }
    }

    #[test]
    fn a_removed_effect_keeps_running_until_it_has_faded() {
        let mut rig = Rig::new();
        rig.set("fx.7.delay.mix", 1.0);
        rig.order(&[Kind::Delay]);
        rig.run_for(20, noise_at);
        assert_eq!(rig.chain.running(), 1);
        rig.order(&[]);
        // Still there after its 10 ms fade (480 frames) and well into the grace
        // period, gone once that has passed (500 ms is 24 000 frames).
        rig.run_for(10, noise_at);
        assert_eq!(rig.chain.running(), 1);
        rig.run_for(400, noise_at);
        assert_eq!(rig.chain.running(), 0);
    }

    #[test]
    fn putting_an_effect_back_does_not_replay_what_it_held() {
        let mut rig = Rig::new();
        rig.set("fx.7.delay.mix", 1.0);
        rig.set("fx.7.delay.feedback", 0.9);
        rig.set("fx.7.delay.time", 400.0);
        rig.order(&[Kind::Delay]);
        // A long, loud tail on the delay line...
        rig.run_for(300, noise_at);
        // ...taken out, and left out until it has gone...
        rig.order(&[]);
        rig.run_for(400, |_| 0.0);
        assert_eq!(rig.chain.running(), 0);
        // ...and put back into silence: nothing may come out of it.
        rig.order(&[Kind::Delay]);
        let out = rig.run_for(400, |_| 0.0);
        assert!(
            out.iter().all(|v| *v == 0.0),
            "an old repeat came back after {} frames",
            out.iter().position(|v| *v != 0.0).unwrap_or(0)
        );
    }

    #[test]
    fn effects_keep_their_state_through_a_reorder() {
        let mut rig = Rig::new();
        rig.set("fx.7.delay.mix", 1.0);
        rig.set("fx.7.delay.feedback", 0.0);
        rig.set("fx.7.delay.time", 400.0);
        rig.set("fx.3.chorus.mix", 0.0);
        rig.order(&[Kind::Chorus, Kind::Delay]);
        // One burst goes into the delay line...
        rig.run_for(10, noise_at);
        // ...the two are swapped, and the burst still comes out 400 ms later.
        rig.order(&[Kind::Delay, Kind::Chorus]);
        let out = rig.run_for(600, |_| 0.0);
        let loud = out.iter().map(|v| v.abs()).fold(0.0, f32::max);
        assert!(loud > 0.1, "the delay line was lost in the reorder: {loud}");
    }

    #[test]
    fn a_duplicated_or_impossible_order_runs_each_effect_once() {
        let rig = Rig::new();
        // A hand-edited file: the same effect twice, an impossible number,
        // and a gap.
        rig.set("fx.order.0", 5.0);
        rig.set("fx.order.1", 0.0);
        rig.set("fx.order.2", 5.0);
        let p = ChainParams::read(&rig.slots, |s| rig.bank.get(s), |s| rig.bank.get(s));
        assert_eq!(p.order.as_slice(), &[4]);
    }

    #[test]
    fn mix_is_a_crossfade_so_full_mix_never_passes_the_original() {
        // Mix at 100% is the effect alone. For the effects that start from
        // silence (repeats, a tail, layers a little late), the frame the
        // impulse arrives in must come out empty.
        for kind in [
            Kind::Delay,
            Kind::Echo,
            Kind::Rise,
            Kind::Reverb,
            Kind::Overtone,
        ] {
            let mut rig = Rig::new();
            let n = POOL.iter().position(|&(k, c)| k == kind && c == 0).unwrap();
            rig.set(&format!("fx.{n}.{}.mix", kind.name()), 1.0);
            rig.order(&[kind]);
            rig.run_for(400, |_| 0.0); // the fades in land and the smoothers settle
            let out = rig.run(64, |i| if i == 0 { 1.0 } else { 0.0 });
            assert!(
                out[0].abs() < 1e-4,
                "{} at full mix still plays the original: {}",
                kind.name(),
                out[0]
            );
        }
    }
}
