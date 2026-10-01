//! Pitch shifter.
//!
//! A building block, not an effect: one voice in, the same voice out at a
//! different pitch, with the length and tempo of the sound left alone. It is
//! meant for the nodes that need a shifted copy (a harmoniser, an octave
//! below a bass, the choir's sharp and flat singers) and is mono, so a
//! stereo user holds one a side.
//!
//! The mechanism is a delay line read by two points half a window apart. A
//! read point moving through the line at `ratio` times the speed it was
//! written plays `ratio` times the pitch, and its delay then grows or shrinks
//! by `1 - ratio` samples a sample. It can only drift so far, so it runs
//! through a window of `WINDOW_MS` and jumps back to the start of it. The
//! jump would click, so each point is faded by a Hann curve that is silent at
//! exactly the moment it wraps, and the other point, half a window along, is
//! at full strength. The two Hann gains sum to one, so a steady tone keeps
//! its level through every wrap.
//!
//! The window is the tradeoff. While a point wraps, the two copies are half
//! a window apart and briefly overlap, which smears transients and, on a
//! pitched sound, beats with a roughness at `|1 - ratio| / window` Hz. A
//! short window smears little but needs the source to repeat inside it; a
//! long one follows a low note properly but smears more and is later. At
//! 40 ms a note at 80 Hz still gets three cycles in a window, which is about
//! the least that keeps the overlap coherent. Higher sources could use a
//! shorter one; the choir uses 20 ms.
//!
//! At a ratio of exactly one the points do not move. Starting fresh, one is
//! at the window's start with its gain at zero and the other halfway with
//! its gain at one, so the output is the input delayed by `latency_samples`
//! and nothing else. After the ratio has been moved and returned, the points
//! rest wherever they stopped and the two delays are no longer equal: the
//! unity output is then a gentle comb rather than a clean delay. It is a
//! shifter's nature, and it is why one should be switched out, not left at
//! one, when no shift is wanted.
//!
//! The ratio is smoothed inside, so a stepped control (a semitone selector)
//! glides rather than clicks. The reads use a four-point Hermite curve, as
//! the chorus does, so a moving point does not dull the top.

use crate::smooth::OnePole;

/// The window the read points run through, in milliseconds. See the header
/// for the tradeoff; the latency is half of it.
pub const WINDOW_MS: f32 = 40.0;
/// The shift limits, as playback-rate multipliers: two octaves down to two up.
pub const MIN_RATIO: f32 = 0.25;
pub const MAX_RATIO: f32 = 4.0;
/// How long the ratio takes to settle, in milliseconds.
const RATIO_SMOOTH_MS: f32 = 5.0;
/// The nearest a read point comes to the newest sample, in samples. The
/// four-point curve reads one sample ahead of the point.
const MIN_DELAY: f32 = 2.0;
/// Past the farthest read: the curve reads two samples behind it.
const GUARD: usize = 4;

/// A shift in semitones as a playback-rate multiplier: 12 is 2.0, -12 is 0.5.
pub fn semitones_to_ratio(st: f32) -> f32 {
    (st / 12.0).exp2()
}

pub struct PitchShifter {
    line: Vec<f32>,
    write: usize,
    /// The window in samples, even and whole, so a half window is a whole
    /// number of samples and unity has an exact delay.
    window: f32,
    /// The first read point's place in its window, in turns. The second is
    /// half a turn along.
    phase: f32,
    ratio: OnePole,
}

impl PitchShifter {
    pub fn new(sample_rate: f32) -> Self {
        let window = ((WINDOW_MS * 0.001 * sample_rate / 2.0).round() * 2.0).max(4.0);
        let mut ratio = OnePole::new();
        ratio.set_time(RATIO_SMOOTH_MS, sample_rate);
        ratio.reset(1.0);
        Self {
            line: vec![0.0; window as usize + MIN_DELAY as usize + GUARD],
            write: 0,
            window,
            // Fresh, the first point is silent and the second is at full
            // strength: the unity output is a clean delay.
            phase: 0.0,
            ratio,
        }
    }

    /// Forgets the audio it holds, without allocating. The ratio smoother
    /// keeps its place: it follows a control, not a signal.
    pub fn reset(&mut self) {
        self.line.fill(0.0);
    }

    /// What `process` delays the signal by at a ratio of one, in samples:
    /// half the window and the nearest-read margin.
    pub fn latency_samples(&self) -> usize {
        (self.window * 0.5 + MIN_DELAY) as usize
    }

