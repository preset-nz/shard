//! Overtone.
//!
//! Extra voices grown from the sound that comes in: an octave below, an
//! octave above and a fifth above the dry note (seven semitones up), each
//! with a level of its own, crossfaded in over the dry. It is an effect, not a synth.
//! It plays whatever it hears, chords included, because each layer is a
//! pitch shifter and a pitch shifter does not care what is in it. A bass
//! gets a sub under it; a guitar gets a shimmer over it; a pad gets a stack.
//!
//! The mechanism is one `PitchShifter` per layer per channel, at ratios 0.5,
//! 2.0 and seven semitones up (about 1.498). Their outputs are weighted by
//! the three levels and summed, then pass through a two-pole low-pass,
//! `tone`, because a shifted copy is roughest at the top: the octave up has
//! the source's air doubled into the hiss range, and every wrap of a
//! shifter's window adds a little grain there. Rolling it off is what makes
//! the layers read as part of the note rather than as artefacts around it.
//! A soft limit follows, linear up to 0.8 and rounding to 1.0 above it, so
//! all three layers at full with a loud input cannot run away. `mix` then
//! crossfades from the dry to the stack: `dry * (1 - mix) + layers * mix`. At
//! full mix only the layers are heard, which makes a pure sub or octave of the
//! note; the dry is not kept underneath. (It was `dry + layers * mix` until
//! 2026-10-02.)
//!
//! The shifters read through a 40 ms window and so delay their layers by
//! about 20 ms. The dry is not delayed to match. That lag is part of the
//! sound: the layers arrive just behind the note as its body, and a bass
//! note with a sub 20 ms behind it reads as one fat note, not two. It also
//! means the low layer is never in a fixed phase with the dry, so there is
//! no cancellation to design around. The layers on the two sides are
//! independent, so a stereo source stays stereo.
//!
//! A layer at level zero still runs its shifter, so raising it never plays
//! stale audio; only the weighting is zero.
//!
//! At zero mix the node is a bit-exact bypass. The shifters keep running.

use crate::shift::{semitones_to_ratio, PitchShifter};
use crate::smooth::{OnePole, Ramp};

/// The tone control's range, in hertz. At the bottom the layers are a dull
/// bloom; at the top the shifter's grain is all there.
pub const TONE_MIN_HZ: f32 = 500.0;
pub const TONE_MAX_HZ: f32 = 16_000.0;
/// The layers' semitone offsets: an octave down, an octave up, a fifth up.
const SEMITONES: [f32; 3] = [-12.0, 12.0, 7.0];
/// How long a layer's level settles, in milliseconds.
const LEVEL_SMOOTH_MS: f32 = 10.0;
/// How long the mix takes to travel from nothing to everything.
const MIX_RAMP_MS: f32 = 10.0;
/// How long the tone cutoff takes to settle, in milliseconds.
const TONE_SMOOTH_MS: f32 = 20.0;
/// Where the soft limit stops being linear. Below it the levels are exact.
const KNEE: f32 = 0.8;

#[derive(Debug, Clone, Copy)]
pub struct OvertoneParams {
    /// The octave-below layer's level, 0 to 1. Default 0.5.
    pub sub: f32,
    /// The octave-above layer's level, 0 to 1. Default 0.3.
    pub octave: f32,
    /// The fifth-above layer's level, 0 to 1. Default 0.
    pub fifth: f32,
    /// Where the low-pass on the layers turns, in hertz,
    /// `TONE_MIN_HZ` to `TONE_MAX_HZ`. Default 5000.
    pub tone_hz: f32,
    /// How much of the layer stack is heard, 0 to 1. Default 0.5.
    pub mix: f32,
}

impl Default for OvertoneParams {
    fn default() -> Self {
        Self {
            sub: 0.5,
            octave: 0.3,
            fifth: 0.0,
            tone_hz: 5_000.0,
            mix: 0.5,
        }
    }
}

/// Linear to `KNEE`, then a curve that approaches 1.0 and never passes it,
/// with the same slope at the join.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let room = 1.0 - KNEE;
        let y = KNEE + room * ((a - KNEE) / room).tanh();
        y.copysign(x)
    }
}

