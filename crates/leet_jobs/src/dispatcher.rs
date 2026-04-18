use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_queue::ArrayQueue;
use thread_priority::{set_current_thread_priority, ThreadPriority};

use leet_log::{error, warn};

use crate::counter::{Counter, CounterInner, WaitingJob};
use crate::priority::{Priority, PRIORITY_COUNT};
use crate::semaphore::Semaphore;

// ---------------------------------------------------------------------------
// QueuedJob — an entry in the global or local queue
// ---------------------------------------------------------------------------

pub(crate) struct QueuedJob {
    pub(crate) job: Box<dyn FnOnce() + Send>,
    pub(crate) accumulate: Arc<CounterInner>,
}

// ---------------------------------------------------------------------------
// Thread-local child queue
// ---------------------------------------------------------------------------
//
// When a job runs and spawns children, those children are pushed here instead
// of the global queue.  The owning worker thread drains this queue before
// going back to sleep — hot path, zero contention, no semaphore involved.
//
// Other threads NEVER access this queue (no stealing).

thread_local! {
    static LOCAL_QUEUE: RefCell<VecDeque<(QueuedJob, Priority)>> =
        RefCell::new(VecDeque::with_capacity(256));
}

// ---------------------------------------------------------------------------
// DispatcherInner — shared state between Dispatcher and worker threads
// ---------------------------------------------------------------------------

