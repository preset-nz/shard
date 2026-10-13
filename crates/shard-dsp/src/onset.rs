//! Transient detection: where the sharp starts of sounds are in a buffer.
//!
//! **Offline only.** This allocates freely and walks a whole buffer, so it is
//! never called from the audio thread. The session runs it on the command
//! thread when a take is hunted for regions (roadmap rows 47 and 49).
//!
//! The method is energy flux with an adaptive threshold:
//!
//! 1. A short-time RMS `r` over 5 ms windows, one value per millisecond.
//! 2. For each step, a reference: the mean of `r` from 40 ms to 6 ms before
//!    it. The sound's own recent past, kept clear of the rise itself.
//! 3. The flux `o = max(0, r - reference)`. A steady tone has none, however
//!    loud; silence has none; a click or a drum hit has a lot.
//! 4. The candidates are the flux's local maxima over a 30 ms neighbourhood
//!    (leftmost wins a tie), so two onsets are never closer than 30 ms.
//! 5. A candidate counts when its flux is large against its own reference (the
//!    adaptive part) and large against the loudest the buffer gets (so room
//!    noise does not trigger). Sensitivity moves both limits together.
//!
//! The candidates do not depend on sensitivity, only the limits do, so a
//! higher sensitivity can never lose an onset. Each accepted onset is then
//! pulled back from the end of the rise to the first sample that is audibly
//! part of the attack, which puts it within a millisecond or so of the start.

/// Onsets are never closer than this, in milliseconds.
pub const MIN_GAP_MS: f32 = 30.0;

/// A sensible sensitivity to start from.
pub const DEFAULT_SENSITIVITY: f32 = 0.5;

const WINDOW_MS: f32 = 5.0;
const REF_NEAR_MS: usize = 6;
const REF_FAR_MS: usize = 40;

