//! Public job-system handle and private shared runtime state.
//!
//! This module owns dispatch, worker execution, counter flushing, shutdown, and
//! the public entry points that create counters and builders. Parallel-for
//! expansion lives here because it must queue ordinary jobs through the same
//! counter and dependency lifecycle as every other dispatch path.

use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle, ThreadId},
    time::{Duration, Instant},
};

use crate::{
    builder::Builder,
    config::JobSystemConfig,
    counter::{Counter, CounterEntry, WaitingJob},
    job_decl::{ContinuationContext, JobDecl, JobHint, ParallelForJob, RunContext},
    priority::{Priority, ScheduleParam},
    queue::{JobQueueEntry, QueuePushError, ReadyQueues},
    worker,
};

/// Maximum fallback wait used by the flush thread between progress checks.
///
/// Queue pushes and zero-counter transitions both wake the wait path directly,
/// so this timeout should only be a safety cap for missed or unrelated wakeups.
/// Keep it very small because render-thread flushes sit on the critical path
/// for frame progress.
const FLUSH_IDLE_WAIT: Duration = Duration::from_micros(5);

/// Cloneable public handle to one owned job-system instance.
///
/// Cloning this handle shares the same queues and workers. It does not create a
/// second runtime. Shutdown is explicit and idempotent; dropping a handle does
/// not try to stop worker threads.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Resource))]
#[derive(Clone)]
pub struct LeetJobSystem {
    inner: Arc<Dispatcher>,
}

impl LeetJobSystem {
    /// Starts a job-system instance with the configured worker pool and queues.
    ///
    /// The returned handle owns the worker join handles through shared runtime
    /// state. Call `shutdown()` at teardown; dropping handles is intentionally
    /// not a shutdown mechanism.
    pub fn new(config: JobSystemConfig) -> Self {
        let inner = Arc::new(Dispatcher::new(config));
        let handles = worker::spawn_workers(Arc::clone(&inner), &inner.config);
        *inner
            .worker_handles
            .lock()
            .expect("job worker handle lock poisoned") = handles;

        Self { inner }
    }

    /// Requests worker exit, wakes blocked workers, and joins all worker threads.
    ///
    /// Shutdown does not drain queued work. Jobs still sitting in ready queues
    /// are dropped by the queue shutdown path; jobs already running are allowed
    /// to return so their worker thread can exit cleanly.
    pub fn shutdown(&self) {
        assert!(
            !matches!(worker::current_thread_index(), Some(index) if index != 0),
            "shutdown cannot be called from a job worker thread"
        );

        if self.inner.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }

        self.inner.ready_queues.shutdown();

        let handles = {
            let mut handles = self
                .inner
                .worker_handles
                .lock()
                .expect("job worker handle lock poisoned");
            std::mem::take(&mut *handles)
        };

        for handle in handles {
            handle.join().expect("job worker panicked during shutdown");
        }

