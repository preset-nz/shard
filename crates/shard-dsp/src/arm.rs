//! Arming a take (epic 44, triage #12): wait for the first sound.
//!
//! An armed recorder does not write anything until a frame of the tapped
//! signal reaches a threshold. [`Arm`] holds the last few milliseconds of what
//! it saw, so that when the threshold is crossed the take can open on the
//! attack's lead-in rather than on the attack itself.
//!
//! Everything here is sized at construction and runs on the audio thread:
//! [`Arm::feed`] copies into a fixed ring and hands slices to a callback, and
//! allocates nothing.

/// Peak level that starts an armed take, in dBFS.
pub const THRESHOLD_DB: f32 = -50.0;

/// How much audio before the crossing the take keeps, in milliseconds.
pub const PREROLL_MS: f32 = 20.0;

/// The detector and its pre-roll ring, for interleaved stereo.
pub struct Arm {
    threshold: f32,
    /// `preroll` frames, interleaved, as a ring.
    ring: Vec<f32>,
    preroll: usize,
    /// The frame the next one is written at.
    write: usize,
    /// Frames held, up to `preroll`.
    filled: usize,
}

impl Arm {
    /// A detector for `sample_rate`, with the standard threshold and pre-roll.
    pub fn new(sample_rate: f32) -> Self {
        Self::with(sample_rate, THRESHOLD_DB, PREROLL_MS)
    }

    pub fn with(sample_rate: f32, threshold_db: f32, preroll_ms: f32) -> Self {
        let preroll = ((sample_rate * preroll_ms / 1000.0).ceil() as usize).max(1);
        Self {
            threshold: 10f32.powf(threshold_db / 20.0),
            ring: vec![0.0; preroll * 2],
            preroll,
            write: 0,
            filled: 0,
        }
    }

    /// Frames of pre-roll kept.
    pub fn preroll_frames(&self) -> usize {
        self.preroll
    }

    /// Forget what was heard. Call when the tap is off, so a new arming never
    /// opens on the last one's tail.
    pub fn reset(&mut self) {
        self.write = 0;
        self.filled = 0;
    }

    /// Offer a block of interleaved stereo.
    ///
    /// Nothing at the threshold: the block goes into the pre-roll and this
    /// returns `false`. A frame at or over it: `emit` is called with the
    /// pre-roll (up to [`Arm::preroll_frames`] frames ending just before that
    /// frame, oldest first, in one or more slices) and then the rest of the
    /// block from that frame on, and this returns `true`. After that the caller
    /// passes blocks straight through and stops feeding.
    pub fn feed(&mut self, block: &[f32], mut emit: impl FnMut(&[f32])) -> bool {
        let hit = block
            .chunks_exact(2)
            .position(|f| f[0].abs() >= self.threshold || f[1].abs() >= self.threshold);
        let Some(k) = hit else {
            self.remember(block);
            return false;
        };
        let lead = &block[..k * 2];
        if k >= self.preroll {
            emit(&block[(k - self.preroll) * 2..]);
        } else {
            // The older part comes from the ring: the last `need` frames of it.
            let need = (self.preroll - k).min(self.filled);
            let start = (self.write + self.preroll - need) % self.preroll;
            let first = need.min(self.preroll - start);
            emit(&self.ring[start * 2..(start + first) * 2]);
            if need > first {
                emit(&self.ring[..(need - first) * 2]);
            }
            if !lead.is_empty() {
                emit(lead);
            }
            emit(&block[k * 2..]);
        }
        self.reset();
        true
    }

    fn remember(&mut self, block: &[f32]) {
        // Only the last `preroll` frames of a long block can matter.
        let keep = block.len().min(self.preroll * 2) & !1;
        for f in block[block.len() - keep..].chunks_exact(2) {
            self.ring[self.write * 2] = f[0];
            self.ring[self.write * 2 + 1] = f[1];
            self.write = (self.write + 1) % self.preroll;
        }
        self.filled = (self.filled + keep / 2).min(self.preroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(arm: &mut Arm, block: &[f32]) -> Option<Vec<f32>> {
        let mut out = Vec::new();
        arm.feed(block, |s| out.extend_from_slice(s)).then_some(out)
    }

    /// Frame `i` is (i + 1) * 1e-7 until `at`, which is loud.
    fn signal(n: usize, at: usize) -> Vec<f32> {
        (0..n)
            .flat_map(|i| {
                let v = if i >= at { 0.5 } else { (i + 1) as f32 * 1e-7 };
                [v, -v]
            })
            .collect()
    }

    #[test]
    fn silence_never_triggers() {
        let mut arm = Arm::new(48_000.0);
        for _ in 0..100 {
            assert!(collect(&mut arm, &vec![0.001; 512]).is_none());
        }
    }

    #[test]
    fn a_hit_takes_the_preroll_across_blocks() {
        let mut arm = Arm::new(48_000.0);
        let p = arm.preroll_frames();
        assert_eq!(p, 960);
        let all = signal(4096, 3000);
        assert!(collect(&mut arm, &all[..2048 * 2]).is_none());
        // The hit is frame 3000: 952 frames into the second block.
        let got = collect(&mut arm, &all[2048 * 2..]).expect("triggers");
        assert_eq!(got, all[(3000 - p) * 2..].to_vec());
    }

    #[test]
    fn a_hit_early_in_the_first_block_has_a_short_preroll() {
        let mut arm = Arm::new(48_000.0);
        let all = signal(256, 100);
        let got = collect(&mut arm, &all).unwrap();
        assert_eq!(got, all);
    }

    #[test]
    fn a_hit_with_the_lead_in_split_across_the_ring_end_reads_in_order() {
        let mut arm = Arm::new(48_000.0);
        let p = arm.preroll_frames();
        // Odd block sizes so the ring's write point is not at its start.
        let all = signal(5000, 4400);
        let mut at = 0;
        for n in [700, 1300, 1111] {
            assert!(collect(&mut arm, &all[at * 2..(at + n) * 2]).is_none());
            at += n;
        }
        let got = collect(&mut arm, &all[at * 2..]).unwrap();
        assert_eq!(got, all[(4400 - p) * 2..].to_vec());
    }

    #[test]
    fn a_negative_peak_on_one_channel_counts() {
        let mut arm = Arm::new(48_000.0);
        let mut b = vec![0.0f32; 64];
        b[41] = -0.01;
        let got = collect(&mut arm, &b).unwrap();
        assert_eq!(got.len(), 64);
    }

    #[test]
    fn reset_forgets_the_old_tail() {
        let mut arm = Arm::new(48_000.0);
        assert!(collect(&mut arm, &vec![0.0001; 4096]).is_none());
        arm.reset();
        let mut b = vec![0.0f32; 40];
        b[20] = 0.5;
        let got = collect(&mut arm, &b).unwrap();
        assert_eq!(got, b, "nothing from before the reset is in the take");
    }
}
