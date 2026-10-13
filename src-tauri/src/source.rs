//! Loading and summarising sample material.
//!
//! Everything here allocates, so none of it may be called from the audio
//! thread. Decoded buffers reach the engine through the swap queue.

pub struct Loaded {
    pub name: String,
    pub samples: Vec<f32>,
    /// The waveform as drawn, worked out once at decode rather than on every
    /// redraw.
    pub peaks: Vec<f32>,
}

/// How many peaks a waveform is drawn from.
const PEAKS: usize = 900;

/// Mono, at the engine's rate. Multi-channel files are summed.
pub fn load(path: &str, target_rate: f32) -> Result<Loaded, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("{path}: {e}"))?;
    let spec = reader.spec();

    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };

    let channels = spec.channels.max(1) as usize;
    let mono: Vec<f32> = if channels == 1 {
        raw
    } else {
        raw.chunks(channels)
            .map(|f| f.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    // Naive linear resampling. Adequate because every grain already reads the
    // buffer at an arbitrary rate; this is not material that has to stay in
    // tune with anything.
    let ratio = spec.sample_rate as f32 / target_rate;
    let samples = if (ratio - 1.0).abs() < 1e-6 {
        mono
    } else {
        let out_len = (mono.len() as f32 / ratio) as usize;
        (0..out_len)
            .map(|i| {
                let src = i as f32 * ratio;
                let j = src as usize;
                if j + 1 < mono.len() {
                    mono[j] + (mono[j + 1] - mono[j]) * (src - j as f32)
                } else {
                    *mono.last().unwrap_or(&0.0)
                }
            })
            .collect()
    };

    if samples.len() < 2 {
        return Err(format!("{path}: too short to granulate"));
    }

    let name = std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());

    let peaks = peaks(&samples, PEAKS);
    Ok(Loaded {
        name,
        samples,
        peaks,
    })
}

/// Two seconds of detuned drone, so the app makes sound the moment it opens
/// rather than sitting silent until someone finds a file.
pub fn startup_drone(sample_rate: f32) -> Loaded {
    let n = (sample_rate * 2.0) as usize;
    let samples = (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate;
            let body = (core::f32::consts::TAU * 110.0 * t).sin() * 0.5
                + (core::f32::consts::TAU * 164.8 * t).sin() * 0.3
                + (core::f32::consts::TAU * 220.7 * t).sin() * 0.2;
            body * (0.4 + 0.6 * (core::f32::consts::TAU * 0.25 * t).sin().abs())
        })
        .collect::<Vec<f32>>();
    Loaded {
        name: "built-in drone".into(),
        peaks: peaks(&samples, PEAKS),
        samples,
    }
}

/// The most buckets a zoomed view may ask for.
pub const MAX_RANGE_BUCKETS: usize = 4096;

/// Absolute peaks of the part of `samples` between two fractions of the whole,
/// in `buckets` equal slices of it. This is what a zoomed waveform is drawn
/// from: the same fold as `peaks`, but over the visible range only, so a
/// minute-long take is as sharp at 5 ms as a one-shot is at fit. Fractions
/// are clamped to 0..=1 and put in order; a slice is never empty, so a range
/// narrower than a bucket repeats the sample it lands on.
pub fn peaks_range(samples: &[f32], start: f32, end: f32, buckets: usize) -> Vec<f32> {
    let buckets = buckets.min(MAX_RANGE_BUCKETS);
    if samples.is_empty() || buckets == 0 {
        return Vec::new();
    }
    let n = samples.len();
    let (a, b) = (start.clamp(0.0, 1.0), end.clamp(0.0, 1.0));
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let lo = a as f64 * n as f64;
    let width = (b - a) as f64 * n as f64;
    (0..buckets)
        .map(|i| {
            let from = (lo + width * i as f64 / buckets as f64).floor() as usize;
            let to = (lo + width * (i + 1) as f64 / buckets as f64).ceil() as usize;
            let from = from.min(n - 1);
            let to = to.clamp(from + 1, n);
            samples[from..to].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        })
        .collect()
}

/// Downsample to absolute peaks for drawing. Raw samples never cross the
/// Tauri boundary; this is what the waveform is drawn from.
pub fn peaks(samples: &[f32], buckets: usize) -> Vec<f32> {
    if samples.is_empty() || buckets == 0 {
        return Vec::new();
    }
    let per = (samples.len() / buckets).max(1);
    samples
        .chunks(per)
        .map(|c| c.iter().fold(0.0f32, |a, s| a.max(s.abs())))
        .take(buckets)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp 0, 1, 2 ... so a bucket's peak says exactly where it looked.
    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32).collect()
    }

    #[test]
    fn the_whole_range_covers_every_sample_in_order() {
        let p = peaks_range(&ramp(1000), 0.0, 1.0, 10);
        assert_eq!(p.len(), 10);
        assert_eq!(p[0], 99.0);
        assert_eq!(p[9], 999.0);
        assert!(p.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_narrow_range_sees_only_its_own_samples() {
        // 10_000 samples, the 10 % from 0.5: samples 5000..6000.
        let p = peaks_range(&ramp(10_000), 0.5, 0.6, 4);
        assert_eq!(p.len(), 4);
        assert!((5249.0..=5250.0).contains(&p[0]), "{}", p[0]);
        assert!((5999.0..=6000.0).contains(&p[3]), "{}", p[3]);
        assert!(p.iter().all(|&v| (5000.0..=6000.0).contains(&v)));
    }

    #[test]
    fn a_range_narrower_than_a_bucket_never_comes_back_empty() {
        let p = peaks_range(&ramp(100), 0.5, 0.5001, 8);
        assert_eq!(p, vec![50.0; 8]);
    }

    #[test]
    fn it_takes_absolute_values_and_survives_bad_input() {
        assert_eq!(peaks_range(&[-0.5, 0.25], 0.0, 1.0, 1), vec![0.5]);
        // Backwards and out-of-range fractions are put right.
        assert_eq!(peaks_range(&ramp(10), 2.0, -1.0, 1), vec![9.0]);
        assert!(peaks_range(&[], 0.0, 1.0, 4).is_empty());
        assert!(peaks_range(&ramp(10), 0.0, 1.0, 0).is_empty());
        assert_eq!(
            peaks_range(&ramp(10), 0.0, 1.0, usize::MAX).len(),
            MAX_RANGE_BUCKETS
        );
    }
}