        if self.inner.flush_thread_id.get() == Some(&thread::current().id()) {
            worker::set_current_thread_index(None);
        }
    }

    /// Number of worker threads that are still owned by this runtime.
    ///
    /// This returns zero after a successful shutdown because the join handles
    /// have been consumed.
    pub fn num_worker_threads(&self) -> usize {
        self.inner
            .worker_handles
            .lock()
            .expect("job worker handle lock poisoned")
            .len()
    }

    /// Job-system thread index for the current thread, if any.
    ///
    /// Worker threads report `Some(1..N)`. Ordinary external threads report
    /// `None`; the claimed flush thread reports `Some(0)`.
    pub fn current_thread_index() -> Option<u32> {
        worker::current_thread_index()
    }

    /// Creates a zero-valued counter at the requested scheduling priority.
    ///
    /// Counters are useful for explicit dependencies and completion deferrals.
    /// Normal job submission is usually more ergonomic through `Builder`.
    pub fn create_counter(&self, priority: Priority) -> Counter {
        let entry = self.inner.create_counter(priority, "Counter");
        Counter::from_entry(self.dispatcher_handle(), entry)
    }

    /// Creates a scoped builder for ordinary job dispatch.
    ///
    /// The builder starts with an empty dependency chain. Work submitted through
    /// it uses the requested priority unless later dispatch policy maps that
    /// priority for this runtime's configuration.
    pub fn create_builder(&self, priority: Priority) -> Builder {
        Builder::new(self.dispatcher_handle(), priority)
    }

    /// Creates a continuation builder tied to a currently running job.
    ///
    /// Work submitted through the returned builder extends the parent job's
    /// continuation counter, so the parent is not considered complete until the
    /// continuation work has also resolved.
    pub fn create_builder_from_context(&self, ctx: &RunContext) -> Builder {
        assert!(
            Arc::ptr_eq(&self.inner, &ctx.dispatcher.inner),
            "run context belongs to a different job system"
        );
        Builder::from_context(self.dispatcher_handle(), ctx)
    }

    /// Marks the current thread as the only thread allowed to flush counters.
    ///
    /// The flush thread uses job-system thread index `0`, matching the
    /// `RunContext` contract for work executed while a flush is helping the
    /// queue make progress.
    pub fn claim_flush_thread(&self) {
        assert!(
            worker::current_thread_index().is_none(),
            "job worker thread cannot be claimed as the flush thread"
        );

        self.inner
            .flush_thread_id
            .set(thread::current().id())
            .unwrap_or_else(|_| panic!("flush thread already claimed"));
        worker::set_current_thread_index(Some(0));
    }

    /// Waits until a counter reaches zero while helping execute eligible jobs.
    ///
    /// This must be called from the claimed flush thread. Jobs executed by the
    /// flush loop receive thread index `0` in their run context.
    pub fn flush_counter(&self, counter: &Counter) -> bool {
        let policy = FlushPolicy {
            min_priority: counter.entry.priority(),
            process_large: true,
        };
        self.flush_counter_with_policy(counter, None, policy)
    }

    /// Flushes a counter until it resolves or the timeout expires.
    ///
    /// Returns `false` on timeout. No diagnostic analysis is performed here;
    /// callers that need timeout reporting should layer it outside this core
    /// wait primitive.
    pub fn flush_counter_with_timeout(&self, counter: &Counter, timeout: Duration) -> bool {
        let policy = FlushPolicy {
            min_priority: counter.entry.priority(),
            process_large: true,
        };
        self.flush_counter_with_policy(counter, Some(timeout), policy)
    }

    /// Flushes a render-frame counter using render-path eligibility rules.
    ///
    /// The flush thread helps with render-path-or-higher work. Large jobs are
    /// only run by the flush thread when the worker pool is very small, leaving
    /// long-running work to workers in the common case.
    pub fn flush_counter_render_frame(&self, counter: &Counter) -> bool {
        let policy = FlushPolicy {
            min_priority: Priority::RenderPath,
            process_large: self.num_worker_threads() < 3,
        };
        self.flush_counter_with_policy(counter, None, policy)
    }

    /// Internal cloneable handle for types that outlive the public call site.
    pub(crate) fn dispatcher_handle(&self) -> DispatcherHandle {
        self.inner.dispatcher_handle()
    }

    /// Shared implementation for all public flush variants.
    fn flush_counter_with_policy(
        &self,
        counter: &Counter,
        timeout: Option<Duration>,
        policy: FlushPolicy,
    ) -> bool {
        self.assert_owns_counter(counter);
        self.assert_flush_thread();
        let _guard = self.inner.enter_flush();
        let deadline = timeout.map(|timeout| Instant::now() + timeout);

        while !counter.entry.is_zero() {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return false;
            }

            match self.inner.try_pop_ready_job() {
                Some((entry, priority))
                    if should_run_during_flush(&entry, priority, counter.entry(), policy) =>
                {
                    self.inner.run_job_queue_entry(entry, 0, priority);
                }
                Some((entry, _priority)) => {
                    self.inner.requeue_job(entry);
                    self.inner
                        .wait_for_queue_wakeup_timeout(flush_idle_wait(deadline), || {
                            !counter.entry.is_zero()
                        });
                }
                None => {
                    self.inner
                        .wait_for_ready_job_timeout(flush_idle_wait(deadline), || {
                            !counter.entry.is_zero()
                        });
                }
            }
        }

        true
    }

    /// Panics if a counter came from a different job-system instance.
    fn assert_owns_counter(&self, counter: &Counter) {
        assert!(
            Arc::ptr_eq(&self.inner, &counter.dispatcher.inner),
            "counter belongs to a different job system"
        );
    }

    /// Debug-checks that the current thread is the claimed flush thread.
    fn assert_flush_thread(&self) {
        debug_assert!(
            self.inner.flush_thread_id.get() == Some(&thread::current().id()),
            "flush_counter called from wrong thread; call claim_flush_thread on the flush thread first"
        );
    }
}

