//! The limiter, last on the master.
//!
//! A peak limiter with a millisecond of look-ahead. The output is delayed by
//! that millisecond, and the delay is what makes it work: the limiter sees a
//! peak before it plays, so the gain is already down when the peak arrives
//! rather than stepping down after it, which is a click. Nothing leaves above
//! the ceiling; the hard clip after it is a safety net that no longer engages
//! (Georg, 2026-09-14: "the thing goes red every now and then").
//!
//! Every buffer is sized at construction, from the sample rate. Nothing here
//! allocates per sample.

/// How far ahead the limiter looks, and therefore how late the output is.
pub const LOOKAHEAD_MS: f32 = 1.0;
/// How long the gain takes to come back up after a peak.
const RELEASE_MS: f32 = 100.0;
/// How fast the gain follows the look-ahead minimum down. Shorter than the
/// look-ahead so it has settled by the time the peak plays.
const ATTACK_MS: f32 = 0.3;

#[derive(Debug, Clone, Copy)]
pub struct LimiterParams {
    /// Peak output level, 0 to 1.
    pub ceiling: f32,
    /// Off passes the signal through untouched, still delayed, so switching
    /// does not jump the timing.
    pub on: bool,
}

pub struct Limiter {
    /// Delayed input, interleaved L and R, one look-ahead long.
    delay: Vec<f32>,
    /// The gain each queued sample would need, for the running minimum.
    need: Vec<f32>,
    write: usize,
    gain: f32,
    attack: f32,
    release: f32,
    /// The lowest gain applied since `take_reduction`, for the meter.
    floor: f32,
}

impl Limiter {
    pub fn new(sample_rate: f32) -> Self {
        let n = ((sample_rate * LOOKAHEAD_MS / 1000.0).ceil() as usize).max(1);
        let coef = |ms: f32| (-1.0 / (sample_rate * ms / 1000.0)).exp();
        Self {
            delay: vec![0.0; n * 2],
            need: vec![1.0; n],
            write: 0,
            gain: 1.0,
            attack: coef(ATTACK_MS),
            release: coef(RELEASE_MS),
            floor: 1.0,
        }
    }

    /// The delay in samples, so a caller can account for it.
    pub fn latency(&self) -> usize {
        self.need.len()
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32, p: &LimiterParams) -> (f32, f32) {
        let n = self.need.len();
        let ceiling = p.ceiling.clamp(0.05, 1.0);
        // What the incoming sample will need when it plays, n samples on.
        let peak = l.abs().max(r.abs());
        let wanted = if p.on && peak > ceiling {
            ceiling / peak
        } else {
            1.0
        };
        // Swap the oldest sample out for the newest.
        let i = self.write;
        let (ol, or) = (self.delay[2 * i], self.delay[2 * i + 1]);
        let leaving_needs = self.need[i];
        self.delay[2 * i] = l;
        self.delay[2 * i + 1] = r;
        self.need[i] = wanted;
        self.write = (i + 1) % n;

        // The gain the sample now leaving must not exceed: the least that
        // any sample still queued behind it needs. A linear scan over one
        // millisecond is cheap and has no state to get wrong.
        let target = self.need.iter().fold(1.0f32, |a, g| a.min(*g));
        let coef = if target < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain = target + (self.gain - target) * coef;
        // The smoother shapes the approach; this is the guarantee. A sample
        // leaves at no more than the gain it asked for on the way in.
        self.gain = self.gain.min(leaving_needs);
        self.floor = self.floor.min(self.gain);
        if !p.on {
            return (ol, or);
        }
        (ol * self.gain, or * self.gain)
    }

    /// The lowest gain applied since the last call, as a plain factor: one
    /// means the limiter did nothing. Reset on read, for a meter.
    pub fn take_reduction(&mut self) -> f32 {
        core::mem::replace(&mut self.floor, self.gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn nothing_leaves_above_the_ceiling() {
        let mut lim = Limiter::new(SR);
        let p = LimiterParams {
            ceiling: 0.8,
            on: true,
        };
        // A tone that swells to four times the ceiling, with a spike.
        let mut worst = 0.0f32;
        for i in 0..SR as usize * 2 {
            let t = i as f32 / SR;
            let mut x = (core::f32::consts::TAU * 110.0 * t).sin() * (0.2 + 3.0 * t);
            if i % 9_001 == 0 {
                x = 8.0;
            }
            let (l, r) = lim.process(x, -x, &p);
            worst = worst.max(l.abs()).max(r.abs());
        }
        assert!(
            worst <= 0.8 * (1.0 + 1e-5),
            "left above the ceiling: {worst}"
        );
    }

    #[test]
    fn quiet_material_passes_untouched_but_late() {
        let mut lim = Limiter::new(SR);
        let p = LimiterParams {
            ceiling: 0.9,
            on: true,
        };
        let n = lim.latency();
        let input: Vec<f32> = (0..2_000)
            .map(|i| ((i as f32) * 0.05).sin() * 0.5)
            .collect();
        let out: Vec<f32> = input.iter().map(|&x| lim.process(x, x, &p).0).collect();
        for i in n..input.len() {
            assert_eq!(out[i], input[i - n], "sample {i}");
        }
        assert_eq!(lim.take_reduction(), 1.0);
    }

    #[test]
    fn off_is_the_same_delay_with_no_gain() {
        let mut lim = Limiter::new(SR);
        let p = LimiterParams {
            ceiling: 0.1,
            on: false,
        };
        let n = lim.latency();
        let out: Vec<f32> = (0..500).map(|_| lim.process(2.0, 2.0, &p).0).collect();
        assert!(out[..n].iter().all(|&v| v == 0.0));
        assert!(out[n..].iter().all(|&v| v == 2.0));
    }

    #[test]
    fn the_gain_comes_back_after_a_peak() {
        let mut lim = Limiter::new(SR);
        let p = LimiterParams {
            ceiling: 0.5,
            on: true,
        };
        for _ in 0..200 {
            lim.process(2.0, 2.0, &p);
        }
        assert!(lim.take_reduction() < 0.3);
        for _ in 0..SR as usize {
            lim.process(0.1, 0.1, &p);
        }
        let mut last = 0.0;
        for _ in 0..100 {
            last = lim.process(0.1, 0.1, &p).0;
        }
        assert!((last - 0.1).abs() < 1e-3, "gain did not recover: {last}");
    }

    #[test]
    fn silence_in_silence_out() {
        let mut lim = Limiter::new(SR);
        let p = LimiterParams {
            ceiling: 0.9,
            on: true,
        };
        for _ in 0..10_000 {
            assert_eq!(lim.process(0.0, 0.0, &p), (0.0, 0.0));
        }
    }
}
