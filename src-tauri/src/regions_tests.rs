//! Regions and Extract (roadmap rows 47 and 49): marked on a material, saved
//! in the document, one undo step each, and written out as a new WAV.

use std::path::{Path, PathBuf};

use crate::materials::Region;
use crate::session_tests::rig;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shard-regions-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A WAV of `frames` frames at `rate` and `channels`, every sample the
/// source's own (a distinct value per sample, so a misplaced slice shows).
fn write_wav(path: &Path, rate: u32, channels: u16, frames: usize) -> Vec<f32> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let samples: Vec<f32> = (0..frames * usize::from(channels))
        .map(|i| ((i as f32) * 0.013).sin() * 0.5 + (i % 7) as f32 * 0.01)
        .collect();
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in &samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
    samples
}

/// A mono 48 kHz WAV of short noise bursts at `at` (in samples).
fn write_clicks(path: &Path, at: &[usize], len: usize) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut x = vec![0.0f32; len];
    let mut seed = 7u32;
    for &a in at {
        for i in 0..480 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            x[a + i] =
                ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0) * 0.8 * (1.0 - i as f32 / 480.0);
        }
    }
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in x {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn id_of(r: &crate::session_tests::Rig, name: &str) -> u64 {
    r.session
        .pool()
        .materials
        .iter()
        .find(|m| m.name == name)
        .map(|m| m.id)
        .expect("the material is in the pool")
}

fn r(start: f32, end: f32) -> Region {
    Region { start, end }
}

#[test]
fn regions_round_trip_through_save_and_open() {
    let d = dir("round-trip");
    write_wav(&d.join("take.wav"), 48_000, 1, 48_000);
    let a = rig();
    a.session
        .add_material(d.join("take.wav").to_str().unwrap())
        .unwrap();
    let id = id_of(&a, "take.wav");
    assert!(a.session.regions(id).is_empty(), "none to start with");
    // Added out of order, they sit in time order; a drag keeps its index.
    assert_eq!(a.session.add_region(id, 0.5, 0.75).unwrap(), 0);
    assert_eq!(a.session.add_region(id, 0.9, 0.1).unwrap(), 0);
    a.session.set_region(id, 1, 0.5, 0.8).unwrap();
    assert_eq!(a.session.regions(id), vec![r(0.1, 0.9), r(0.5, 0.8)]);
    assert!(a.session.add_region(id, 0.3, 0.3).is_err());
    assert!(a.session.remove_region(id, 5).is_err());

    let path = d.join("doc.shard");
    a.session.save(&path).unwrap();
    let b = rig();
    let report = b.session.open(&path).unwrap();
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    let id = id_of(&b, "take.wav");
    assert_eq!(b.session.regions(id), vec![r(0.1, 0.9), r(0.5, 0.8)]);

    b.session.remove_region(id, 0).unwrap();
    assert_eq!(b.session.regions(id), vec![r(0.5, 0.8)]);
    assert_eq!(b.session.undo().unwrap().as_deref(), Some("Remove Region"));
    assert_eq!(b.session.regions(id).len(), 2);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_drag_is_one_undo_step() {
    let d = dir("drag");
    write_wav(&d.join("take.wav"), 48_000, 1, 48_000);
    let a = rig();
    a.session
        .add_material(d.join("take.wav").to_str().unwrap())
        .unwrap();
    let id = id_of(&a, "take.wav");
    a.session.add_region(id, 0.2, 0.4).unwrap();
    let before = a.session.history_labels().0.len();
    for end in [0.5, 0.6, 0.7] {
        a.session.set_region(id, 0, 0.2, end).unwrap();
    }
    assert_eq!(a.session.history_labels().0.len(), before + 1);
    a.session.undo().unwrap();
    assert_eq!(a.session.regions(id), vec![r(0.2, 0.4)]);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn detect_is_one_undo_step_and_undo_restores_the_regions() {
    let d = dir("detect");
    let at = [4_800, 24_000, 57_600, 96_000];
    write_clicks(&d.join("hits.wav"), &at, 144_000);
    let a = rig();
    a.session
        .add_material(d.join("hits.wav").to_str().unwrap())
        .unwrap();
    let id = id_of(&a, "hits.wav");
    a.session.add_region(id, 0.0, 0.01).unwrap();
    let before = a.session.history_labels().0.len();

    let n = a.session.detect_regions(id, 0.5).unwrap();
    assert_eq!(n, at.len());
    let found = a.session.regions(id);
    assert_eq!(found.len(), at.len());
    for (region, &hit) in found.iter().zip(&at) {
        let want = hit as f32 / 144_000.0;
        assert!(
            (region.start - want).abs() < 0.002 * 48.0 / 144.0,
            "{region:?} vs {want}"
        );
        assert!(region.end > region.start);
    }
    assert_eq!(a.session.history_labels().0.len(), before + 1);

    assert_eq!(a.session.undo().unwrap().as_deref(), Some("Detect Regions"));
    assert_eq!(a.session.regions(id), vec![r(0.0, 0.01)]);
    std::fs::remove_dir_all(&d).ok();
}

/// The `ICMT` text of a WAV, read by walking its chunks.
fn comment(bytes: &[u8]) -> String {
    let (mut at, mut found) = (12, None);
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..at + 8 + n];
        if id == b"LIST" && &body[..4] == b"INFO" {
            let mut p = 4;
            while p + 8 <= body.len() {
                let m = u32::from_le_bytes(body[p + 4..p + 8].try_into().unwrap()) as usize;
                if &body[p..p + 4] == b"ICMT" {
                    found = Some(String::from_utf8(body[p + 8..p + 7 + m].to_vec()).unwrap());
                }
                p += 8 + m + m % 2;
            }
        }
        at += 8 + n + n % 2;
    }
    found.expect("an ICMT")
}

#[test]
fn extract_writes_the_exact_slice_and_adds_one_material() {
    let d = dir("extract");
    let src = d.join("take.wav");
    // Stereo at 44.1 kHz: the engine plays it mono at 48 kHz, the extract
    // keeps what the file was.
    let samples = write_wav(&src, 44_100, 2, 44_100);
    let a = rig();
    a.session.add_material(src.to_str().unwrap()).unwrap();
    let id = id_of(&a, "take.wav");
    a.session.add_region(id, 0.25, 0.5).unwrap();
    let pool_before = a.session.pool().materials.len();
    let wires_before = a.session.pool().wires;
    let steps = a.session.history_labels().0.len();

    let out = d.join("out");
    let path = a
        .session
        .extract_region_to(&out, id, 0, "Shard", "2026-10-13")
        .unwrap();
    assert_eq!(Path::new(&path).file_name().unwrap(), "take \u{2014} 1.wav");

    let mut reader = hound::WavReader::open(&path).unwrap();
    let spec = reader.spec();
    assert_eq!(
        (
            spec.channels,
            spec.sample_rate,
            spec.bits_per_sample,
            spec.sample_format
        ),
        (2, 44_100, 32, hound::SampleFormat::Float)
    );
    let got: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    let want = &samples[11_025 * 2..22_050 * 2];
    assert!(got == want, "the slice is the source's own samples");

    let text = comment(&std::fs::read(&path).unwrap());
    assert_eq!(
        text,
        format!(
            "shard-extract v1\nfrom: {}\nregion: 0.250\u{2013}0.500 s",
            src.display()
        )
    );

    // One material more, wired to nothing, in one undo step.
    let pool = a.session.pool();
    assert_eq!(pool.materials.len(), pool_before + 1);
    assert_eq!(pool.wires, wires_before);
    assert_eq!(a.session.history_labels().0.len(), steps + 1);
    assert_eq!(a.session.undo().unwrap().as_deref(), Some("Extract"));
    assert_eq!(a.session.pool().materials.len(), pool_before);
    assert!(Path::new(&path).exists(), "the file stays");

    // The same region again takes the next free name.
    let again = a
        .session
        .extract_region_to(&out, id, 0, "Shard", "2026-10-13")
        .unwrap();
    assert_eq!(
        Path::new(&again).file_name().unwrap(),
        "take \u{2014} 2.wav"
    );
    // The built-in drone has no file; a missing region is refused.
    assert!(a
        .session
        .extract_region_to(&out, id, 3, "Shard", "x")
        .is_err());
    std::fs::remove_dir_all(&d).ok();
}
