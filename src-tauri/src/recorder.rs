//! Recording a take (epic 44, story 2): the engine's tap, pushed from the
//! audio callback into a lock-free ring, drained by a writer thread into a
//! 32-bit float stereo WAV in `~/Music/Shard/Materials`.
//!
//! **Nothing in the callback waits.** The ring is preallocated and its two
//! ends are atomics. When the writer falls behind and the ring is full, the
//! block is dropped and the frames counted, never blocked on. The tap is
//! chosen on the command thread, parked in an atomic, and applied by the
//! callback at the top of a block (no mutex, so none of the macOS
//! first-lock-allocates trouble).
//!
//! Times in file names are UTC: no time crate is a dependency.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use shard_dsp::{Engine, Tap, KITS, VOICES};

/// Floats the ring holds: a power of two, 2^20, which is 10.9 s of stereo at
/// 48 kHz and 4 MB.
pub const RING_FLOATS: usize = 1 << 20;

/// A single-producer, single-consumer ring of f32. The producer is the audio
/// callback, the consumer the writer thread. Indices only ever grow and wrap
/// through the mask, so full and empty need no spare slot.
pub struct Ring {
    buf: Box<[AtomicU32]>,
    mask: usize,
    /// Floats written, by the producer.
    head: AtomicUsize,
    /// Floats read, by the consumer.
    tail: AtomicUsize,
    /// Frames dropped because the ring was full.
    dropped: AtomicU64,
    /// Frames accepted.
    pushed: AtomicU64,
}

impl Ring {
    /// `capacity` is rounded up to a power of two.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(2).next_power_of_two();
        Self {
            buf: (0..cap).map(|_| AtomicU32::new(0)).collect(),
            mask: cap - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            pushed: AtomicU64::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Producer: push interleaved stereo. Whole frames only; what does not
    /// fit is dropped and counted. Never blocks, never allocates.
    pub fn push(&self, samples: &[f32]) {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let free = self.capacity() - head.wrapping_sub(tail);
        let n = samples.len().min(free) & !1;
        for (i, s) in samples[..n].iter().enumerate() {
            self.buf[head.wrapping_add(i) & self.mask].store(s.to_bits(), Ordering::Relaxed);
        }
        self.head.store(head.wrapping_add(n), Ordering::Release);
        self.pushed.fetch_add((n / 2) as u64, Ordering::Relaxed);
        let lost = (samples.len() - n) / 2;
        if lost > 0 {
            self.dropped.fetch_add(lost as u64, Ordering::Relaxed);
        }
    }

    /// Consumer: read up to `out.len()` floats, returning how many.
    pub fn pop(&self, out: &mut [f32]) -> usize {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        let n = out.len().min(head.wrapping_sub(tail));
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = f32::from_bits(self.buf[tail.wrapping_add(i) & self.mask].load(Ordering::Relaxed));
        }
        self.tail.store(tail.wrapping_add(n), Ordering::Release);
        n
    }

