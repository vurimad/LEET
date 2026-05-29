//! Static job declaration storage.
//!
//! This module keeps the type-erased job closure, parallel-for declaration, and
//! run-context data in one place. The dispatcher owns execution policy; these
//! types only describe what should run and which context a running job receives.

use std::sync::{Arc, Mutex};

use crate::{
    builder::Builder, counter::CounterEntry, dispatcher::DispatcherHandle, priority::ScheduleParam,
};

/// Scheduling hint attached to a job declaration.
///
/// Hints are advisory policy inputs for the dispatcher. They do not change the
/// closure's type or ownership model, and they must never be used to bypass the
/// normal counter/dependency lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobHint {
    /// Default behavior.
    None,
    /// Work expected to be cheaper than normal scheduling overhead.
    Trivial,
    /// Long-running work that flush paths may choose not to steal.
    Large,
}

/// Type-erased single-shot job closure plus the metadata needed to schedule it.
///
/// The closure is `FnOnce` because a queued job has exactly one execution
/// owner. The box keeps the ready queue homogeneous while still letting callers
/// dispatch arbitrary owned closures.
pub(crate) struct JobDecl {
    func: Box<dyn FnOnce(&RunContext) + Send + 'static>,
    name: &'static str,
    hint: JobHint,
}

impl JobDecl {
    /// Stores a single-shot closure with the metadata needed by the dispatcher.
    ///
    /// The closure is boxed at construction time so ready queues can hold
    /// heterogeneous jobs without exposing allocation or type erasure at the
    /// public dispatch call site.
    pub(crate) fn new<F>(name: &'static str, hint: JobHint, f: F) -> Self
    where
        F: FnOnce(&RunContext) + Send + 'static,
    {
        Self {
            func: Box::new(f),
            name,
            hint,
        }
    }

    /// Creates a trivial no-op job for dependency-only queue entries.
    pub(crate) fn empty(name: &'static str) -> Self {
        Self::new(name, JobHint::Trivial, |_ctx| {})
    }

    /// Static name used for diagnostics and execution hook points.
    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    /// Scheduling hint attached to this declaration.
    pub(crate) fn hint(&self) -> JobHint {
        self.hint
    }

    /// Consumes the declaration and invokes its closure exactly once.
    pub(crate) fn run(self, ctx: &RunContext) {
        (self.func)(ctx);
    }
}

pub(crate) type ParallelForRangeFn = dyn Fn(u32, u32, &RunContext) + Send + Sync + 'static;
pub(crate) type ParallelForEpilogue = Box<dyn FnOnce(&RunContext) + Send + 'static>;

/// Holds a parallel-for epilogue so multiple team jobs can race to observe
/// completion while still allowing exactly one of them to run the `FnOnce`.
pub(crate) struct TakeOnceEpilogue {
    inner: Mutex<Option<ParallelForEpilogue>>,
}

impl TakeOnceEpilogue {
    /// Wraps an epilogue closure in a take-once container.
    pub(crate) fn new<E>(epilogue: E) -> Self
    where
        E: FnOnce(&RunContext) + Send + 'static,
    {
        Self {
            inner: Mutex::new(Some(Box::new(epilogue))),
        }
    }

    /// Attempts to run the epilogue, returning whether this call claimed it.
    pub(crate) fn run_once(&self, ctx: &RunContext) -> bool {
        let epilogue = self
            .inner
            .lock()
            .expect("parallel-for epilogue lock poisoned")
            .take();

        match epilogue {
            Some(epilogue) => {
                epilogue(ctx);
                true
            }
            None => false,
        }
    }
}

/// Shared declaration for all team jobs in one logical parallel-for dispatch.
///
/// The range function is `Fn + Sync` because several team jobs may call it at
/// the same time. The epilogue remains a single `FnOnce`, guarded by
/// `TakeOnceEpilogue`, so completion logic can run it exactly once.
pub(crate) struct ParallelForJob {
    func: Box<ParallelForRangeFn>,
    epilogue: Option<Arc<TakeOnceEpilogue>>,
    num_elements: u32,
    max_batch_size: u32,
    name: &'static str,
    hint: JobHint,
}

impl ParallelForJob {
    /// Creates a parallel-for declaration without an epilogue.
    ///
    /// The range function receives inclusive-exclusive element ranges. It must
    /// be safe to call from multiple team jobs at the same time.
    pub(crate) fn new<F>(
        name: &'static str,
        hint: JobHint,
        num_elements: u32,
        max_batch_size: u32,
        f: F,
    ) -> Self
    where
        F: Fn(u32, u32, &RunContext) + Send + Sync + 'static,
    {
        Self {
            func: Box::new(f),
            epilogue: None,
            num_elements,
            max_batch_size,
            name,
            hint,
        }
    }

