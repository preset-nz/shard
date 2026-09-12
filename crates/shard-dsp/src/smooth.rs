//! One-pole smoothing.
//!
//! The single most reused primitive in the app. It smooths parameters so a
//! 7-bit MIDI step does not click, it follows envelopes when you give it
//! different attack and release times, and it slews modulation matrix rows.
//! Build it first; everything after depends on it.

/// A first-order low-pass. One multiply, one add, one piece of state.
///
/// The output approaches the input exponentially and never exactly arrives,
/// which is why stepped parameters must not be smoothed at all.
#[derive(Debug, Clone, Copy)]
pub struct OnePole {
    z: f32,
    a: f32,
}

impl OnePole {
    /// Starts fully open: the first `process` call jumps straight to the input.
    pub fn new() -> Self {
        Self { z: 0.0, a: 0.0 }
    }

    /// Time constant in milliseconds. Zero means no smoothing.
    pub fn set_time(&mut self, ms: f32, sample_rate: f32) {
        self.a = if ms <= 0.0 {
            0.0
        } else {
            (-1.0 / (ms * 0.001 * sample_rate)).exp()
        };
    }

    /// Jump to a value without gliding. Use at load and on patch recall.
    pub fn reset(&mut self, value: f32) {
        self.z = value;
    }

    pub fn value(&self) -> f32 {
        self.z
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.z = x + self.a * (self.z - x);
        self.z
    }
}

impl Default for OnePole {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_time_passes_through() {
        let mut p = OnePole::new();
        p.set_time(0.0, 48_000.0);
        assert_eq!(p.process(1.0), 1.0);
        assert_eq!(p.process(-0.5), -0.5);
    }

    #[test]
    fn approaches_target_without_overshoot() {
        let mut p = OnePole::new();
        p.set_time(10.0, 48_000.0);
        p.reset(0.0);
        let mut last = 0.0;
        for _ in 0..48_000 {
            let v = p.process(1.0);
            assert!(v >= last, "must be monotonic while rising");
            assert!(v <= 1.0, "must never overshoot the target");
            last = v;
        }
        assert!((last - 1.0).abs() < 1e-3, "should have essentially arrived");
    }

    #[test]
    fn reaches_63_percent_after_one_time_constant() {
        // The defining property of a one-pole: after one time constant the
        // output has covered 1 - 1/e of the distance.
        let sr = 48_000.0;
        let ms = 10.0;
        let mut p = OnePole::new();
        p.set_time(ms, sr);
        p.reset(0.0);
        let n = (ms * 0.001 * sr) as usize;
        let mut v = 0.0;
        for _ in 0..n {
            v = p.process(1.0);
        }
        let expected = 1.0 - (-1.0f32).exp();
        assert!((v - expected).abs() < 0.01, "got {v}, expected ~{expected}");
    }

    #[test]
    fn longer_time_smooths_more() {
        let mut fast = OnePole::new();
        let mut slow = OnePole::new();
        fast.set_time(1.0, 48_000.0);
        slow.set_time(100.0, 48_000.0);
        fast.reset(0.0);
        slow.reset(0.0);
        for _ in 0..480 {
            fast.process(1.0);
            slow.process(1.0);
        }
        assert!(fast.value() > slow.value());
    }
}
