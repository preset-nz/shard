//! Modulation: LFOs, and links from parameters to them.
//!
//! A `ModSet` is built on the command thread and swapped into the engine
//! whole, the way a new sample arrives. It may allocate while it is being built
//! and never afterwards; the engine only advances it and reads through it. So
//! there is no ceiling on how many LFOs a patch has. Adding one builds a new
//! set, and `inherit` keeps the ones already running where they were.
//!
//! Two rules from `guidance/projects/shard/design/modulation.md`:
//!
//! - **Modulation never writes a stored value.** The bank holds the hand's
//!   value. `apply` bends it where the engine reads, in normalised space and
//!   through the parameter's own taper, so saving saves what the hand set.
//! - **One source per parameter, with a signed depth.** A link is shaped so
//!   it could become one row of a matrix later.

use crate::params::{index_of, Taper, PARAMS};
use crate::rng::Rng;

/// The slowest an LFO runs. Below this a cycle takes nearly two minutes, and
/// the control stops meaning anything you can hear move.
pub const MIN_RATE_HZ: f32 = 0.01;
/// The fastest, for now (Georg, 2026-09-13). LFOs advance once per block, and
/// at 256-frame blocks that is still around ten updates per cycle here.
/// Audio-rate modulation is a different path, added only if it is missed.
pub const MAX_RATE_HZ: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Sine,
    Triangle,
    Saw,
    Square,
    /// A new random point once per cycle, eased into with a raised cosine, so
    /// it wanders without corners. The movement drift used to give.
    SmoothRandom,
}

impl Shape {
    pub const ALL: [Shape; 5] = [
        Shape::Sine,
        Shape::Triangle,
        Shape::Saw,
        Shape::Square,
        Shape::SmoothRandom,
    ];
}

/// An LFO as the document describes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LfoSpec {
    /// Stable for the life of the LFO and never reused. A link names it, and a
    /// rebuilt set matches running LFOs by it, so it has to outlive renames
    /// and reordering.
    pub id: u64,
    /// Clamped to `MIN_RATE_HZ..=MAX_RATE_HZ`.
    pub rate_hz: f32,
    pub shape: Shape,
    /// Phase offset, 0 to 1, so two LFOs at one rate can move apart. Smooth
    /// random has no phase to offset and ignores it.
    pub phase: f32,
}

#[derive(Debug, Clone, Copy)]
struct Lfo {
    spec: LfoSpec,
    /// How far through the current cycle, 0 to 1, before the offset.
    cycle: f32,
    /// This block's output, -1 to 1.
    value: f32,
    rng: Rng,
    from: f32,
    to: f32,
}

impl Lfo {
    fn new(spec: LfoSpec) -> Self {
        let spec = LfoSpec {
            rate_hz: spec.rate_hz.clamp(MIN_RATE_HZ, MAX_RATE_HZ),
            phase: spec.phase.rem_euclid(1.0),
            ..spec
        };
        // Seeded from the id, so an LFO wanders the same way every time the
        // patch that holds it loads.
        let mut rng = Rng::new((spec.id as u32) ^ ((spec.id >> 32) as u32) ^ 0x9E37_79B9);
        let (from, to) = (rng.next_bipolar(), rng.next_bipolar());
        let mut lfo = Self {
            spec,
            cycle: 0.0,
            value: 0.0,
            rng,
            from,
            to,
        };
        lfo.value = lfo.output();
        lfo
    }

    fn output(&self) -> f32 {
        let p = (self.cycle + self.spec.phase).fract();
        match self.spec.shape {
            Shape::Sine => (core::f32::consts::TAU * p).sin(),
            Shape::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
            Shape::Saw => 2.0 * p - 1.0,
            Shape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Shape::SmoothRandom => {
                let ease = 0.5 - 0.5 * (core::f32::consts::PI * self.cycle).cos();
                self.from + (self.to - self.from) * ease
            }
        }
    }

    #[inline]
    fn advance(&mut self, frames: f32, sample_rate: f32) {
        self.value = self.output();
        self.cycle += self.spec.rate_hz * frames / sample_rate;
        // At 20 Hz and any sane block this runs at most once; the loop only
        // guards a very long block.
        while self.cycle >= 1.0 {
            self.cycle -= 1.0;
            self.from = self.to;
            self.to = self.rng.next_bipolar();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Link {
    /// Index into this set's LFOs, resolved from an id when the link was made.
    lfo: usize,
    depth: f32,
}

/// Why a link was refused. The document layer reports these rather than
/// dropping the link, the way a patch load reports an unknown id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    UnknownParameter(String),
    /// Stepped parameters are discrete choices. Walking through window
    /// shapes is a different feature, and not this one.
    Stepped(&'static str),
    UnknownLfo(u64),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::UnknownParameter(id) => write!(f, "there is no parameter called {id}"),
            LinkError::Stepped(id) => {
                write!(f, "{id} is a set of choices and cannot follow an LFO")
            }
            LinkError::UnknownLfo(id) => write!(f, "there is no LFO {id} to link to"),
        }
    }
}

/// Every LFO in a patch, and every link from a parameter to one of them.
#[derive(Debug, Clone, Default)]
pub struct ModSet {
    lfos: Vec<Lfo>,
    /// Indexed as `PARAMS`. Empty when nothing has been linked.
    links: Vec<Option<Link>>,
}

impl ModSet {
    /// No LFOs and no links. Allocates nothing, so the engine can start with
    /// one.
    pub const fn empty() -> Self {
        Self {
            lfos: Vec::new(),
            links: Vec::new(),
        }
    }

