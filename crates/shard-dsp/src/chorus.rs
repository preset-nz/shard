//! Chorus.
//!
//! A short delay line a channel, read at up to eight points a millisecond or
//! two apart. Each read point has its own LFO sweeping it back and forth. A
//! moving read point bends the pitch of its copy slightly, and copies
//! drifting sharp and flat against the original read as an ensemble rather
//! than one voice.
//!
//! `voices` sets how many, and it is continuous: the next voice fades in as
//! the control turns, so it sweeps without a click and an LFO can drive it.
//! One or two voices beat audibly; six or eight blur the beating into a
//! smooth wall. Voices are added in an order that keeps the ones already
//! sounding where they were, so turning it up only adds. The right channel's LFOs sit a sixth of a turn from the
//! left's, halfway between them, so the two sides never move together and a
//! mono source comes out wide.
//!
//! The voices are summed at equal power, so the wet signal is as loud as the
//! dry and `mix` crossfades between them like every other effect's. At full
//! mix nothing dry is left, and what remains is vibrato: the chorus is the
//! dry beating against the copies, strongest around half.
//!
//! EQ is the Boss CH-1's knob: a shelf on the wet signal only. Up brightens
//! the copies, down darkens them, and at zero it is flat.
//!
//! Ensemble is the other voicing, after the string machines (Solina, ARP,
//! Eminent): copies spread from 9 to 26 ms, each swept by a slow
//! LFO for the swirl and a small fast one, about six times a second, for the
//! shimmer. The two sides read the mono sum rather than their own channel, and
//! the right sweeps against the left, so a centred source is pulled apart into
//! a wall rather than doubled.
//!
//! Choir is for voices, and works differently. A swept delay only bends
//! pitch while it moves, so a slow chorus barely detunes, and a voice already
//! wavers more than that. Choir holds each copy a fixed amount sharp or flat
//! instead, with a small pitch shifter per copy: two read points drifting
//! through a 20 ms window at the shifted speed, each faded out as it wraps.
//! That is the micro-pitch trick of the old studio harmonizers. Depth sets
//! the spread, up to 25 cents; copies on the left go one way and their
//! partners on the right the other. The copies sit 12 to 50 ms back, far
//! enough apart to read as more singers rather than a thicker one, and each
//! wanders in time on its own slow random path, at Rate. Nothing about it
//! repeats, which is what makes a crowd sound like one.
//!
//! Switching voicings crossfades, and only the voicing being heard is
//! computed.
//!
//! Two controls shape every voicing alike. Spread scales where the copies
//! sit, from half to twice: pulled in, a chorus turns towards a flanger and a
//! choir towards one thick voice; pushed out, towards separate singers and
//! then a slapback. Low cut is a high-pass on the copies, so the bass stays
//! in the dry.
//!
//! At zero mix the node is a bit-exact bypass. The line keeps filling, so
//! switching on never plays stale audio.

use crate::rng::Rng;
use crate::smooth::OnePole;

/// The most voices a side, and the top of `chorus.voices`.
pub const MAX_VOICES: usize = 8;
/// Where the plain chorus's copies sit, in milliseconds, in the order they
/// come in. Long enough to separate from the dry signal, short enough not to
/// be an echo. All different, so the copies are different even at zero depth
/// and their equal-power sum holds its level.
const CENTRES_MS: [f32; MAX_VOICES] = [8.0, 10.0, 12.0, 9.0, 11.0, 13.0, 7.0, 14.0];
/// The widest a read point strays from the centre, at full depth.
const SWEEP_MS: f32 = 3.0;
/// The fastest LFO. `chorus.rate` stops here, and a test holds the pitch
/// swing this allows at full depth to under a couple of semitones.
pub const MAX_RATE_HZ: f32 = 5.0;
/// Where the ensemble's copies sit, in milliseconds, in the order they come
/// in: the left's, then the right's, interleaved so no two sides share a
/// delay. The first four a side span the range; the next four fill between.
const ENS_CENTRES_MS: [[f32; MAX_VOICES]; 2] = [
    [9.0, 13.5, 18.0, 22.5, 10.1, 14.6, 19.1, 23.6],
    [11.25, 15.75, 20.25, 24.75, 12.4, 16.9, 21.4, 25.9],
];
/// The slow sweep at full depth. The swirl. Kept under the plain chorus's
/// three so the slow component alone stays a chorus's pitch swing.
pub const ENS_SLOW_MS: f32 = 2.5;
/// The fast sweep at full depth. The shimmer: small, because a read point's
/// pitch bend is its depth times its speed, and at six hertz a quarter of a
/// millisecond is already about fifteen cents.
pub const ENS_FAST_MS: f32 = 0.25;
/// The fast LFO's rate, fixed. Around six hertz is where a string machine's
/// vibrato-like flutter lives; `chorus.rate` moves the slow one.
pub const ENS_FAST_HZ: f32 = 6.3;
/// The low cut's range, in hertz. At the bottom it passes everything that
/// matters; at the top only the copies' presence and air are left.
pub const LOW_CUT_MIN_HZ: f32 = 20.0;
pub const LOW_CUT_MAX_HZ: f32 = 1_000.0;
/// Spread's range: how far the copies' delays are scaled from where each
/// voicing puts them. Down pulls them in towards a flanger; up pushes them
/// out towards separate voices, and past that a slapback.
pub const SPREAD_MIN: f32 = 0.5;
pub const SPREAD_MAX: f32 = 2.0;
/// Where the choir's copies start, in milliseconds, in the order they come
/// in: each reads from here to a window further back. Left, then right, no
/// two alike.
const CHOIR_BASES_MS: [[f32; MAX_VOICES]; 2] = [
    [14.0, 24.0, 19.0, 29.0, 12.0, 22.0, 17.0, 27.0],
    [17.0, 27.0, 12.5, 22.5, 20.0, 30.0, 15.0, 25.0],
];
/// The pitch shifter's window. Longer smears less as the read points wrap,
/// but reads further back.
const CHOIR_WINDOW_MS: f32 = 20.0;
/// The widest detune, in cents, at full depth, for a copy whose share is one.
pub const CHOIR_DETUNE_CENTS: f32 = 25.0;
/// Each copy's share of the detune, left side; the right's are the opposite,
/// so a copy sharp on one side has a partner as flat on the other. The first
/// two already make a stereo pair.
const CHOIR_SHARES: [f32; MAX_VOICES] = [0.6, -0.6, 1.0, -1.0, 0.3, -0.3, 0.8, -0.8];
/// How far each copy wanders in time, either way.
pub const CHOIR_DRIFT_MS: f32 = 0.6;
/// Each copy's wander runs at Rate times this, so no two keep step.
const CHOIR_DRIFT_RATES: [f32; MAX_VOICES] = [1.0, 1.13, 0.87, 1.21, 0.79, 1.07, 0.93, 1.17];
/// The fastest any copy wanders, as a multiple of Rate.
pub const CHOIR_DRIFT_FASTEST: f32 = 1.21;
/// The latest any choir copy starts.
const CHOIR_LATEST_MS: f32 = 30.0;
/// Room for the latest copy of any voicing at the widest spread plus its
/// reach, and a margin for the interpolator's four points. The choir reaches
/// furthest.
const MAX_MS: f32 = CHOIR_LATEST_MS * SPREAD_MAX + CHOIR_WINDOW_MS + CHOIR_DRIFT_MS + 2.0;
/// Frame width of the line: left, right and their mono sum.
const STRIDE: usize = 3;
/// The mono sum's slot in a frame.
const MID: usize = 2;
/// Where the EQ's shelf turns, in hertz.
const EQ_CORNER_HZ: f32 = 1_500.0;
/// The EQ's reach either way, in decibels.
pub const EQ_RANGE_DB: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChorusType {
    /// Copies close together, each channel from its own input. The classic.
    Chorus,
    /// Copies spread wider, from the mono sum, slow swirl plus fast shimmer,
    /// the low end left dry.
    Ensemble,
    /// Copies held sharp and flat, further apart, each wandering on its own.
    /// For voices.
    Choir,
}