/// Shared runtime state behind `LeetJobSystem` and internal dispatcher handles.
pub(crate) struct Dispatcher {
    pub(crate) config: JobSystemConfig,
    pub(crate) ready_queues: ReadyQueues,
    worker_handles: Mutex<Vec<JoinHandle<()>>>,
    shutdown: AtomicBool,
    pub(crate) flush_thread_id: OnceLock<ThreadId>,
    pub(crate) is_flushing: AtomicBool,
}

impl Dispatcher {
    /// Creates shared runtime state before workers are spawned.
    fn new(config: JobSystemConfig) -> Self {
        Self {
            ready_queues: ReadyQueues::new(&config),
            config,
            worker_handles: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
            flush_thread_id: OnceLock::new(),
            is_flushing: AtomicBool::new(false),
        }
    }

    /// Allocates a zero-valued counter entry after applying priority mapping.
    fn create_counter(&self, priority: Priority, name: &'static str) -> Arc<CounterEntry> {
        CounterEntry::new(map_priority(priority, &self.config), name)
    }

    /// Snapshot of whether shutdown has begun.
    pub(crate) fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// Blocks until a ready job is available or shutdown wakes the queue.
    pub(crate) fn pop_ready_job_blocking(&self) -> Option<(JobQueueEntry, Priority)> {
        self.ready_queues.pop_blocking()
    }

    /// Attempts to take a ready job without blocking.
    pub(crate) fn try_pop_ready_job(&self) -> Option<(JobQueueEntry, Priority)> {
        self.ready_queues.try_pop()
    }

    /// Waits briefly for new ready work while preserving the caller predicate.
    pub(crate) fn wait_for_ready_job_timeout<F>(&self, timeout: Duration, should_wait: F)
    where
        F: FnOnce() -> bool,
    {
        self.ready_queues
            .wait_for_ready_job_timeout(timeout, should_wait);
    }

    /// Waits briefly for any queue wakeup without consuming ready work.
    pub(crate) fn wait_for_queue_wakeup_timeout<F>(&self, timeout: Duration, should_wait: F)
    where
        F: FnOnce() -> bool,
    {
        self.ready_queues
            .wait_for_queue_wakeup_timeout(timeout, should_wait);
    }

    /// Pushes a ready entry unless shutdown has already made new work invalid.
    fn try_push_ready_job(
        &self,
        entry: JobQueueEntry,
        priority: Priority,
    ) -> Result<(), JobQueueEntry> {
        if self.is_shutdown() {
            return Err(entry);
        }

        match self.ready_queues.try_push_if_open(entry, priority) {
            Ok(()) => Ok(()),
            Err(QueuePushError::Shutdown(entry)) => Err(entry),
            Err(QueuePushError::Full(full)) => {
                let priority = full.priority();
                let capacity = full.capacity();
                drop(full.into_entry());
                panic!(
                    "ready queue for {:?} is full; capacity is {}",
                    priority, capacity
                );
            }
        }
    }

