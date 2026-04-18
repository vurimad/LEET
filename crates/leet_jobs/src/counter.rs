use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::priority::{Priority, ScheduleParam};

// NOTE(perf): Two allocations happen per counter that would benefit from pooling when profiler
// data confirms they are hot:
//
//   1. `Arc::new(CounterInner)` — one heap allocation per Counter::new(), called on every
//      Builder fence rotation (~2x per dispatch() in a sequential chain). A future optimized
//      design could use a lockless typed slab owned by DispatcherInner, with Counter holding a
//      `(SlotIndex, Arc<DispatcherInner>)` instead of `Arc<CounterInner>`. Do NOT attempt this
//      without profiler data first — a naive `Mutex<Slab<...>>` would add a contention point hit
//      by every worker simultaneously.
//
//   2. `Vec<WaitingJob>` inside CounterInner — allocated on first park, may reallocate on
//      growth. One possible future optimization is an intrusive singly-linked waiting list:
//      replace `Vec<WaitingJob>` with `Option<Box<WaitingJob>>` as `waiting_head`, with a `next`
//      field on `WaitingJob`. This is a smaller, self-contained change that removes one
//      `Vec::new` per counter per frame with no unsafe code required. Worth evaluating
//      independently of (1).

// ---------------------------------------------------------------------------
// Internal waiting-job entry
// ---------------------------------------------------------------------------

/// A job that is parked, waiting for a counter to reach zero.
pub(crate) struct WaitingJob {
    /// The actual work to execute once the gate opens.
    pub(crate) job: Box<dyn FnOnce() + Send>,
    /// Counter to decrement when this job finishes.
    pub(crate) accumulate: Arc<CounterInner>,
    /// The priority lane this job should be queued into when released.
    pub(crate) priority: Priority,
}

// ---------------------------------------------------------------------------
// CounterInner — the shared state behind every Counter handle
// ---------------------------------------------------------------------------

/// Shared, heap-allocated counter state.
///
/// Mirrors `CounterEntry` from the C++ original:
/// - `value` tracks how many jobs are in-flight / "not done yet".
/// - `waiting` holds jobs that must not run until `value` reaches zero.
/// - The owning `Arc` keeps it alive as long as any job or builder references it.
pub struct CounterInner {
    pub(crate) value: AtomicI32,
    /// Jobs parked on this counter, guarded by a Mutex (same as C++ `waitingListLock`).
    pub(crate) waiting: Mutex<Vec<WaitingJob>>,
    pub(crate) param: ScheduleParam,
    /// Notified by `decrement` when `value` transitions to zero.
    /// `Dispatcher::flush` parks here instead of spinning when queues are empty.
    pub(crate) zero_condvar: (Mutex<()>, Condvar),
    /// Debug label for diagnostics; not used in hot paths.
    #[allow(dead_code)]
    pub(crate) debug_name: &'static str,
}

impl CounterInner {
    pub(crate) fn new(param: ScheduleParam, debug_name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            value: AtomicI32::new(0),
            waiting: Mutex::new(Vec::new()),
            param,
            zero_condvar: (Mutex::new(()), Condvar::new()),
            debug_name,
        })
    }

    /// Non-locking snapshot: true if value is currently zero.
    ///
    /// This is a best-effort check — callers must re-check under the lock
    /// before committing to any action (same as the C++ pattern).
    pub(crate) fn is_zero_snapshot(&self) -> bool {
        self.value.load(Ordering::Acquire) == 0
    }
}

// ---------------------------------------------------------------------------
// Counter — public Arc-wrapper handle
// ---------------------------------------------------------------------------

/// A reference-counted handle to a shared job counter.
///
/// Cheap to clone (just bumps an `Arc`).  Multiple clones share the same
/// underlying `CounterInner`, exactly like the C++ `Counter` copying a pointer
/// and calling `AddRef`.
#[derive(Clone)]
pub struct Counter(pub(crate) Arc<CounterInner>);

impl Counter {
    /// Create a new counter initialised to zero.
    pub fn new(param: ScheduleParam, debug_name: &'static str) -> Self {
        Self(CounterInner::new(param, debug_name))
    }

    /// True if the counter's value is currently zero (non-blocking snapshot).
    pub fn is_zero(&self) -> bool {
        self.0.is_zero_snapshot()
    }

    /// Expose the inner `Arc` for use by the dispatcher internals.
    pub(crate) fn inner(&self) -> &Arc<CounterInner> {
        &self.0
    }
}