    /// A set with these LFOs and nothing linked yet.
    pub fn new(lfos: &[LfoSpec]) -> Self {
        Self {
            lfos: lfos.iter().map(|s| Lfo::new(*s)).collect(),
            links: Vec::new(),
        }
    }

    /// Link `param` to the LFO with `lfo_id`, replacing any link it had. Depth
    /// is clamped to ±1, which is the parameter's whole range either way.
    pub fn link(&mut self, param: &str, lfo_id: u64, depth: f32) -> Result<(), LinkError> {
        let slot = index_of(param).ok_or_else(|| LinkError::UnknownParameter(param.to_string()))?;
        if matches!(PARAMS[slot].taper, Taper::Stepped(_)) {
            return Err(LinkError::Stepped(PARAMS[slot].id));
        }
        let lfo = self
            .lfos
            .iter()
            .position(|l| l.spec.id == lfo_id)
            .ok_or(LinkError::UnknownLfo(lfo_id))?;
        if self.links.len() < PARAMS.len() {
            self.links.resize(PARAMS.len(), None);
        }
        self.links[slot] = Some(Link {
            lfo,
            depth: depth.clamp(-1.0, 1.0),
        });
        Ok(())
    }

    /// Carry running state across from the set this one replaces, matched by
    /// id. Changing a rate or adding an LFO rebuilds the set, and that must not
    /// restart the LFOs already moving. Copies only; allocates nothing.
    pub fn inherit(&mut self, old: &ModSet) {
        for lfo in &mut self.lfos {
            if let Some(prev) = old.lfos.iter().find(|p| p.spec.id == lfo.spec.id) {
                lfo.cycle = prev.cycle;
                lfo.value = prev.value;
                lfo.rng = prev.rng;
                lfo.from = prev.from;
                lfo.to = prev.to;
            }
        }
    }

    /// Once per block, before anything reads a parameter.
    #[inline]
    pub fn advance(&mut self, frames: usize, sample_rate: f32) {
        for lfo in &mut self.lfos {
            lfo.advance(frames as f32, sample_rate);
        }
    }

    /// The value the engine should use for parameter `slot`, given the hand's
    /// `base`. Exactly `base` when nothing is linked or the depth is zero, not a
    /// round trip through the taper, which would not be bit-exact.
    #[inline]
    pub fn apply(&self, slot: usize, base: f32) -> f32 {
        let Some(Some(link)) = self.links.get(slot) else {
            return base;
        };
        if link.depth == 0.0 {
            return base;
        }
        let Some(lfo) = self.lfos.get(link.lfo) else {
            return base;
        };
        let def = &PARAMS[slot];
        def.denormalise((def.normalise(base) + link.depth * lfo.value).clamp(0.0, 1.0))
    }

