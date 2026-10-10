//! Modulation: LFOs and envelopes, and links from parameters to them.
//!
//! **Two kinds of modulator, one id space.** An LFO runs on its own clock. A
//! modulation envelope (Georg, 2026-09-27: the ADSR a Dark Energy switches onto
//! its filter, now any curve with a sustain mark) runs on the pass's clock, as
//! the amplitude envelope does: a step starts it, and it rests between steps.
//! A link names either by id, so ids are unique across both lists.
//!
//! **Resolution.** Both are read once per block and then smoothed per row, so
//! no attack is faster than a row's smoothing (about 20 ms on most) and a
//! step's onset can land up to a block late. Fine for a filter sweep or a
//! brass index, not a DX transient. The envelope's value is a pure function
//! of the pass's clock, so a row can read it per sample later with no new
//! state.
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
//! - **One source per parameter, sweeping a range.** A link names a low and
//!   a high end as shares of the parameter's range (Georg, 2026-09-14: "40 to
//!   50 Hz on the ring modulator"), and the LFO travels between them. The
//!   hand's value does not take part while a link exists; it is what you get
//!   back when you unlink. Low above high runs the sweep the other way.

use crate::params::{index_of, Taper, PARAMS};
use crate::rng::Rng;
use preset_math::{Basis, Curve as EnvCurve, CurvePoint};

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
    /// Rises from -1 to 1 along an easing curve, then falls back along the
    /// same curve: a triangle whose ramps have character. Continuous at both
    /// turns, because every easing starts at 0 and ends at 1.
    Eased(Curve, Ease),
}

impl Shape {
    pub const ALL: [Shape; 20] = [
        Shape::Sine,
        Shape::Triangle,
        Shape::Saw,
        Shape::Square,
        Shape::SmoothRandom,
        Shape::Eased(Curve::Quad, Ease::In),
        Shape::Eased(Curve::Quad, Ease::Out),
        Shape::Eased(Curve::Quad, Ease::InOut),
        Shape::Eased(Curve::Cubic, Ease::In),
        Shape::Eased(Curve::Cubic, Ease::Out),
        Shape::Eased(Curve::Cubic, Ease::InOut),
        Shape::Eased(Curve::Expo, Ease::In),
        Shape::Eased(Curve::Expo, Ease::Out),
        Shape::Eased(Curve::Expo, Ease::InOut),
        Shape::Eased(Curve::Elastic, Ease::In),
        Shape::Eased(Curve::Elastic, Ease::Out),
        Shape::Eased(Curve::Elastic, Ease::InOut),
        Shape::Eased(Curve::Bounce, Ease::In),
        Shape::Eased(Curve::Bounce, Ease::Out),
        Shape::Eased(Curve::Bounce, Ease::InOut),
    ];

    /// How each shape in `ALL` is spelled in a patch, in the same order.
    ///
    /// **A wire format**, like parameter ids: renaming one breaks every saved
    /// LFO that uses it. Flat kebab-case, so a hand-edited `.shard` file stays
    /// readable, and so this crate needs no serialiser to define it.
    pub const NAMES: [&'static str; 20] = [
        "sine",
        "triangle",
        "saw",
        "square",
        "smooth-random",
        "quad-in",
        "quad-out",
        "quad-in-out",
        "cubic-in",
        "cubic-out",
        "cubic-in-out",
        "expo-in",
        "expo-out",
        "expo-in-out",
        "elastic-in",
        "elastic-out",
        "elastic-in-out",
        "bounce-in",
        "bounce-out",
        "bounce-in-out",
    ];

    pub fn name(self) -> &'static str {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::NAMES[i]
    }

    pub fn from_name(name: &str) -> Option<Shape> {
        Self::NAMES
            .iter()
            .position(|n| *n == name)
            .map(|i| Self::ALL[i])
    }
}

