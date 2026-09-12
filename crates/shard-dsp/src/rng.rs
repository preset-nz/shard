//! A tiny xorshift, so the audio thread has randomness without a dependency
//! and without allocation. Deterministic from its seed, which makes the
//! granular tests reproducible.

#[derive(Debug, Clone, Copy)]
pub struct Rng {
    state: u32,
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        // Zero is a fixed point for xorshift, so steer away from it.
        Self {
            state: if seed == 0 { 0x9E37_79B9 } else { seed },
        }
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Uniform in `[0, 1)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        // 24 bits is the full mantissa of an f32; taking the high bits avoids
        // the weaker low bits of xorshift.
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `[-1, 1)`.
    #[inline]
    pub fn next_bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }
}

impl Default for Rng {
    fn default() -> Self {
        Self::new(0x1234_5678)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_in_unit_range() {
        let mut r = Rng::new(1);
        for _ in 0..100_000 {
            let v = r.next_f32();
            assert!((0.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn bipolar_stays_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..100_000 {
            let v = r.next_bipolar();
            assert!((-1.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn mean_is_roughly_centred() {
        let mut r = Rng::new(42);
        let n = 200_000;
        let sum: f64 = (0..n).map(|_| r.next_f32() as f64).sum();
        let mean = sum / n as f64;
        assert!((mean - 0.5).abs() < 0.01, "mean was {mean}");
    }

    #[test]
    fn is_deterministic_from_seed() {
        let a: Vec<u32> = (0..64)
            .scan(Rng::new(99), |r, _| Some(r.next_u32()))
            .collect();
        let b: Vec<u32> = (0..64)
            .scan(Rng::new(99), |r, _| Some(r.next_u32()))
            .collect();
        assert_eq!(a, b);
    }

    #[test]
    fn zero_seed_does_not_get_stuck() {
        let mut r = Rng::new(0);
        let first = r.next_u32();
        assert_ne!(first, 0);
        assert_ne!(r.next_u32(), first);
    }
}