    /// The current output of the LFO with `id`, -1 to 1. For drawing and tests.
    pub fn lfo_value(&self, id: u64) -> Option<f32> {
        self.lfos.iter().find(|l| l.spec.id == id).map(|l| l.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 256;

    fn spec(id: u64, rate_hz: f32, shape: Shape) -> LfoSpec {
        LfoSpec {
            id,
            rate_hz,
            shape,
            phase: 0.0,
        }
    }

    fn continuous() -> impl Iterator<Item = (usize, &'static crate::params::ParamDef)> {
        PARAMS
            .iter()
            .enumerate()
            .filter(|(_, p)| !matches!(p.taper, Taper::Stepped(_)))
    }

    #[test]
    fn every_shape_stays_within_one_and_uses_its_range() {
        for shape in Shape::ALL {
            let mut set = ModSet::new(&[spec(1, 3.7, shape)]);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for _ in 0..(SR as usize * 4 / BLOCK) {
                set.advance(BLOCK, SR);
                let v = set.lfo_value(1).unwrap();
                assert!((-1.0..=1.0).contains(&v), "{shape:?} gave {v}");
                lo = lo.min(v);
                hi = hi.max(v);
            }
            assert!(hi - lo > 1.0, "{shape:?} barely moved: {lo}..{hi}");
        }
    }

    #[test]
    fn the_rate_is_the_rate() {
        // Count saw wraps over ten seconds. At 2 Hz that is twenty cycles, give
        // or take the block the count happens to end in.
        let mut set = ModSet::new(&[spec(1, 2.0, Shape::Saw)]);
        let (mut prev, mut wraps) = (f32::MIN, 0);
        for _ in 0..(SR as usize * 10 / BLOCK) {
            set.advance(BLOCK, SR);
            let v = set.lfo_value(1).unwrap();
            if v < prev {
                wraps += 1;
            }
            prev = v;
        }
        assert!(
            (19..=21).contains(&wraps),
            "{wraps} cycles in ten seconds at 2 Hz"
        );
    }

    #[test]
    fn unlinked_and_zero_depth_are_exactly_the_base_value() {
        let mut set = ModSet::new(&[spec(7, 5.0, Shape::Square)]);
        let size = index_of("grain.size").unwrap();
        let density = index_of("grain.density").unwrap();
        set.link("grain.density", 7, 0.0).unwrap();
        for _ in 0..50 {
            set.advance(BLOCK, SR);
            assert_eq!(set.apply(size, 123.4), 123.4);
            assert_eq!(set.apply(density, 37.7), 37.7);
        }
        assert_eq!(ModSet::empty().apply(size, 123.4), 123.4);
    }

    #[test]
    fn a_modulated_value_never_leaves_its_range() {
        let specs: Vec<LfoSpec> = Shape::ALL
            .iter()
            .enumerate()
            .map(|(i, s)| spec(i as u64 + 1, 1.0 + i as f32 * 3.3, *s))
            .collect();
        let mut set = ModSet::new(&specs);
        for (i, (_, p)) in continuous().enumerate() {
            let depth = if i % 2 == 0 { 1.0 } else { -1.0 };
            set.link(p.id, (i % specs.len()) as u64 + 1, depth).unwrap();
        }
        for _ in 0..2_000 {
            set.advance(BLOCK, SR);
            for (slot, p) in continuous() {
                for base in [p.min, (p.min + p.max) * 0.5, p.max] {
                    let v = set.apply(slot, base);
                    let (lo, hi) = (p.min.min(p.max), p.max.max(p.min));
                    assert!(v.is_finite() && v >= lo && v <= hi, "{} went to {v}", p.id);
                }
            }
        }
    }

    #[test]
    fn links_are_refused_where_they_cannot_mean_anything() {
        let mut set = ModSet::new(&[spec(1, 1.0, Shape::Sine)]);
        assert_eq!(
            set.link("grain.nonsense", 1, 0.5),
            Err(LinkError::UnknownParameter("grain.nonsense".into()))
        );
        assert_eq!(
            set.link("grain.window", 1, 0.5),
            Err(LinkError::Stepped("grain.window"))
        );
        assert_eq!(
            set.link("grain.size", 99, 0.5),
            Err(LinkError::UnknownLfo(99))
        );
        // Out-of-range depth is clamped rather than refused.
        set.link("grain.size", 1, 7.0).unwrap();
        assert_eq!(
            set.links[index_of("grain.size").unwrap()].unwrap().depth,
            1.0
        );
    }

    #[test]
    fn rebuilding_the_set_keeps_running_lfos_where_they_were() {
        let mut old = ModSet::new(&[spec(1, 1.3, Shape::Sine)]);
        for _ in 0..77 {
            old.advance(BLOCK, SR);
        }
        // What adding an LFO produces: the same LFO 1 plus a new one, in a
        // different order.
        let mut next = ModSet::new(&[spec(2, 4.0, Shape::Saw), spec(1, 1.3, Shape::Sine)]);
        next.inherit(&old);
        let mut restarted = ModSet::new(&[spec(1, 1.3, Shape::Sine)]);
        let mut new_alone = ModSet::new(&[spec(2, 4.0, Shape::Saw)]);

        old.advance(BLOCK, SR);
        next.advance(BLOCK, SR);
        restarted.advance(BLOCK, SR);
        new_alone.advance(BLOCK, SR);

        assert_eq!(next.lfo_value(1), old.lfo_value(1), "LFO 1 must carry on");
        assert_ne!(next.lfo_value(1), restarted.lfo_value(1), "not restart");
        assert_eq!(
            next.lfo_value(2),
            new_alone.lfo_value(2),
            "a new LFO starts fresh"
        );
    }

    #[test]
    fn smooth_random_has_no_corners() {
        // At the fastest rate allowed, no block may move further than the
        // raised-cosine ease between two random points can: its slope is at
        // most π per cycle.
        let mut set = ModSet::new(&[spec(3, MAX_RATE_HZ, Shape::SmoothRandom)]);
        let per_block = MAX_RATE_HZ * BLOCK as f32 / SR;
        let bound = core::f32::consts::PI * per_block + 1e-4;
        let mut prev: Option<f32> = None;
        for _ in 0..5_000 {
            set.advance(BLOCK, SR);
            let v = set.lfo_value(3).unwrap();
            if let Some(p) = prev {
                assert!(
                    (v - p).abs() <= bound,
                    "stepped by {} in one block",
                    (v - p).abs()
                );
            }
            prev = Some(v);
        }
    }
}