    /// Dispatches a job, making it outstanding before it can become visible.
    ///
    /// Increment-before-queue is the core counter invariant: once another
    /// thread can observe or run the job, the accumulate counter already has a
    /// matching outstanding unit that will be decremented after execution.
    pub(crate) fn run_job(
        self: &Arc<Self>,
        job: JobDecl,
        wait_counter: Option<Arc<CounterEntry>>,
        accum_counter: Arc<CounterEntry>,
    ) {
        if let Some(wait_counter) = &wait_counter {
            assert!(
                !Arc::ptr_eq(wait_counter, &accum_counter),
                "job cannot wait on its own accumulate counter"
            );
        }

        accum_counter.increment();
        self.queue_job_or_wait(job, wait_counter, accum_counter);
    }

    /// Expands one logical parallel-for into ordinary queued team jobs.
    ///
    /// Every team job goes through `run_job`, so the logical dispatch is counted
    /// by the same increment-before-queue rule as single jobs. The epilogue, if
    /// present, runs inside the final team job before that job decrements the
    /// accumulate counter; this keeps dependent work from observing completion
    /// before the epilogue has actually returned.
    pub(crate) fn run_parallel_for(
        self: &Arc<Self>,
        job: ParallelForJob,
        wait_counter: Option<Arc<CounterEntry>>,
        accum_counter: Arc<CounterEntry>,
    ) {
        let job = Arc::new(job);
        let num_elements = job.num_elements();

        if num_elements == 0 {
            self.run_zero_element_parallel_for(job, wait_counter, accum_counter);
            return;
        }

        let team_size = self.parallel_for_team_size(num_elements);
        if team_size == 1 {
            self.run_single_team_parallel_for(job, wait_counter, accum_counter);
            return;
        }

        let (batch_count, batch_size) =
            parallel_for_batch_geometry(num_elements, team_size, job.max_batch_size());
        let shared = Arc::new(ParallelForSharedState::new(batch_count, batch_size));

        for team_index in 0..team_size {
            let team_job = Arc::clone(&job);
            let team_shared = Arc::clone(&shared);
            let team_wait_counter = wait_counter.as_ref().map(Arc::clone);
            let team_accum_counter = Arc::clone(&accum_counter);
            self.run_job(
                JobDecl::new(job.name(), job.hint(), move |ctx| {
                    run_parallel_for_team(team_job, team_shared, team_size, team_index, ctx);
                }),
                team_wait_counter,
                team_accum_counter,
            );
        }
    }

    /// Queues the dependency-preserving zero-element parallel-for path.
    fn run_zero_element_parallel_for(
        self: &Arc<Self>,
        job: Arc<ParallelForJob>,
        wait_counter: Option<Arc<CounterEntry>>,
        accum_counter: Arc<CounterEntry>,
    ) {
        let queued_job = if job.has_epilogue() {
            JobDecl::new(job.name(), job.hint(), move |ctx| {
                job.run_epilogue_once(ctx);
            })
        } else {
            JobDecl::empty(job.name())
        };

        self.run_job(queued_job, wait_counter, accum_counter);
    }

    /// Queues a single team job that owns the whole element range.
    fn run_single_team_parallel_for(
        self: &Arc<Self>,
        job: Arc<ParallelForJob>,
        wait_counter: Option<Arc<CounterEntry>>,
        accum_counter: Arc<CounterEntry>,
    ) {
        self.run_job(
            JobDecl::new(job.name(), job.hint(), move |ctx| {
                let chunk_ctx = parallel_for_context(ctx, 0);
                job.run_range(0, job.num_elements(), &chunk_ctx);
                job.run_epilogue_once(ctx);
            }),
            wait_counter,
            accum_counter,
        );
    }