    /// Creates a parallel-for declaration with a single completion epilogue.
    ///
    /// The epilogue is stored separately from the range function because it is a
    /// one-shot closure that runs only after every team has finished.
    pub(crate) fn with_epilogue<F, E>(
        name: &'static str,
        hint: JobHint,
        num_elements: u32,
        max_batch_size: u32,
        f: F,
        epilogue: E,
    ) -> Self
    where
        F: Fn(u32, u32, &RunContext) + Send + Sync + 'static,
        E: FnOnce(&RunContext) + Send + 'static,
    {
        Self {
            func: Box::new(f),
            epilogue: Some(Arc::new(TakeOnceEpilogue::new(epilogue))),
            num_elements,
            max_batch_size,
            name,
            hint,
        }
    }

    /// Static name shared by every team job created from this declaration.
    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    /// Scheduling hint applied to every team job.
    pub(crate) fn hint(&self) -> JobHint {
        self.hint
    }

    /// Total number of elements in the logical parallel-for.
    pub(crate) fn num_elements(&self) -> u32 {
        self.num_elements
    }

    /// Requested maximum batch size hint, or zero for automatic batching.
    pub(crate) fn max_batch_size(&self) -> u32 {
        self.max_batch_size
    }

    /// Whether this declaration carries a completion epilogue.
    pub(crate) fn has_epilogue(&self) -> bool {
        self.epilogue.is_some()
    }

    /// Runs one claimed element range.
    pub(crate) fn run_range(&self, start: u32, end: u32, ctx: &RunContext) {
        (self.func)(start, end, ctx);
    }

    /// Attempts to run the epilogue, returning whether this call claimed it.
    pub(crate) fn run_epilogue_once(&self, ctx: &RunContext) -> bool {
        self.epilogue
            .as_ref()
            .is_some_and(|epilogue| epilogue.run_once(ctx))
    }
}

/// Read-only execution context passed to every job closure.
///
/// Public fields are stable job metadata. The internal dispatcher and
/// continuation state let jobs create child work without exposing job-system
/// internals to crate users.
pub struct RunContext {
    /// Static job name used for diagnostics and profiling hooks.
    pub name: &'static str,
    /// `0` for the claimed flush thread and `1..N` for worker threads.
    pub thread_index: u32,
    /// Parallel-for team index, or `-1` for non-parallel work and epilogues.
    pub parallel_for_index: i32,
    pub(crate) dispatcher: DispatcherHandle,
    pub(crate) continuation: ContinuationContext,
}

impl RunContext {
    /// Creates a continuation builder tied to this running job.
    ///
    /// Work submitted through the returned builder extends this job's
    /// continuation counter, so the job is not considered complete until the
    /// continuation work has also resolved.
    pub fn create_builder(&self) -> Builder {
        Builder::from_context(self.dispatcher.clone(), self)
    }
}

/// Internal continuation state carried through a running job.
///
/// Dispatcher execution builds this from the running job's accumulate counter so
/// child builders can extend the parent job lifetime before the parent counter
/// is decremented.
pub(crate) struct ContinuationContext {
    /// Counter extended by child builders created from this run context.
    pub counter: Arc<CounterEntry>,
    /// Scheduling parameters inherited by continuation builders.
    pub param: ScheduleParam,
}

// Test bodies live in `src/tests`; the declarations stay here so the unit
// tests remain child modules with access to private declaration internals.
#[cfg(test)]
#[path = "tests/job_decl_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "tests/job_decl.rs"]
mod tests;