/// An easing family, after Robert Penner's easing equations (BSD-licensed;
/// reimplemented here from their definitions, which are only arithmetic).
/// Georg, 2026-09-13: a handful, not the full set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    Quad,
    Cubic,
    Expo,
    /// Overshoots past both ends before settling. The LFO clamps it, so the
    /// overshoot reads as a flattened wobble at the peaks.
    Elastic,
    Bounce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ease {
    In,
    Out,
    InOut,
}

/// Ease `t`, from 0 to 1, along a curve. Starts at 0 and ends at 1 for every
/// curve and mode; elastic leaves that range in between.
///
/// Out and in-out are derived from in, rather than written out per family,
/// so all fifteen combinations share one definition of each curve.
pub fn ease(curve: Curve, mode: Ease, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match mode {
        Ease::In => ease_in(curve, t),
        Ease::Out => 1.0 - ease_in(curve, 1.0 - t),
        Ease::InOut => {
            if t < 0.5 {
                0.5 * ease_in(curve, 2.0 * t)
            } else {
                1.0 - 0.5 * ease_in(curve, 2.0 - 2.0 * t)
            }
        }
    }
}

fn ease_in(curve: Curve, t: f32) -> f32 {
    match curve {
        Curve::Quad => t * t,
        Curve::Cubic => t * t * t,
        // Penner's expo never quite reaches zero, so zero is pinned.
        Curve::Expo => {
            if t <= 0.0 {
                0.0
            } else {
                2.0f32.powf(10.0 * (t - 1.0))
            }
        }
        Curve::Elastic => {
            if t <= 0.0 || t >= 1.0 {
                t
            } else {
                let c4 = core::f32::consts::TAU / 3.0;
                -(2.0f32.powf(10.0 * t - 10.0)) * ((10.0 * t - 10.75) * c4).sin()
            }
        }
        Curve::Bounce => 1.0 - bounce_out(1.0 - t),
    }
}

fn bounce_out(t: f32) -> f32 {
    const N: f32 = 7.5625;
    const D: f32 = 2.75;
    if t < 1.0 / D {
        N * t * t
    } else if t < 2.0 / D {
        let t = t - 1.5 / D;
        N * t * t + 0.75
    } else if t < 2.5 / D {
        let t = t - 2.25 / D;
        N * t * t + 0.9375
    } else {
        let t = t - 2.625 / D;
        N * t * t + 0.984375
    }
}

/// The curve an ADSR draws: from rest up to the peak over the attack, down to
/// the sustain level over the decay, and back to rest over the release, with
/// the sustain mark on the third point. Linear throughout. Matches ux-kit's
/// `adsrCurve`, which the sliders and the editor both write. The stages are
/// shares of the note, so x runs 0 to 1 and shares that sum to 1 fill it.
pub fn adsr_curve(attack: f32, decay: f32, sustain: f32, release: f32) -> EnvCurve {
    let a = attack.max(0.0);
    let d = decay.max(0.0);
    let r = release.max(0.0);
    let point = |x: f32, y: f32| CurvePoint {
        x,
        y,
        basis: Basis::Linear,
        tension: 0.0,
    };
    EnvCurve {
        points: vec![
            point(0.0, 0.0),
            point(a, 1.0),
            point(a + d, sustain.clamp(0.0, 1.0)),
            point(a + d + r, 0.0),
        ],
        sustain: Some(2),
    }
}

/// A modulation envelope as the document describes it: a curve whose x runs
/// 0 to 1 across the note, or across the pass. An ADSR is the curve
/// `adsr_curve` draws.
///
/// The curve is built on the command thread and crosses in the modulation
/// swap, so evaluating it (which never allocates) is all the audio thread does.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvSpec {
    /// Stable and never reused, and never the id of an LFO in the same set.
    pub id: u64,
    pub curve: EnvCurve,
}

impl EnvSpec {
    /// The shape at `elapsed` samples into a pass `length` long, 0 at rest and
    /// 1 at the peak. Rest whenever no pass is sounding. The curve spans the
    /// pass: x is the share of it elapsed.
    pub fn level_at(&self, elapsed: f32, length: f32, sounding: bool) -> f32 {
        if !sounding || length < 2.0 || self.curve.points.is_empty() {
            return 0.0;
        }
        let t = elapsed.clamp(0.0, length);
        self.curve.evaluate(t / length).clamp(0.0, 1.0)
    }

