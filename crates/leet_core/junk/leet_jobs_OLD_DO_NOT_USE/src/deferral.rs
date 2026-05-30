use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::counter::{Counter, CounterInner};
use crate::dispatcher::DispatcherInner;

// ---------------------------------------------------------------------------
// CompletionDeferral
// ---------------------------------------------------------------------------

/// A RAII handle that keeps a counter artificially non-zero until explicitly
/// finished.
///
/// # Purpose
///
/// Normally a counter reaches zero when all jobs that were submitted against
/// it have finished.  But some work has no job — it's driven by an external
/// event: an OS async-IO callback, a network packet arriving, a GPU fence
/// being signalled.  Those things happen outside the job system and have no
/// natural "job finish" to decrement the counter.
///
/// `CompletionDeferral` fills that gap.  Creating one increments the target
/// counter by 1.  Any jobs gated on that counter are therefore blocked until
/// the deferral is consumed.  When [`CompletionDeferral::finish`] is called
/// from *anywhere* — a callback, a different thread, the main thread — the
/// counter is decremented and any waiting jobs are released.
///
/// # Ownership model
///
/// - Move-only (no `Clone`): only one piece of code ever "holds" the deferral.
/// - If dropped without calling [`finish`], the decrement still happens
///   automatically — so a scope-guard pattern is safe and will never leak
///   a permanently-blocked counter.
///
/// # Example
///
/// ```rust,no_run
/// use leet_jobs::{Dispatcher, JobSystemConfig, Counter, ScheduleParam};
///
/// let dispatcher = Dispatcher::new(JobSystemConfig::default());
/// let counter = Counter::new(ScheduleParam::default(), "load_texture");
///
/// // Create the deferral BEFORE kicking the async request.
/// let deferral = dispatcher.create_deferral(&counter, "texture_load");
///
/// // Kick async work — passes the deferral somewhere that will finish it.
/// kick_async_io(deferral);
///
/// // Job B is now parked. It will be released when finish() is called.
/// let mut builder = dispatcher.builder(ScheduleParam::default());
/// builder.dispatch_wait(&counter);
/// builder.dispatch(|| println!("texture is ready, use it here"));
/// let done = builder.extract_wait_counter();
/// dispatcher.flush(&done);
///
/// fn kick_async_io(_deferral: leet_jobs::CompletionDeferral) { /* ... */ }
/// ```
///
#[must_use = "deferral must be finished by calling .finish() or it will be dropped automatically"]
pub struct CompletionDeferral {
    /// The counter this deferral is holding non-zero.
    /// `None` only after `finish()` has been called.
    counter: Option<Arc<CounterInner>>,
    /// The dispatcher, needed to run the decrement-and-flush logic.
    dispatcher: Arc<DispatcherInner>,
    /// Guard against double-finish.  Matches `m_isFinished` in C++.
    finished: AtomicBool,
    /// Debug label.
    debug_name: &'static str,
    /// RED-style opaque debug pointer/user data.
    debug_user_data: Option<usize>,
}

// SAFETY: The `Arc<CounterInner>` is `Send + Sync`, `AtomicBool` is
// `Send + Sync`, and `DispatcherInner` is `Send + Sync`.
// The deferral may be created on one thread and finished on another —
// that is the entire point of the type.
unsafe impl Send for CompletionDeferral {}
unsafe impl Sync for CompletionDeferral {}

impl CompletionDeferral {
    /// Internal constructor — called only by [`Dispatcher::create_deferral`].
    ///
    /// Increments the counter immediately so the gate is held before any
    /// caller has a chance to call `finish()`.
    pub(crate) fn new(
        counter: Arc<CounterInner>,
        dispatcher: Arc<DispatcherInner>,
        debug_name: &'static str,
        debug_user_data: Option<usize>,
    ) -> Self {
        // Increment: "one more thing pending" — matches the C++
        // `counterValue.ExchangeAdd(1)` in `Dispatcher::CreateDeferral`.
        counter.value.fetch_add(1, Ordering::AcqRel);

        Self {
            counter: Some(counter),
            dispatcher,
            finished: AtomicBool::new(false),
            debug_name,
            debug_user_data,
        }
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    /// Release the held counter increment.
    ///
    /// If the counter reaches zero as a result, all jobs waiting on it are
    /// immediately moved to the global work queue.
    ///
    /// # Panics
    ///
    /// Panics if called more than once. This is intentional: a double finish
    /// almost always
    /// means a logic error (two pieces of code both think they own the
    /// deferral).
    pub fn finish(mut self) {
        if !self.try_finish_internal() {
            panic!(
                "[leet_jobs] CompletionDeferral '{}' finished twice — logic error",
                self.debug_name
            );
        }
    }

    /// Returns `true` if [`finish`] has already been called.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Debug label provided at creation time.
    pub fn debug_name(&self) -> &'static str {
        self.debug_name
    }

    /// RED-style opaque debug user data.
    pub fn debug_user_data(&self) -> Option<usize> {
        self.debug_user_data
    }

    #[allow(dead_code)]
    pub(crate) fn debug_counter_addr(&self) -> Option<usize> {
        self.counter
            .as_ref()
            .map(|counter| Arc::as_ptr(counter) as usize)
    }

    // ------------------------------------------------------------------
    // Internal
    // ------------------------------------------------------------------

    /// Perform the decrement exactly once.  Returns `false` if already done.
    fn try_finish_internal(&mut self) -> bool {
        // CAS: if already true, someone already finished — return false.
        if self
            .finished
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }

        // Take the counter out — it will be decremented and potentially freed.
        if let Some(counter) = self.counter.take() {
            self.dispatcher.decrement(&counter);
        }

        true
    }
}

impl Drop for CompletionDeferral {
    /// If the deferral goes out of scope without an explicit `finish()`,
    /// automatically release the counter.  This prevents a permanently-blocked
    /// counter if a code path forgets to call finish.
    ///
    /// This mirrors the C++ destructor calling `TryFinishDeferral()`.
    fn drop(&mut self) {
        // Silently finish — no panic, drops are not the right place for panics.
        self.try_finish_internal();
    }
}

// ---------------------------------------------------------------------------
// Hook into Counter and Dispatcher
// ---------------------------------------------------------------------------

impl Counter {
    /// Create a deferral tied to this counter via the given dispatcher.
    ///
    /// Prefer [`Dispatcher::create_deferral`] which is the public entry point.
    pub(crate) fn create_deferral(
        &self,
        dispatcher: &Arc<DispatcherInner>,
        debug_name: &'static str,
        debug_user_data: Option<usize>,
    ) -> CompletionDeferral {
        CompletionDeferral::new(
            Arc::clone(self.inner()),
            Arc::clone(dispatcher),
            debug_name,
            debug_user_data,
        )
    }
}

impl crate::Dispatcher {
    /// Create a [`CompletionDeferral`] that holds `counter` non-zero until
    /// [`CompletionDeferral::finish`] is called.
    ///
    /// Call this **before** kicking any async work, so the counter is
    /// incremented before anything else can observe it as zero.
    pub fn create_deferral(
        &self,
        counter: &Counter,
        debug_name: &'static str,
    ) -> CompletionDeferral {
        counter.create_deferral(self.inner(), debug_name, None)
    }

    /// Create a [`CompletionDeferral`] with RED-style opaque debug user data.
    pub fn create_deferral_with_debug_user_data(
        &self,
        counter: &Counter,
        debug_name: &'static str,
        debug_user_data: Option<usize>,
    ) -> CompletionDeferral {
        counter.create_deferral(self.inner(), debug_name, debug_user_data)
    }
}
