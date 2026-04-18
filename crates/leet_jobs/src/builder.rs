use std::sync::Arc;

use crate::counter::Counter;
use crate::dispatcher::DispatcherInner;
use crate::priority::ScheduleParam;

// ---------------------------------------------------------------------------
// Fence mode
// ---------------------------------------------------------------------------

/// Controls whether a fence is automatically inserted after a `dispatch_*` call.
pub enum Fence {
    /// Insert a full ordering fence: subsequent jobs will not start until this
    /// job completes.  This is the default and mirrors `Fence::Full` in C++.
    Full,
    /// No fence: this job may run in parallel with the next dispatched job.
    /// You **must** call [`Builder::dispatch_fence`] before the next
    /// `Fence::Full` dispatch or before the builder is dropped.
    None,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Fluent, scoped helper for building job dependency graphs.
///
/// # Design
///
/// Internally the builder tracks two counters that together form a
/// "chain of responsibility":
///
/// ```text
/// wait_for_zero ──► (jobs run once this hits 0) ──► accumulate ──► (counts in-flight jobs)
/// ```
///
/// After every `Fence::Full` dispatch the counters rotate:
/// `wait_for_zero` becomes the old `accumulate`, and a new `accumulate`
/// is allocated.  This creates an implicit sequential dependency chain
/// without requiring explicit handles.
///
/// To run jobs in parallel, use `Fence::None` for all but the last in the
/// parallel group, then call [`Builder::dispatch_fence`] explicitly.
///
/// # Extraction
///
/// Call [`Builder::extract_wait_counter`] to get a [`Counter`] that reaches
/// zero only when every job dispatched through this builder has finished.
/// That counter can then be passed as a `wait_for` argument to another
/// builder's [`Builder::dispatch_wait`].
///
/// This mirrors `Builder::ExtractWaitCounter()` in the C++ original.
#[must_use = "call extract_wait_counter() or let the builder drop to finalise the job chain"]
pub struct Builder {
    dispatcher: Arc<DispatcherInner>,
    /// Jobs submitted through this builder wait for THIS to reach zero
    /// before they are allowed to start.
    wait_for_zero: Counter,
    /// Every submitted job increments this on entry and decrements on exit.
    accumulate: Counter,
    param: ScheduleParam,
    /// Debug flag: true between a `Fence::None` dispatch and the required
    /// `dispatch_fence()` call.
    needs_explicit_fence: bool,
}

impl Builder {
    /// Create a builder that is not chained to any existing work.
    pub(crate) fn new(dispatcher: &Arc<DispatcherInner>, param: ScheduleParam) -> Self {
        Self {
            dispatcher: Arc::clone(dispatcher),
            wait_for_zero: Counter::new(param, "builder_wait"),
            accumulate: Counter::new(param, "builder_accum"),
            param,
            needs_explicit_fence: false,
        }
    }

    // ------------------------------------------------------------------
    // Dispatch variants
    // ------------------------------------------------------------------

    /// Dispatch a job with a full ordering fence (most common case).
    ///
    /// The next dispatched job will not start until this one finishes.
    pub fn dispatch<F: FnOnce() + Send + 'static>(&mut self, job: F) {
        self.dispatch_with_fence(job, Fence::Full);
    }

    /// Dispatch a job with explicit fence control.
    pub fn dispatch_with_fence<F: FnOnce() + Send + 'static>(&mut self, job: F, fence: Fence) {
        self.dispatcher.submit(
            Box::new(job),
            Some(Arc::clone(self.wait_for_zero.inner())),
            Arc::clone(self.accumulate.inner()),
            self.param.priority,
        );

        match fence {
            Fence::Full => {
                // Matches C++ DoFence(Fence::Full): assert only here, not on Fence::None.
                assert!(
                    !self.needs_explicit_fence,
                    "[leet_jobs] Must call dispatch_fence() before a Fence::Full dispatch after using Fence::None"
                );
                self.do_fence();
            }
            Fence::None => {
                self.needs_explicit_fence = true;
            }
        }
    }

    /// Explicitly insert an ordering fence.
    ///
    /// Required after using [`Fence::None`].  Harmless if called redundantly.
    pub fn dispatch_fence(&mut self) {
        self.do_fence();
        self.needs_explicit_fence = false;
    }

    /// Insert a sync point: subsequent jobs will not start until both the
    /// current batch **and** `external` have both reached zero.
    ///
    /// Implemented by submitting an empty job that waits on `external` and
    /// accumulates into `wait_for_zero`, exactly as the C++ `Counter::operator+=`.
    pub fn dispatch_wait(&mut self, external: &Counter) {
        assert!(
            !self.needs_explicit_fence,
            "[leet_jobs] Must call dispatch_fence() before dispatch_wait()"
        );

        if external.is_zero() {
            return; // Nothing to wait for.
        }

        // Bridge job priority = max(caller priority, external priority).
        // If we used external.priority unconditionally a RenderPath builder could
        // end up with a Latent bridge job, stalling the whole chain at Latent
        // and causing silent frame-time spikes.
        let bridge_priority = self.param.priority.max(external.0.param.priority);

        // Empty job: waits for `external`, accumulates into `wait_for_zero`.
        // This keeps `wait_for_zero` non-zero until `external` clears.
        self.dispatcher.submit(
            Box::new(|| {}),
            Some(Arc::clone(external.inner())),
            Arc::clone(self.wait_for_zero.inner()),
            bridge_priority,
        );
    }

    // ------------------------------------------------------------------
    // Counter extraction
    // ------------------------------------------------------------------

    /// Finalise the builder and return a counter that reaches zero when all
    /// dispatched jobs have completed.
    ///
    /// Consumes `self`.  After calling this the builder must not be used again.
    pub fn extract_wait_counter(mut self) -> Counter {
        assert!(
            !self.needs_explicit_fence,
            "[leet_jobs] Must call dispatch_fence() before extract_wait_counter()"
        );
        self.final_sync();
        self.wait_for_zero.clone()
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Rotate `wait_for_zero` ← `accumulate`, allocate a fresh `accumulate`.
    ///
    /// If `accumulate` is already zero (all submitted jobs already finished)
    /// the rotation is skipped — reusing the existing counter avoids breaking
    /// the dependency chain (same logic as C++ `Sync_NoGuard`).
    fn do_fence(&mut self) {
        if self.accumulate.is_zero() {
            return;
        }
        let old_accumulate = std::mem::replace(
            &mut self.accumulate,
            Counter::new(self.param, "builder_accum"),
        );
        self.wait_for_zero = old_accumulate;
    }

    /// Final version of `do_fence` used by `extract_wait_counter`.
    ///
    /// After this, `wait_for_zero` is the counter callers should wait on.
    fn final_sync(&mut self) {
        if self.accumulate.is_zero() {
            // No in-flight jobs; `wait_for_zero` is already the correct handle.
        } else {
            let old_accumulate = std::mem::replace(
                &mut self.accumulate,
                // Placeholder — will not be used after extract.
                Counter::new(self.param, "builder_accum_final"),
            );
            self.wait_for_zero = old_accumulate;
        }
    }
}

// ---------------------------------------------------------------------------
// Ergonomic entry point usable from outside the crate
// ---------------------------------------------------------------------------

impl crate::Dispatcher {
    /// Create a new [`Builder`] tied to this dispatcher.
    pub fn builder(&self, param: ScheduleParam) -> Builder {
        Builder::new(self.inner(), param)
    }
}