pub(crate) struct DispatcherInner {
    /// One bounded lock-free MPMC queue per priority lane.
    /// Index = Priority as usize (0 = Latent … 3 = Immediate).
    queues: [ArrayQueue<QueuedJob>; PRIORITY_COUNT],
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
    fn push_to_queue(&self, job: QueuedJob, priority: Priority) {
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
    pub(crate) fn push_global(&self, job: QueuedJob, priority: Priority) {
        self.push_to_queue(job, priority);
        self.semaphore.release(1);
    }

    /// Returns `true` once the dispatcher has been signalled to shut down.
    ///
    /// Jobs that block internally (e.g., polling a condvar) should check this
    /// flag in their park loop so workers can exit cleanly during `Drop`.
    pub(crate) fn is_exiting(&self) -> bool {
        self.exit.load(Ordering::Acquire)
    }

    /// Try to pop the highest-priority available job without blocking.
    pub(crate) fn try_pop(&self) -> Option<(QueuedJob, Priority)> {
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
        job: Box<dyn FnOnce() + Send>,
        wait_for: Option<Arc<CounterInner>>,
        accumulate: Arc<CounterInner>,
        priority: Priority,
    ) {
        // Count this job as in-flight in the accumulate counter.
        accumulate.value.fetch_add(1, Ordering::Relaxed);

        let queued = QueuedJob { job, accumulate };

        if let Some(ref gate) = wait_for {
            if !gate.is_zero_snapshot() {
                // Attempt to park on the waiting list (under lock, with re-check).
                // The re-check under lock is critical: the C++ has a detailed comment
                // about a race where the counter hits zero between our snapshot and
                // the lock acquisition.
                let mut list = gate.waiting.lock().unwrap();
                if !gate.is_zero_snapshot() {
                    list.push(WaitingJob {
                        job: queued.job,
                        accumulate: queued.accumulate,
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

    /// Push a child job to the calling thread's local queue.
    ///
    /// Only callable from inside a running job (i.e., from a worker thread).
    /// Never touches the global queue or the semaphore.
    pub(crate) fn submit_local(
        job: Box<dyn FnOnce() + Send>,
        accumulate: Arc<CounterInner>,
        priority: Priority,
    ) {
        accumulate.value.fetch_add(1, Ordering::Relaxed);
        LOCAL_QUEUE.with(|q| {
            q.borrow_mut()
                .push_back((QueuedJob { job, accumulate }, priority));
        });
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
        let waiting: Vec<WaitingJob> = {
            let mut list = counter.waiting.lock().unwrap();
            if !counter.is_zero_snapshot() {
                return; // Incremented again while we were locking — leave it.
            }
            std::mem::take(&mut *list)
        };

        // Push all jobs first, then release the semaphore once for the whole batch.
        // Avoids N condvar syscalls (one per job) collapsing to a single wakeup cost.
        let n = waiting.len();
        for w in waiting {
            self.push_to_queue(
                QueuedJob {
                    job: w.job,
                    accumulate: w.accumulate,
                },
                w.priority,
            );
        }
        if n > 0 {
            self.semaphore.release(n as i32);
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
        job: Box<dyn FnOnce() + Send>,
        accumulate: Arc<CounterInner>,
    ) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
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

impl WorkerPriority {
    fn to_thread_priority(self) -> ThreadPriority {
        match self {
            WorkerPriority::Lowest => ThreadPriority::Min,
            WorkerPriority::Low => ThreadPriority::Crossplatform(
                thread_priority::ThreadPriorityValue::try_from(20u8)
                    .unwrap_or(thread_priority::ThreadPriorityValue::MIN),
            ),
            WorkerPriority::Normal => ThreadPriority::Crossplatform(
                thread_priority::ThreadPriorityValue::try_from(50u8)
                    .unwrap_or(thread_priority::ThreadPriorityValue::MIN),
            ),
            WorkerPriority::High => ThreadPriority::Crossplatform(
                thread_priority::ThreadPriorityValue::try_from(80u8)
                    .unwrap_or(thread_priority::ThreadPriorityValue::MIN),
            ),
            WorkerPriority::Highest => ThreadPriority::Max,
        }
    }
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
    threads: Vec<JoinHandle<()>>,
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
            let d = Arc::clone(&inner);
            let worker_cfg = config.worker_configs.get(i).cloned().unwrap_or_default();

            let thread_index = i + 1; // 0 is reserved for the main thread (matches C++)
            let handle = thread::Builder::new()
                .name(format!("leet-worker-{}", thread_index))
                .spawn(move || {
                    apply_thread_config(thread_index, &worker_cfg);
                    work_loop(d);
                })
                .expect("[leet_jobs] failed to spawn worker thread");
            threads.push(handle);
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
            Box::new(job),
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
        DispatcherInner::submit_local(Box::new(job), Arc::clone(accumulate.inner()), priority);
    }

    /// Block the calling thread until `counter` reaches zero, executing jobs
    /// inline while waiting (including the caller's own thread-local queue).
    ///
    /// Mirrors `Dispatcher::FlushCounter`.
    pub fn flush(&self, counter: &Counter) {
        while !counter.is_zero() {
            // Drain our own local queue first (if called from a worker).
            let local = LOCAL_QUEUE.with(|q| q.borrow_mut().pop_front());
            let (job, _priority) = if let Some(item) = local {
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

            let QueuedJob { job, accumulate } = job;
            self.inner.execute_job(job, accumulate);
        }
    }

    /// Access the inner shared state (used by `Builder`).
    pub(crate) fn inner(&self) -> &Arc<DispatcherInner> {
        &self.inner
    }
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        // Signal exit and wake all sleeping workers.
        self.inner.exit.store(true, Ordering::Release);
        self.inner.semaphore.release(self.threads.len() as i32);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Thread configuration — applied once at the start of each worker thread
// ---------------------------------------------------------------------------

/// Set OS thread priority and core affinity for the calling thread.
///
/// **Fail-open by design**: if we cannot set priority or affinity (e.g.
/// insufficient permissions), the thread continues running at the OS default
/// rather than crashing the engine.  The error is printed to stderr so it
/// is visible in logs.
fn apply_thread_config(thread_index: usize, cfg: &WorkerConfig) {
    // --- Core affinity ---
    if let Some(ref cores) = cfg.core_affinity {
        let core_ids: Vec<core_affinity::CoreId> = cores
            .iter()
            .map(|&c| core_affinity::CoreId { id: c })
            .collect();

        if core_ids.is_empty() {
            warn!(
                "[leet_jobs] worker-{}: core_affinity list is empty, skipping affinity",
                thread_index,
            );
        } else {
            // core_affinity::set_for_current accepts a single CoreId.
            // To pin to multiple cores we'd need platform-specific calls.
            // Pin to the *first* listed core — canonical behaviour matching
            // the C++ single-affinity-mask-per-thread model.
            let target = core_ids[0];
            if !core_affinity::set_for_current(target) {
                warn!(
                    "[leet_jobs] worker-{}: failed to set core affinity to core {}",
                    thread_index, target.id,
                );
            }
        }
    }

    // --- OS thread priority ---
    let tp = cfg.priority.to_thread_priority();
    if let Err(e) = set_current_thread_priority(tp) {
        warn!(
            "[leet_jobs] worker-{}: failed to set thread priority {:?}: {:?}",
            thread_index, cfg.priority, e,
        );
    }
}

// ---------------------------------------------------------------------------
// Worker loop
// ---------------------------------------------------------------------------

/// The function each worker thread runs.
///
/// Priority order: thread-local child queue → global queue (highest lane first).
/// No work is ever stolen from another thread's local queue.
fn work_loop(dispatcher: Arc<DispatcherInner>) {
    loop {
        // 1. Drain thread-local child jobs (spawned by whatever job ran last).
        let local = LOCAL_QUEUE.with(|q| q.borrow_mut().pop_front());
        if let Some((queued, _priority)) = local {
            let QueuedJob { job, accumulate } = queued;
            dispatcher.execute_job(job, accumulate);
            continue;
        }

        // 2. Sleep until the global queue has work.
        dispatcher.semaphore.acquire();
        if dispatcher.exit.load(Ordering::Acquire) {
            break;
        }

        // 3. Pop from global queue (highest priority first).
        if let Some((queued, _priority)) = dispatcher.try_pop() {
            let QueuedJob { job, accumulate } = queued;
            dispatcher.execute_job(job, accumulate);
        }
        // If try_pop() returned None it was a spurious semaphore wake —
        // loop back and acquire again.
    }
}
