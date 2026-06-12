//! Scoped builder for ordered and parallel groups of ordinary jobs.

use std::{marker::PhantomData, rc::Rc, sync::Arc};

use crate::{
    counter::{Counter, CounterEntry},
    dispatcher::DispatcherHandle,
    job_decl::{JobDecl, JobHint, ParallelForJob, RunContext},
    priority::Priority,
};

/// Ordering mode for a builder dispatch.
pub enum Fence {
    /// Dispatch this job and then make later builder work wait for it.
    Full,
    /// Dispatch this job as part of the current parallel group.
    None,
}

/// Scoped helper for constructing a counter dependency chain.
///
/// A builder keeps two counters: the current wait counter that new work depends
/// on, and the current accumulator that newly dispatched work increments. A
/// fence rotates a nonempty accumulator into the wait position, making later
/// jobs wait for the group that was just dispatched.
///
/// ```compile_fail
/// fn assert_send<T: Send>() {}
/// assert_send::<leet_jobs2::Builder>();
/// ```
pub struct Builder {
    dispatcher: DispatcherHandle,
    wait_counter: Option<Counter>,
    accum_counter: Option<Counter>,
    continuation_counter: Option<Arc<CounterEntry>>,
    priority: Priority,
    is_extracted: bool,
    debug_needs_fence: bool,
    not_send: PhantomData<Rc<()>>,
}

impl Builder {
    /// Creates a fresh dependency-chain builder at the requested priority.
    pub(crate) fn new(dispatcher: DispatcherHandle, priority: Priority) -> Self {
        let wait_counter = dispatcher.create_counter(priority, "BuilderWait");
        let accum_counter = dispatcher.create_counter(priority, "BuilderAccum");
        Self {
            dispatcher,
            wait_counter: Some(wait_counter),
            accum_counter: Some(accum_counter),
            continuation_counter: None,
            priority,
            is_extracted: false,
            debug_needs_fence: false,
            not_send: PhantomData,
        }
    }

    /// Creates a builder whose final synchronization extends a parent job.
    pub(crate) fn from_context(dispatcher: DispatcherHandle, ctx: &RunContext) -> Self {
        let mut builder = Self::new(dispatcher, ctx.continuation.param.priority);
        builder.continuation_counter = Some(Arc::clone(&ctx.continuation.counter));
        builder
    }