/// The onsets in `samples` (mono), as sample indices in ascending order.
/// `sensitivity` is 0 to 1: higher finds more, softer ones included.
pub fn detect_onsets(samples: &[f32], sample_rate: f32, sensitivity: f32) -> Vec<usize> {
    let s = if sensitivity.is_finite() {
        sensitivity.clamp(0.0, 1.0)
    } else {
        DEFAULT_SENSITIVITY
    };
    let hop = (sample_rate * 0.001).round().max(1.0) as usize;
    let win = ((sample_rate * WINDOW_MS * 0.001).round() as usize).max(hop);
    if samples.len() < win + hop {
        return Vec::new();
    }

    // r[k]: RMS of the window ending at the end of block k, for the blocks
    // whose window lies wholly inside the buffer.
    let first_end = win;
    let blocks = (samples.len() - first_end) / hop + 1;
    let r: Vec<f32> = (0..blocks)
        .map(|k| {
            let end = first_end + k * hop;
            let sum: f32 = samples[end - win..end].iter().map(|x| x * x).sum();
            (sum / win as f32).sqrt()
        })
        .collect();
    let peak = r.iter().copied().fold(0.0f32, f32::max);
    if peak < 1e-6 {
        return Vec::new();
    }
    let eps = 0.02 * peak;

    // The flux and the reference it was measured against.
    let mut flux = vec![0.0f32; blocks];
    let mut reference = vec![0.0f32; blocks];
    for k in REF_NEAR_MS..blocks {
        let lo = k.saturating_sub(REF_FAR_MS);
        let hi = k - REF_NEAR_MS;
        let mean = r[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32;
        reference[k] = mean;
        flux[k] = (r[k] - mean).max(0.0);
    }

    // The limits sensitivity moves: how many times its reference the flux
    // must be, and what share of the loudest block.
    let ratio_limit = 8.0 * 0.05f32.powf(s);
    let level_limit = 0.3 * 0.03f32.powf(s);

    let near = (MIN_GAP_MS * 0.001 * sample_rate / hop as f32).round() as usize;
    let mut out = Vec::new();
    for k in 0..blocks {
        let o = flux[k];
        if o <= 0.0 {
            continue;
        }
        let lo = k.saturating_sub(near);
        let hi = (k + near).min(blocks - 1);
        let top = (lo..k).all(|j| flux[j] < o) && (k + 1..=hi).all(|j| flux[j] <= o);
        if !top || o / (reference[k] + eps) < ratio_limit || o / peak < level_limit {
            continue;
        }
        out.push(attack_start(samples, first_end + k * hop, win + hop));
    }
    // Pulling back can never reorder two onsets 30 ms apart, but keep the
    // result strictly ascending whatever the input does.
    out.dedup();
    out
}

/// The first sample of the attack whose rise ended in the window that closes
/// at `end`: the first within `reach` samples before it that is at least a
/// fifth as loud as the loudest of them.
fn attack_start(samples: &[f32], end: usize, reach: usize) -> usize {
    let from = end.saturating_sub(reach);
    let span = &samples[from..end.min(samples.len())];
    let top = span.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    span.iter()
        .position(|x| x.abs() >= 0.2 * top)
        .map_or(from, |i| from + i)
}

/// A region from each onset to the next, as `(start, end)` sample indices. A
/// region ends early where the sound dies away: where it stays under about
/// -34 dB of the buffer's loudest for 40 ms. The last one ends at the same
/// kind of silence, or at the end of the buffer.
pub fn regions_from_onsets(
    samples: &[f32],
    sample_rate: f32,
    onsets: &[usize],
) -> Vec<(usize, usize)> {
    let block = ((sample_rate * 0.005).round() as usize).max(1);
    let quiet_blocks = 8; // 40 ms
    let peak = samples.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    let floor = 0.02 * peak;
    let loud: Vec<bool> = samples
        .chunks(block)
        .map(|c| c.iter().any(|x| x.abs() > floor))
        .collect();
    onsets
        .iter()
        .enumerate()
        .filter_map(|(i, &start)| {
            let limit = onsets.get(i + 1).copied().unwrap_or(samples.len());
            let mut quiet = 0;
            let mut end = limit;
            for b in start / block..=(limit.saturating_sub(1)) / block {
                if loud.get(b).copied().unwrap_or(false) {
                    quiet = 0;
                } else {
                    quiet += 1;
                    if quiet == quiet_blocks {
                        end = ((b + 1 - quiet_blocks) * block).max(start);
                        break;
                    }
                }
            }
            let end = end.clamp(start, limit);
            (end > start).then_some((start, end))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A short decaying noise burst at each place, in a quiet bed.
    fn clicks(len: usize, at: &[usize], amp: f32) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        let mut seed = 12_345u32;
        for &a in at {
            for i in 0..480 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let n = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                if a + i < len {
                    x[a + i] = n * amp * (1.0 - i as f32 / 480.0);
                }
            }
        }
        x
    }

    #[test]
    fn clicks_at_known_places_are_found_within_two_milliseconds() {
        let at = [4_800, 24_000, 57_600, 96_000, 130_000];
        let x = clicks(160_000, &at, 0.8);
        let found = detect_onsets(&x, SR, DEFAULT_SENSITIVITY);
        assert_eq!(found.len(), at.len(), "{found:?}");
        for (f, a) in found.iter().zip(at) {
            let off = (*f as f32 - a as f32).abs() / SR * 1000.0;
            assert!(off <= 2.0, "onset at {f} is {off} ms from {a}");
        }
    }

    #[test]
    fn a_click_a_single_sample_wide_is_found_too() {
        let mut x = vec![0.0f32; 48_000];
        x[20_000] = 1.0;
        let found = detect_onsets(&x, SR, DEFAULT_SENSITIVITY);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0] as i64 - 20_000).abs() < 96);
    }

    #[test]
    fn silence_and_a_steady_tone_have_no_onsets() {
        assert!(detect_onsets(&vec![0.0; 48_000], SR, 1.0).is_empty());
        for hz in [110.0f32, 440.0, 3_000.0] {
            let tone: Vec<f32> = (0..96_000)
                .map(|i| 0.7 * (core::f32::consts::TAU * hz * i as f32 / SR).sin())
                .collect();
            assert!(detect_onsets(&tone, SR, 1.0).is_empty(), "{hz} Hz");
        }
    }

    #[test]
    fn more_sensitivity_never_finds_fewer() {
        // Hits of very different loudness, some close together.
        let mut x = clicks(200_000, &[3_000, 40_000, 80_000], 0.9);
        for (a, amp) in [(41_800usize, 0.2f32), (120_000, 0.08), (150_000, 0.02)] {
            for (i, v) in clicks(1_000, &[0], amp).iter().enumerate() {
                x[a + i] += v;
            }
        }
        let mut last = 0;
        for step in 0..=20 {
            let n = detect_onsets(&x, SR, step as f32 / 20.0).len();
            assert!(n >= last, "{n} onsets at {step}/20 after {last}");
            last = n;
        }
        assert!(last > detect_onsets(&x, SR, 0.0).len(), "it should matter");
    }

    #[test]
    fn onsets_are_never_closer_than_the_gap() {
        let x = clicks(100_000, &[1_000, 2_000, 3_000, 30_000, 31_000], 0.9);
        let found = detect_onsets(&x, SR, 1.0);
        for w in found.windows(2) {
            assert!(w[1] - w[0] >= (MIN_GAP_MS * 0.001 * SR) as usize - 96);
        }
    }

    #[test]
    fn regions_run_to_the_next_onset_or_the_silence() {
        let at = [4_800usize, 48_000];
        let x = clicks(96_000, &at, 0.8);
        let onsets = detect_onsets(&x, SR, DEFAULT_SENSITIVITY);
        let regions = regions_from_onsets(&x, SR, &onsets);
        assert_eq!(regions.len(), 2);
        // Each burst is 10 ms; the region stops where it dies, not at the
        // next hit or the end.
        for (s, e) in regions {
            assert!(e - s < 4_800, "{s}..{e}");
            assert!(e > s);
        }
    }

    #[test]
    fn a_buffer_too_short_for_a_window_has_none() {
        assert!(detect_onsets(&[1.0; 10], SR, 1.0).is_empty());
        assert!(detect_onsets(&[], SR, 1.0).is_empty());
    }
}
