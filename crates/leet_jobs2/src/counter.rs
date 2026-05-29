//! Counter state and the public counter handle.
//!
//! `CounterEntry` owns the outstanding-work count and the waiting list for jobs
//! parked behind that count. `Counter` is the public move-only handle that keeps
//! the entry alive and carries the private dispatcher handle needed for
//! dependency composition and deferrals.

use std::{
    mem,
    ops::AddAssign,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
};

use crate::{
    deferral::CompletionDeferral, dispatcher::DispatcherHandle, job_decl::JobDecl,
    priority::Priority,
};

/// A job parked behind a nonzero wait counter.
///
/// The accumulate counter is stored with the job because the job already became
/// outstanding before it was parked. When the dispatcher releases this waiting
/// job, it must move this same counter into the ready queue without incrementing
/// it again.
pub(crate) struct WaitingJob {
    job: JobDecl,
    accum_counter: Arc<CounterEntry>,
}

impl WaitingJob {
    /// Bundles a parked job with the accumulate counter it already incremented.
    pub(crate) fn new(job: JobDecl, accum_counter: Arc<CounterEntry>) -> Self {
        Self { job, accum_counter }
    }

    /// Splits the parked job back into ready-queue dispatch parts.
    pub(crate) fn into_parts(self) -> (JobDecl, Arc<CounterEntry>) {
        (self.job, self.accum_counter)
    }
}

pub(crate) type WaitingList = Vec<WaitingJob>;

/// Shared counter state used to track outstanding work and dependent jobs.
///
/// `value` is the only outstanding-work count. `Arc<CounterEntry>` is the only
/// lifetime mechanism. Keeping those two responsibilities separate is what lets
/// queued jobs, public counters, and deferrals hold the state alive without
/// changing whether the dependency has resolved.
pub(crate) struct CounterEntry {
    value: AtomicU32,
    waiting: Mutex<WaitingList>,
    priority: Priority,
    name: &'static str,
}

impl CounterEntry {
    /// Allocates a new zero-valued counter entry with scheduling metadata.
    pub(crate) fn new(priority: Priority, name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            value: AtomicU32::new(0),
            waiting: Mutex::new(Vec::new()),
            priority,
            name,
        })
    }

    /// Increments the outstanding-work count and reports whether it was zero.
    ///
    /// Dispatch code relies on this transition happening before a job becomes
    /// visible to any queue or waiting list. The return value is a snapshot of
    /// the old state, useful for future optimizations, but it is not a license
    /// to skip the locked waiting-list rechecks.
    pub(crate) fn increment(&self) -> bool {
        let old = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .expect("counter value overflow");

        old == 0
    }

    /// Decrements the outstanding-work count and reports whether it reached zero.
    ///
    /// Underflow is always a logic bug: a job or deferral finished without first
    /// being counted as outstanding.
    pub(crate) fn decrement(&self) -> bool {
        let old = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_sub(1)
            })
            .expect("counter value underflow");

        old == 1
    }

    /// Parks a job if this counter is still nonzero while the waiting lock is held.
    ///
    /// The lock and recheck are one invariant: a counter may reach zero, then be
    /// incremented again before an older zero-observer drains the waiting list.
    /// Returning the job on failure lets the dispatcher queue it immediately
    /// without losing the already-incremented accumulate counter.
    pub(crate) fn try_add_to_waiting(&self, job: WaitingJob) -> Result<(), WaitingJob> {
        let mut waiting = self
            .waiting
            .lock()
            .expect("counter waiting-list lock poisoned");

        if self.is_zero() {
            Err(job)
        } else {
            waiting.push(job);
            Ok(())
        }
    }

    /// Takes all waiting jobs only if this counter is still zero under the lock.
    ///
    /// This is the release half of the same invariant enforced by
    /// `try_add_to_waiting`: jobs parked behind a reused nonzero counter must not
    /// be released by an older decrement that merely observed zero earlier.
    pub(crate) fn flush_waiting(&self) -> WaitingList {
        let mut waiting = self
            .waiting
            .lock()
            .expect("counter waiting-list lock poisoned");

        if self.is_zero() {
            mem::take(&mut *waiting)
        } else {
            Vec::new()
        }
    }

    /// Snapshot of whether the outstanding-work count is zero.
    ///
    /// This is useful for fast paths and tests, but it is not authoritative for
    /// waiting-list mutation. Any decision to park or release jobs must recheck
    /// while holding the waiting-list lock.
    pub(crate) fn is_zero(&self) -> bool {
        self.value.load(Ordering::Acquire) == 0
    }

    /// Scheduling priority used when jobs accumulating into this counter run.
    pub(crate) fn priority(&self) -> Priority {
        self.priority
    }

    /// Static diagnostic name attached to this counter entry.
    pub(crate) fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
