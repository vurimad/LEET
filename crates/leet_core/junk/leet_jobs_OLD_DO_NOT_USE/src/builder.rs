use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::{self, ThreadId};

use crate::counter::Counter;
use crate::dispatcher::DispatcherInner;
use crate::job_decl::JobHint;
use crate::priority::ScheduleParam;

/// Private data carried by RED's `RunContext::continuationContext`.
///
/// In RED this lets a running job continue the current dependency chain. In
/// LEET the same role is represented by the optional continuation counter.
#[derive(Clone)]
pub struct ContinuationContext {
    pub counter: Option<Counter>,
    pub instrumentation_object: Option<&'static str>,
    pub param: ScheduleParam,
}

/// RED-style run context used to continue a job chain from inside a worker.
#[derive(Clone)]
pub struct RunContext {
    pub debug_name: &'static str,
    pub debug_stack_traces: Arc<[&'static str]>,
    pub parallel_for_team_index: i32,
    pub dispatcher_thread_index: u32,
    pub continuation_context: ContinuationContext,
    pub(crate) dispatcher: Arc<DispatcherInner>,
    pub(crate) param: ScheduleParam,
}

impl RunContext {
    #[allow(dead_code)]
    pub(crate) fn new(dispatcher: &Arc<DispatcherInner>, param: ScheduleParam) -> Self {
        Self::for_job(dispatcher, param, "", None, 0, -1, None)
    }

    pub(crate) fn for_job(
        dispatcher: &Arc<DispatcherInner>,
        param: ScheduleParam,
        debug_name: &'static str,
        instrumentation_object: Option<&'static str>,
        dispatcher_thread_index: u32,
        parallel_for_team_index: i32,
        continuation_counter: Option<Counter>,
    ) -> Self {
        Self {
            debug_name,
            debug_stack_traces: Arc::from([]),
            parallel_for_team_index,
            dispatcher_thread_index,
            continuation_context: ContinuationContext {
                counter: continuation_counter,
                instrumentation_object,
                param,
            },
            dispatcher: Arc::clone(dispatcher),
            param,
        }
    }
}

// ---------------------------------------------------------------------------
// Fence mode
// ---------------------------------------------------------------------------

/// Controls whether a fence is automatically inserted after a dispatch call.
pub enum Fence {
    Full,
    None,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Fluent, scoped helper for building job dependency graphs.
#[must_use = "call extract_wait_counter() or let the builder drop to finalise the job chain"]
pub struct Builder {
    dispatcher: Arc<DispatcherInner>,
    wait_for_zero_counter: Counter,
    accumulate_counter: Counter,
    continuation_counter: Option<Counter>,
    param: ScheduleParam,
    thread_owner: ThreadId,
    is_extracted: bool,
    debug_needs_explicit_fence: bool,
}

impl Builder {
    /// Create a builder that is not chained to any existing work.
    pub(crate) fn new(dispatcher: &Arc<DispatcherInner>, param: ScheduleParam) -> Self {
        Self {
            dispatcher: Arc::clone(dispatcher),
            wait_for_zero_counter: Counter::new(param, "builder_wait"),
            accumulate_counter: Counter::new(param, "builder_accum"),
            continuation_counter: None,
            param,
            thread_owner: thread::current().id(),
            is_extracted: false,
            debug_needs_explicit_fence: false,
        }
    }

    /// RED-style continuation builder constructor.
    pub fn from_run_context(run_context: &RunContext) -> Self {
        let mut builder = Self::new(&run_context.dispatcher, run_context.param);
        builder.continuation_counter = run_context.continuation_context.counter.clone();
        builder
    }

    // ------------------------------------------------------------------
    // Dispatch variants
    // ------------------------------------------------------------------

    pub fn dispatch<F: FnOnce() + Send + 'static>(&mut self, job: F) {
        self.dispatch_with_fence(job, Fence::Full);
    }

    pub fn dispatch_with_fence<F: FnOnce() + Send + 'static>(&mut self, job: F, fence: Fence) {
        self.guard_thread();
        self.assert_not_extracted();
        self.dispatch_job_internal(job, fence);
    }

    pub fn dispatch_fence_explicitly(&mut self) {
        self.guard_thread();
        self.assert_not_extracted();
        self.sync_no_guard();
        self.debug_needs_explicit_fence = false;
    }

    pub fn dispatch_fence(&mut self) {
        self.dispatch_fence_explicitly();
    }

    pub fn dispatch_wait(&mut self, external_wait_counter: &Counter) {
        self.guard_thread();
        self.assert_not_extracted();
        assert!(
            !self.debug_needs_explicit_fence,
            "[leet_jobs] Must call dispatch_fence() before dispatch_wait()"
        );
        self.bridge_counter_dependency(external_wait_counter, &self.wait_for_zero_counter);
    }

