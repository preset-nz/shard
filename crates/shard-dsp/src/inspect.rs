//! The grain log: what the cloud actually did, so it can be looked at.
//!
//! A grain is not an object with a lifetime you can inspect. It is an *event*.
//! The pool reuses slots, a grain is six numbers, and at the default settings
//! only about a dozen exist at any instant — stop the transport and they are
//! gone, because stopping clears the pool. "Show me the grains" cannot mean
//! "show me the live ones"; there are never enough of them and they are never
//! still.
//!
//! So what gets recorded here is **spawns**, not grains. Every time the
//! scheduler fires, the six numbers that define that grain are pushed into a
//! fixed ring. The UI drains the ring, keeps a scrollback, and can freeze it.
//! Because those six numbers fully determine a grain, any entry can be handed
//! back to the engine and played again exactly as it was.
//!
//! Single producer (the audio thread), single consumer (the UI poll). The ring
//! is preallocated and every field is an atomic, so writing costs six relaxed
//! stores and takes no lock.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// How many spawns are kept. At the ceiling of 200 grains a second and a 30 Hz
/// poll that is roughly seven per drain, so the ring holds about a minute of
/// slack — far more than it needs, and still only a few kilobytes.
const CAPACITY: usize = 1024;

/// One grain as it was spawned. Everything needed to draw it, and everything
/// needed to play it again.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GrainSpawn {
    /// Read position, 0 to 1 across the **whole source**, not the trim window.
    /// Stored that way so a mark lands under the right part of the drawn
    /// waveform whatever the trim is doing.
    pub position: f32,
    /// Samples advanced per output sample. Negative means it played backwards.
    pub rate: f32,
    /// Length in samples.
    pub len: f32,
    /// 0 hard left, 1 hard right.
    pub pan: f32,
    /// Index into `Window::ALL`.
    pub window: u32,
    /// Spawn number since the engine started. Strictly increasing, so the UI
    /// can order entries and notice when it missed some.
    pub seq: u64,
}

/// One ring slot. `seq` is both the entry's identity and its publication
/// flag — see `push` and `drain`.
#[derive(Default)]
struct Slot {
    seq: AtomicU64,
    position: AtomicU32,
    rate: AtomicU32,
    len: AtomicU32,
    pan: AtomicU32,
    window: AtomicU32,
}

pub struct GrainLog {
    slots: Vec<Slot>,
    /// Total spawns ever written. The write index is this modulo capacity.
    written: AtomicU64,
    /// Total drained by the consumer.
    read: AtomicU64,
}

impl Default for GrainLog {
    fn default() -> Self {
        Self::new()
    }
}

impl GrainLog {
    pub fn new() -> Self {
        Self {
            slots: (0..CAPACITY).map(|_| Slot::default()).collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
        }
    }

    /// Record a spawn. Called from the audio thread, once per grain.
    ///
    /// `seq` is cleared before the fields are written and restored after, so a
    /// consumer that reads a slot mid-write sees a sequence number that does
    /// not match and discards the entry rather than drawing a grain whose
    /// position came from one spawn and whose length came from the next. That
    /// is the whole reason the sequence number is stored alongside the data
    /// rather than only in the counter.
    pub fn push(&self, g: &GrainSpawn) {
        let n = self.written.load(Ordering::Relaxed);
        let slot = &self.slots[(n as usize) % CAPACITY];

        slot.seq.store(0, Ordering::Relaxed);
        // Release, so the zeroed sequence is visible before the fields move.
        std::sync::atomic::fence(Ordering::Release);

        slot.position.store(g.position.to_bits(), Ordering::Relaxed);
        slot.rate.store(g.rate.to_bits(), Ordering::Relaxed);
        slot.len.store(g.len.to_bits(), Ordering::Relaxed);
        slot.pan.store(g.pan.to_bits(), Ordering::Relaxed);
        slot.window.store(g.window, Ordering::Relaxed);

        // Publish. Sequence numbers start at one so zero can mean "in flux".
        slot.seq.store(n + 1, Ordering::Release);
        self.written.store(n + 1, Ordering::Release);
    }