    /// The shape on the note clock: `age` samples into a note held for `gate`
    /// samples, or for ever when `gate` is none. `pass_len` is the sample's
    /// window, the only length a Loop note has to span. 0 at rest, 1 at the
    /// peak.
    ///
    /// The whole curve spans the gate (Georg, 2026-10-10), release included,
    /// then holds its last value; with no gate it spans one pass of the
    /// sample.
    ///
    /// The first segment climbs from `from`, the level the last note had
    /// reached, so a retrigger mid-release does not click: the curve's own
    /// first segment is offset by the gap, and the offset fades out across
    /// that segment. For a linear attack that is `from + (1 - from) * t / a`.
    pub fn level_at_note(&self, age: f32, gate: Option<f32>, pass_len: f32, from: f32) -> f32 {
        let pts = &self.curve.points;
        if pts.is_empty() {
            return 0.0;
        }
        let from = from.clamp(0.0, 1.0);
        let age = age.max(0.0);
        let span = gate.unwrap_or(pass_len).max(1.0);
        let x = age / span;
        (self.curve.evaluate(x) + self.retrigger_offset(x, from)).clamp(0.0, 1.0)
    }

    /// How far above the curve a retriggered note sits at `x`: the gap
    /// between where the last note had got to and where the curve starts,
    /// fading to nothing across the first segment. Nothing after it, and
    /// nothing when the first segment has no width or the curve holds from
    /// its first point.
    fn retrigger_offset(&self, x: f32, from: f32) -> f32 {
        let pts = &self.curve.points;
        if pts.len() < 2 || self.curve.sustain == Some(0) {
            return 0.0;
        }
        let width = pts[1].x - pts[0].x;
        if width <= 0.0 || x < pts[0].x || x >= pts[1].x {
            return 0.0;
        }
        (from - pts[0].y) * (1.0 - (x - pts[0].x) / width)
    }
}

#[derive(Debug, Clone)]
struct Env {
    spec: EnvSpec,
    /// This block's output, -1 at rest to 1 at the peak, so a link reads it
    /// exactly as it reads an LFO: rest on the low end, peak on the high.
    value: f32,
    /// On the note clock: the level a new note's attack climbs from, and the
    /// note it was taken for, so a retrigger mid-release does not jump.
    from: f32,
    note: u32,
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
                let t = 0.5 - 0.5 * (core::f32::consts::PI * self.cycle).cos();
                self.from + (self.to - self.from) * t
            }
            Shape::Eased(curve, mode) => {
                let v = if p < 0.5 {
                    -1.0 + 2.0 * ease(curve, mode, 2.0 * p)
                } else {
                    1.0 - 2.0 * ease(curve, mode, 2.0 * p - 1.0)
                };
                v.clamp(-1.0, 1.0)
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

/// What a link follows, resolved from an id when the link was made.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    /// Index into this set's LFOs.
    Lfo(usize),
    /// Index into this set's envelopes.
    Env(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Link {
    source: Source,
    /// Where the LFO's trough and crest land, 0 to 1 of the parameter's range.
    lo: f32,
    hi: f32,
}

/// Why a link was refused. The document layer reports these rather than
/// dropping the link, the way a patch load reports an unknown id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    UnknownParameter(String),
    /// Stepped parameters are discrete choices. Walking through window
    /// shapes is a different feature, and not this one.
    Stepped(&'static str),
    UnknownSource(u64),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::UnknownParameter(id) => write!(f, "there is no parameter called {id}"),
            LinkError::Stepped(id) => {
                write!(f, "{id} is a set of choices and cannot follow a modulator")
            }
            LinkError::UnknownSource(id) => {
                write!(f, "there is no LFO or envelope {id} to link to")
            }
        }
    }
}

/// Every modulator in a patch, and every link from a parameter to one of them.
#[derive(Debug, Clone, Default)]
pub struct ModSet {
    lfos: Vec<Lfo>,
    envs: Vec<Env>,
    /// Indexed as `PARAMS`. Empty when nothing has been linked.
    links: Vec<Option<Link>>,
}