    /// Dispatches one ordered job.
    ///
    /// A full fence is inserted after the job. Later work submitted through
    /// this builder will wait for this job before becoming ready.
    pub fn dispatch_job<F>(&mut self, name: &'static str, f: F)
    where
        F: FnOnce(&RunContext) + Send + 'static,
    {
        self.dispatch_job_with_hint_and_fence(name, JobHint::None, Fence::Full, f);
    }

    /// Dispatches one ordered job with an explicit scheduling hint.
    ///
    /// Hints influence queueing and flush policy only. They do not change the
    /// counter lifecycle: the job is still counted before it is queued and
    /// decremented after it returns.
    pub fn dispatch_job_with_hint<F>(&mut self, name: &'static str, hint: JobHint, f: F)
    where
        F: FnOnce(&RunContext) + Send + 'static,
    {
        self.dispatch_job_with_hint_and_fence(name, hint, Fence::Full, f);
    }

    /// Dispatches one job into the current parallel group.
    ///
    /// No-fence dispatch lets several jobs share the current accumulator. The
    /// caller must close the group with `dispatch_fence_explicitly()` before
    /// submitting ordered work, adding waits, extracting, or dropping the
    /// builder.
    pub fn dispatch_job_no_fence<F>(&mut self, name: &'static str, f: F)
    where
        F: FnOnce(&RunContext) + Send + 'static,
    {
        self.dispatch_job_with_hint_and_fence(name, JobHint::None, Fence::None, f);
    }

    /// Dispatches range-based parallel work and fences it before later work.
    ///
    /// The closure receives an inclusive-exclusive element range and a run
    /// context. The dispatcher may split the range across multiple team jobs;
    /// dependent work observes completion only after all teams have returned.
    pub fn dispatch_parallel_for<F>(&mut self, name: &'static str, count: u32, f: F)
    where
        F: Fn(u32, u32, &RunContext) + Send + Sync + 'static,
    {
        self.dispatch_parallel_for_with_hint_batch_and_fence(
            name,
            count,
            JobHint::None,
            0,
            Fence::Full,
            f,
        );
    }

    /// Dispatches range-based parallel work into the current no-fence group.
    ///
    /// This follows the same fence rules as `dispatch_job_no_fence()`: the
    /// group must be closed explicitly before the builder can move on to
    /// ordered work.
    pub fn dispatch_parallel_for_no_fence<F>(&mut self, name: &'static str, count: u32, f: F)
    where
        F: Fn(u32, u32, &RunContext) + Send + Sync + 'static,
    {
        self.dispatch_parallel_for_with_hint_batch_and_fence(
            name,
            count,
            JobHint::None,
            0,
            Fence::None,
            f,
        );
    }

    /// Dispatches range-based parallel work with an explicit batching hint.
    ///
    /// A zero batch size selects automatic batching. A nonzero value guides the
    /// number of chunks, but the dispatcher still chooses ranges that evenly
    /// cover the requested element count.
    pub fn dispatch_parallel_for_with_max_batch_size<F>(
        &mut self,
        name: &'static str,
        count: u32,
        max_batch_size: u32,
        f: F,
    ) where
        F: Fn(u32, u32, &RunContext) + Send + Sync + 'static,
    {
        self.dispatch_parallel_for_with_hint_batch_and_fence(
            name,
            count,
            JobHint::None,
            max_batch_size,
            Fence::Full,
            f,
        );
    }

    /// Dispatches parallel work followed by one epilogue.
    ///
    /// The epilogue runs exactly once after every range chunk has finished and
    /// before the builder's counter can resolve. This makes it suitable for
    /// final reductions or publishing results from the parallel section.
    pub fn dispatch_parallel_for_with_epilogue<F, E>(
        &mut self,
        name: &'static str,
        count: u32,
        f: F,
        epilogue: E,
    ) where
        F: Fn(u32, u32, &RunContext) + Send + Sync + 'static,
        E: FnOnce(&RunContext) + Send + 'static,
    {
        self.dispatch_parallel_for_decl_with_fence(
            ParallelForJob::with_epilogue(name, JobHint::None, count, 0, f, epilogue),
            Fence::Full,
        );
    }

    /// Dispatches parallel work with an epilogue into the current no-fence group.
    ///
    /// The epilogue is still tied to the parallel-for completion. The no-fence
    /// part only controls how this logical dispatch relates to other work
    /// submitted through the builder.
    pub fn dispatch_parallel_for_with_epilogue_no_fence<F, E>(
        &mut self,
        name: &'static str,
        count: u32,
        f: F,
        epilogue: E,
    ) where
        F: Fn(u32, u32, &RunContext) + Send + Sync + 'static,
        E: FnOnce(&RunContext) + Send + 'static,
    {
        self.dispatch_parallel_for_decl_with_fence(
            ParallelForJob::with_epilogue(name, JobHint::None, count, 0, f, epilogue),
            Fence::None,
        );
    }

    /// Adds an external dependency to the current wait counter.
    ///
    /// This does not rotate counters. It simply means subsequent builder work
    /// waits for both the existing chain and the supplied counter.
    pub fn dispatch_wait(&mut self, counter: &Counter) {
        self.assert_active();
        self.assert_no_pending_no_fence();

        let wait_counter = self
            .wait_counter
            .as_mut()
            .expect("active builder must have a wait counter");
        *wait_counter += counter;
    }

    /// Creates a counter through the same dispatcher and priority as this builder.
    pub fn create_counter(&self, name: &'static str) -> Counter {
        self.dispatcher.create_counter(self.priority, name)
    }

    /// Returns the dispatcher used by this builder.
    pub fn dispatcher(&self) -> DispatcherHandle {
        self.dispatcher.clone()
    }

    /// Ends a no-fence group and makes later work wait for that group.
    ///
    /// Empty accumulators are deliberately not rotated. Rotating an empty
    /// accumulator would replace a meaningful wait counter with a counter no
    /// dispatched work can ever complete.
    pub fn dispatch_fence_explicitly(&mut self) {
        self.assert_active();
        self.rotate_accumulator_if_nonzero();
        self.debug_needs_fence = false;
    }

    /// Finalizes the builder and returns the counter for all work it dispatched.
    ///
    /// After extraction the builder is invalid and must not be used again. If
    /// this is a continuation builder, the returned counter is already linked
    /// through the parent continuation counter.
    pub fn extract_wait_counter(&mut self) -> Counter {
        self.assert_active();
        let (final_wait, continuation_counter) = self.final_sync();
        self.is_extracted = true;

        if let Some(continuation_counter) = continuation_counter {
            let mut wrapper = self
                .dispatcher
                .create_counter(self.priority, "BuilderExtractedContinuation");
            let continuation =
                Counter::from_entry(self.dispatcher.clone(), Arc::clone(&continuation_counter));
            wrapper += &continuation;
            wrapper
        } else {
            final_wait
        }
    }

    /// Shared implementation for single-job dispatch variants.
    fn dispatch_job_with_hint_and_fence<F>(
        &mut self,
        name: &'static str,
        hint: JobHint,
        fence: Fence,
        f: F,
    ) where
        F: FnOnce(&RunContext) + Send + 'static,
    {
        self.assert_active();
        if matches!(fence, Fence::Full) {
            self.assert_no_pending_no_fence();
        }

        let wait_entry = Arc::clone(
            self.wait_counter
                .as_ref()
                .expect("active builder must have a wait counter")
                .entry(),
        );
        let accum_entry = Arc::clone(
            self.accum_counter
                .as_ref()
                .expect("active builder must have an accumulate counter")
                .entry(),
        );
        self.dispatcher
            .run_job(JobDecl::new(name, hint, f), Some(wait_entry), accum_entry);

        match fence {
            Fence::Full => self.rotate_accumulator_if_nonzero(),
            Fence::None => self.debug_needs_fence = true,
        }
    }

    /// Builds a parallel-for declaration for the selected fence mode.
    fn dispatch_parallel_for_with_hint_batch_and_fence<F>(
        &mut self,
        name: &'static str,
        count: u32,
        hint: JobHint,
        max_batch_size: u32,
        fence: Fence,
        f: F,
    ) where
        F: Fn(u32, u32, &RunContext) + Send + Sync + 'static,
    {
        self.dispatch_parallel_for_decl_with_fence(
            ParallelForJob::new(name, hint, count, max_batch_size, f),
            fence,
        );
    }

    /// Dispatches a prepared parallel-for declaration through the builder chain.
    fn dispatch_parallel_for_decl_with_fence(&mut self, job: ParallelForJob, fence: Fence) {
        self.assert_active();
        if matches!(fence, Fence::Full) {
            self.assert_no_pending_no_fence();
        }

        let wait_entry = Arc::clone(
            self.wait_counter
                .as_ref()
                .expect("active builder must have a wait counter")
                .entry(),
        );
        let accum_entry = Arc::clone(
            self.accum_counter
                .as_ref()
                .expect("active builder must have an accumulate counter")
                .entry(),
        );
        self.dispatcher
            .run_parallel_for(job, Some(wait_entry), accum_entry);

        match fence {
            Fence::Full => self.rotate_accumulator_if_nonzero(),
            Fence::None => self.debug_needs_fence = true,
        }
    }

    /// Rotates the current accumulator into the wait position if it has work.
    fn rotate_accumulator_if_nonzero(&mut self) {
        let accum_counter = self
            .accum_counter
            .take()
            .expect("active builder must have an accumulate counter");

        if accum_counter.is_zero() {
            self.accum_counter = Some(accum_counter);
            return;
        }

        self.wait_counter = Some(accum_counter);
        self.accum_counter = Some(
            self.dispatcher
                .create_counter(self.priority, "BuilderAccum"),
        );
    }

    /// Finishes the dependency chain and links continuation work if needed.
    fn final_sync(&mut self) -> (Counter, Option<Arc<CounterEntry>>) {
        self.assert_no_pending_no_fence();

        let wait_counter = self
            .wait_counter
            .take()
            .expect("active builder must have a wait counter");
        let accum_counter = self
            .accum_counter
            .take()
            .expect("active builder must have an accumulate counter");

        let final_wait = if accum_counter.is_zero() {
            wait_counter
        } else {
            accum_counter
        };

        let continuation_counter = self.continuation_counter.take();
        if let Some(parent_counter) = &continuation_counter {
            if !final_wait.is_zero() {
                self.dispatcher.run_job(
                    JobDecl::empty("BuilderContinuation"),
                    Some(Arc::clone(final_wait.entry())),
                    Arc::clone(parent_counter),
                );
            }
        }

        (final_wait, continuation_counter)
    }

    /// Panics if this builder has already been finalized or extracted.
    fn assert_active(&self) {
        assert!(!self.is_extracted, "builder was already extracted");
        assert!(
            self.wait_counter.is_some() && self.accum_counter.is_some(),
            "builder has already been finalized"
        );
    }

    /// Panics if a no-fence group is still open.
    fn assert_no_pending_no_fence(&self) {
        assert!(
            !self.debug_needs_fence,
            "builder dispatched no-fence work without an explicit fence"
        );
    }
}

impl Drop for Builder {
    fn drop(&mut self) {
        if self.is_extracted || self.wait_counter.is_none() {
            return;
        }

        let _ = self.final_sync();
    }
}

// Test bodies live in `src/tests`; the declaration stays here so the unit tests
// remain child modules with access to private builder state.
#[cfg(test)]
#[path = "tests/builder.rs"]
mod tests;
