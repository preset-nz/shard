//! Loading and summarising sample material.
//!
//! Everything here allocates, so none of it may be called from the audio
//! thread. Decoded buffers reach the engine through the swap queue.

pub struct Loaded {
    pub name: String,
    /// Where it was read from, so a patch can point back at it. None for the
    /// built-in drone, which has nowhere to point.
    pub path: Option<String>,
    pub samples: Vec<f32>,
}

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

    Ok(Loaded {
        name,
        path: Some(path.to_string()),
        samples,
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
        .collect();
    Loaded {
        name: "built-in drone".into(),
        path: None,
        samples,
    }
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