#[path = "tests/counter_support.rs"]
pub(crate) mod test_support;

/// Public move-only handle to a `CounterEntry`.
///
/// A `Counter` owns one `Arc` reference to the shared entry and carries the
/// private dispatcher handle needed by operations that may release work after
/// the original job-system call has returned. It intentionally does not implement
/// `Clone`; sharing the internal entry is a dispatcher concern, not a public
/// handle operation.
///
/// ```compile_fail
/// fn assert_clone<T: Clone>() {}
/// assert_clone::<leet_jobs2::Counter>();
/// ```
pub struct Counter {
    pub(crate) dispatcher: DispatcherHandle,
    pub(crate) entry: Arc<CounterEntry>,
}

impl Counter {
    /// Wraps an existing counter entry in a public move-only handle.
    pub(crate) fn from_entry(dispatcher: DispatcherHandle, entry: Arc<CounterEntry>) -> Self {
        Self { dispatcher, entry }
    }

    /// Internal access to the shared counter entry.
    pub(crate) fn entry(&self) -> &Arc<CounterEntry> {
        &self.entry
    }

    /// Creates an externally finished unit of work on this counter.
    ///
    /// The deferral is counted before it is returned, so callers can hand it to
    /// another owner without a window where the counter could reach zero early.
    pub fn create_deferral(&self, name: &'static str) -> CompletionDeferral {
        CompletionDeferral::new(self.dispatcher.clone(), Arc::clone(&self.entry), name)
    }

    /// Replaces this handle with another counter handle.
    ///
    /// This is a handle move, not a synchronization operation. Callers should
    /// only use it while they have exclusive ownership of this public handle and
    /// no thread or job can still access the old value through the handle being
    /// reset. Existing internal `Arc` holders remain valid and keep their
    /// counter entries alive independently.
    pub fn reset(&mut self, other: Counter) {
        *self = other;
    }

    /// Snapshot of whether this counter currently has no outstanding work.
    ///
    /// This is a convenience observation, not a synchronization fence for
    /// waiting-list mutation. The dispatcher still performs locked rechecks
    /// before parking or releasing dependent jobs.
    pub fn is_zero(&self) -> bool {
        self.entry.is_zero()
    }
}

// Composing counters is implemented as an invisible empty job: `self` is kept
// nonzero until `other` resolves, and then the empty job decrements `self`.
// This preserves the normal dispatch lifecycle without exposing a separate
// dependency graph object.
impl AddAssign<&Counter> for Counter {
    fn add_assign(&mut self, other: &Counter) {
        assert!(
            Arc::ptr_eq(&self.dispatcher.inner, &other.dispatcher.inner),
            "cannot compose counters from different job systems"
        );
        assert!(
            !Arc::ptr_eq(&self.entry, &other.entry),
            "counter cannot depend on itself"
        );

        if other.is_zero() {
            return;
        }

        let job = JobDecl::empty("CounterDependency");
        self.dispatcher
            .run_job(job, Some(Arc::clone(&other.entry)), Arc::clone(&self.entry));
    }
}

// Test bodies live in `src/tests`; the declaration stays here so the unit tests
// remain child modules with access to private counter invariants.
#[cfg(test)]
#[path = "tests/counter.rs"]
mod tests;