    /// Floats the producer could push now.
    #[cfg(test)]
    fn free(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        self.capacity() - head.wrapping_sub(self.tail.load(Ordering::Acquire))
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn pushed(&self) -> u64 {
        self.pushed.load(Ordering::Relaxed)
    }

    /// Start a take clean. Only while neither end is running.
    fn reset(&self) {
        let head = self.head.load(Ordering::Acquire);
        self.tail.store(head, Ordering::Release);
        self.dropped.store(0, Ordering::Relaxed);
        self.pushed.store(0, Ordering::Relaxed);
    }
}

/// What a take is recorded from, as the webview names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Source {
    /// The string "master".
    Master(MasterTag),
    /// A track's place in the arrangement's order.
    Track(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MasterTag {
    Master,
}

impl Source {
    #[cfg(test)]
    pub const MASTER: Source = Source::Master(MasterTag::Master);
}

/// Where a take came from: what goes into the WAV's `INFO` chunk, for
/// whatever reads the file later (Strata) to find its way back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeMeta {
    /// The product name, `ISFT`.
    pub product: String,
    /// The document's absolute path; none while it is untitled.
    pub document: Option<PathBuf>,
    /// The patch the take was made from; none for a drums track or when the
    /// master is recorded from no track.
    pub patch: Option<String>,
    /// The track's name; none for the master.
    pub track: Option<String>,
    /// `master` or `track`.
    pub source: &'static str,
}

impl TakeMeta {
    /// The `ICMT` text: line-based, one `key: value` per line after the
    /// `shard-take v1` header, a line omitted when it has no value. Control
    /// characters in a value become spaces so a value cannot add a line.
    pub fn comment(&self) -> String {
        let clean = |s: &str| -> String {
            s.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect()
        };
        let mut lines = vec!["shard-take v1".to_string()];
        if let Some(d) = &self.document {
            lines.push(format!("document: {}", clean(&d.to_string_lossy())));
        }
        if let Some(p) = &self.patch {
            lines.push(format!("patch: {}", clean(p)));
        }
        if let Some(t) = &self.track {
            lines.push(format!("track: {}", clean(t)));
        }
        lines.push(format!("source: {}", self.source));
        lines.join("\n")
    }
}

/// `YYYY-MM-DD` (UTC) for `now`.
pub fn date_stamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Appends a `LIST` chunk of type `INFO` to a finished WAV (`ISFT`, `ICRD`,
/// `ICMT`; strings NUL-terminated, each subchunk padded to an even length)
/// and rewrites the RIFF size in the header. hound cannot write one.
pub fn append_info(path: &Path, meta: &TakeMeta, date: &str) -> std::io::Result<()> {
    fn sub(out: &mut Vec<u8>, id: &[u8; 4], text: &str) {
        let mut body = text.as_bytes().to_vec();
        body.push(0);
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
    }
    let mut body = b"INFO".to_vec();
    sub(&mut body, b"ISFT", &meta.product);
    sub(&mut body, b"ICRD", date);
    sub(&mut body, b"ICMT", &meta.comment());

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut head = [0u8; 12];
    file.read_exact(&mut head)?;
    if &head[..4] != b"RIFF" || &head[8..] != b"WAVE" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        ));
    }
    let mut len = file.seek(SeekFrom::End(0))?;
    if len % 2 == 1 {
        file.write_all(&[0])?;
        len += 1;
    }
    file.write_all(b"LIST")?;
    file.write_all(&(body.len() as u32).to_le_bytes())?;
    file.write_all(&body)?;
    let total = len + 8 + body.len() as u64;
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&((total - 8) as u32).to_le_bytes())?;
    file.sync_all()
}

/// A finished take.
#[derive(Debug, Clone, Serialize)]
pub struct TakeInfo {
    pub path: String,
    pub seconds: f64,
    pub dropped: u64,
}

/// What `recording` reports.
#[derive(Debug, Clone, Serialize)]
pub struct RecordingState {
    pub active: bool,
    pub source: Option<Source>,
    pub elapsed_seconds: f64,
    pub dropped: u64,
}

struct Running {
    path: PathBuf,
    source: Source,
    stop: Arc<AtomicBool>,
    writer: JoinHandle<Result<(), String>>,
}

pub struct Recorder {
    ring: Arc<Ring>,
    /// The tap the command thread wants, encoded (`encode`); zero is off.
    want: AtomicU32,
    /// The tap the callback last applied, the same way: the stop handshake.
    seen: AtomicU32,
    sample_rate: u32,
    running: Mutex<Option<Running>>,
}

fn encode(tap: Option<Tap>) -> u32 {
    match tap {
        None => 0,
        Some(Tap::Master) => 1,
        Some(Tap::Voice(i)) => 2 + i as u32,
        Some(Tap::Kit(k)) => 2 + VOICES as u32 + k as u32,
    }
}

fn decode(code: u32) -> Option<Tap> {
    let n = code as usize;
    match n {
        0 => None,
        1 => Some(Tap::Master),
        n if n < 2 + VOICES => Some(Tap::Voice(n - 2)),
        n if n < 2 + VOICES + KITS => Some(Tap::Kit(n - 2 - VOICES)),
        _ => None,
    }
}

