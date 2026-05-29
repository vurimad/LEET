use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_queue::ArrayQueue;
use leet_log::error;

use crate::builder::RunContext;
use crate::counter::{Counter, CounterInner};
use crate::dispatcher_entries::{JobQueueEntry, WaitingListEntry};
use crate::dispatcher_thread::{
    current_dispatcher_thread_index, pop_local_job, push_local_job, DispatcherThread,
    DispatcherThreadSetup,
};
use crate::job_decl::{JobDecl, JobHint, OwnedJobDecl};
use crate::priority::{Priority, PRIORITY_COUNT};
use crate::semaphore::Semaphore;

// ---------------------------------------------------------------------------
// DispatcherInner — shared state between Dispatcher and worker threads
// ---------------------------------------------------------------------------

pub(crate) struct DispatcherInner {
    /// One bounded lock-free MPMC queue per priority lane.
    /// Index = Priority as usize (0 = Latent … 3 = Immediate).
    queues: [ArrayQueue<JobQueueEntry>; PRIORITY_COUNT],
    /// Workers sleep here when all queues are empty.
    semaphore: Semaphore,
    /// Set to true on shutdown; workers exit their loops.
    exit: AtomicBool,
}

impl DispatcherInner {
    fn new(queue_capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            // ArrayQueue requires a const capacity; we allocate four separately.
            queues: [
                ArrayQueue::new(queue_capacity), // Latent
                ArrayQueue::new(queue_capacity), // RenderPath
                ArrayQueue::new(queue_capacity), // CriticalPath
                ArrayQueue::new(queue_capacity), // Immediate
            ],
            semaphore: Semaphore::new(0),
            exit: AtomicBool::new(false),
        })
    }

    // ------------------------------------------------------------------
    // Global queue helpers
    // ------------------------------------------------------------------

    /// Push a job into the global queue at the given priority WITHOUT signalling.
    ///
    /// The caller is responsible for calling `semaphore.release` afterwards.
    /// Use this when pushing multiple jobs in a batch so the semaphore is only
    /// hit once (see `decrement`).
    fn push_to_queue(&self, job: JobQueueEntry, priority: Priority) {
        let queue = &self.queues[priority as usize];
        let mut job = job;
        loop {
            match queue.push(job) {
                Ok(()) => break,
                Err(dropped) => {
                    job = dropped;
                    core::hint::spin_loop();
                }
            }
        }
    }

    /// Push a job into the global queue at the given priority and signal one worker.
    ///
    /// If the queue is full this spins in place — matching the C++ back-pressure
    /// behaviour where the dispatcher stalls rather than dropping work.
    pub(crate) fn push_global(&self, job: JobQueueEntry, priority: Priority) {
        self.push_to_queue(job, priority);
        self.semaphore.release(1);
    }

    /// Returns `true` once the dispatcher has been signalled to shut down.
    ///
    /// Jobs that block internally (e.g., polling a condvar) should check this
    /// flag in their park loop so workers can exit cleanly during `Drop`.
    #[allow(dead_code)]
    pub(crate) fn is_exiting(&self) -> bool {
        self.exit.load(Ordering::Acquire)
    }

    /// Block until a worker signal is available.
    pub(crate) fn acquire_work_signal(&self) {
        self.semaphore.acquire();
    }

    /// Try to pop the highest-priority available job without blocking.
    pub(crate) fn try_pop(&self) -> Option<(JobQueueEntry, Priority)> {
        // Drain from highest priority downward (Immediate first).
        for idx in (0..PRIORITY_COUNT).rev() {
            if let Some(job) = self.queues[idx].pop() {
                let priority = Priority::ALL[idx];
                return Some((job, priority));
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // Job submission
    // ------------------------------------------------------------------

    /// Increment the accumulate counter and either queue the job immediately
    /// or park it on the wait counter's waiting list.
    ///
    /// This is the single entry point for all job submissions — mirrors
    /// `Dispatcher::RunJob` + `QueueJobOrWait` in the C++ original.
    pub(crate) fn submit(
        &self,
        job_decl: OwnedJobDecl,
        wait_for: Option<Arc<CounterInner>>,
        accumulate: Arc<CounterInner>,
        priority: Priority,
    ) {
        // Count this job as in-flight in the accumulate counter.
        accumulate.value.fetch_add(1, Ordering::Relaxed);

        let queued = JobQueueEntry {
            job_decl,
            accumulate_counter_entry: accumulate,
        };

        if let Some(ref gate) = wait_for {
            if !gate.is_zero_snapshot() {
                // Attempt to park on the waiting list (under lock, with re-check).
                // The re-check under lock is critical: the C++ has a detailed comment
                // about a race where the counter hits zero between our snapshot and
                // the lock acquisition.
                let mut list = gate.waiting.lock().unwrap();
                if !gate.is_zero_snapshot() {
                    list.push(WaitingListEntry {
                        job_decl: queued.job_decl,
                        accumulate_counter_entry: queued.accumulate_counter_entry,
                        priority,
                    });
                    return;
                }
                // Counter became zero while we were acquiring the lock — fall through.
                drop(list);
            }
        }

        self.push_global(queued, priority);
    }

    pub(crate) fn submit_closure<F>(
        &self,
        job: F,
        wait_for: Option<Arc<CounterInner>>,
        accumulate: Arc<CounterInner>,
        priority: Priority,
        hint: JobHint,
    ) where
        F: FnOnce() + Send + 'static,
    {
        self.submit(
            OwnedJobDecl::from_closure(job, hint, None),
            wait_for,
            accumulate,
            priority,
        );
    }

    /// Push a child job to the calling thread's local queue.
    ///
    /// Only callable from inside a running job (i.e., from a worker thread).
    /// Never touches the global queue or the semaphore.
    pub(crate) fn submit_local(
        job_decl: OwnedJobDecl,
        accumulate: Arc<CounterInner>,
        priority: Priority,
    ) {
        accumulate.value.fetch_add(1, Ordering::Relaxed);
        push_local_job(
            JobQueueEntry {
                job_decl,
                accumulate_counter_entry: accumulate,
            },
            priority,
        );
    }

    pub(crate) fn submit_local_closure<F>(
        job: F,
        accumulate: Arc<CounterInner>,
        priority: Priority,
        hint: JobHint,
    ) where
        F: FnOnce() + Send + 'static,
    {
        Self::submit_local(
            OwnedJobDecl::from_closure(job, hint, None),
            accumulate,
            priority,
        );
    }

    // ------------------------------------------------------------------
    // Counter decrement + waiting list flush
    // ------------------------------------------------------------------

    /// Decrement `counter`; if it reaches zero, release all parked jobs back
    /// into the global queue.
    ///
    /// Mirrors `Dispatcher::DecrementCounterEntryInternal`.
    pub(crate) fn decrement(self: &Arc<Self>, counter: &Arc<CounterInner>) {
        let prev = counter.value.fetch_sub(1, Ordering::AcqRel);
        if prev != 1 {
            return; // Still non-zero — nothing to flush.
        }

        // Value just hit zero.  Under lock, verify it's still zero, then drain
        // the waiting list (same race-guard as the C++ `FlushWaitingList`).
        // mem::take — O(1) and releases the lock before we do any push_global work.
        let waiting: Vec<WaitingListEntry> = {
            let mut list = counter.waiting.lock().unwrap();
            if !counter.is_zero_snapshot() {
                return; // Incremented again while we were locking — leave it.
            }
            std::mem::take(&mut *list)
        };

        // RED mirrors an intrusive LIFO waiting list and calls QueueJobAndSignal
        // for every released entry. Keep that shape here instead of batching
        // semaphore wakeups: the job system is too central to let cleverness
        // drift from RED's scheduling behavior.
        for w in waiting.into_iter().rev() {
            self.push_global(
                JobQueueEntry {
                    job_decl: w.job_decl,
                    accumulate_counter_entry: w.accumulate_counter_entry,
                },
                w.priority,
            );
        }

        // Wake any thread parked in Dispatcher::flush waiting on this counter.
        counter.zero_condvar.1.notify_all();
    }

    /// Execute a job, **always** decrementing `accumulate` even if the job panics.
    ///
    /// Without this guard a panicking job would leave `accumulate` permanently
    /// non-zero, silently gating every downstream job forever while the worker
    /// thread dies.  Using `catch_unwind` + unconditional `decrement` ensures:
    ///
    /// * The accumulate counter correctly reaches zero.
    /// * Waiting jobs that were parked on `accumulate` are released.
    /// * The worker (or flushing thread) stays alive for subsequent work.
    ///
    /// The panic payload is printed to stderr and then discarded, matching the
    /// "log and continue" philosophy for shared infrastructure.
    ///
    /// Requires `panic = "unwind"` (the Rust default for dev/test profiles).
    /// With `panic = "abort"` the process terminates before catch_unwind acts.
    pub(crate) fn execute_job(
        self: &Arc<Self>,
        job_decl: OwnedJobDecl,
        accumulate: Arc<CounterInner>,
    ) {
        let raw_job_decl = job_decl.job_decl();
        let instrumentation_object = raw_job_decl.instrumentation_object;
        let debug_name = instrumentation_object.unwrap_or(accumulate.debug_name);
        let run_context = RunContext::for_job(
            self,
            accumulate.param,
            debug_name,
            instrumentation_object,
            current_dispatcher_thread_index(),
            -1,
            Some(Counter(Arc::clone(&accumulate))),
        );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            job_decl.run(&run_context);
        }));
        // Decrement unconditionally — panic or not.
        self.decrement(&accumulate);
        if let Err(payload) = result {
            let msg = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("<non-string panic>");
            error!("[leet_jobs] caught panic in job: {}", msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Per-worker thread configuration
// ---------------------------------------------------------------------------

/// OS thread priority for a worker.
///
/// Maps directly to `thread_priority::ThreadPriority`.
/// Provided as a simpler enum to avoid leaking the dependency at the API boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerPriority {
    /// Background priority.  The OS should only schedule this thread when no
    /// higher-priority thread is runnable.  Mirrors C++ `TP_Lowest`.
    Lowest,
    /// Below normal — useful for streaming / latent work.
    Low,
    /// Default — standard game-logic work.
    Normal,
    /// Above normal — render-path work.
    High,
    /// Highest available user-mode priority (not real-time).
    Highest,
}

/// Configuration for a single worker thread.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// OS thread priority for this worker.
    pub priority: WorkerPriority,
    /// Optional CPU core affinity.
    ///
    /// If `Some`, the thread is pinned to the listed core IDs.
    /// If `None`, the OS is free to schedule the thread on any core.
    pub core_affinity: Option<Vec<usize>>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            priority: WorkerPriority::Normal,
            core_affinity: None,
        }
    }
}