impl ModSet {
    /// No LFOs and no links. Allocates nothing, so the engine can start with
    /// one.
    pub const fn empty() -> Self {
        Self {
            lfos: Vec::new(),
            envs: Vec::new(),
            links: Vec::new(),
        }
    }

    /// A set with these LFOs and nothing linked yet.
    pub fn new(lfos: &[LfoSpec]) -> Self {
        Self::with_envelopes(lfos, &[])
    }

    /// A set with these LFOs and envelopes and nothing linked yet. The caller
    /// keeps ids unique across both; a link to a shared id finds the LFO.
    pub fn with_envelopes(lfos: &[LfoSpec], envs: &[EnvSpec]) -> Self {
        Self {
            lfos: lfos.iter().map(|s| Lfo::new(*s)).collect(),
            envs: envs
                .iter()
                .map(|s| Env {
                    spec: s.clone(),
                    value: -1.0,
                    from: 0.0,
                    note: 0,
                })
                .collect(),
            links: Vec::new(),
        }
    }

    /// Link `param` to the LFO or envelope with `source_id`, replacing any
    /// link it had. The ends are clamped to 0 to 1, the parameter's own range.
    pub fn link(&mut self, param: &str, source_id: u64, lo: f32, hi: f32) -> Result<(), LinkError> {
        let slot = index_of(param).ok_or_else(|| LinkError::UnknownParameter(param.to_string()))?;
        if matches!(PARAMS[slot].taper, Taper::Stepped(_)) {
            return Err(LinkError::Stepped(PARAMS[slot].id));
        }
        let source = self
            .lfos
            .iter()
            .position(|l| l.spec.id == source_id)
            .map(Source::Lfo)
            .or_else(|| {
                self.envs
                    .iter()
                    .position(|e| e.spec.id == source_id)
                    .map(Source::Env)
            })
            .ok_or(LinkError::UnknownSource(source_id))?;
        if self.links.len() < PARAMS.len() {
            self.links.resize(PARAMS.len(), None);
        }
        self.links[slot] = Some(Link {
            source,
            lo: lo.clamp(0.0, 1.0),
            hi: hi.clamp(0.0, 1.0),
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
        for env in &mut self.envs {
            if let Some(prev) = old.envs.iter().find(|p| p.spec.id == env.spec.id) {
                env.value = prev.value;
                env.from = prev.from;
                env.note = prev.note;
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

    /// Once per block, after `advance`: where each envelope is on the pass's
    /// clock. `elapsed` and `length` are in samples of the pass; `sounding`
    /// is false between steps, where every envelope rests.
    #[inline]
    pub fn clock_envelopes(&mut self, elapsed: f32, length: f32, sounding: bool) {
        for env in &mut self.envs {
            let level = env.spec.level_at(elapsed, length, sounding);
            env.value = 2.0 * level - 1.0;
        }
    }

    /// The same, on the note clock, for the Loop and Hold lengths: `age`
    /// samples into note number `note`, held for `gate` samples or for ever, and `pass_len` the sample's window.
    /// A note number it has not seen is a new note, whose attack climbs from
    /// where the last one had got to.
    #[inline]
    pub fn clock_envelopes_note(&mut self, age: f32, gate: Option<f32>, note: u32, pass_len: f32) {
        for env in &mut self.envs {
            if env.note != note {
                env.note = note;
                env.from = 0.5 + 0.5 * env.value;
            }
            let level = env.spec.level_at_note(age, gate, pass_len, env.from);
            env.value = 2.0 * level - 1.0;
        }
    }

    /// The value the engine should use for parameter `slot`, given the hand's
    /// `base`. Exactly `base` when nothing is linked, not a round trip through
    /// the taper, which would not be bit-exact. Linked, the LFO's −1 lands on
    /// the low end and its +1 on the high end.
    #[inline]
    pub fn apply(&self, slot: usize, base: f32) -> f32 {
        let Some(Some(link)) = self.links.get(slot) else {
            return base;
        };
        let value = match link.source {
            Source::Lfo(i) => self.lfos.get(i).map(|l| l.value),
            Source::Env(i) => self.envs.get(i).map(|e| e.value),
        };
        let Some(value) = value else {
            return base;
        };
        let def = &PARAMS[slot];
        let t = 0.5 + 0.5 * value.clamp(-1.0, 1.0);
        def.denormalise(link.lo + (link.hi - link.lo) * t)
    }

    /// The current output of the LFO with `id`, -1 to 1. For drawing and tests.
    pub fn lfo_value(&self, id: u64) -> Option<f32> {
        self.lfos.iter().find(|l| l.spec.id == id).map(|l| l.value)
    }

    /// The current output of the envelope with `id`, -1 at rest to 1 at the
    /// peak. For drawing and tests.
    pub fn env_value(&self, id: u64) -> Option<f32> {
        self.envs.iter().find(|e| e.spec.id == id).map(|e| e.value)
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
    fn every_shape_has_one_name_and_comes_back_from_it() {
        for shape in Shape::ALL {
            assert_eq!(Shape::from_name(shape.name()), Some(shape));
        }
        let mut names = Shape::NAMES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Shape::ALL.len(), "two shapes share a name");
        assert_eq!(Shape::from_name("wobble"), None);
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
    fn unlinked_is_exactly_the_base_value_and_a_link_ignores_it() {
        let mut set = ModSet::new(&[spec(7, 5.0, Shape::Square)]);
        let size = index_of("grain.size").unwrap();
        let density = index_of("grain.density").unwrap();
        let d = &PARAMS[density];
        set.link("grain.density", 7, 0.25, 0.75).unwrap();
        for _ in 0..50 {
            set.advance(BLOCK, SR);
            assert_eq!(set.apply(size, 123.4), 123.4);
            let v = set.apply(density, 37.7);
            let t = d.normalise(v);
            assert!((t - 0.25).abs() < 1e-3 || (t - 0.75).abs() < 1e-3, "{t}");
        }
        assert_eq!(ModSet::empty().apply(size, 123.4), 123.4);
    }

    #[test]
    fn the_ends_of_a_link_are_the_ends_of_the_sweep() {
        // A triangle visits both ends; the sweep must reach exactly the
        // values the ends name, and never pass them.
        let mut set = ModSet::new(&[spec(1, 2.0, Shape::Triangle)]);
        let slot = index_of("fx.2.ring.freq").unwrap();
        let d = &PARAMS[slot];
        set.link("fx.2.ring.freq", 1, 0.4, 0.5).unwrap();
        let (lo, hi) = (d.denormalise(0.4), d.denormalise(0.5));
        let (mut min, mut max) = (f32::MAX, f32::MIN);
        for _ in 0..4_000 {
            set.advance(BLOCK, SR);
            let v = set.apply(slot, d.default);
            assert!(v >= lo - 1e-3 && v <= hi + 1e-3, "{v} outside {lo}..{hi}");
            min = min.min(v);
            max = max.max(v);
        }
        assert!(
            (min - lo).abs() < hi * 0.01,
            "never reached the low end: {min}"
        );
        assert!(
            (max - hi).abs() < hi * 0.01,
            "never reached the high end: {max}"
        );
        // Low above high sweeps the other way, still inside the ends.
        set.link("fx.2.ring.freq", 1, 0.5, 0.4).unwrap();
        for _ in 0..400 {
            set.advance(BLOCK, SR);
            let v = set.apply(slot, d.default);
            assert!(v >= lo - 1e-3 && v <= hi + 1e-3);
        }
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
            let (lo, hi) = if i % 2 == 0 { (0.0, 1.0) } else { (1.0, 0.0) };
            set.link(p.id, (i % specs.len()) as u64 + 1, lo, hi)
                .unwrap();
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
            set.link("grain.nonsense", 1, 0.0, 0.5),
            Err(LinkError::UnknownParameter("grain.nonsense".into()))
        );
        assert_eq!(
            set.link("grain.window", 1, 0.0, 0.5),
            Err(LinkError::Stepped("grain.window"))
        );
        assert_eq!(
            set.link("grain.size", 99, 0.0, 0.5),
            Err(LinkError::UnknownSource(99))
        );
        // Out-of-range ends are clamped rather than refused.
        set.link("grain.size", 1, -3.0, 7.0).unwrap();
        let l = set.links[index_of("grain.size").unwrap()].unwrap();
        assert_eq!((l.lo, l.hi), (0.0, 1.0));
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

    #[test]
    fn every_easing_starts_at_zero_and_ends_at_one() {
        // What makes an eased LFO continuous at both turns.
        for curve in [
            Curve::Quad,
            Curve::Cubic,
            Curve::Expo,
            Curve::Elastic,
            Curve::Bounce,
        ] {
            for mode in [Ease::In, Ease::Out, Ease::InOut] {
                let (start, end) = (ease(curve, mode, 0.0), ease(curve, mode, 1.0));
                assert!(start.abs() < 1e-3, "{curve:?} {mode:?} starts at {start}");
                assert!((end - 1.0).abs() < 1e-3, "{curve:?} {mode:?} ends at {end}");
            }
        }
    }

    #[test]
    fn in_out_is_symmetric_about_its_middle() {
        for curve in [
            Curve::Quad,
            Curve::Cubic,
            Curve::Expo,
            Curve::Elastic,
            Curve::Bounce,
        ] {
            for i in 0..=20 {
                let t = i as f32 / 20.0;
                let sum = ease(curve, Ease::InOut, t) + ease(curve, Ease::InOut, 1.0 - t);
                assert!((sum - 1.0).abs() < 1e-4, "{curve:?} at {t}: {sum}");
            }
        }
    }

    #[test]
    fn an_eased_lfo_turns_without_a_jump() {
        // At the turn from rising to falling, and at the wrap, the eased
        // triangle meets itself. A slow rate makes any jump stand out against
        // the tiny per-block movement either side of it.
        for curve in [Curve::Quad, Curve::Cubic, Curve::Bounce] {
            for mode in [Ease::In, Ease::Out, Ease::InOut] {
                let mut set = ModSet::new(&[spec(1, 0.5, Shape::Eased(curve, mode))]);
                let mut prev: Option<f32> = None;
                for _ in 0..(SR as usize * 8 / BLOCK) {
                    set.advance(BLOCK, SR);
                    let v = set.lfo_value(1).unwrap();
                    if let Some(p) = prev {
                        assert!(
                            (v - p).abs() < 0.25,
                            "{curve:?} {mode:?} jumped by {}",
                            (v - p).abs()
                        );
                    }
                    prev = Some(v);
                }
            }
        }
    }

    // ----- Curve envelopes -----

    fn env(curve: EnvCurve) -> EnvSpec {
        EnvSpec { id: 1, curve }
    }

    fn ramp() -> EnvCurve {
        let p = |x: f32, y: f32| CurvePoint {
            x,
            y,
            basis: Basis::Linear,
            tension: 0.0,
        };
        EnvCurve {
            points: vec![p(0.0, 0.0), p(1.0, 1.0)],
            sustain: None,
        }
    }

    #[test]
    fn a_ramp_reads_half_way_through_the_pass_and_the_gate() {
        let e = env(ramp());
        assert!((e.level_at(500.0, 1_000.0, true) - 0.5).abs() < 1e-6);
        assert!((e.level_at(1_000.0, 1_000.0, true) - 1.0).abs() < 1e-6);
        assert_eq!(e.level_at(500.0, 1_000.0, false), 0.0, "rests between");
        // Hold: the gate is the span, and the end holds after it.
        let hold = |age: f32| e.level_at_note(age, Some(2_000.0), 9_999.0, 0.0);
        assert!((hold(1_000.0) - 0.5).abs() < 1e-6);
        assert!((hold(2_000.0) - 1.0).abs() < 1e-6);
        assert!((hold(50_000.0) - 1.0).abs() < 1e-6, "holds the end");
        // Loop: there is no gate, so the sample's pass is the span.
        let lp = |age: f32| e.level_at_note(age, None, 4_000.0, 0.0);
        assert!((lp(1_000.0) - 0.25).abs() < 1e-6);
        assert!((lp(4_000.0) - 1.0).abs() < 1e-6);
        assert!((lp(90_000.0) - 1.0).abs() < 1e-6, "holds the end");
    }

    #[test]
    fn a_curve_has_its_release_inside_the_gate() {
        // An adsr drawn over 0 to 1: its release is part of the span, so the
        // level is back at rest when the gate closes.
        let e = env(adsr_curve(0.1, 0.2, 0.5, 0.7));
        let at = |x: f32| e.level_at_note(x * 1_000.0, Some(1_000.0), 0.0, 0.0);
        assert!((at(0.1) - 1.0).abs() < 1e-5);
        assert!((at(0.3) - 0.5).abs() < 1e-5);
        assert!((at(0.65) - 0.25).abs() < 1e-5, "mid-release, while gated");
        assert!(at(1.0).abs() < 1e-5);
    }

    #[test]
    fn a_retrigger_mid_release_starts_from_the_level_reached_without_a_jump() {
        {
            let curve = adsr_curve(0.1, 0.2, 0.6, 0.7);
            let mut set = ModSet::with_envelopes(&[], &[env(curve)]);
            let gate = 0.2 * SR;
            let mut prev: Option<f32> = None;
            let mut max_step = 0.0f32;
            let mut at_retrigger = None;
            // Note 1 for 0.3 s (released at 0.2 s, mid-release), then note 2.
            for note in [1u32, 2] {
                for block in 0..((0.5 * SR) as usize / BLOCK) {
                    let age = (block * BLOCK) as f32;
                    set.clock_envelopes_note(age, Some(gate), note, 1.0 * SR);
                    let v = set.env_value(1).unwrap();
                    assert!((-1.0..=1.0).contains(&v));
                    if let Some(p) = prev {
                        let step = (v - p).abs();
                        if block == 0 && note == 2 {
                            at_retrigger = Some(step);
                        } else {
                            max_step = max_step.max(step);
                        }
                    }
                    prev = Some(v);
                    if age > 0.3 * SR {
                        break;
                    }
                }
            }
            assert!(
                at_retrigger.unwrap() < 1e-5,
                "the new note jumped by {at_retrigger:?}"
            );
            // An attack is a tenth of a 0.2 s gate, so one block of it
            // climbs about 0.53 of the range; a jump would be the whole gap.
            assert!(max_step < 0.6, "blocks differ by {max_step}");
        }
    }

    #[test]
    fn a_curve_that_overshoots_still_modulates_within_one() {
        let p = |x: f32, y: f32| CurvePoint {
            x,
            y,
            basis: Basis::CatmullRom,
            tension: 0.0,
        };
        let curve = EnvCurve {
            points: vec![
                p(0.0, 0.0),
                p(0.2, 0.0),
                p(0.4, 1.0),
                p(0.6, 1.0),
                p(1.0, 0.0),
            ],
            sustain: Some(3),
        };
        let raw_max = (0..=1000)
            .map(|i| curve.evaluate(i as f32 / 1000.0))
            .fold(f32::MIN, f32::max);
        let raw_min = (0..=1000)
            .map(|i| curve.evaluate(i as f32 / 1000.0))
            .fold(f32::MAX, f32::min);
        assert!(raw_max > 1.0 || raw_min < 0.0, "the curve should overshoot");
        {
            let e = env(curve.clone());
            for i in 0..=400 {
                let age = i as f32 * 0.005 * SR;
                for v in [
                    e.level_at(age, 0.8 * SR, true),
                    e.level_at_note(age, None, 0.8 * SR, 0.3),
                    e.level_at_note(age, Some(0.5 * SR), 0.8 * SR, 1.0),
                ] {
                    assert!((0.0..=1.0).contains(&v), "gave {v}");
                }
            }
        }
    }
}