    /// Chooses the number of team jobs for a nonempty parallel-for.
    fn parallel_for_team_size(&self, num_elements: u32) -> u32 {
        let max_team_size = self
            .worker_handles
            .lock()
            .expect("job worker handle lock poisoned")
            .len()
            .saturating_add(1)
            .min(u32::MAX as usize) as u32;

        num_elements.min(max_team_size.max(1))
    }

    /// Queues a job immediately or parks it behind a nonzero wait counter.
    fn queue_job_or_wait(
        self: &Arc<Self>,
        job: JobDecl,
        wait_counter: Option<Arc<CounterEntry>>,
        accum_counter: Arc<CounterEntry>,
    ) {
        if let Some(wait_counter) = wait_counter {
            if !wait_counter.is_zero() {
                match wait_counter
                    .try_add_to_waiting(WaitingJob::new(job, Arc::clone(&accum_counter)))
                {
                    Ok(()) => return,
                    Err(waiting_job) => {
                        let (job, returned_accum_counter) = waiting_job.into_parts();
                        self.queue_job_and_signal(job, returned_accum_counter);
                        return;
                    }
                }
            }
        }

        self.queue_job_and_signal(job, accum_counter);
    }

    /// Queues a counted job and treats shutdown as a caller-visible panic.
    fn queue_job_and_signal(&self, job: JobDecl, accum_counter: Arc<CounterEntry>) {
        if self.try_queue_job_and_signal(job, accum_counter).is_err() {
            panic!("cannot dispatch job after shutdown");
        }
    }

    /// Fallible queueing path used when shutdown may discard pending work.
    fn try_queue_job_and_signal(
        &self,
        job: JobDecl,
        accum_counter: Arc<CounterEntry>,
    ) -> Result<(), (JobDecl, Arc<CounterEntry>)> {
        let priority = queue_priority_for_job(job.hint(), &accum_counter, &self.config);
        self.try_push_ready_job(JobQueueEntry::new(job, accum_counter), priority)
            .map_err(JobQueueEntry::into_parts)
    }

    /// Requeues a ready entry without incrementing its counter again.
    fn requeue_job(&self, entry: JobQueueEntry) {
        let (job, accum_counter) = entry.into_parts();
        self.queue_job_and_signal(job, accum_counter);
    }

    /// Runs a ready queue entry through the central execution hook.
    ///
    /// Workers and flush execution enter here instead of calling closures
    /// directly. The entry's accumulate counter is decremented after the closure
    /// returns, and a zero transition releases dependent waiting jobs.
    pub(crate) fn run_job_queue_entry(
        self: &Arc<Self>,
        entry: JobQueueEntry,
        thread_index: u32,
        priority: Priority,
    ) {
        let name = entry.job_name();
        on_job_start(name, thread_index, priority);

        let (job, accum_counter) = entry.into_parts();
        let continuation_counter = Arc::clone(&accum_counter);
        let ctx = RunContext {
            name,
            thread_index,
            parallel_for_index: -1,
            dispatcher: self.dispatcher_handle(),
            continuation: ContinuationContext {
                counter: continuation_counter,
                param: ScheduleParam { priority },
            },
        };

        job.run(&ctx);
        on_job_finish(name, thread_index, priority);
        self.decrement_counter_entry(accum_counter);
    }

    /// Completes one outstanding unit and releases waiters on a zero transition.
    pub(crate) fn decrement_counter_entry(&self, counter: Arc<CounterEntry>) {
        if !counter.decrement() || self.is_shutdown() {
            return;
        }

        self.ready_queues.notify_all_waiters();

        let mut waiting_jobs = counter.flush_waiting();
        while let Some(waiting_job) = waiting_jobs.pop() {
            if self.is_shutdown() {
                break;
            }

            let (job, accum_counter) = waiting_job.into_parts();
            if self.try_queue_job_and_signal(job, accum_counter).is_err() {
                break;
            }
        }
    }

    /// Creates a private cloneable handle to this runtime.
    fn dispatcher_handle(self: &Arc<Self>) -> DispatcherHandle {
        DispatcherHandle {
            inner: Arc::clone(self),
        }
    }