impl Recorder {
    pub fn new(sample_rate: f32) -> Self {
        Self::with_capacity(sample_rate, RING_FLOATS)
    }

    pub fn with_capacity(sample_rate: f32, floats: usize) -> Self {
        Self {
            ring: Arc::new(Ring::new(floats)),
            want: AtomicU32::new(0),
            seen: AtomicU32::new(0),
            sample_rate: sample_rate.round() as u32,
            running: Mutex::new(None),
        }
    }

    /// Audio thread, at the top of a block: apply the wanted tap to the
    /// engine. `applied` is the callback's own record of what it set, so the
    /// engine is only touched when the want changes.
    pub fn before_render(&self, engine: &mut Engine, applied: &mut u32) {
        let want = self.want.load(Ordering::Acquire);
        if want != *applied {
            engine.set_tap(decode(want));
            *applied = want;
            self.seen.store(want, Ordering::Release);
        }
    }

    /// Audio thread, after the block is rendered: push what was tapped.
    pub fn after_render(&self, engine: &Engine, applied: u32) {
        if applied != 0 {
            self.ring.push(engine.tapped());
        }
    }

    /// Start a take from `tap` into a new file in `dir`, named for `now` and
    /// `track`. The folder is made if it is missing.
    pub fn start(
        &self,
        dir: &Path,
        tap: Tap,
        source: Source,
        track: Option<&str>,
        meta: TakeMeta,
        now: SystemTime,
    ) -> Result<PathBuf, String> {
        let mut running = lock(&self.running);
        if running.is_some() {
            return Err("already recording".into());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = unused(dir, &take_name(now, track));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let file = File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let wav = hound::WavWriter::new(BufWriter::new(file), spec).map_err(|e| e.to_string())?;

        self.ring.reset();
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (ring, stop) = (Arc::clone(&self.ring), Arc::clone(&stop));
            let (path, date) = (path.clone(), date_stamp(now));
            std::thread::Builder::new()
                .name("shard-take-writer".into())
                .spawn(move || write_out(&ring, wav, &stop, &path, &meta, &date))
                .map_err(|e| format!("could not start the writer: {e}"))?
        };
        *running = Some(Running {
            path: path.clone(),
            source,
            stop,
            writer,
        });
        self.want.store(encode(Some(tap)), Ordering::Release);
        Ok(path)
    }