impl ChorusType {
    pub const ALL: [ChorusType; 3] = [ChorusType::Chorus, ChorusType::Ensemble, ChorusType::Choir];
    pub const NAMES: [&'static str; 3] = ["Chorus", "Ensemble", "Choir"];

    pub fn from_value(v: f32) -> ChorusType {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChorusParams {
    /// Which voicing.
    pub kind: ChorusType,
    /// LFO rate in hertz.
    pub rate: f32,
    /// How far the read points sweep, 0 to 1.
    pub depth: f32,
    /// How many copies a side, 1 to `MAX_VOICES`, fractional between.
    pub voices: f32,
    /// How far the copies' delays are scaled, `SPREAD_MIN` to `SPREAD_MAX`.
    pub spread: f32,
    /// Where the high-pass on the copies turns, in hertz.
    pub low_cut_hz: f32,
    /// Shelf on the wet signal in decibels, ±`EQ_RANGE_DB`. Zero is flat.
    pub eq_db: f32,
    /// Dry to wet, 0 to 1.
    pub mix: f32,
}

impl Default for ChorusParams {
    fn default() -> Self {
        Self {
            kind: ChorusType::Chorus,
            rate: 0.6,
            depth: 0.5,
            voices: 4.0,
            spread: 1.0,
            low_cut_hz: 240.0,
            eq_db: 0.0,
            mix: 0.0,
        }
    }
}

/// One choir copy's slow random path: from one point to the next along a
/// half cosine, so it never turns a corner.
#[derive(Debug, Clone, Copy, Default)]
struct Wander {
    from: f32,
    to: f32,
    t: f32,
}

pub struct Chorus {
    /// Interleaved frames, sized once. `write` is the newest frame.
    line: Vec<f32>,
    write: usize,
    frames: usize,
    phase: f32,
    /// The ensemble's fast LFO, in turns.
    fast_phase: f32,
    sample_rate: f32,
    /// How much of each voicing is heard, in `ChorusType::ALL` order: one
    /// at a time, two while switching.
    blend: [OnePole; 3],
    /// The low cut is the wet less this low-pass, a side each.
    cut_lows: [f32; 2],
    low_cut: OnePole,
    spread: OnePole,
    /// This frame's spread, for the voicings to read.
    spread_now: f32,
    /// Each choir copy's place in its pitch shifter's window, in turns.
    shift: [[f32; MAX_VOICES]; 2],
    wander: [[Wander; MAX_VOICES]; 2],
    rng: Rng,
    /// The EQ's one-pole low-pass, one a channel: the shelf is the lows plus
    /// the highs scaled.
    lows: [f32; 2],
    lows_coef: f32,
    rate: OnePole,
    depth: OnePole,
    voices: OnePole,
    eq_gain: OnePole,
    mix: OnePole,
}

impl Chorus {
    pub fn new(sample_rate: f32) -> Self {
        let frames = (MAX_MS * 0.001 * sample_rate).ceil() as usize + 4;
        let d = ChorusParams::default();
        let smoother = |ms: f32, v: f32| {
            let mut p = OnePole::new();
            p.set_time(ms, sample_rate);
            p.reset(v);
            p
        };
        Self {
            line: vec![0.0; frames * STRIDE],
            write: 0,
            frames,
            phase: 0.0,
            fast_phase: 0.0,
            sample_rate,
            // A voicing change is a different wet signal, so fade it.
            blend: [
                smoother(30.0, 1.0),
                smoother(30.0, 0.0),
                smoother(30.0, 0.0),
            ],
            cut_lows: [0.0; 2],
            low_cut: smoother(20.0, d.low_cut_hz),
            // Spread moves every read point, and a choir copy's by up to
            // 45 ms, so a step glides in pitch like a tape delay's time.
            // Slow, to keep that glide to a few semitones.
            spread: smoother(150.0, d.spread),
            spread_now: d.spread,
            // Spread through the window, so no two copies wrap together.
            shift: [
                core::array::from_fn(|v| (v as f32 * 0.382) % 1.0),
                core::array::from_fn(|v| (v as f32 * 0.382 + 0.19) % 1.0),
            ],
            wander: [[Wander::default(); MAX_VOICES]; 2],
            rng: Rng::new(0x00C4_01E5),
            lows: [0.0; 2],
            lows_coef: 1.0 - (-core::f32::consts::TAU * EQ_CORNER_HZ / sample_rate).exp(),
            // Rate and depth move the read points, so a step in either is a
            // pitch jump. Slower than the house 20 ms for that reason.
            rate: smoother(50.0, d.rate),
            depth: smoother(50.0, d.depth),
            // A voice fading in is a level change, not a pitch one: 20 ms.
            voices: smoother(20.0, d.voices),
            eq_gain: smoother(20.0, 1.0),
            mix: smoother(20.0, d.mix),
        }
    }

    /// One channel's read at `delay` frames behind the newest, through a
    /// four-point Hermite curve. Linear interpolation dulls the top by up to
    /// a couple of decibels halfway between samples, and a swept read point
    /// passes halfway constantly, so the copies would wobble in brightness.
    #[inline]
    fn tap(&self, ch: usize, delay: f32) -> f32 {
        let d = delay.clamp(2.0, (self.frames - 3) as f32);
        let whole = d.floor();
        let t = d - whole;
        let n = self.frames;
        let at = |back: usize| self.line[((self.write + n - back) % n) * STRIDE + ch];
        let w = whole as usize;
        let (xm1, x0, x1, x2) = (at(w - 1), at(w), at(w + 1), at(w + 2));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    /// How loud each voice is, and the equal-power sum's correction: whole
    /// voices at one, the next fading in, the rest silent.
    #[inline]
    fn weights(voices: f32) -> ([f32; MAX_VOICES], f32) {
        let mut w = [0.0; MAX_VOICES];
        let mut power = 0.0;
        for (v, w) in w.iter_mut().enumerate() {
            *w = (voices - v as f32).clamp(0.0, 1.0);
            power += *w * *w;
        }
        (w, 1.0 / power.max(1.0).sqrt())
    }

    /// One channel's plain voices, summed at equal power. `offset` is where
    /// its LFOs sit in the turn. The first three are a third of a turn apart;
    /// each later three sit a ninth further on, between them.
    #[inline]
    fn voices(&self, ch: usize, offset: f32, sweep: f32, w: &[f32; MAX_VOICES]) -> f32 {
        let ms = 0.001 * self.sample_rate;
        let mut sum = 0.0;
        for (v, (&w, centre)) in w.iter().zip(CENTRES_MS).enumerate() {
            if w == 0.0 {
                break;
            }
            let turn = self.phase + offset + (v % 3) as f32 / 3.0 + (v / 3) as f32 / 9.0;
            let lfo = (core::f32::consts::TAU * turn).sin();
            sum += w * self.tap(ch, centre * self.spread_now * ms + sweep * lfo);
        }
        sum
    }

    /// One side of the ensemble, from the mono sum. Each copy's read point is
    /// its centre plus the slow sweep and the fast one. The first four slow
    /// LFOs sit a quarter turn apart, the next four an eighth past them, and
    /// the right's a half turn from the left's, so the sides sweep against
    /// each other. The fast LFOs are spread by an irrational-ish step so no
    /// two copies flutter together.
    #[inline]
    fn ensemble_side(&self, side: usize, slow: f32, fast: f32, w: &[f32; MAX_VOICES]) -> f32 {
        let ms = 0.001 * self.sample_rate;
        let mut sum = 0.0;
        for (v, (&w, centre)) in w.iter().zip(ENS_CENTRES_MS[side]).enumerate() {
            if w == 0.0 {
                break;
            }
            let s_turn =
                self.phase + 0.5 * side as f32 + (v % 4) as f32 / 4.0 + (v / 4) as f32 / 8.0;
            let f_turn = self.fast_phase + 0.37 * v as f32 + 0.21 * side as f32;
            let sweep = slow * (core::f32::consts::TAU * s_turn).sin()
                + fast * (core::f32::consts::TAU * f_turn).sin();
            sum += w * self.tap(MID, centre * self.spread_now * ms + sweep);
        }
        sum
    }

    /// One side of the choir, from the mono sum, and a step of every copy's
    /// shifter and wander. `cents` is the full-depth detune scaled by depth,
    /// `rate` the wander's.
    #[inline]
    fn choir_side(&mut self, side: usize, cents: f32, rate: f32, w: &[f32; MAX_VOICES]) -> f32 {
        let ms = 0.001 * self.sample_rate;
        let window = CHOIR_WINDOW_MS * ms;
        let sign = if side == 0 { 1.0 } else { -1.0 };
        let mut sum = 0.0;
        for v in 0..MAX_VOICES {
            // A read point moving at speed r plays r times the pitch, and
            // its delay then grows by 1 - r a frame.
            let ratio = (sign * CHOIR_SHARES[v] * cents / 1200.0).exp2();
            let mut phase = self.shift[side][v] + (1.0 - ratio) / window;
            phase -= phase.floor();
            self.shift[side][v] = phase;

            let wander = &mut self.wander[side][v];
            wander.t += rate * CHOIR_DRIFT_RATES[v] / self.sample_rate;
            if wander.t >= 1.0 {
                wander.t -= wander.t.floor();
                wander.from = wander.to;
                wander.to = self.rng.next_bipolar();
            }
            let ease = 0.5 - 0.5 * (core::f32::consts::PI * wander.t).cos();
            let drift = (wander.from + (wander.to - wander.from) * ease) * CHOIR_DRIFT_MS * ms;

            if w[v] == 0.0 {
                continue;
            }
            // Two read points half a window apart, each silent as it wraps.
            // Sine gains, so their powers sum to one.
            let base = CHOIR_BASES_MS[side][v] * self.spread_now * ms + drift;
            let other = (phase + 0.5) % 1.0;
            let a = (core::f32::consts::PI * phase).sin() * self.tap(MID, base + window * phase);
            let b = (core::f32::consts::PI * other).sin() * self.tap(MID, base + window * other);
            sum += w[v] * (a + b);
        }
        sum
    }

    /// Forgets all audio held in the delay line and the filters. The sweeps
    /// and the choir copies' wander carry on where they were.
    pub fn reset(&mut self) {
        self.line.fill(0.0);
        self.cut_lows = [0.0; 2];
        self.lows = [0.0; 2];
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &ChorusParams) -> (f32, f32) {
        let rate = self.rate.process(p.rate.clamp(0.0, MAX_RATE_HZ));
        let depth = self.depth.process(p.depth.clamp(0.0, 1.0));
        let voices = self.voices.process(p.voices.clamp(1.0, MAX_VOICES as f32));
        self.spread_now = self.spread.process(p.spread.clamp(SPREAD_MIN, SPREAD_MAX));
        let low_cut = self
            .low_cut
            .process(p.low_cut_hz.clamp(LOW_CUT_MIN_HZ, LOW_CUT_MAX_HZ));
        let eq_db = p.eq_db.clamp(-EQ_RANGE_DB, EQ_RANGE_DB);
        let eq_gain = self.eq_gain.process(10f32.powf(eq_db / 20.0));
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));
        // Snap the last of a fade to a true zero, as the ring modulator does:
        // a section switched off has to be bit-exact with no chorus at all.
        let mix = if p.mix <= 0.0 && mix < 1e-5 {
            self.mix.reset(0.0);
            0.0
        } else {
            mix
        };

        self.write = (self.write + 1) % self.frames;
        self.line[self.write * STRIDE] = l;
        self.line[self.write * STRIDE + 1] = r;
        self.line[self.write * STRIDE + MID] = 0.5 * (l + r);
        self.phase += rate / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
        self.fast_phase += ENS_FAST_HZ / self.sample_rate;
        if self.fast_phase >= 1.0 {
            self.fast_phase -= self.fast_phase.floor();
        }
        let mut blend = [0.0; 3];
        for (i, (b, kind)) in blend.iter_mut().zip(ChorusType::ALL).enumerate() {
            *b = self.blend[i].process(if p.kind == kind { 1.0 } else { 0.0 });
        }
        if mix == 0.0 {
            return (l, r);
        }

        // Only the voicing being heard is computed, or two while one fades
        // into the other.
        let (weights, norm) = Self::weights(voices);
        let mut wet = [0.0; 2];
        if blend[0] > 0.001 {
            let sweep = SWEEP_MS * depth * 0.001 * self.sample_rate;
            // The right's LFOs halfway between the left's first three.
            let offset_r = 1.0 / 6.0;
            wet[0] += blend[0] * self.voices(0, 0.0, sweep, &weights) * norm;
            wet[1] += blend[0] * self.voices(1, offset_r, sweep, &weights) * norm;
        }
        if blend[1] > 0.001 {
            let k = 0.001 * self.sample_rate * depth;
            let (slow, fast) = (ENS_SLOW_MS * k, ENS_FAST_MS * k);
            for (side, w) in wet.iter_mut().enumerate() {
                *w += blend[1] * self.ensemble_side(side, slow, fast, &weights) * norm;
            }
        }
        if blend[2] > 0.001 {
            let cents = CHOIR_DETUNE_CENTS * depth;
            for (side, w) in wet.iter_mut().enumerate() {
                *w += blend[2] * self.choir_side(side, cents, rate, &weights) * norm;
            }
        }
        // The low cut, on the copies of every voicing: the bass stays dry
        // rather than swimming.
        let cut_coef = 1.0 - (-core::f32::consts::TAU * low_cut / self.sample_rate).exp();
        let mut out = [0.0; 2];
        for ch in 0..2 {
            self.cut_lows[ch] += cut_coef * (wet[ch] - self.cut_lows[ch]);
            let wet = wet[ch] - self.cut_lows[ch];
            self.lows[ch] += self.lows_coef * (wet - self.lows[ch]);
            let shaped = self.lows[ch] + (wet - self.lows[ch]) * eq_gain;
            let dry = if ch == 0 { l } else { r };
            out[ch] = dry * (1.0 - mix) + shaped * mix;
        }
        (out[0], out[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn tone(i: usize) -> f32 {
        (core::f32::consts::TAU * 220.0 * i as f32 / SR).sin() * 0.5
    }

    /// White noise from a fixed seed, so a test hears every frequency.
    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn wet(depth: f32, rate: f32, eq_db: f32) -> ChorusParams {
        ChorusParams {
            kind: ChorusType::Chorus,
            rate,
            depth,
            voices: 4.0,
            spread: 1.0,
            low_cut_hz: 240.0,
            eq_db,
            mix: 1.0,
        }
    }

    fn ensemble(depth: f32, rate: f32, eq_db: f32) -> ChorusParams {
        ChorusParams {
            kind: ChorusType::Ensemble,
            ..wet(depth, rate, eq_db)
        }
    }

    /// The first two voicings, for comparisons between them.
    fn both(depth: f32, rate: f32, eq_db: f32) -> [ChorusParams; 2] {
        [wet(depth, rate, eq_db), ensemble(depth, rate, eq_db)]
    }

    /// Every voicing, for a contract that has to hold in each.
    fn every(depth: f32, rate: f32, eq_db: f32) -> [ChorusParams; 3] {
        ChorusType::ALL.map(|kind| ChorusParams {
            kind,
            ..wet(depth, rate, eq_db)
        })
    }

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        for kind in ChorusType::ALL {
            let mut c = Chorus::new(SR);
            let on = ChorusParams {
                kind,
                mix: 1.0,
                ..Default::default()
            };
            let off = ChorusParams {
                kind,
                ..Default::default()
            };
            for i in 0..4_800 {
                c.process(tone(i), tone(i), &on);
            }
            for i in 0..48_000 {
                c.process(tone(i), tone(i), &off);
            }
            for _ in 0..1_000 {
                assert_eq!(c.process(0.5, -0.25, &off), (0.5, -0.25));
            }
        }
    }

    #[test]
    fn reset_forgets_the_audio() {
        for p in every(1.0, 2.0, 0.0) {
            let mut c = Chorus::new(SR);
            let mut seed = 0x1234_5678u32;
            for _ in 0..48_000 {
                c.process(noise(&mut seed), noise(&mut seed), &p);
            }
            c.reset();
            // The line is the longest tail; run well past it.
            for _ in 0..(c.frames * 2) {
                assert_eq!(c.process(0.0, 0.0, &p), (0.0, 0.0));
            }
        }
    }

    #[test]
    fn reset_on_a_fresh_chorus_changes_nothing() {
        for p in every(0.7, 1.5, 3.0) {
            let mut a = Chorus::new(SR);
            let mut b = Chorus::new(SR);
            b.reset();
            for i in 0..10_000 {
                assert_eq!(
                    a.process(tone(i), tone(i), &p),
                    b.process(tone(i), tone(i), &p)
                );
            }
        }
    }

    #[test]
    fn silence_in_silence_out() {
        for p in every(1.0, 2.0, EQ_RANGE_DB) {
            let mut c = Chorus::new(SR);
            for _ in 0..10_000 {
                assert_eq!(c.process(0.0, 0.0, &p), (0.0, 0.0));
            }
        }
    }

    #[test]
    fn wet_changes_the_signal() {
        let mut c = Chorus::new(SR);
        let p = wet(0.5, 0.6, 0.0);
        let mut diff = 0.0f32;
        for i in 0..9_600 {
            let x = tone(i);
            let (l, _) = c.process(x, x, &p);
            if i > 4_800 {
                diff = diff.max((l - x).abs());
            }
        }
        assert!(diff > 0.1, "wet should differ from dry, max diff {diff}");
    }

    #[test]
    fn the_channels_decorrelate() {
        // The right's LFOs sit between the left's: the same mono input must
        // come out different on each side, or the chorus is not widening.
        let mut c = Chorus::new(SR);
        let p = wet(1.0, 2.0, 0.0);
        let mut apart = 0.0f32;
        for i in 0..48_000 {
            let x = tone(i);
            let (l, r) = c.process(x, x, &p);
            if i > 4_800 {
                apart = apart.max((l - r).abs());
            }
        }
        assert!(apart > 0.05, "left and right stayed together: {apart}");
    }

    #[test]
    fn wet_is_as_loud_as_dry() {
        // Mix is a crossfade, like every other effect's, so turning it up
        // must not turn the sound up or down. Noise, so every frequency and
        // every phase between the voices is heard.
        // And whatever the voice count, fractional ones included.
        let cases = [0.0, 0.5, 1.0].into_iter().flat_map(|d| {
            [1.0, 2.5, 4.0, MAX_VOICES as f32]
                .into_iter()
                .flat_map(move |voices| {
                    every(d, 0.6, 0.0).map(|p| (d, ChorusParams { voices, ..p }))
                })
        });
        for (depth, p) in cases {
            let mut c = Chorus::new(SR);
            let mut seed = 0x1234_5678;
            let mut tilt = 0.0f32;
            let (mut dry, mut out) = (0.0f64, 0.0f64);
            for i in 0..96_000 {
                // Tilted down, as music is: every read point between samples
                // loses a little of the top octave to the interpolator, up to
                // 1.6 dB on white noise where the choir holds still, and that
                // is a loss of air, not of level.
                tilt += 0.6 * (noise(&mut seed) * 0.5 - tilt);
                let x = tilt;
                let (l, _) = c.process(x, x, &p);
                if i > 4_800 {
                    dry += (x * x) as f64;
                    out += (l * l) as f64;
                }
            }
            let db = 10.0 * (out / dry).log10();
            assert!(
                db.abs() < 1.5,
                "{:?} depth {depth} voices {}: wet is {db:.2} dB off dry",
                p.kind,
                p.voices
            );
        }
    }

    #[test]
    fn output_is_bounded_at_the_extremes() {
        // Copies at equal power can line up, so a full-scale input can reach
        // the square root of the voice count, times √2 for the choir's two
        // read points a copy, plus the EQ's boost. Never more, and never
        // anything that is not a number.
        let bound = (2.0 * MAX_VOICES as f32).sqrt() * 10f32.powf(EQ_RANGE_DB / 20.0) * 1.2;
        for p in every(1.0, MAX_RATE_HZ, EQ_RANGE_DB) {
            let p = ChorusParams {
                voices: MAX_VOICES as f32,
                ..p
            };
            let mut c = Chorus::new(SR);
            for i in 0..96_000 {
                let x = if i % 200 < 100 { 1.0 } else { -1.0 };
                let (l, r) = c.process(x, -x, &p);
                assert!(l.abs() <= bound && r.abs() <= bound, "{l} {r}");
            }
        }
    }

    #[test]
    fn the_pitch_swing_stays_a_chorus() {
        // A read point moving at speed v bends its copy's pitch by 1 - v, so
        // the fastest sweep at full depth sets the widest swing. Past about
        // two semitones this stops being a chorus and becomes a warble.
        let swing = core::f32::consts::TAU * MAX_RATE_HZ * SWEEP_MS * 0.001;
        let semitones = 12.0 * (1.0 + swing).log2();
        assert!(semitones < 2.0, "widest swing is {semitones:.2} semitones");

        // The ensemble's read points move by two sweeps at once, and the
        // widest swing is both at their fastest and in step.
        let swing = core::f32::consts::TAU
            * (MAX_RATE_HZ * ENS_SLOW_MS + ENS_FAST_HZ * ENS_FAST_MS)
            * 0.001;
        let semitones = 12.0 * (1.0 + swing).log2();
        assert!(
            semitones < 2.0,
            "widest ensemble swing is {semitones:.2} semitones"
        );
        // Its fast shimmer alone, the part that must stay small, at full depth.
        let swing = core::f32::consts::TAU * ENS_FAST_HZ * ENS_FAST_MS * 0.001;
        let cents = 1200.0 * (1.0 + swing).log2();
        assert!(cents < 20.0, "ensemble shimmer is {cents:.1} cents");
        // And at the ensemble's defaults, both sweeps together.
        let d = ChorusParams::default();
        let swing = core::f32::consts::TAU
            * (d.rate * ENS_SLOW_MS + ENS_FAST_HZ * ENS_FAST_MS)
            * d.depth
            * 0.001;
        let cents = 1200.0 * (1.0 + swing).log2();
        assert!(cents < 20.0, "default ensemble swing is {cents:.1} cents");

        // And at the defaults, about a Juno's: under twenty cents.
        let d = ChorusParams::default();
        let swing = core::f32::consts::TAU * d.rate * SWEEP_MS * d.depth * 0.001;
        let cents = 1200.0 * (1.0 + swing).log2();
        assert!(cents < 20.0, "default swing is {cents:.1} cents");

        // The choir detunes on purpose, but its widest copy plus the fastest
        // wander stays under half a semitone: a crowd, not out of tune. The
        // wander's steepest slope is its span times π/2 over its period.
        let wander =
            CHOIR_DRIFT_MS * 0.001 * core::f32::consts::PI * MAX_RATE_HZ * CHOIR_DRIFT_FASTEST;
        let cents = CHOIR_DETUNE_CENTS + 1200.0 * (1.0 + wander).log2();
        assert!(cents < 50.0, "widest choir swing is {cents:.1} cents");
    }

    #[test]
    fn the_choir_holds_its_copies_sharp_and_flat() {
        // One copy a side at full depth: the left's is 15 cents sharp, the
        // right's as flat, and held there, not swept through. Counted from
        // upward zero crossings of a steady 440 Hz, wet only.
        let mut c = Chorus::new(SR);
        let p = ChorusParams {
            kind: ChorusType::Choir,
            voices: 1.0,
            ..wet(1.0, 0.05, 0.0)
        };
        let a440 = |i: usize| (core::f32::consts::TAU * 440.0 * i as f32 / SR).sin() * 0.5;
        let (mut prev, mut ups, mut n) = ((0.0f32, 0.0f32), (0usize, 0usize), 0usize);
        let settle = SR as usize;
        for i in 0..settle * 5 {
            let (l, r) = c.process(a440(i), a440(i), &p);
            if i > settle {
                ups.0 += (prev.0 < 0.0 && l >= 0.0) as usize;
                ups.1 += (prev.1 < 0.0 && r >= 0.0) as usize;
                n += 1;
            }
            prev = (l, r);
        }
        let hz = |ups: usize| ups as f32 * SR / n as f32;
        let (left, right) = (hz(ups.0), hz(ups.1));
        let expect = 440.0 * (0.6 * CHOIR_DETUNE_CENTS / 1200.0).exp2();
        assert!(
            (left - expect).abs() < 1.0,
            "left at {left} Hz, want {expect}"
        );
        assert!(
            (right - 440.0 * 440.0 / expect).abs() < 1.0,
            "right at {right} Hz"
        );
    }

    #[test]
    fn the_choir_is_a_different_sound_from_the_ensemble() {
        let run = |kind| {
            let mut c = Chorus::new(SR);
            let p = ChorusParams {
                kind,
                ..wet(0.5, 0.6, 0.0)
            };
            (0..48_000)
                .map(|i| c.process(tone(i), tone(i), &p).0)
                .collect::<Vec<_>>()
        };
        let (a, b) = (run(ChorusType::Ensemble), run(ChorusType::Choir));
        let diff = a
            .iter()
            .zip(&b)
            .skip(4_800)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(diff > 0.1, "choir and ensemble came out alike: {diff}");
    }

    /// Correlation of the two channels' wet signals, from a mono noise input.
    fn side_correlation(p: &ChorusParams) -> f64 {
        let mut c = Chorus::new(SR);
        let mut seed = 0x5eed_1234;
        let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..96_000 {
            let x = noise(&mut seed) * 0.5;
            let (l, r) = c.process(x, x, p);
            if i > 9_600 {
                ll += (l * l) as f64;
                rr += (r * r) as f64;
                lr += (l * r) as f64;
            }
        }
        lr / (ll * rr).sqrt()
    }

    #[test]
    fn the_ensemble_decorrelates_the_sides_more_than_the_chorus() {
        // The ensemble's whole claim to width: the same mono source comes out
        // less alike on the two sides than the plain chorus makes it.
        let [plain, ens] = both(1.0, 0.6, 0.0).map(|p| side_correlation(&p).abs());
        assert!(
            ens < plain,
            "ensemble {ens:.3} is not below chorus {plain:.3}"
        );
        assert!(ens < 0.2, "ensemble sides still correlate at {ens:.3}");
    }

    #[test]
    fn the_ensemble_keeps_the_lows_out_of_the_wet() {
        // A low sine, wet only: the high-pass takes most of it away, where the
        // plain chorus passes it. That is the low end staying dry.
        let energy = |p: &ChorusParams| {
            let mut c = Chorus::new(SR);
            let mut e = 0.0f64;
            for i in 0..48_000 {
                let x = (core::f32::consts::TAU * 50.0 * i as f32 / SR).sin() * 0.5;
                let (l, _) = c.process(x, x, p);
                if i > 9_600 {
                    e += (l * l) as f64;
                }
            }
            e
        };
        let [plain, ens] = both(0.5, 0.6, 0.0).map(|p| energy(&p));
        assert!(ens < plain * 0.1, "lows: ensemble {ens} chorus {plain}");
    }

    #[test]
    fn the_ensemble_is_a_different_sound_from_the_chorus() {
        let run = |p: &ChorusParams| {
            let mut c = Chorus::new(SR);
            (0..24_000)
                .map(|i| c.process(tone(i), tone(i), p).0)
                .collect::<Vec<_>>()
        };
        let [a, b] = both(0.5, 0.6, 0.0).map(|p| run(&p));
        let apart = a[12_000..]
            .iter()
            .zip(&b[12_000..])
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(apart > 0.1, "the voicings sound the same: {apart}");
    }

    #[test]
    fn sweeping_the_voices_does_not_click() {
        // A voice fades in as the control turns, so jumping from one to all
        // of them and back steps no more than holding any count.
        for kind in ChorusType::ALL {
            let worst = |sweep: bool, hold: f32| {
                let mut c = Chorus::new(SR);
                let mut prev = 0.0f32;
                let mut worst = 0.0f32;
                for i in 0..96_000 {
                    let voices = if sweep {
                        // Up and back once a second, with jumps at the turns.
                        if (i / 4_800) % 2 == 0 {
                            1.0
                        } else {
                            MAX_VOICES as f32
                        }
                    } else {
                        hold
                    };
                    let p = ChorusParams {
                        kind,
                        voices,
                        ..wet(0.5, 0.6, 0.0)
                    };
                    let (l, _) = c.process(tone(i), tone(i), &p);
                    if i > 4_800 {
                        worst = worst.max((l - prev).abs());
                    }
                    prev = l;
                }
                worst
            };
            // Some counts between beat harder than either end, so the
            // baseline is the worst of holding any of them.
            let steady = (1..=MAX_VOICES)
                .map(|v| worst(false, v as f32))
                .fold(0.0, f32::max);
            let swept = worst(true, 0.0);
            assert!(
                swept < steady * 1.2,
                "{kind:?}: sweeping voices jumped {swept} vs {steady}"
            );
        }
    }

    #[test]
    fn a_voice_only_adds() {
        // Turning voices up must keep the voices already sounding where they
        // were: the wet at 2.5 voices lies between 2 and 3, not somewhere
        // new. Checked on the unnormalised sums, so it is the voices and not
        // the level correction being tested.
        let c = {
            let mut c = Chorus::new(SR);
            let mut seed = 0x5eed;
            for _ in 0..4_800 {
                let x = noise(&mut seed);
                c.process(x, x, &ChorusParams::default());
            }
            c
        };
        let sweep = SWEEP_MS * 0.5 * 0.001 * SR;
        let at = |v: f32| c.voices(0, 0.0, sweep, &Chorus::weights(v).0);
        let (two, half, three) = (at(2.0), at(2.5), at(3.0));
        assert!(
            (half - (two + three) / 2.0).abs() < 1e-5,
            "{two} {half} {three}"
        );
        let e = |v: f32| c.ensemble_side(0, 50.0, 5.0, &Chorus::weights(v).0);
        let (two, half, three) = (e(2.0), e(2.5), e(3.0));
        assert!(
            (half - (two + three) / 2.0).abs() < 1e-5,
            "{two} {half} {three}"
        );
    }

    #[test]
    fn more_voices_smooth_the_beating() {
        // One copy against the dry combs hard, and the comb sweeps, so a
        // steady tone's level swings. More copies average that out: the
        // swing in level shrinks as voices are added.
        let swing = |voices: f32| {
            let mut c = Chorus::new(SR);
            let p = ChorusParams {
                voices,
                mix: 0.5,
                // The tone sits under the default low cut; open it.
                low_cut_hz: LOW_CUT_MIN_HZ,
                ..wet(1.0, 0.6, 0.0)
            };
            let (mut lo, mut hi) = (f32::MAX, 0.0f32);
            let mut acc = 0.0f32;
            for i in 0..(SR as usize * 4) {
                let (l, _) = c.process(tone(i), tone(i), &p);
                acc += l * l;
                // Short-term level every 10 ms, past the settle.
                if i % 480 == 479 {
                    if i > 9_600 {
                        lo = lo.min(acc);
                        hi = hi.max(acc);
                    }
                    acc = 0.0;
                }
            }
            hi / lo.max(1e-9)
        };
        let (one, eight) = (swing(1.0), swing(MAX_VOICES as f32));
        assert!(eight < one, "one voice swings {one}, eight swing {eight}");
    }

    /// When a click through one copy first comes out, in frames.
    fn arrival(kind: ChorusType, spread: f32) -> usize {
        let mut c = Chorus::new(SR);
        let p = ChorusParams {
            kind,
            voices: 1.0,
            spread,
            low_cut_hz: LOW_CUT_MIN_HZ,
            ..wet(0.0, 0.05, 0.0)
        };
        // Let the spread settle on silence, then click.
        for _ in 0..SR as usize {
            c.process(0.0, 0.0, &p);
        }
        (0..SR as usize)
            .position(|i| {
                let x = if i == 0 { 1.0 } else { 0.0 };
                c.process(x, x, &p).0.abs() > 0.05
            })
            .expect("the click came out")
    }

    #[test]
    fn spread_moves_the_copies() {
        // Pushed out, every voicing's copies arrive later; pulled in,
        // sooner. The choir's shifter window does not scale, so its copies
        // move by less than the others'.
        for kind in ChorusType::ALL {
            let (near, far) = (arrival(kind, SPREAD_MIN), arrival(kind, SPREAD_MAX));
            assert!(
                far as f32 > near as f32 * 1.8,
                "{kind:?}: {near} frames pulled in, {far} pushed out"
            );
        }
    }

    #[test]
    fn a_jump_in_spread_glides_rather_than_steps() {
        // Spread moves every read point, so a jump in it bends the pitch
        // on the way, as a tape delay's time does. Smoothed, the bend stays
        // modest, and nothing steps.
        for kind in ChorusType::ALL {
            let worst = |first: f32, second: f32| {
                let mut c = Chorus::new(SR);
                let mut prev = 0.0f32;
                let mut worst = 0.0f32;
                for i in 0..96_000 {
                    let spread = if (i / 24_000) % 2 == 0 { first } else { second };
                    let p = ChorusParams {
                        kind,
                        spread,
                        ..wet(0.5, 0.6, 0.0)
                    };
                    let (l, _) = c.process(tone(i), tone(i), &p);
                    if i > 4_800 {
                        worst = worst.max((l - prev).abs());
                    }
                    prev = l;
                }
                worst
            };
            let steady = worst(SPREAD_MIN, SPREAD_MIN).max(worst(SPREAD_MAX, SPREAD_MAX));
            let jumped = worst(SPREAD_MIN, SPREAD_MAX);
            assert!(
                jumped < steady * 2.0,
                "{kind:?}: a spread jump stepped {jumped} vs {steady}"
            );
        }
    }

    #[test]
    fn low_cut_takes_the_bass_out_of_the_copies() {
        // A 60 Hz tone, wet only: with the cut at the bottom the copies keep
        // it, at the top they lose most of it. In every voicing.
        for kind in ChorusType::ALL {
            let level = |low_cut_hz: f32| {
                let mut c = Chorus::new(SR);
                let p = ChorusParams {
                    kind,
                    low_cut_hz,
                    ..wet(0.5, 0.6, 0.0)
                };
                let mut sum = 0.0f64;
                for i in 0..96_000 {
                    let x = (core::f32::consts::TAU * 60.0 * i as f32 / SR).sin() * 0.5;
                    let (l, _) = c.process(x, x, &p);
                    if i > 9_600 {
                        sum += (l * l) as f64;
                    }
                }
                sum
            };
            let (open, cut) = (level(LOW_CUT_MIN_HZ), level(LOW_CUT_MAX_HZ));
            assert!(cut < open * 0.05, "{kind:?}: open {open}, cut {cut}");
        }
    }

    #[test]
    fn switching_voicing_does_not_click() {
        // A hard cut between two different wet signals would step; the
        // crossfade keeps the worst jump near what either voicing makes by
        // itself, both ways round.
        let worst = |first: ChorusType, second: ChorusType| {
            let mut c = Chorus::new(SR);
            let mut prev = 0.0f32;
            let mut worst = 0.0f32;
            for i in 0..48_000 {
                let kind = if i < 24_000 { first } else { second };
                let p = ChorusParams {
                    kind,
                    ..wet(0.5, 0.6, 0.0)
                };
                let (l, _) = c.process(tone(i), tone(i), &p);
                if i > 4_800 {
                    worst = worst.max((l - prev).abs());
                }
                prev = l;
            }
            worst
        };
        let steady = worst(ChorusType::Chorus, ChorusType::Chorus)
            .max(worst(ChorusType::Ensemble, ChorusType::Ensemble));
        let steady = ChorusType::ALL
            .into_iter()
            .map(|k| worst(k, k))
            .fold(steady, f32::max);
        let pairs = ChorusType::ALL
            .into_iter()
            .flat_map(|a| ChorusType::ALL.into_iter().map(move |b| (a, b)))
            .filter(|(a, b)| a != b);
        for (a, b) in pairs {
            let swept = worst(a, b);
            assert!(
                swept < steady * 1.5,
                "{a:?} to {b:?} jumped {swept} vs {steady}"
            );
        }
    }

    #[test]
    fn eq_brightens_and_darkens_the_wet() {
        // Energy above the corner against the flat setting: up adds, down
        // takes away. A first difference is a crude high-pass, enough here.
        let highs = |eq_db: f32| {
            let mut c = Chorus::new(SR);
            let p = wet(0.5, 0.6, eq_db);
            let mut seed = 0x0bad_cafe;
            let (mut prev, mut energy) = (0.0f32, 0.0f64);
            for i in 0..48_000 {
                let x = noise(&mut seed) * 0.5;
                let (l, _) = c.process(x, x, &p);
                if i > 4_800 {
                    energy += ((l - prev) * (l - prev)) as f64;
                }
                prev = l;
            }
            energy
        };
        let (down, flat, up) = (highs(-EQ_RANGE_DB), highs(0.0), highs(EQ_RANGE_DB));
        assert!(up > flat * 2.0, "up {up} flat {flat}");
        assert!(down < flat * 0.5, "down {down} flat {flat}");
    }

    #[test]
    fn a_swept_parameter_does_not_jump() {
        // Depth and rate move the read points, so an unsmoothed step in
        // either would click. The baseline is the worst of holding either
        // setting steady, since the copies beat harder at full depth.
        let slow = (0.1, 0.1);
        let fast = (1.0, MAX_RATE_HZ);
        for kind in ChorusType::ALL {
            let worst = |first: (f32, f32), second: (f32, f32)| {
                let mut c = Chorus::new(SR);
                let mut prev = 0.0f32;
                let mut worst = 0.0f32;
                for i in 0..96_000 {
                    let (depth, rate) = if (i / 4_800) % 2 == 0 { first } else { second };
                    let p = ChorusParams {
                        kind,
                        ..wet(depth, rate, 0.0)
                    };
                    let (l, _) = c.process(tone(i), tone(i), &p);
                    if i > 4_800 {
                        worst = worst.max((l - prev).abs());
                    }
                    prev = l;
                }
                worst
            };
            let steady = worst(slow, slow).max(worst(fast, fast));
            let swept = worst(fast, slow);
            assert!(
                swept < steady * 1.2,
                "{kind:?}: a parameter step jumped by {swept} against a steady {steady}"
            );
        }
    }

    #[test]
    fn works_at_other_sample_rates() {
        for sr in [22_050.0, 44_100.0, 96_000.0, 192_000.0] {
            for p in every(1.0, MAX_RATE_HZ, EQ_RANGE_DB) {
                let mut c = Chorus::new(sr);
                for i in 0..10_000 {
                    let x = (i as f32 * 0.05).sin();
                    let (l, r) = c.process(x, x, &p);
                    assert!(l.is_finite() && r.is_finite());
                }
            }
        }
    }
}