    /// Four-point Hermite read `delay` samples behind the newest.
    #[inline]
    fn tap(&self, delay: f32) -> f32 {
        let whole = delay.floor();
        let t = delay - whole;
        let n = self.line.len();
        let at = |back: usize| self.line[(self.write + n - back) % n];
        let w = (whole as usize).max(1);
        let (xm1, x0, x1, x2) = (at(w - 1), at(w), at(w + 1), at(w + 2));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    /// One sample in, one out, shifted by `ratio` (clamped to
    /// `MIN_RATIO..=MAX_RATIO`).
    #[inline]
    pub fn process(&mut self, x: f32, ratio: f32) -> f32 {
        let ratio = self.ratio.process(ratio.clamp(MIN_RATIO, MAX_RATIO));
        // Flush anything denormal out of the input before it is stored.
        let x = if x.abs() < 1e-30 { 0.0 } else { x };
        self.write = (self.write + 1) % self.line.len();
        self.line[self.write] = x;

        self.phase += (1.0 - ratio) / self.window;
        self.phase -= self.phase.floor();
        let other = (self.phase + 0.5).fract();
        // Hann gains: they sum to one, and each is zero at its wrap.
        let ga = (core::f32::consts::PI * self.phase).sin().powi(2);
        let gb = 1.0 - ga;
        let a = self.tap(MIN_DELAY + self.window * self.phase);
        let b = self.tap(MIN_DELAY + self.window * other);
        ga * a + gb * b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(f: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| 0.5 * (core::f32::consts::TAU * f * i as f32 / SR).sin())
            .collect()
    }

    /// The strongest frequency near `centre`, from a DFT scanned in 0.25 Hz
    /// steps over the last second. Zero crossings are thrown by the
    /// crossfades, which move them without moving the pitch.
    fn measure(y: &[f32], centre: f32) -> f32 {
        let y = &y[y.len() - SR as usize..];
        let mut best = (0.0, 0.0);
        let mut f = centre * 0.9;
        while f < centre * 1.1 {
            let w = core::f32::consts::TAU * f / SR;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in y.iter().enumerate() {
                re += v as f64 * (w as f64 * i as f64).cos();
                im += v as f64 * (w as f64 * i as f64).sin();
            }
            let mag = re * re + im * im;
            if mag > best.0 {
                best = (mag, f);
            }
            f += 0.25;
        }
        best.1
    }

    // 450 Hz fits a whole number of cycles into the window, so the handoff
    // between read points lands in phase. A tone that does not fit gets a
    // phase step at each handoff, which tugs the average pitch by a few Hz:
    // the shifter's roughness, not an error in the ratio.
    #[test]
    fn it_shifts_a_sine_to_the_ratio() {
        for ratio in [0.5, 2.0, 1.5] {
            let mut sh = PitchShifter::new(SR);
            let y: Vec<f32> = sine(450.0, SR as usize * 2)
                .iter()
                .map(|&s| sh.process(s, ratio))
                .collect();
            let want = 450.0 * ratio;
            let f = measure(&y, want);
            assert!(
                (f - want).abs() / want < 0.01,
                "ratio {ratio}: {f} Hz, wanted {want}"
            );
        }
    }

    #[test]
    fn unity_is_a_clean_delay() {
        let mut sh = PitchShifter::new(SR);
        let x = sine(220.0, 9600);
        let y: Vec<f32> = x.iter().map(|&s| sh.process(s, 1.0)).collect();
        let lat = sh.latency_samples();
        for i in lat..x.len() {
            assert!(
                (y[i] - x[i - lat]).abs() < 1e-4,
                "sample {i}: {} against {}",
                y[i],
                x[i - lat]
            );
        }
        let peak = y[lat..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let db = 20.0 * (peak / 0.5).log10();
        assert!(db.abs() < 1.0, "level moved {db} dB");
    }

    #[test]
    fn a_shifted_tone_keeps_its_level() {
        let mut sh = PitchShifter::new(SR);
        let y: Vec<f32> = sine(440.0, SR as usize)
            .iter()
            .map(|&s| sh.process(s, 1.5))
            .collect();
        let peak = y[SR as usize / 2..]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.4 && peak < 0.6, "peak {peak}");
    }

    #[test]
    fn sweeping_the_ratio_does_not_click() {
        let mut sh = PitchShifter::new(SR);
        let x = sine(330.0, SR as usize * 2);
        let in_step = x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        let mut prev = 0.0;
        let mut worst = 0.0f32;
        for (i, &s) in x.iter().enumerate() {
            // Down two octaves, up two, and back, in two seconds.
            let st = 24.0 * (core::f32::consts::TAU * i as f32 / (SR * 2.0)).sin();
            let y = sh.process(s, semitones_to_ratio(st));
            if i > 4800 {
                worst = worst.max((y - prev).abs());
            }
            prev = y;
        }
        // At 4x the tone's own steps are up to four times larger.
        assert!(worst < in_step * 4.5, "step {worst}, input's {in_step}");
    }

    #[test]
    fn silence_in_is_silence_out() {
        let mut sh = PitchShifter::new(SR);
        for r in [0.25, 0.7, 1.0, 3.0] {
            for _ in 0..10_000 {
                assert_eq!(sh.process(0.0, r), 0.0);
            }
        }
    }

    #[test]
    fn noise_stays_bounded() {
        let mut sh = PitchShifter::new(SR);
        let mut rng = crate::rng::Rng::new(7);
        for i in 0..SR as usize * 3 {
            let r = semitones_to_ratio(24.0 * ((i / 5000) as f32 * 1.7).sin());
            let y = sh.process(rng.next_bipolar(), r);
            assert!(y.is_finite() && y.abs() <= 1.5, "sample {i}: {y}");
        }
    }

    #[test]
    fn it_works_at_other_rates() {
        for sr in [44_100.0, 96_000.0] {
            let mut sh = PitchShifter::new(sr);
            let want = (WINDOW_MS * 0.001 * sr * 0.5) as usize;
            assert!(sh.latency_samples().abs_diff(want) <= 3);
            for i in 0..sr as usize {
                let x = (core::f32::consts::TAU * 200.0 * i as f32 / sr).sin();
                assert!(sh.process(x, 1.3).is_finite());
            }
        }
    }

    #[test]
    fn ratios_convert_from_semitones() {
        assert!((semitones_to_ratio(12.0) - 2.0).abs() < 1e-6);
        assert!((semitones_to_ratio(-12.0) - 0.5).abs() < 1e-6);
        assert_eq!(semitones_to_ratio(0.0), 1.0);
    }

    #[test]
    fn reset_forgets_what_it_held() {
        let mut s = PitchShifter::new(48_000.0);
        for i in 0..4_800 {
            s.process(((i % 97) as f32 / 97.0) - 0.5, 1.5);
        }
        s.reset();
        for _ in 0..4_800 {
            assert_eq!(s.process(0.0, 1.5), 0.0);
        }
    }
}
