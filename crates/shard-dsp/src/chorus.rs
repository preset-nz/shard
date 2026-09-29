//! Chorus.
//!
//! A short delay line a channel, read at three points a couple of
//! milliseconds apart. Each read point has
//! its own LFO sweeping it back and forth, a third of a turn from the next.
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
/// Room for the latest voice's centre plus the widest sweep, and a margin
/// for the interpolator's four points.
const MAX_MS: f32 = CENTRE_MS + SPREAD_MS + SWEEP_MS + 2.0;
/// Voices a channel.
const VOICES: usize = 3;
/// Where the EQ's shelf turns, in hertz.
const EQ_CORNER_HZ: f32 = 1_500.0;
/// The EQ's reach either way, in decibels.
pub const EQ_RANGE_DB: f32 = 12.0;

#[derive(Debug, Clone, Copy)]
pub struct ChorusParams {
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
    sample_rate: f32,
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
            line: vec![0.0; frames * 2],
            write: 0,
            frames,
            phase: 0.0,
            sample_rate,
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
        let at = |back: usize| self.line[((self.write + n - back) % n) * 2 + ch];
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
        self.line[self.write * 2] = l;
        self.line[self.write * 2 + 1] = r;
        self.phase += rate / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
        if mix == 0.0 {
            return (l, r);
        }

        let sweep = SWEEP_MS * depth * 0.001 * self.sample_rate;
        let offset_r = 0.5 / VOICES as f32;
        let wet = [self.voices(0, 0.0, sweep), self.voices(1, offset_r, sweep)];
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
            rate,
            depth,
            eq_db,
            mix: 1.0,
        }
    }

    #[test]
    fn a_mix_faded_to_zero_ends_exactly_transparent() {
        let mut c = Chorus::new(SR);
        let on = ChorusParams {
            mix: 1.0,
            ..Default::default()
        };
        let off = ChorusParams::default();
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

    #[test]
    fn silence_in_silence_out() {
        let mut c = Chorus::new(SR);
        let p = wet(1.0, 2.0, EQ_RANGE_DB);
        for _ in 0..10_000 {
            assert_eq!(c.process(0.0, 0.0, &p), (0.0, 0.0));
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
        for depth in [0.0, 0.5, 1.0] {
            let mut c = Chorus::new(SR);
            let p = wet(depth, 0.6, 0.0);
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
            assert!(db.abs() < 1.5, "depth {depth}: wet is {db:.2} dB off dry");
        }
    }

    #[test]
    fn output_is_bounded_at_the_extremes() {
        // Three copies at equal power can line up, so a full-scale input can
        // reach √3 of itself, plus the EQ's boost. Never more, and never
        // anything that is not a number.
        let bound = 3.0f32.sqrt() * 10f32.powf(EQ_RANGE_DB / 20.0) * 1.2;
        let mut c = Chorus::new(SR);
        let p = wet(1.0, MAX_RATE_HZ, EQ_RANGE_DB);
        for i in 0..96_000 {
            let x = if i % 200 < 100 { 1.0 } else { -1.0 };
            let (l, r) = c.process(x, -x, &p);
            assert!(l.abs() <= bound && r.abs() <= bound, "{l} {r}");
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

        // And at the defaults, about a Juno's: under twenty cents.
        let d = ChorusParams::default();
        let swing = core::f32::consts::TAU * d.rate * SWEEP_MS * d.depth * 0.001;
        let cents = 1200.0 * (1.0 + swing).log2();
        assert!(cents < 20.0, "default swing is {cents:.1} cents");
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
        let worst = |first: (f32, f32), second: (f32, f32)| {
            let mut c = Chorus::new(SR);
            let mut prev = 0.0f32;
            let mut worst = 0.0f32;
            for i in 0..96_000 {
                let (depth, rate) = if (i / 4_800) % 2 == 0 { first } else { second };
                let (l, _) = c.process(tone(i), tone(i), &wet(depth, rate, 0.0));
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
            "a parameter step jumped by {swept} against a steady {steady}"
        );
    }

    #[test]
    fn works_at_other_sample_rates() {
        for sr in [22_050.0, 44_100.0, 96_000.0, 192_000.0] {
            let mut c = Chorus::new(sr);
            let p = wet(1.0, MAX_RATE_HZ, EQ_RANGE_DB);
            for i in 0..10_000 {
                let x = (i as f32 * 0.05).sin();
                let (l, r) = c.process(x, x, &p);
                assert!(l.is_finite() && r.is_finite());
            }
        }
    }
}
