//! Chorus.
//!
//! A short delay line a channel, read at three points a couple of
//! milliseconds apart. Each read point has its own LFO sweeping it back and
//! forth, a third of a turn from the next.
//! A moving read point bends the pitch of its copy slightly, and three copies
//! drifting sharp and flat against the original read as an ensemble rather
//! than one voice. The right channel's LFOs sit a sixth of a turn from the
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
//! Eminent): four copies a side, spread from 9 to 25 ms, each swept by a slow
//! LFO for the swirl and a small fast one, about six times a second, for the
//! shimmer. The two sides read the mono sum rather than their own channel, and
//! the right sweeps against the left, so a centred source is pulled apart into
//! a wall rather than doubled. A gentle high-pass on the copies keeps the low
//! end dry and focused; the lushness lives above it. Switching between the
//! two voicings crossfades, and only the voicing being heard is computed.
//!
//! At zero mix the node is a bit-exact bypass. The line keeps filling, so
//! switching on never plays stale audio.

use crate::smooth::OnePole;

/// The centre of the middle voice's sweep, in milliseconds. Long enough to
/// separate the copies from the dry signal, short enough not to be an echo.
const CENTRE_MS: f32 = 10.0;
/// How far apart the voices' centres sit. Apart, they are three different
/// copies even at zero depth, so their equal-power sum holds its level.
const SPREAD_MS: f32 = 2.0;
/// The widest a read point strays from the centre, at full depth.
const SWEEP_MS: f32 = 3.0;
/// The fastest LFO. `chorus.rate` stops here, and a test holds the pitch
/// swing this allows at full depth to under a couple of semitones.
pub const MAX_RATE_HZ: f32 = 5.0;
/// Voices a channel, in the plain chorus.
const VOICES: usize = 3;

/// Voices a side in the ensemble. Four a side is eight reads a frame, which
/// the block timer shows is cheap next to the drive's oversampling.
const ENS_VOICES: usize = 4;
/// Where the ensemble's copies sit, in milliseconds: the left's, then the
/// right's, interleaved between them so no two sides share a delay.
const ENS_CENTRES_MS: [[f32; ENS_VOICES]; 2] =
    [[9.0, 13.5, 18.0, 22.5], [11.25, 15.75, 20.25, 24.75]];
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
/// Where the ensemble's high-pass turns, in hertz. Below it the wet is left
/// out, so the bass stays dry rather than swimming.
const ENS_HIGHPASS_HZ: f32 = 240.0;
/// Room for the latest ensemble copy plus both sweeps, and a margin for the
/// interpolator's four points.
const MAX_MS: f32 = ENS_CENTRES_MS[1][ENS_VOICES - 1] + ENS_SLOW_MS + ENS_FAST_MS + 2.0;
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
    /// Three copies a side, each channel from its own input. The classic.
    Chorus,
    /// Four copies a side from the mono sum, slow swirl plus fast shimmer,
    /// the low end left dry. Angelic.
    Ensemble,
}

