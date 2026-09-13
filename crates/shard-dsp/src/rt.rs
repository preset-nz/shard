//! Instruments for the audio thread.
//!
//! Two of them, both standard-library only, so this crate keeps its
//! no-dependency rule:
//!
//! - [`GuardedAlloc`] wraps the system allocator and counts every call made
//!   while [`no_alloc`] is running on the current thread. A binary opts in by
//!   registering it as the global allocator, in debug builds. Nothing is
//!   counted unless it is registered.
//! - [`BlockTimer`] holds the slowest block since it was last read.
//!
//! **The guard counts; it does not panic.** A panic that unwinds out of a
//! CoreAudio callback aborts the whole process, and the debug build is the one
//! that actually gets played. So the app reports the count beside its meters,
//! and the tests — where a failure costs nothing — assert that it is zero.
//!
//! **Frees count as well as allocations.** Dropping a `Vec` on the audio
//! thread takes the allocator's lock exactly as creating one does, and a drop
//! is the easier of the two to write without noticing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

thread_local! {
    // `const` initialisers and no destructors, on purpose. A lazily
    // initialised thread-local can allocate on first access, and the
    // allocator is the one place that must never recurse into itself.
    static FORBIDDEN: Cell<bool> = const { Cell::new(false) };
    static CAUGHT: Cell<u64> = const { Cell::new(0) };
}

/// Every allocator call caught inside a [`no_alloc`] scope, on any thread,
/// since launch.
static TOTAL: AtomicU64 = AtomicU64::new(0);

/// The system allocator, plus a count of calls made where they are forbidden.
pub struct GuardedAlloc;

impl GuardedAlloc {
    #[inline]
    fn check(&self) {
        if FORBIDDEN.try_with(|f| f.get()).unwrap_or(false) {
            TOTAL.fetch_add(1, Ordering::Relaxed);
            let _ = CAUGHT.try_with(|c| c.set(c.get() + 1));
        }
    }
}

// SAFETY: every method forwards to `System` with the caller's pointer and
// layout unchanged. The only addition is touching two thread-locals that
// neither allocate nor run destructors, and one atomic.
unsafe impl GlobalAlloc for GuardedAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.check();
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.check();
        System.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.check();
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.check();
        System.realloc(ptr, layout, new_size)
    }
}

/// Run `f` with the allocator forbidden on this thread.
///
/// Returns what `f` returned and how many allocator calls it made. The count
/// is only meaningful when [`GuardedAlloc`] is the global allocator; without
/// it this is a plain call that always reports zero.
///
/// Scopes nest, and the flag is restored even if `f` panics.
pub fn no_alloc<R>(f: impl FnOnce() -> R) -> (R, u64) {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = FORBIDDEN.try_with(|c| c.set(self.0));
        }
    }

    let before = CAUGHT.try_with(|c| c.get()).unwrap_or(0);
    let restore = Restore(FORBIDDEN.try_with(|c| c.replace(true)).unwrap_or(false));
    let out = f();
    drop(restore);
    let after = CAUGHT.try_with(|c| c.get()).unwrap_or(0);
    (out, after - before)
}

/// Allocator calls caught inside [`no_alloc`] scopes since launch, across
/// every thread. Zero when the guard is not installed.
pub fn violations() -> u64 {
    TOTAL.load(Ordering::Relaxed)
}

/// The slowest block since the last read, and the time one block is allowed.
///
/// The average is not interesting: a callback that is fast on average and
/// late once a minute still drops out once a minute. The maximum is the only
/// number that predicts a dropout, so that is all this keeps.
pub struct BlockTimer {
    worst_us: AtomicU32,
    budget_us: AtomicU32,
}

impl BlockTimer {
    pub const fn new() -> Self {
        Self {
            worst_us: AtomicU32::new(0),
            budget_us: AtomicU32::new(0),
        }
    }

    /// Called on the audio thread once per block. `budget` is how long the
    /// device allows for this block: its frames over the sample rate.
    #[inline]
    pub fn record(&self, took: Duration, budget: Duration) {
        self.worst_us.fetch_max(micros(took), Ordering::Relaxed);
        self.budget_us.store(micros(budget), Ordering::Relaxed);
    }

    /// The slowest block since the last call, in microseconds, and reset.
    /// Held until read, so a spike between two reads is never missed.
    pub fn take_worst_us(&self) -> u32 {
        self.worst_us.swap(0, Ordering::Relaxed)
    }

    /// The most recent block's budget, in microseconds.
    pub fn budget_us(&self) -> u32 {
        self.budget_us.load(Ordering::Relaxed)
    }
}

impl Default for BlockTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn micros(d: Duration) -> u32 {
    d.as_micros().min(u32::MAX as u128) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timer_holds_the_worst_block_until_it_is_read() {
        let t = BlockTimer::new();
        let budget = Duration::from_micros(5_333);
        t.record(Duration::from_micros(900), budget);
        t.record(Duration::from_micros(4_100), budget);
        t.record(Duration::from_micros(1_200), budget);

        assert_eq!(
            t.take_worst_us(),
            4_100,
            "a later, faster block must not hide the spike"
        );
        assert_eq!(t.budget_us(), 5_333);
        assert_eq!(t.take_worst_us(), 0, "reading resets the hold");
    }

    #[test]
    fn without_the_guard_installed_a_scope_reports_nothing() {
        // This test binary uses the plain system allocator, so allocating
        // inside the scope is not counted. The counting itself is tested in
        // `tests/audio_thread.rs`, which installs the guard.
        let (len, caught) = no_alloc(|| std::hint::black_box(vec![0u8; 64]).len());
        assert_eq!(len, 64);
        assert_eq!(caught, 0);
    }
}