// ---------------------------------------------------------------------------
// JobSystemConfig
// ---------------------------------------------------------------------------

/// Configuration for the job dispatcher.
pub struct JobSystemConfig {
    /// Number of worker threads to spawn.
    /// Defaults to `hardware_concurrency - 1` (reserves the main thread).
    pub num_threads: usize,
    /// Capacity of each per-priority MPMC queue (must be ≥ 1).
    /// Defaults to 4096 slots per lane.
    pub queue_capacity: usize,
    /// Per-worker overrides.  If shorter than `num_threads`, remaining workers
    /// use [`WorkerConfig::default()`].  If empty, all workers use the default.
    pub worker_configs: Vec<WorkerConfig>,
}

impl Default for JobSystemConfig {
    fn default() -> Self {
        let hw = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .saturating_sub(1)
            .max(1);
        Self {
            num_threads: hw,
            queue_capacity: 4096,
            worker_configs: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatcher — public API
// ---------------------------------------------------------------------------

/// The job dispatcher.
///
/// Owns a thread pool and four priority-ordered MPMC queues (no work-stealing).
/// Each worker drains its own thread-local child queue before sleeping on the
/// global semaphore — mirroring the C++ `DoWorkLoop` exactly.
pub struct Dispatcher {
    inner: Arc<DispatcherInner>,
    threads: Vec<DispatcherThread>,
}

impl Dispatcher {
    /// Create a dispatcher with the given configuration and spawn worker threads.
    ///
    /// Each worker applies its OS thread priority and core affinity before
    /// entering the work loop.  If setting either one fails, the thread still
    /// runs (fail-open) but an error message is printed to stderr — we never
    /// silently lose a worker.
    pub fn new(config: JobSystemConfig) -> Self {
        assert!(
            config.num_threads >= 1,
            "[leet_jobs] num_threads must be >= 1"
        );
        assert!(
            config.queue_capacity >= 1,
            "[leet_jobs] queue_capacity must be >= 1"
        );

        let inner = DispatcherInner::new(config.queue_capacity);
        let mut threads = Vec::with_capacity(config.num_threads);

        for i in 0..config.num_threads {
            let worker_cfg = config.worker_configs.get(i).cloned().unwrap_or_default();
            let thread_index = i + 1; // 0 is reserved for the main thread (matches C++)
            let setup = DispatcherThreadSetup {
                stack_size_kb: 0,
                core_affinity: worker_cfg.core_affinity.clone(),
                dispatcher_thread_index: thread_index as u32,
            };

            let worker = DispatcherThread::spawn(
                format!("leet-worker-{}", thread_index),
                Arc::clone(&inner),
                setup,
                worker_cfg.priority,
            );

            while !worker.is_ready() {
                std::thread::yield_now();
            }

            threads.push(worker);
        }

        Self { inner, threads }
    }

    /// Submit a job to the dispatcher.
    ///
    /// - `wait_for`  — optional gate: job will not start until this counter reaches zero.
    /// - `accumulate`— counter that tracks this job (incremented now, decremented on completion).
    /// - `priority`  — which lane this job belongs to.
    pub fn submit(
        &self,
        job: impl FnOnce() + Send + 'static,
        wait_for: Option<&Counter>,
        accumulate: &Counter,
        priority: Priority,
    ) {
        self.inner.submit(
            OwnedJobDecl::from_closure(job, JobHint::None, None),
            wait_for.map(|c| Arc::clone(c.inner())),
            Arc::clone(accumulate.inner()),
            priority,
        );
    }

    /// Submit a job with a RED-style hint used by flush policy.
    pub fn submit_with_hint(
        &self,
        job: impl FnOnce() + Send + 'static,
        wait_for: Option<&Counter>,
        accumulate: &Counter,
        priority: Priority,
        hint: JobHint,
    ) {
        self.inner.submit(
            OwnedJobDecl::from_closure(job, hint, None),
            wait_for.map(|c| Arc::clone(c.inner())),
            Arc::clone(accumulate.inner()),
            priority,
        );
    }

    /// Submit a raw RED-style [`JobDecl`].
    ///
    /// Prefer [`Dispatcher::submit`] for Rust-owned jobs. This entry point is
    /// only for compatibility with RED-style function-pointer jobs carrying
    /// non-owning `void*` payloads.
    ///
    /// # Safety
    ///
    /// `job_decl.job_data` must remain valid until the queued job has run or
    /// the dispatcher has been dropped, whichever happens first. The pointed-to
    /// data must be safe to access from a worker thread, and `job_func` must
    /// uphold Rust's aliasing and synchronization rules for that data.
    pub unsafe fn submit_job_decl(
        &self,
        job_decl: JobDecl,
        wait_for: Option<&Counter>,
        accumulate: &Counter,
        priority: Priority,
    ) {
        self.inner.submit(
            OwnedJobDecl::borrowed(job_decl),
            wait_for.map(|c| Arc::clone(c.inner())),
            Arc::clone(accumulate.inner()),
            priority,
        );
    }

    /// Submit a child job directly to the calling thread's local queue.
    ///
    /// No semaphore signal is issued; only the current worker thread will pick
    /// this up.  Must only be called from within a running job.
    pub fn submit_local(
        job: impl FnOnce() + Send + 'static,
        accumulate: &Counter,
        priority: Priority,
    ) {
        DispatcherInner::submit_local_closure(
            job,
            Arc::clone(accumulate.inner()),
            priority,
            JobHint::None,
        );
    }

    /// Block the calling thread until `counter` reaches zero, executing jobs
    /// inline while waiting (including the caller's own thread-local queue).
    ///
    /// Mirrors `Dispatcher::FlushCounter`.
    pub fn flush(&self, counter: &Counter) {
        let _ = self.flush_with_priority(counter, Priority::Latent, -1, true);
    }

    /// RED-style `Dispatcher::FlushCounter(counter, processLatent, timeout)`.
    pub fn flush_counter(
        &self,
        counter: &Counter,
        process_latent: bool,
        timeout_milliseconds: i32,
    ) -> bool {
        let process_priority = if process_latent {
            Priority::Latent
        } else {
            counter.inner().param.priority
        };
        self.flush_with_priority(counter, process_priority, timeout_milliseconds, true)
    }

    /// RED-style `Dispatcher::FlushCounter(counter, processPriority, timeout, processLarge)`.
    pub fn flush_with_priority(
        &self,
        counter: &Counter,
        process_priority: Priority,
        timeout_milliseconds: i32,
        process_large: bool,
    ) -> bool {
        let deadline = (timeout_milliseconds >= 0)
            .then(|| Instant::now() + Duration::from_millis(timeout_milliseconds as u64));

        while !counter.is_zero() {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return false;
            }

            let local = pop_local_job();
            let (queued, priority) = if let Some(item) = local {
                item
            } else if let Some(item) = self.inner.try_pop() {
                item
            } else {
                // No work available — workers are mid-execution.
                // Park on the counter's zero condvar to avoid burning a core.
                // A 1 ms timeout guards against any edge case where the
                // notification is missed (e.g. counter already zero at lock time).
                let (ref mtx, ref cvar) = counter.inner().zero_condvar;
                let guard = mtx.lock().unwrap();
                if !counter.is_zero() {
                    let _ = cvar.wait_timeout(guard, Duration::from_millis(1));
                }
                // Exit-aware: if the dispatcher is shutting down, return immediately
                // rather than blocking the calling thread (and work_loop's join) forever.
                // Callers must not rely on the counter being zero after this returns.
                if self.inner.exit.load(Ordering::Acquire) {
                    break;
                }
                continue;
            };

            let mapped_priority = priority;
            let job_hint = queued.job_decl.hint();
            let is_trivial_job = job_hint == JobHint::Trivial;
            let is_large_job = matches!(job_hint, JobHint::Large | JobHint::AudioEvent);
            let can_run_job = process_large || !is_large_job;
            let targets_flushed_counter =
                Arc::ptr_eq(&queued.accumulate_counter_entry, counter.inner());

            if targets_flushed_counter
                || (mapped_priority >= process_priority && can_run_job)
                || is_trivial_job
            {
                let JobQueueEntry {
                    job_decl,
                    accumulate_counter_entry,
                    ..
                } = queued;
                self.inner.execute_job(job_decl, accumulate_counter_entry);
            } else {
                self.inner.push_global(queued, priority);
                std::thread::yield_now();
            }
        }

        counter.is_zero()
    }

    /// Access the inner shared state (used by `Builder`).
    pub(crate) fn inner(&self) -> &Arc<DispatcherInner> {
        &self.inner
    }

    /// Number of worker threads owned by this dispatcher.
    pub(crate) fn num_dispatcher_threads(&self) -> usize {
        self.threads.len()
    }
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        // Signal exit and wake all sleeping workers.
        self.inner.exit.store(true, Ordering::Release);
        self.inner.semaphore.release(self.threads.len() as i32);
        for worker in self.threads.drain(..) {
            let _ = worker.join();
        }
    }
}