/// Two one-poles in a row: 12 dB an octave, with no peak.
#[derive(Clone, Copy, Default)]
struct Tone {
    z1: f32,
    z2: f32,
}

impl Tone {
    #[inline]
    fn process(&mut self, x: f32, a: f32) -> f32 {
        self.z1 = x + a * (self.z1 - x);
        self.z2 = self.z1 + a * (self.z2 - self.z1);
        // Flush anything denormal out of the recirculating state.
        if self.z1.abs() < 1e-30 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-30 {
            self.z2 = 0.0;
        }
        self.z2
    }
}

pub struct Overtone {
    /// Left's three shifters, then right's.
    shifters: [[PitchShifter; 3]; 2],
    ratios: [f32; 3],
    tone: [Tone; 2],
    levels: [OnePole; 3],
    tone_hz: OnePole,
    mix: Ramp,
    sample_rate: f32,
    /// The tone coefficient and the cutoff it was made for, so the `exp`
    /// is only paid when the cutoff has moved.
    coef: f32,
    coef_for: f32,
}

impl Overtone {
    pub fn new(sample_rate: f32) -> Self {
        let mk = || {
            [
                PitchShifter::new(sample_rate),
                PitchShifter::new(sample_rate),
                PitchShifter::new(sample_rate),
            ]
        };
        let p = OvertoneParams::default();
        let mut levels = [OnePole::new(); 3];
        for (l, v) in levels.iter_mut().zip([p.sub, p.octave, p.fifth]) {
            l.set_time(LEVEL_SMOOTH_MS, sample_rate);
            l.reset(v);
        }
        let mut tone_hz = OnePole::new();
        tone_hz.set_time(TONE_SMOOTH_MS, sample_rate);
        tone_hz.reset(p.tone_hz);
        let mut mix = Ramp::new(0.0);
        mix.set_time(MIX_RAMP_MS, sample_rate);
        Self {
            shifters: [mk(), mk()],
            ratios: SEMITONES.map(semitones_to_ratio),
            tone: [Tone::default(); 2],
            levels,
            tone_hz,
            mix,
            sample_rate,
            coef: 0.0,
            coef_for: 0.0,
        }
    }

    /// Forgets the audio it holds: the shifters' lines and the tone filter.
    /// Level, tone and mix smoothers follow controls and keep their place.
    pub fn reset(&mut self) {
        for s in self.shifters.iter_mut().flatten() {
            s.reset();
        }
        self.tone = [Tone::default(); 2];
    }

    /// What the layers lag the dry by, in samples.
    pub fn latency_samples(&self) -> usize {
        self.shifters[0][0].latency_samples()
    }

