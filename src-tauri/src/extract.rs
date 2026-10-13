//! Extract (roadmap row 49): a marked region of a material, written out as a
//! new short WAV in Materials. The source is left as it was.
//!
//! The slice is cut from the source file itself, not from the decoded copy
//! the engine plays, which is mono and resampled: the new file keeps the
//! source's rate and channel count, and its samples are the source's own.
//! Written as 32-bit float, so an integer source is converted exactly (an
//! integer of up to 24 bits is exact in an `f32`).

use std::path::{Path, PathBuf};

use crate::recorder;

/// The source's frames from `start` to `end` (fractions of the file), still
/// interleaved, and where they sit.
pub struct Slice {
    pub spec: hound::WavSpec,
    pub samples: Vec<f32>,
    /// The first and one past the last frame taken.
    pub frames: (usize, usize),
}

/// Reads the frames of `path` under the region `start..end`.
pub fn read_slice(path: &str, start: f32, end: f32) -> Result<Slice, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("{path}: {e}"))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));
    let total = reader.len() as usize / channels;
    let frame = |f: f32| ((f.clamp(0.0, 1.0) as f64 * total as f64).round() as usize).min(total);
    let (a, b) = (frame(start), frame(end));
    if b <= a {
        return Err("that region is empty".into());
    }
    let (skip, take) = (a * channels, (b - a) * channels);
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .skip(skip)
            .take(take)
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .skip(skip)
                .take(take)
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    Ok(Slice {
        spec,
        samples,
        frames: (a, b),
    })
}

/// `<stem> — <n>.wav` in `dir`, the first `n` from 1 that is unused.
fn unused_name(dir: &Path, source: &Path) -> PathBuf {
    let stem = source.file_stem().map_or_else(
        || "Extract".to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    (1..)
        .map(|n| dir.join(format!("{stem} \u{2014} {n}.wav")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{stem} \u{2014} 0.wav")))
}

/// The `ICMT` text: the `shard-extract v1` header, then where the slice came
/// from and which stretch of it, in seconds of the source. Control characters
/// in the path become spaces so it cannot add a line.
fn comment(source: &str, from: f64, to: f64) -> String {
    let source: String = source
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("shard-extract v1\nfrom: {source}\nregion: {from:.3}\u{2013}{to:.3} s")
}

/// Writes the region `start..end` of the WAV at `source` into `dir` and
/// returns the new file's path.
pub fn write_region(
    dir: &Path,
    source: &str,
    start: f32,
    end: f32,
    product: &str,
    date: &str,
) -> Result<String, String> {
    let slice = read_slice(source, start, end)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = unused_name(dir, Path::new(source));
    let spec = hound::WavSpec {
        channels: slice.spec.channels,
        sample_rate: slice.spec.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let written = (|| -> Result<(), String> {
        let mut wav = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
        for s in &slice.samples {
            wav.write_sample(*s).map_err(|e| e.to_string())?;
        }
        wav.finalize().map_err(|e| e.to_string())?;
        let rate = f64::from(spec.sample_rate);
        let note = comment(
            source,
            slice.frames.0 as f64 / rate,
            slice.frames.1 as f64 / rate,
        );
        recorder::append_info_text(&path, product, date, &note).map_err(|e| e.to_string())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(path.to_string_lossy().into_owned())
}