    /// Enters the non-reentrant flush section.
    fn enter_flush(&self) -> FlushGuard<'_> {
        if self.is_flushing.swap(true, Ordering::AcqRel) {
            panic!("reentrantly flushing counter");
        }

        FlushGuard {
            is_flushing: &self.is_flushing,
        }
    }
}

/// Private handle carried by counters, builders, deferrals, and run contexts.
///
/// The handle is intentionally small: cloning it only clones the `Arc` to the
/// shared runtime state. It does not clone queues, workers, or any counter.
#[derive(Clone)]
pub(crate) struct DispatcherHandle {
    pub(crate) inner: Arc<Dispatcher>,
}

impl DispatcherHandle {
    /// Creates a public counter handle through the shared runtime.
    pub(crate) fn create_counter(&self, priority: Priority, name: &'static str) -> Counter {
        Counter::from_entry(self.clone(), self.inner.create_counter(priority, name))
    }

    /// Dispatches a single job through the shared runtime.
    pub(crate) fn run_job(
        &self,
        job: JobDecl,
        wait_counter: Option<Arc<CounterEntry>>,
        accum_counter: Arc<CounterEntry>,
    ) {
        self.inner.run_job(job, wait_counter, accum_counter);
    }

    /// Dispatches a parallel-for declaration through the shared runtime.
    pub(crate) fn run_parallel_for(
        &self,
        job: ParallelForJob,
        wait_counter: Option<Arc<CounterEntry>>,
        accum_counter: Arc<CounterEntry>,
    ) {
        self.inner
            .run_parallel_for(job, wait_counter, accum_counter);
    }

    /// Decrements a counter through the shared runtime.
    pub(crate) fn decrement_counter_entry(&self, counter: Arc<CounterEntry>) {
        self.inner.decrement_counter_entry(counter);
    }
}

impl fmt::Debug for DispatcherHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DispatcherHandle").finish_non_exhaustive()
    }
}

/// Hook called immediately before a job closure is invoked.
///
/// The v1 runtime keeps this as a no-op so every execution path already has a
/// single stable instrumentation point. Worker jobs and flush-thread jobs both
/// enter through `run_job_queue_entry`, so future tracing or profiler markers can
/// be added here without changing queue, worker, or flush control flow.
fn on_job_start(_name: &'static str, _thread_index: u32, _priority: Priority) {}

/// Hook called immediately after a job closure returns.
///
/// The hook runs before the accumulate counter is decremented. That ordering lets
/// future diagnostics measure the closure body separately from dependency
/// release work, while preserving the current no-op cost model in normal builds.
fn on_job_finish(_name: &'static str, _thread_index: u32, _priority: Priority) {}

#[derive(Clone, Copy)]
struct FlushPolicy {
    min_priority: Priority,
    process_large: bool,
}

struct FlushGuard<'a> {
    is_flushing: &'a AtomicBool,
}

impl Drop for FlushGuard<'_> {
    fn drop(&mut self) {
        self.is_flushing.store(false, Ordering::Release);
    }
}

/// Maps caller priority through runtime-wide scheduling policy.
fn map_priority(priority: Priority, config: &JobSystemConfig) -> Priority {
    if config.all_jobs_critical_path {
        Priority::CriticalPath
    } else {
        priority
    }
}

/// Chooses the actual ready-queue lane for a job.
fn queue_priority_for_job(
    hint: JobHint,
    accum_counter: &CounterEntry,
    config: &JobSystemConfig,
) -> Priority {
    if config.all_jobs_critical_path && hint == JobHint::Large {
        Priority::Latent
    } else {
        accum_counter.priority()
    }
}

/// Computes the flush thread's bounded idle wait for this loop iteration.
fn flush_idle_wait(deadline: Option<Instant>) -> Duration {
    let Some(deadline) = deadline else {
        return FLUSH_IDLE_WAIT;
    };

    std::cmp::min(
        FLUSH_IDLE_WAIT,
        deadline.saturating_duration_since(Instant::now()),
    )
}