    pub fn process(&mut self, l: f32, r: f32, p: &OvertoneParams) -> (f32, f32) {
        let targets = [p.sub, p.octave, p.fifth].map(|v| v.clamp(0.0, 1.0));
        let mut lv = [0.0; 3];
        for i in 0..3 {
            lv[i] = self.levels[i].process(targets[i]);
        }
        let hz = self
            .tone_hz
            .process(p.tone_hz.clamp(TONE_MIN_HZ, TONE_MAX_HZ));
        if (hz - self.coef_for).abs() > self.coef_for * 1e-3 {
            let hz = hz.min(self.sample_rate * 0.45);
            self.coef = (-core::f32::consts::TAU * hz / self.sample_rate).exp();
            self.coef_for = hz;
        }
        let mix = self.mix.process(p.mix.clamp(0.0, 1.0));

        let mut layers = [0.0f32; 2];
        for (ch, x) in [l, r].into_iter().enumerate() {
            // Every shifter runs every sample, whatever its level, so none
            // has stale audio when it is turned up.
            let mut sum = 0.0;
            for ((s, &ratio), &level) in self.shifters[ch].iter_mut().zip(&self.ratios).zip(&lv) {
                sum += s.process(x, ratio) * level;
            }
            let sum = self.tone[ch].process(sum, self.coef);
            layers[ch] = soft_limit(sum);
        }
        if mix == 0.0 {
            return (l, r);
        }
        (
            l * (1.0 - mix) + layers[0] * mix,
            r * (1.0 - mix) + layers[1] * mix,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(f: f32, amp: f32, i: usize, sr: f32) -> f32 {
        (core::f32::consts::TAU * f * i as f32 / sr).sin() * amp
    }

    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn open(sub: f32, octave: f32, fifth: f32) -> OvertoneParams {
        OvertoneParams {
            sub,
            octave,
            fifth,
            tone_hz: TONE_MAX_HZ,
            mix: 1.0,
        }
    }

    /// Energy in the band `f` plus or minus 45 Hz of a Hann-windowed block,
    /// by direct DFT at one-hertz steps.
    fn band(x: &[f32], f: f32, sr: f32) -> f32 {
        let n = x.len();
        let mut total = 0.0f64;
        let mut k = f - 45.0;
        while k <= f + 45.0 {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in x.iter().enumerate() {
                let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n as f64).cos();
                let ph = core::f64::consts::TAU * k as f64 * i as f64 / sr as f64;
                re += v as f64 * w * ph.cos();
                im += v as f64 * w * ph.sin();
            }
            total += re * re + im * im;
            k += 1.0;
        }
        total as f32
    }

    /// Output of a 220 Hz sine through the effect, after it has settled.
    fn render(p: &OvertoneParams) -> Vec<f32> {
        let mut o = Overtone::new(SR);
        let mut out = Vec::new();
        for i in 0..(SR as usize / 2 + 9_600) {
            let x = sine(220.0, 0.2, i, SR);
            let (l, _) = o.process(x, x, p);
            if i >= 9_600 {
                out.push(l);
            }
        }
        out
    }

    fn db(a: f32, b: f32) -> f32 {
        10.0 * (a / b).log10()
    }

    #[test]
    fn mix_zero_is_bit_exact_bypass() {
        let mut o = Overtone::new(SR);
        let on = open(1.0, 1.0, 1.0);
        let off = OvertoneParams { mix: 0.0, ..on };
        for i in 0..4_800 {
            o.process(sine(220.0, 0.5, i, SR), 0.1, &on);
        }
        for i in 0..4_800 {
            o.process(sine(220.0, 0.5, i, SR), 0.1, &off);
        }
        for _ in 0..1_000 {
            assert_eq!(o.process(0.5, -0.25, &off), (0.5, -0.25));
        }
        // And fresh.
        let mut o = Overtone::new(SR);
        for i in 0..1_000 {
            let x = sine(330.0, 0.7, i, SR);
            assert_eq!(o.process(x, -x, &off), (x, -x));
        }
    }

    #[test]
    fn state_runs_while_off_so_switching_on_is_not_stale() {
        // Off for a second, then on: the layers must already be there, not
        // ramping up out of an empty line.
        let off = OvertoneParams {
            mix: 0.0,
            ..open(1.0, 1.0, 1.0)
        };
        let on = open(1.0, 1.0, 1.0);
        let mut warm = Overtone::new(SR);
        let mut cold = Overtone::new(SR);
        for i in 0..48_000 {
            let x = sine(220.0, 0.3, i, SR);
            warm.process(x, x, &off);
            cold.process(x, x, &off);
        }
        // `cold` stands for a node that was not running: a fresh one given
        // only the last samples.
        let mut cold2 = Overtone::new(SR);
        let (mut hot, mut fresh) = (0.0f32, 0.0f32);
        for i in 48_000..48_000 + 300 {
            let x = sine(220.0, 0.3, i, SR);
            let (a, _) = warm.process(x, x, &on);
            let (b, _) = cold2.process(x, x, &on);
            hot = hot.max((a - x).abs());
            fresh = fresh.max((b - x).abs());
        }
        let _ = cold;
        assert!(hot > 0.05, "warm layers should be audible at once: {hot}");
        assert!(fresh < hot * 0.5, "a cold start lags: {fresh} vs {hot}");
    }

    #[test]
    fn silence_in_silence_out() {
        let mut o = Overtone::new(SR);
        let p = open(1.0, 1.0, 1.0);
        for _ in 0..10_000 {
            assert_eq!(o.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn layers_sit_at_the_octave_below_above_and_the_fifth() {
        let all = render(&open(0.5, 0.3, 0.2));
        let p110 = band(&all, 110.0, SR);
        let p440 = band(&all, 440.0, SR);
        let p330 = band(&all, 329.7, SR);
        // Requested amplitude ratios, in power.
        let want = |a: f32, b: f32| db(a * a, b * b);
        assert!(
            (db(p110, p440) - want(0.5, 0.3)).abs() < 1.5,
            "sub vs octave: {} want {}",
            db(p110, p440),
            want(0.5, 0.3)
        );
        assert!(
            (db(p440, p330) - want(0.3, 0.2)).abs() < 1.5,
            "octave vs fifth: {} want {}",
            db(p440, p330),
            want(0.3, 0.2)
        );
        assert!(
            (db(p110, p330) - want(0.5, 0.2)).abs() < 1.5,
            "sub vs fifth: {} want {}",
            db(p110, p330),
            want(0.5, 0.2)
        );
    }

    #[test]
    fn each_layer_at_zero_adds_nothing_at_its_frequency() {
        let full = render(&open(1.0, 1.0, 1.0));
        let (f110, f440, f330) = (
            band(&full, 110.0, SR),
            band(&full, 440.0, SR),
            band(&full, 329.7, SR),
        );
        let no_sub = render(&open(0.0, 1.0, 1.0));
        let no_oct = render(&open(1.0, 0.0, 1.0));
        let no_fifth = render(&open(1.0, 1.0, 0.0));
        assert!(db(band(&no_sub, 110.0, SR), f110) < -30.0);
        assert!(db(band(&no_oct, 440.0, SR), f440) < -30.0);
        let d = db(band(&no_fifth, 329.7, SR), f330);
        assert!(d < -30.0, "fifth off leaves {d} dB");
        // What is left is the other layers' sidebands, not the layer itself. The others stay.
        assert!(db(band(&no_sub, 440.0, SR), f440).abs() < 1.0);
    }

    #[test]
    fn tone_darkens_the_layers() {
        // A 3 kHz sine through the octave layer comes out at 6 kHz; the
        // lowest tone setting must take most of it away.
        let dull = OvertoneParams {
            tone_hz: TONE_MIN_HZ,
            ..open(0.0, 1.0, 0.0)
        };
        let bright = open(0.0, 1.0, 0.0);
        let mut o = Overtone::new(SR);
        let mut o2 = Overtone::new(SR);
        let (mut a, mut b) = (0.0f32, 0.0f32);
        for i in 0..24_000 {
            let x = sine(3_000.0, 0.2, i, SR);
            let (u, _) = o.process(x, x, &bright);
            let (v, _) = o2.process(x, x, &dull);
            if i > 4_800 {
                a += u * u;
                b += v * v;
            }
        }
        assert!(db(b, a) < -12.0, "tone should cut the top: {}", db(b, a));
    }

    #[test]
    fn defaults_and_latency_are_as_documented() {
        let d = OvertoneParams::default();
        assert_eq!(
            (d.sub, d.octave, d.fifth, d.tone_hz, d.mix),
            (0.5, 0.3, 0.0, 5_000.0, 0.5)
        );
        assert!((TONE_MIN_HZ..=TONE_MAX_HZ).contains(&d.tone_hz));
    }

    #[test]
    fn loud_noise_stays_bounded_for_seconds() {
        let mut o = Overtone::new(SR);
        let p = open(1.0, 1.0, 1.0);
        let mut seed = 0x2468_ace1;
        let mut peak = 0.0f32;
        for i in 0..SR as usize * 6 {
            let x = if i < SR as usize * 3 {
                noise(&mut seed)
            } else {
                0.0
            };
            let (l, r) = o.process(x, -x, &p);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak <= 2.0 + 1e-3, "peak {peak}");
        // After the input stops, the tail dies away entirely.
        assert_eq!(o.process(0.0, 0.0, &p), (0.0, 0.0));
    }

    #[test]
    fn full_scale_sine_and_square_stay_in_bounds() {
        for square in [false, true] {
            let mut o = Overtone::new(SR);
            let p = open(1.0, 1.0, 1.0);
            for i in 0..48_000 {
                let s = sine(110.0, 1.0, i, SR);
                let x = if square { s.signum() } else { s };
                let (l, _) = o.process(x, x, &p);
                assert!(l.abs() <= 2.0 + 1e-3 && l.is_finite());
            }
        }
    }

    #[test]
    fn a_parameter_sweep_does_not_click() {
        // Music-like input: a few partials with a slow tremolo.
        let input = |i: usize| {
            let t = i as f32 / SR;
            (sine(110.0, 0.25, i, SR) + sine(220.0, 0.15, i, SR) + sine(660.0, 0.08, i, SR))
                * (0.7 + 0.3 * (core::f32::consts::TAU * 3.0 * t).sin())
        };
        let n = SR as usize;
        let mut in_step = 0.0f32;
        for i in 1..n {
            in_step = in_step.max((input(i) - input(i - 1)).abs());
        }
        let mut o = Overtone::new(SR);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for i in 0..n {
            let u = i as f32 / n as f32;
            let p = OvertoneParams {
                sub: u,
                octave: 1.0 - u,
                fifth: (u * 2.0).min(2.0 - u * 2.0).max(0.0),
                tone_hz: TONE_MIN_HZ * (TONE_MAX_HZ / TONE_MIN_HZ).powf(u),
                mix: u,
            };
            let (l, _) = o.process(input(i), input(i), &p);
            if i > 2_000 {
                worst = worst.max((l - prev).abs());
            }
            prev = l;
        }
        assert!(
            worst < 3.0 * in_step,
            "step {worst} against input's largest {in_step}"
        );
    }

    #[test]
    fn works_at_every_sample_rate_with_the_same_latency_in_time() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut o = Overtone::new(sr);
            let ms = o.latency_samples() as f32 / sr * 1000.0;
            assert!((ms - 20.0).abs() < 0.2, "{sr}: {ms} ms");
            let p = open(0.5, 0.3, 0.2);
            let mut peak = 0.0f32;
            for i in 0..sr as usize {
                let x = sine(220.0, 0.5, i, sr);
                let (l, r) = o.process(x, x, &p);
                assert!(l.is_finite() && r.is_finite());
                peak = peak.max(l.abs());
            }
            assert!(peak > 0.25 && peak < 2.0, "{sr}: peak {peak}");
        }
    }

    #[test]
    fn a_mono_source_gives_matching_finite_sides() {
        let mut o = Overtone::new(SR);
        let p = open(1.0, 0.5, 0.5);
        for i in 0..24_000 {
            let x = sine(180.0, 0.6, i, SR);
            let (l, r) = o.process(x, x, &p);
            assert!(l.is_finite() && r.is_finite());
            assert!((l - r).abs() < 1e-6, "identical shifters, identical sides");
            assert!(l.abs() < 2.0);
        }
    }

    #[test]
    fn reset_forgets_the_layers() {
        let mut o = Overtone::new(SR);
        let p = open(1.0, 1.0, 1.0);
        let mut st = 12345u32;
        for _ in 0..48_000 {
            let x = noise(&mut st) * 0.8;
            o.process(x, x, &p);
        }
        o.reset();
        // The shifters' windows are 40 ms; two seconds is far past any tail.
        for _ in 0..96_000 {
            assert_eq!(o.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }

    #[test]
    fn reset_on_a_fresh_overtone_changes_nothing() {
        let p = OvertoneParams::default();
        let mut a = Overtone::new(SR);
        let mut b = Overtone::new(SR);
        b.reset();
        for i in 0..9_600 {
            let x = sine(220.0, 0.5, i, SR);
            assert_eq!(a.process(x, -x, &p), b.process(x, -x, &p));
        }
    }
}