    pub fn dispatch_job_after_wait_no_fence<F: FnOnce() + Send + 'static>(
        &mut self,
        external_wait_counter: &Counter,
        job: F,
    ) {
        self.dispatch_wait(external_wait_counter);
        self.dispatch_job_internal(job, Fence::None);
    }

    pub fn dispatch_job_with_hint<F: FnOnce() + Send + 'static>(&mut self, hint: JobHint, job: F) {
        self.guard_thread();
        self.assert_not_extracted();
        self.dispatch_job_internal_with_hint(job, Fence::Full, hint);
    }

    pub fn dispatch_parallel_for_job<F>(&mut self, num_elements: usize, func: F)
    where
        F: Fn(usize) + Send + Sync + 'static,
    {
        self.dispatch_parallel_for_job_with_fence(num_elements, func, Fence::Full);
    }

    pub fn dispatch_parallel_for_job_after_wait_no_fence<F>(
        &mut self,
        external_wait_counter: &Counter,
        num_elements: usize,
        func: F,
    ) where
        F: Fn(usize) + Send + Sync + 'static,
    {
        self.dispatch_wait(external_wait_counter);
        self.dispatch_parallel_for_job_with_fence(num_elements, func, Fence::None);
    }

    pub fn dispatch_parallel_for_job_with_epilogue<F, E>(
        &mut self,
        num_elements: usize,
        func: F,
        epilogue: E,
    ) where
        F: Fn(usize) + Send + Sync + 'static,
        E: FnOnce() + Send + 'static,
    {
        self.dispatch_parallel_for_job_with_epilogue_inner(
            num_elements,
            func,
            epilogue,
            Fence::Full,
        );
    }

    pub fn dispatch_parallel_for_job_with_epilogue_after_wait_no_fence<F, E>(
        &mut self,
        external_wait_counter: &Counter,
        num_elements: usize,
        func: F,
        epilogue: E,
    ) where
        F: Fn(usize) + Send + Sync + 'static,
        E: FnOnce() + Send + 'static,
    {
        self.dispatch_wait(external_wait_counter);
        self.dispatch_parallel_for_job_with_epilogue_inner(
            num_elements,
            func,
            epilogue,
            Fence::None,
        );
    }

    pub fn dispatch_parallel_for_job_with_epilogue_with_batch_size<F, E>(
        &mut self,
        num_elements: usize,
        func: F,
        epilogue: E,
        _max_batch_size: u32,
    ) where
        F: Fn(usize) + Send + Sync + 'static,
        E: FnOnce() + Send + 'static,
    {
        // The batching hint is a future optimization hook in LEET.
        self.dispatch_parallel_for_job_with_epilogue(num_elements, func, epilogue);
    }

    // ------------------------------------------------------------------
    // Counter extraction
    // ------------------------------------------------------------------

    pub fn extract_wait_counter(mut self) -> Counter {
        self.guard_thread();
        assert!(!self.is_extracted, "[leet_jobs] Already extracted counter");
        assert!(
            !self.debug_needs_explicit_fence,
            "[leet_jobs] Must call dispatch_fence() before extract_wait_counter()"
        );

        self.final_sync_no_guard();
        self.is_extracted = true;

        if self.continuation_counter.is_some() {
            let extracted = Counter::new(self.param, "builder_extract");
            if let Some(ref continuation_counter) = self.continuation_counter {
                self.bridge_counter_dependency(continuation_counter, &extracted);
            }
            extracted
        } else {
            self.wait_for_zero_counter.clone()
        }
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn dispatch_job_internal<F: FnOnce() + Send + 'static>(&mut self, job: F, fence: Fence) {
        self.dispatch_job_internal_with_hint(job, fence, JobHint::None);
    }

    fn dispatch_job_internal_with_hint<F: FnOnce() + Send + 'static>(
        &mut self,
        job: F,
        fence: Fence,
        hint: JobHint,
    ) {
        self.guard_thread();
        self.assert_not_extracted();

        self.dispatcher.submit_closure(
            job,
            Some(Arc::clone(self.wait_for_zero_counter.inner())),
            Arc::clone(self.accumulate_counter.inner()),
            self.param.priority,
            hint,
        );

        self.do_fence(fence);
    }

    fn dispatch_parallel_for_job_with_fence<F>(
        &mut self,
        num_elements: usize,
        func: F,
        fence: Fence,
    ) where
        F: Fn(usize) + Send + Sync + 'static,
    {
        self.guard_thread();
        self.assert_not_extracted();

        if num_elements == 0 {
            self.do_fence(fence);
            return;
        }

        let func = Arc::new(func);
        for index in 0..num_elements {
            let func = Arc::clone(&func);
            self.dispatcher.submit_closure(
                move || func.as_ref()(index),
                Some(Arc::clone(self.wait_for_zero_counter.inner())),
                Arc::clone(self.accumulate_counter.inner()),
                self.param.priority,
                JobHint::None,
            );
        }

        self.do_fence(fence);
    }

    fn dispatch_parallel_for_job_with_epilogue_inner<F, E>(
        &mut self,
        num_elements: usize,
        func: F,
        epilogue: E,
        fence: Fence,
    ) where
        F: Fn(usize) + Send + Sync + 'static,
        E: FnOnce() + Send + 'static,
    {
        self.guard_thread();
        self.assert_not_extracted();

        if num_elements == 0 {
            self.dispatch_job_internal(epilogue, fence);
            return;
        }

        let epilogue_gate = Counter::new(self.param, "builder_parallel_epilogue");
        epilogue_gate
            .0
            .value
            .fetch_add(num_elements as i32, Ordering::Relaxed);

        let func = Arc::new(func);
        for index in 0..num_elements {
            let func = Arc::clone(&func);
            let epilogue_gate = epilogue_gate.clone();
            let dispatcher = Arc::clone(&self.dispatcher);
            self.dispatcher.submit_closure(
                move || {
                    let result = catch_unwind(AssertUnwindSafe(|| func.as_ref()(index)));
                    dispatcher.decrement(epilogue_gate.inner());
                    if let Err(payload) = result {
                        resume_unwind(payload);
                    }
                },
                Some(Arc::clone(self.wait_for_zero_counter.inner())),
                Arc::clone(self.accumulate_counter.inner()),
                self.param.priority,
                JobHint::None,
            );
        }

        self.dispatcher.submit_closure(
            epilogue,
            Some(Arc::clone(epilogue_gate.inner())),
            Arc::clone(self.accumulate_counter.inner()),
            self.param.priority,
            JobHint::None,
        );

        self.do_fence(fence);
    }

    fn bridge_counter_dependency(&self, source: &Counter, target: &Counter) {
        if source.is_zero() {
            return;
        }

        let bridge_priority = self.param.priority.max(source.0.as_ref().param.priority);
        self.dispatcher.submit_closure(
            || {},
            Some(Arc::clone(source.inner())),
            Arc::clone(target.inner()),
            bridge_priority,
            JobHint::Trivial,
        );
    }

    fn sync_no_guard(&mut self) {
        if self.accumulate_counter.is_zero() {
            return;
        }

        self.wait_for_zero_counter = std::mem::replace(
            &mut self.accumulate_counter,
            Counter::new(self.param, "builder_accum"),
        );
    }

    fn final_sync_no_guard(&mut self) {
        assert!(
            !self.is_extracted,
            "[leet_jobs] Cannot use Builder once final counter extracted from it"
        );
        assert!(
            !self.debug_needs_explicit_fence,
            "[leet_jobs] Must call dispatch_fence() before extract_wait_counter()"
        );

        if self.accumulate_counter.is_zero() {
            // Keep the existing wait counter.
        } else {
            self.wait_for_zero_counter = std::mem::replace(
                &mut self.accumulate_counter,
                Counter::new(self.param, "builder_accum_final"),
            );
        }

        if let Some(ref continuation_counter) = self.continuation_counter {
            self.bridge_counter_dependency(&self.wait_for_zero_counter, continuation_counter);
        }
    }

    fn do_fence(&mut self, fence: Fence) {
        match fence {
            Fence::None => {
                self.debug_needs_explicit_fence = true;
            }
            Fence::Full => {
                assert!(
                    !self.debug_needs_explicit_fence,
                    "[leet_jobs] Must call dispatch_fence() before a Fence::Full dispatch after using Fence::None"
                );
                self.sync_no_guard();
            }
        }
    }

    fn guard_thread(&self) {
        assert_eq!(
            self.thread_owner,
            thread::current().id(),
            "[leet_jobs] Builder used from a different thread"
        );
    }

    fn assert_not_extracted(&self) {
        assert!(
            !self.is_extracted,
            "[leet_jobs] Cannot use Builder once final counter extracted from it"
        );
    }
}

impl Drop for Builder {
    fn drop(&mut self) {
        if thread::panicking() {
            return;
        }
        self.guard_thread();
        if !self.is_extracted {
            assert!(
                !self.debug_needs_explicit_fence,
                "[leet_jobs] Must call dispatch_fence() before a Fence::Full dispatch after using Fence::None"
            );
            self.final_sync_no_guard();
        }
    }
}

// ---------------------------------------------------------------------------
// Ergonomic entry point usable from outside the crate
// ---------------------------------------------------------------------------

impl crate::Dispatcher {
    pub fn builder(&self, param: ScheduleParam) -> Builder {
        Builder::new(self.inner(), param)
    }
}