    /// Take everything written since the last call. Called from the UI thread.
    ///
    /// When the producer has lapped the consumer — the UI stalled, or the app
    /// was in the background — the oldest entries are simply gone and the
    /// drain resumes at the oldest slot still intact. `seq` tells the caller
    /// that happened.
    pub fn drain(&self, out: &mut Vec<GrainSpawn>) {
        let written = self.written.load(Ordering::Acquire);
        let mut read = self.read.load(Ordering::Relaxed);

        // Skip whatever has already been overwritten.
        let cap = CAPACITY as u64;
        if written.saturating_sub(read) > cap {
            read = written - cap;
        }

        for n in read..written {
            let slot = &self.slots[(n as usize) % CAPACITY];
            let seq = slot.seq.load(Ordering::Acquire);
            if seq != n + 1 {
                // Being written, or already lapped. Either way not coherent.
                continue;
            }
            let g = GrainSpawn {
                position: f32::from_bits(slot.position.load(Ordering::Relaxed)),
                rate: f32::from_bits(slot.rate.load(Ordering::Relaxed)),
                len: f32::from_bits(slot.len.load(Ordering::Relaxed)),
                pan: f32::from_bits(slot.pan.load(Ordering::Relaxed)),
                window: slot.window.load(Ordering::Relaxed),
                seq,
            };
            // Re-check: if the producer started overwriting this slot while we
            // were copying it out, the read is torn and the entry is dropped.
            if slot.seq.load(Ordering::Acquire) == seq {
                out.push(g);
            }
        }

        self.read.store(written, Ordering::Relaxed);
    }

    /// Total spawns since the engine started, drained or not.
    pub fn spawned(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn(seq: u64) -> GrainSpawn {
        GrainSpawn {
            position: seq as f32 * 0.001,
            rate: 1.0,
            len: 8_000.0,
            pan: 0.5,
            window: 0,
            seq,
        }
    }

    #[test]
    fn drains_what_was_pushed_in_order() {
        let log = GrainLog::new();
        for i in 0..10 {
            log.push(&spawn(i));
        }
        let mut out = Vec::new();
        log.drain(&mut out);
        assert_eq!(out.len(), 10);
        for (i, g) in out.iter().enumerate() {
            assert_eq!(g.seq, i as u64 + 1, "sequence numbers must be in order");
            assert!((g.position - i as f32 * 0.001).abs() < 1e-6);
        }
    }

    #[test]
    fn a_second_drain_returns_only_what_is_new() {
        let log = GrainLog::new();
        for i in 0..5 {
            log.push(&spawn(i));
        }
        let mut out = Vec::new();
        log.drain(&mut out);
        out.clear();
        log.drain(&mut out);
        assert!(out.is_empty(), "drained the same entries twice");

        log.push(&spawn(99));
        log.drain(&mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn overrunning_the_ring_drops_the_oldest_not_the_newest() {
        // A UI that stalls must come back to recent grains, not to a minute of
        // stale ones. Losing the oldest is the correct failure.
        let log = GrainLog::new();
        let total = CAPACITY + 500;
        for i in 0..total {
            log.push(&spawn(i as u64));
        }
        let mut out = Vec::new();
        log.drain(&mut out);
        assert_eq!(out.len(), CAPACITY);
        assert_eq!(
            out.last().unwrap().seq,
            total as u64,
            "the newest entry must survive"
        );
    }

    #[test]
    fn survives_a_producer_running_flat_out_against_a_consumer() {
        // The audio thread never waits for the UI. This drains while a
        // producer laps the ring many times over, and asserts the property
        // that actually matters: every entry that comes out was pushed as a
        // unit. A torn read would show a grain whose length came from one
        // spawn and whose pan came from another.
        use std::sync::Arc;

        let log = Arc::new(GrainLog::new());
        let pushes = CAPACITY as u64 * 200;

        let producer = {
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                for i in 0..pushes {
                    log.push(&spawn(i));
                }
            })
        };

        let mut out = Vec::new();
        let mut seen = 0u64;
        let check = |out: &mut Vec<GrainSpawn>, seen: &mut u64| {
            out.clear();
            log.drain(out);
            for g in out.iter() {
                assert_eq!(g.rate, 1.0, "torn read: rate");
                assert_eq!(g.len, 8_000.0, "torn read: len");
                assert_eq!(g.pan, 0.5, "torn read: pan");
                assert!(g.seq > 0, "an unpublished slot was handed out");
                *seen += 1;
            }
        };

        while !producer.is_finished() {
            check(&mut out, &mut seen);
        }
        producer.join().expect("producer panicked");
        // One last drain, for whatever landed after the final check.
        check(&mut out, &mut seen);

        assert!(seen > 0, "the consumer saw nothing at all");
        assert!(
            seen <= pushes,
            "drained {seen} entries from {pushes} pushes"
        );
    }
}