    /// End the take: stop the tap, let the writer drain the ring and finalise
    /// the WAV. Blocks the command thread (never the callback) for as long as
    /// the disk takes.
    pub fn stop(&self) -> Result<TakeInfo, String> {
        let Some(run) = lock(&self.running).take() else {
            return Err("not recording".into());
        };
        self.want.store(0, Ordering::Release);
        // Wait for the callback to take the tap off, so nothing is pushed
        // after the writer is told to finish. A stream that is not running
        // (a test) never answers; stop waiting after half a second.
        let give_up = std::time::Instant::now() + Duration::from_millis(500);
        while self.seen.load(Ordering::Acquire) != 0 && std::time::Instant::now() < give_up {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.seen.store(0, Ordering::Release);
        run.stop.store(true, Ordering::Release);
        run.writer
            .join()
            .map_err(|_| "the take writer panicked".to_string())??;
        let frames = self.ring.pushed();
        Ok(TakeInfo {
            path: run.path.to_string_lossy().into_owned(),
            seconds: frames as f64 / f64::from(self.sample_rate),
            dropped: self.ring.dropped(),
        })
    }

    pub fn state(&self) -> RecordingState {
        let running = lock(&self.running);
        let dropped = self.ring.dropped();
        match running.as_ref() {
            Some(r) => RecordingState {
                active: true,
                source: Some(r.source),
                elapsed_seconds: (self.ring.pushed() + dropped) as f64
                    / f64::from(self.sample_rate),
                dropped,
            },
            None => RecordingState {
                active: false,
                source: None,
                elapsed_seconds: 0.0,
                dropped: 0,
            },
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Drain the ring to the file until told to stop and the ring is empty. A
/// write error is remembered and the ring still drained, so the callback's
/// side never notices.
fn write_out(
    ring: &Ring,
    mut wav: hound::WavWriter<BufWriter<File>>,
    stop: &AtomicBool,
    path: &Path,
    meta: &TakeMeta,
    date: &str,
) -> Result<(), String> {
    let mut chunk = vec![0.0f32; 8192];
    let mut failed: Option<String> = None;
    loop {
        // Read the flag before the ring: whatever was pushed before stop was
        // set is then certain to be seen.
        let stopping = stop.load(Ordering::Acquire);
        let n = ring.pop(&mut chunk);
        if n == 0 {
            if stopping {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        if failed.is_none() {
            for s in &chunk[..n] {
                if let Err(e) = wav.write_sample(*s) {
                    failed = Some(e.to_string());
                    break;
                }
            }
        }
    }
    match failed {
        Some(e) => Err(e),
        None => {
            wav.finalize().map_err(|e| e.to_string())?;
            append_info(path, meta, date).map_err(|e| format!("{}: {e}", path.display()))
        }
    }
}

/// `~/Music/Shard/Materials`.
pub fn materials_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("no home folder")?;
    Ok(PathBuf::from(home).join("Music/Shard/Materials"))
}

/// `Take YYYY-MM-DD HHMMSS.wav`, with ` — <track>` before the extension for
/// a track's take. UTC.
pub fn take_name(now: SystemTime, track: Option<&str>) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    let stamp = format!(
        "Take {y:04}-{m:02}-{d:02} {:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
    match track {
        Some(t) => format!("{stamp} \u{2014} {t}.wav"),
        None => format!("{stamp}.wav"),
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Hinnant).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `name` in `dir`, or `name (2)`, `(3)`… when a take of that second exists.
fn unused(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name.strip_suffix(".wav").unwrap_or(name);
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).wav")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shard_dsp::rt::no_alloc;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("shard-recorder-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn meta() -> TakeMeta {
        TakeMeta {
            product: "Shard".into(),
            document: Some(PathBuf::from("/Users/me/Songs/a song.shard")),
            patch: Some("Patch 2".into()),
            track: Some("Track 1".into()),
            source: "track",
        }
    }

    /// Every chunk of a RIFF/WAVE file as (id, body), walked by the header.
    fn chunks(bytes: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let riff = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff, bytes.len() - 8, "the RIFF size is the file's");
        let (mut at, mut out) = (12, Vec::new());
        while at < bytes.len() {
            let id: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
            let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            out.push((id, bytes[at + 8..at + 8 + n].to_vec()));
            at += 8 + n + n % 2;
        }
        assert_eq!(at, bytes.len());
        out
    }

    fn info(body: &[u8]) -> Vec<([u8; 4], String)> {
        assert_eq!(&body[..4], b"INFO");
        let (mut at, mut out) = (4, Vec::new());
        while at < body.len() {
            let id: [u8; 4] = body[at..at + 4].try_into().unwrap();
            let n = u32::from_le_bytes(body[at + 4..at + 8].try_into().unwrap()) as usize;
            let text = &body[at + 8..at + 8 + n];
            assert_eq!(text.last(), Some(&0), "NUL-terminated");
            out.push((id, String::from_utf8(text[..n - 1].to_vec()).unwrap()));
            at += 8 + n + n % 2;
        }
        out
    }

    fn read(path: &Path) -> (hound::WavSpec, Vec<f32>) {
        let mut r = hound::WavReader::open(path).expect("a valid WAV");
        let spec = r.spec();
        (spec, r.samples::<f32>().map(Result::unwrap).collect())
    }

    #[test]
    fn a_known_signal_comes_back_identical_through_ring_and_writer() {
        let dir = scratch_dir("roundtrip");
        // A ring much smaller than the signal, pushed in callback-sized
        // blocks while the writer drains, so the wrap is exercised.
        let rec = Recorder::with_capacity(48_000.0, 4096);
        rec.start(
            &dir,
            Tap::Master,
            Source::MASTER,
            None,
            meta(),
            SystemTime::now(),
        )
        .unwrap();
        let signal: Vec<f32> = (0..2 * 20_000)
            .map(|i| (i as f32 * 0.0137).sin() * 0.9)
            .collect();
        for block in signal.chunks(512) {
            // Block until it fits: the writer is fast, and this test is about
            // the content, not the drops.
            while rec.ring.free() < block.len() {
                std::thread::sleep(Duration::from_millis(1));
            }
            rec.ring.push(block);
        }
        let info = rec.stop().unwrap();
        assert_eq!(info.dropped, 0);
        assert!((info.seconds - 20_000.0 / 48_000.0).abs() < 1e-9);
        let (spec, back) = read(Path::new(&info.path));
        assert_eq!(
            (spec.channels, spec.sample_rate, spec.bits_per_sample),
            (2, 48_000, 32)
        );
        assert_eq!(spec.sample_format, hound::SampleFormat::Float);
        assert!(back == signal, "samples differ");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_ring_drops_and_counts_without_blocking() {
        let ring = Ring::new(1024);
        let block = vec![0.5f32; 512];
        let started = std::time::Instant::now();
        // Nobody drains: two blocks fit, the rest are dropped.
        for _ in 0..10 {
            ring.push(&block);
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(ring.pushed(), 2 * 256);
        assert_eq!(ring.dropped(), 8 * 256);
        // A partial fit keeps whole frames only.
        let mut sink = vec![0.0; 100];
        assert_eq!(ring.pop(&mut sink), 100);
        ring.push(&block);
        assert_eq!(ring.pushed(), 2 * 256 + 50);
        assert_eq!(ring.dropped(), 8 * 256 + (512 - 100) / 2);
    }

    #[test]
    fn stop_with_a_stuffed_ring_finalises_a_valid_wav() {
        let dir = scratch_dir("stuffed");
        let rec = Recorder::with_capacity(44_100.0, 2048);
        rec.start(
            &dir,
            Tap::Voice(0),
            Source::Track(0),
            Some("Track 1"),
            meta(),
            UNIX_EPOCH + Duration::from_secs(1_791_900_000),
        )
        .unwrap();
        assert!(rec.state().active);
        // Faster than any writer could drain.
        let block = vec![0.25f32; 2048];
        for _ in 0..50 {
            rec.ring.push(&block);
        }
        let info = rec.stop().unwrap();
        assert!(!rec.state().active);
        assert!(info.dropped > 0);
        assert!(info.path.ends_with("Track 1.wav"), "{}", info.path);
        let (spec, back) = read(Path::new(&info.path));
        assert_eq!(spec.sample_rate, 44_100);
        assert_eq!(back.len() as f64 / 2.0, info.seconds * 44_100.0);
        assert!(back.iter().all(|s| *s == 0.25));
        assert!(rec.stop().is_err(), "a second stop has nothing to stop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_engines_tapped_output_is_recorded_whole() {
        let dir = scratch_dir("engine");
        let rec = Recorder::new(48_000.0);
        let mut engine = Engine::new(48_000.0, 64);
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (core::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect();
        engine.set_source(tone);
        engine.set_playing(true);
        let bank = shard_dsp::ParamBank::new();
        rec.start(
            &dir,
            Tap::Master,
            Source::MASTER,
            None,
            meta(),
            SystemTime::now(),
        )
        .unwrap();
        let (mut applied, mut heard) = (0, Vec::new());
        let mut out = vec![0.0f32; 512];
        for _ in 0..100 {
            rec.before_render(&mut engine, &mut applied);
            engine.process_block(&mut out, &bank);
            rec.after_render(&engine, applied);
            heard.extend_from_slice(&out);
        }
        // Stop from this thread would wait for the "callback" to answer; run
        // one more block, as the stream would, while it does.
        let stopper = std::thread::scope(|s| {
            let t = s.spawn(|| rec.stop());
            while !t.is_finished() {
                rec.before_render(&mut engine, &mut applied);
                engine.process_block(&mut out, &bank);
                rec.after_render(&engine, applied);
                if applied == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            t.join().unwrap()
        });
        let info = stopper.unwrap();
        let (_, back) = read(Path::new(&info.path));
        assert!(back.len() >= heard.len());
        assert!(
            back[..heard.len()] == heard[..],
            "the take is not the output"
        );
        assert!(heard.iter().any(|s| s.abs() > 0.05));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_callbacks_side_never_touches_the_allocator() {
        let rec = Recorder::new(48_000.0);
        let mut engine = Engine::new(48_000.0, 64);
        let bank = shard_dsp::ParamBank::new();
        let mut out = vec![0.0f32; 1024];
        let mut applied = 0;
        let ((), caught) = no_alloc(|| {
            for block in 0..200 {
                rec.want.store(
                    encode([None, Some(Tap::Master), Some(Tap::Voice(0))][block % 3]),
                    Ordering::Release,
                );
                rec.before_render(&mut engine, &mut applied);
                engine.process_block(&mut out, &bank);
                rec.after_render(&engine, applied);
            }
        });
        assert_eq!(caught, 0);
    }

    #[test]
    fn taps_survive_the_hand_off_encoding() {
        for tap in [None, Some(Tap::Master)]
            .into_iter()
            .chain((0..VOICES).map(|i| Some(Tap::Voice(i))))
            .chain((0..KITS).map(|k| Some(Tap::Kit(k))))
        {
            assert_eq!(decode(encode(tap)), tap);
        }
    }

    #[test]
    fn take_names_carry_the_date_and_the_track() {
        // 2026-10-13 21:05:09 UTC.
        let at = UNIX_EPOCH + Duration::from_secs(1_791_925_509);
        assert_eq!(take_name(at, None), "Take 2026-10-13 210509.wav");
        assert_eq!(
            take_name(at, Some("Drums 1")),
            "Take 2026-10-13 210509 \u{2014} Drums 1.wav"
        );
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn a_take_in_the_same_second_gets_its_own_file() {
        let dir = scratch_dir("same-second");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Take x.wav"), b"").unwrap();
        assert_eq!(unused(&dir, "Take x.wav"), dir.join("Take x (2).wav"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_source_reads_master_or_a_track_number() {
        assert_eq!(
            serde_json::from_str::<Source>("\"master\"").unwrap(),
            Source::MASTER
        );
        assert_eq!(
            serde_json::from_str::<Source>("3").unwrap(),
            Source::Track(3)
        );
        assert!(serde_json::from_str::<Source>("\"nope\"").is_err());
    }

    #[test]
    fn the_info_chunk_reads_back_and_the_samples_are_untouched() {
        let dir = scratch_dir("info");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let signal: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.05).sin()).collect();
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for s in &signal {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();

        append_info(&path, &meta(), "2026-10-13").unwrap();

        let all = chunks(&std::fs::read(&path).unwrap());
        let list = all.iter().find(|(id, _)| id == b"LIST").expect("a LIST");
        assert_eq!(
            info(&list.1),
            vec![
                (*b"ISFT", "Shard".to_string()),
                (*b"ICRD", "2026-10-13".to_string()),
                (
                    *b"ICMT",
                    "shard-take v1\ndocument: /Users/me/Songs/a song.shard\npatch: Patch 2\ntrack: Track 1\nsource: track"
                        .to_string()
                ),
            ]
        );
        assert!(all.iter().any(|(id, _)| id == b"data"));
        let (spec2, back) = read(&path);
        assert_eq!(spec2, spec);
        assert!(back == signal, "samples differ");
    }

    #[test]
    fn the_comment_omits_what_is_absent_and_cannot_gain_a_line() {
        let master = TakeMeta {
            product: "Shard".into(),
            document: None,
            patch: Some("a\nb".into()),
            track: None,
            source: "master",
        };
        assert_eq!(
            master.comment(),
            "shard-take v1\npatch: a b\nsource: master"
        );
        assert_eq!(
            date_stamp(UNIX_EPOCH + Duration::from_secs(1_791_925_509)),
            "2026-10-13"
        );
    }
}