/// Returns whether a popped job is eligible to run during the active flush.
fn should_run_during_flush(
    entry: &JobQueueEntry,
    priority: Priority,
    target: &Arc<CounterEntry>,
    policy: FlushPolicy,
) -> bool {
    let same_counter = Arc::ptr_eq(entry.accum_counter(), target);
    let trivial = entry.job_hint() == JobHint::Trivial;
    let large = entry.job_hint() == JobHint::Large;
    let priority_allowed = priority >= policy.min_priority;
    let size_allowed = policy.process_large || !large;

    same_counter || trivial || (priority_allowed && size_allowed)
}

/// Shared progress state for multi-team parallel-for dispatch.
///
/// `next_batch` hands out batch indices to whichever team job is available.
/// `finished_teams` is a completion barrier: the last team job to leave the
/// batch-claim loop is responsible for running the optional epilogue.
struct ParallelForSharedState {
    next_batch: AtomicU32,
    finished_teams: AtomicU32,
    batch_count: u32,
    batch_size: u32,
}

impl ParallelForSharedState {
    /// Creates shared multi-team progress state.
    fn new(batch_count: u32, batch_size: u32) -> Self {
        Self {
            next_batch: AtomicU32::new(0),
            finished_teams: AtomicU32::new(0),
            batch_count,
            batch_size,
        }
    }
}

/// Runs one team job until all available batches have been claimed.
fn run_parallel_for_team(
    job: Arc<ParallelForJob>,
    shared: Arc<ParallelForSharedState>,
    team_size: u32,
    team_index: u32,
    ctx: &RunContext,
) {
    let chunk_ctx = parallel_for_context(ctx, team_index as i32);

    loop {
        let batch_index = shared.next_batch.fetch_add(1, Ordering::AcqRel);
        if batch_index >= shared.batch_count {
            break;
        }

        let start = batch_index.saturating_mul(shared.batch_size);
        let end = start
            .saturating_add(shared.batch_size)
            .min(job.num_elements());
        if start < end {
            job.run_range(start, end, &chunk_ctx);
        }
    }

    if shared.finished_teams.fetch_add(1, Ordering::AcqRel) + 1 == team_size {
        job.run_epilogue_once(ctx);
    }
}

/// Creates a borrowed-context clone with a parallel-for team index.
fn parallel_for_context(ctx: &RunContext, parallel_for_index: i32) -> RunContext {
    RunContext {
        name: ctx.name,
        thread_index: ctx.thread_index,
        parallel_for_index,
        dispatcher: ctx.dispatcher.clone(),
        continuation: ContinuationContext {
            counter: Arc::clone(&ctx.continuation.counter),
            param: ctx.continuation.param,
        },
    }
}

/// Computes the batch count and element count per batch for a parallel-for.
fn parallel_for_batch_geometry(
    num_elements: u32,
    team_size: u32,
    max_batch_size: u32,
) -> (u32, u32) {
    // A zero max batch size means "make one batch per team job". A nonzero
    // value chooses the number of batches with integer division first, then
    // derives the actual chunk size from that batch count. That preserves the
    // established behavior where the resulting chunk can be slightly larger
    // than the requested max; callers should treat this as a batching hint, not
    // a hard per-closure element cap.
    let batch_count = num_elements
        .checked_div(max_batch_size)
        .map(|batch_count| batch_count.max(1))
        .unwrap_or_else(|| team_size.max(1));
    let batch_size = num_elements.div_ceil(batch_count);

    (batch_count, batch_size)
}

// Test bodies live in `src/tests`; the declarations stay here so the unit tests
// remain child modules with access to private dispatcher invariants.
#[cfg(test)]
#[path = "tests/dispatcher_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "tests/dispatcher.rs"]
mod tests;