impl ChorusType {
    pub const ALL: [ChorusType; 2] = [ChorusType::Chorus, ChorusType::Ensemble];
    pub const NAMES: [&'static str; 2] = ["Chorus", "Ensemble"];

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
            eq_db: 0.0,
            mix: 0.0,
        }
    }
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
    /// Zero is the plain chorus, one the ensemble, between while switching.
    ensemble: OnePole,
    /// The ensemble's high-pass is the wet less this low-pass, a side each.
    hp_lows: [f32; 2],
    hp_coef: f32,
    /// The EQ's one-pole low-pass, one a channel: the shelf is the lows plus
    /// the highs scaled.
    lows: [f32; 2],
    lows_coef: f32,
    rate: OnePole,
    depth: OnePole,
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
            ensemble: smoother(30.0, 0.0),
            hp_lows: [0.0; 2],
            hp_coef: 1.0 - (-core::f32::consts::TAU * ENS_HIGHPASS_HZ / sample_rate).exp(),
            lows: [0.0; 2],
            lows_coef: 1.0 - (-core::f32::consts::TAU * EQ_CORNER_HZ / sample_rate).exp(),
            // Rate and depth move the read points, so a step in either is a
            // pitch jump. Slower than the house 20 ms for that reason.
            rate: smoother(50.0, d.rate),
            depth: smoother(50.0, d.depth),
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

    /// One channel's voices, summed at equal power. `offset` is where its
    /// first LFO sits in the turn.
    #[inline]
    fn voices(&self, ch: usize, offset: f32, sweep: f32) -> f32 {
        let ms = 0.001 * self.sample_rate;
        let mut sum = 0.0;
        for v in 0..VOICES {
            let turn = self.phase + offset + v as f32 / VOICES as f32;
            let lfo = (core::f32::consts::TAU * turn).sin();
            let centre = (CENTRE_MS + (v as f32 - 1.0) * SPREAD_MS) * ms;
            sum += self.tap(ch, centre + sweep * lfo);
        }
        sum / (VOICES as f32).sqrt()
    }

    /// One side of the ensemble, from the mono sum. Each copy's read point is
    /// its centre plus the slow sweep and the fast one; the slow LFOs sit a
    /// quarter turn apart and the right's a half turn from the left's, so the
    /// sides sweep against each other. The fast LFOs are spread by an
    /// irrational-ish step so no two copies flutter together.
    #[inline]
    fn ensemble_side(&self, side: usize, slow: f32, fast: f32) -> f32 {
        let ms = 0.001 * self.sample_rate;
        let mut sum = 0.0;
        for (v, centre) in ENS_CENTRES_MS[side].iter().enumerate() {
            let s_turn = self.phase + 0.5 * side as f32 + v as f32 / ENS_VOICES as f32;
            let f_turn = self.fast_phase + 0.37 * v as f32 + 0.21 * side as f32;
            let sweep = slow * (core::f32::consts::TAU * s_turn).sin()
                + fast * (core::f32::consts::TAU * f_turn).sin();
            sum += self.tap(MID, centre * ms + sweep);
        }
        sum / (ENS_VOICES as f32).sqrt()
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &ChorusParams) -> (f32, f32) {
        let rate = self.rate.process(p.rate.clamp(0.0, MAX_RATE_HZ));
        let depth = self.depth.process(p.depth.clamp(0.0, 1.0));
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
        let target = if p.kind == ChorusType::Ensemble {
            1.0
        } else {
            0.0
        };
        let ens = self.ensemble.process(target);
        if mix == 0.0 {
            return (l, r);
        }

        // Only the voicing being heard is computed, or both while one fades
        // into the other.
        let mut wet = [0.0; 2];
        if ens < 0.999 {
            let sweep = SWEEP_MS * depth * 0.001 * self.sample_rate;
            let offset_r = 0.5 / VOICES as f32;
            wet = [self.voices(0, 0.0, sweep), self.voices(1, offset_r, sweep)];
        }
        if ens > 0.001 {
            let k = 0.001 * self.sample_rate * depth;
            let (slow, fast) = (ENS_SLOW_MS * k, ENS_FAST_MS * k);
            for (side, w) in wet.iter_mut().enumerate() {
                let raw = self.ensemble_side(side, slow, fast);
                self.hp_lows[side] += self.hp_coef * (raw - self.hp_lows[side]);
                let e = raw - self.hp_lows[side];
                *w = if ens < 0.999 { *w + ens * (e - *w) } else { e };
            }
        } else {
            self.hp_lows = [0.0; 2];
        }
        let mut out = [0.0; 2];
        for ch in 0..2 {
            self.lows[ch] += self.lows_coef * (wet[ch] - self.lows[ch]);
            let shaped = self.lows[ch] + (wet[ch] - self.lows[ch]) * eq_gain;
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

    /// Both voicings, for a contract that has to hold in each.
    fn both(depth: f32, rate: f32, eq_db: f32) -> [ChorusParams; 2] {
        [wet(depth, rate, eq_db), ensemble(depth, rate, eq_db)]
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
    fn silence_in_silence_out() {
        for p in both(1.0, 2.0, EQ_RANGE_DB) {
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
        for (depth, p) in [0.0, 0.5, 1.0]
            .into_iter()
            .flat_map(|d| both(d, 0.6, 0.0).map(|p| (d, p)))
        {
            let mut c = Chorus::new(SR);
            let mut seed = 0x1234_5678;
            let (mut dry, mut out) = (0.0f64, 0.0f64);
            for i in 0..96_000 {
                let x = noise(&mut seed) * 0.5;
                let (l, _) = c.process(x, x, &p);
                if i > 4_800 {
                    dry += (x * x) as f64;
                    out += (l * l) as f64;
                }
            }
            let db = 10.0 * (out / dry).log10();
            assert!(
                db.abs() < 1.5,
                "{:?} depth {depth}: wet is {db:.2} dB off dry",
                p.kind
            );
        }
    }

    #[test]
    fn output_is_bounded_at_the_extremes() {
        // Copies at equal power can line up, so a full-scale input can reach
        // √3 of itself (√4 in the ensemble), plus the EQ's boost. Never more,
        // and never anything that is not a number.
        let bound = 4.0f32.sqrt() * 10f32.powf(EQ_RANGE_DB / 20.0) * 1.2;
        for p in both(1.0, MAX_RATE_HZ, EQ_RANGE_DB) {
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
        for (a, b) in [
            (ChorusType::Chorus, ChorusType::Ensemble),
            (ChorusType::Ensemble, ChorusType::Chorus),
        ] {
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
            for p in both(1.0, MAX_RATE_HZ, EQ_RANGE_DB) {
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
