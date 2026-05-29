//! Bounded ready queues for jobs that are eligible to run.
//!
//! Ready queues own capacity limits, FIFO lanes, strict priority selection,
//! blocking wakeups, timed flush waits, and shutdown notification. Job execution
//! and counter lifecycle stay in the dispatcher and worker modules.

use std::{
    collections::VecDeque,
    fmt, mem,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use crate::{
    config::JobSystemConfig,
    counter::CounterEntry,
    job_decl::{JobDecl, JobHint},
    priority::Priority,
};

/// A ready-to-run job plus the counter it must decrement after execution.
///
/// The entry owns the `Arc` that keeps the accumulate counter alive while the
/// job sits in a ready queue. Later passes must move this same entry into the
/// runner without cloning or re-counting the job.
pub(crate) struct JobQueueEntry {
    job: JobDecl,
    accum_counter: Arc<CounterEntry>,
}

impl JobQueueEntry {
    /// Creates a ready entry from a job and its already-incremented counter.
    pub(crate) fn new(job: JobDecl, accum_counter: Arc<CounterEntry>) -> Self {
        Self { job, accum_counter }
    }

    /// Static job name for diagnostics and execution hooks.
    pub(crate) fn job_name(&self) -> &'static str {
        self.job.name()
    }

    /// Scheduling hint carried by the job.
    pub(crate) fn job_hint(&self) -> JobHint {
        self.job.hint()
    }

    /// Counter that must be decremented when the job finishes.
    pub(crate) fn accum_counter(&self) -> &Arc<CounterEntry> {
        &self.accum_counter
    }

    /// Splits the entry so dispatch code can run or requeue its parts.
    pub(crate) fn into_parts(self) -> (JobDecl, Arc<CounterEntry>) {
        (self.job, self.accum_counter)
    }
}

impl fmt::Debug for JobQueueEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobQueueEntry")
            .field("job_name", &self.job.name())
            .field("job_hint", &self.job.hint())
            .field("accum_counter", &self.accum_counter.name())
            .finish()
    }
}

/// Returned when a priority lane is already at its fixed capacity.
///
/// The rejected entry is returned to the caller so queue pressure never causes
/// a closure to be dropped silently. Dispatch code can choose to panic, retry,
/// or apply a later explicit backpressure policy.
pub(crate) struct QueueFull<T> {
    entry: T,
    priority: Priority,
    capacity: usize,
}

impl<T> QueueFull<T> {
    /// Priority lane whose capacity was exhausted.
    pub(crate) fn priority(&self) -> Priority {
        self.priority
    }

    /// Fixed capacity of the exhausted lane.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the rejected entry to the caller.
    pub(crate) fn into_entry(self) -> T {
        self.entry
    }
}

impl<T> fmt::Debug for QueueFull<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueFull")
            .field("priority", &self.priority)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

/// Failure modes for pushing into a ready queue.
pub(crate) enum QueuePushError<T> {
    /// The selected priority lane is full.
    Full(QueueFull<T>),
    /// Shutdown began before the entry could be queued.
    Shutdown(T),
}

impl<T> fmt::Debug for QueuePushError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(full) => full.fmt(f),
            Self::Shutdown(_) => f.debug_tuple("QueuePushError::Shutdown").finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReadyQueueCapacities {
    latent: usize,
    render_path: usize,
    critical_path: usize,
    immediate: usize,
}

impl ReadyQueueCapacities {
    /// Builds lane capacities from public job-system configuration.
    pub(crate) fn from_config(config: &JobSystemConfig) -> Self {
        let critical_path = config.max_critical_path_jobs;

        // Large jobs may still be routed to the latent lane when ordinary work
        // is collapsed onto the critical path. Give that escape lane matching
        // headroom so the priority mapping cannot create an accidental tiny cap.
        let latent = if config.all_jobs_critical_path {
            config.max_latent_jobs.max(critical_path)
        } else {
            config.max_latent_jobs
        };

        Self::new(
            latent,
            config.max_critical_path_jobs,
            critical_path,
            config.max_immediate_jobs,
        )
    }

    /// Creates explicit lane capacities and rejects unusable zero-sized lanes.
    pub(crate) fn new(
        latent: usize,
        render_path: usize,
        critical_path: usize,
        immediate: usize,
    ) -> Self {
        assert!(latent > 0, "latent ready-queue capacity must be nonzero");
        assert!(
            render_path > 0,
            "render-path ready-queue capacity must be nonzero"
        );
        assert!(
            critical_path > 0,
            "critical-path ready-queue capacity must be nonzero"
        );
        assert!(
            immediate > 0,
            "immediate ready-queue capacity must be nonzero"
        );

        Self {
            latent,
            render_path,
            critical_path,
            immediate,
        }
    }

    /// Capacity for one priority lane.
    fn for_priority(self, priority: Priority) -> usize {
        match priority {
            Priority::Latent => self.latent,
            Priority::RenderPath => self.render_path,
            Priority::CriticalPath => self.critical_path,
            Priority::Immediate => self.immediate,
        }
    }
}

/// Multi-producer, multi-consumer ready queues with strict priority pop order.
///
/// All lanes are protected by one mutex so a pop observes a coherent priority
/// choice: the highest nonempty lane wins, and FIFO order is preserved inside
/// that lane. The condvar is only a wake mechanism; the mutex-protected state is
/// still the source of truth after every wake, including spurious ones.
pub(crate) struct ReadyQueues {
    state: Mutex<ReadyQueueState>,
    available: Condvar,
    capacities: ReadyQueueCapacities,
}

impl ReadyQueues {
    /// Creates ready queues with capacities derived from configuration.
    pub(crate) fn new(config: &JobSystemConfig) -> Self {
        Self::with_capacities(ReadyQueueCapacities::from_config(config))
    }

    /// Creates ready queues with explicit capacities.
    pub(crate) fn with_capacities(capacities: ReadyQueueCapacities) -> Self {
        Self {
            state: Mutex::new(ReadyQueueState::new()),
            available: Condvar::new(),
            capacities,
        }
    }

    /// Pushes one ready job, returning the entry if shutdown won the race.
    ///
    /// This variant is for internal teardown paths where abandoned jobs should
    /// be dropped instead of panicking worker threads. Capacity exhaustion still
    /// remains explicit because it means the configured queue size was too small
    /// for normal operation.
    pub(crate) fn try_push_if_open(
        &self,
        entry: JobQueueEntry,
        priority: Priority,
    ) -> Result<(), QueuePushError<JobQueueEntry>> {
        let mut state = self.state.lock().expect("ready-queue lock poisoned");
        if state.is_shutdown {
            return Err(QueuePushError::Shutdown(entry));
        }

        let capacity = self.capacities.for_priority(priority);
        let lane = state.lane_mut(priority);
        if lane.len() >= capacity {
            return Err(QueuePushError::Full(QueueFull {
                entry,
                priority,
                capacity,
            }));
        }

        lane.push_back(entry);
        drop(state);
        self.available.notify_one();
        Ok(())
    }

    /// Blocks until a ready job is available or shutdown begins.
    ///
    /// Shutdown is a stop signal, not a drain request. Once shutdown is set,
    /// blocked poppers wake and return `None` even if work had been queued.
    pub(crate) fn pop_blocking(&self) -> Option<(JobQueueEntry, Priority)> {
        let mut state = self.state.lock().expect("ready-queue lock poisoned");

        loop {
            if state.is_shutdown {
                return None;
            }

            if let Some(entry) = state.pop_highest_priority() {
                return Some(entry);
            }

            state = self
                .available
                .wait(state)
                .expect("ready-queue lock poisoned while waiting");
        }
    }

    /// Attempts to pop a ready job without blocking.
    pub(crate) fn try_pop(&self) -> Option<(JobQueueEntry, Priority)> {
        let mut state = self.state.lock().expect("ready-queue lock poisoned");
        if state.is_shutdown {
            None
        } else {
            state.pop_highest_priority()
        }
    }

    /// Waits briefly for ready work to appear without taking ownership of it.
    ///
    /// This is used by the flush thread after a nonblocking pop found no work.
    /// A push wakes the wait early, while the timeout keeps counter-only
    /// completions responsive even when no new job is queued.
    pub(crate) fn wait_for_ready_job_timeout<F>(&self, timeout: Duration, should_wait: F)
    where
        F: FnOnce() -> bool,
    {
        let state = self.state.lock().expect("ready-queue lock poisoned");
        if state.is_shutdown || state.has_ready_job() || !should_wait() {
            return;
        }

        drop(
            self.available
                .wait_timeout(state, timeout)
                .expect("ready-queue lock poisoned while waiting for ready work"),
        );
    }

    /// Waits for a queue wake even if ready work is already present.
    ///
    /// Flush uses this after it declines and requeues a job. Sleeping on the
    /// queue wake path gives workers a chance to claim the requeued entry and
    /// prevents the flush thread from repeatedly popping the same ineligible
    /// work item in a tight loop.
    pub(crate) fn wait_for_queue_wakeup_timeout<F>(&self, timeout: Duration, should_wait: F)
    where
        F: FnOnce() -> bool,
    {
        let state = self.state.lock().expect("ready-queue lock poisoned");
        if state.is_shutdown || !should_wait() {
            return;
        }

        drop(
            self.available
                .wait_timeout(state, timeout)
                .expect("ready-queue lock poisoned while waiting for queue wakeup"),
        );
    }

    /// Wakes every thread sleeping on queue progress without adding work.
    ///
    /// Counter completion can make a flush wait finish even when no ready job is
    /// queued. Taking the queue mutex before notifying pairs with the wait-side
    /// predicate check so that a zero-counter transition cannot be missed
    /// between "checked the counter" and "started sleeping".
    pub(crate) fn notify_all_waiters(&self) {
        let state = self.state.lock().expect("ready-queue lock poisoned");
        drop(state);
        self.available.notify_all();
    }

    /// Stops the queue and wakes every blocked popper.
    ///
    /// Queued jobs are dropped here because shutdown does not promise to drain
    /// pending work. Any job already popped by another thread is outside the
    /// queue and will be handled by the worker/dispatcher shutdown policy.
    pub(crate) fn shutdown(&self) {
        let mut state = self.state.lock().expect("ready-queue lock poisoned");
        if state.is_shutdown {
            return;
        }

        state.is_shutdown = true;
        let detached_jobs = state.take_all();
        drop(state);
        self.available.notify_all();
        drop(detached_jobs);
    }
}

struct ReadyQueueState {
    latent: VecDeque<JobQueueEntry>,
    render_path: VecDeque<JobQueueEntry>,
    critical_path: VecDeque<JobQueueEntry>,
    immediate: VecDeque<JobQueueEntry>,
    is_shutdown: bool,
}

impl ReadyQueueState {
    /// Creates empty lanes in the open state.
    fn new() -> Self {
        Self {
            latent: VecDeque::new(),
            render_path: VecDeque::new(),
            critical_path: VecDeque::new(),
            immediate: VecDeque::new(),
            is_shutdown: false,
        }
    }

    /// Pops from the highest nonempty priority lane.
    fn pop_highest_priority(&mut self) -> Option<(JobQueueEntry, Priority)> {
        self.immediate
            .pop_front()
            .map(|entry| (entry, Priority::Immediate))
            .or_else(|| {
                self.critical_path
                    .pop_front()
                    .map(|entry| (entry, Priority::CriticalPath))
            })
            .or_else(|| {
                self.render_path
                    .pop_front()
                    .map(|entry| (entry, Priority::RenderPath))
            })
            .or_else(|| {
                self.latent
                    .pop_front()
                    .map(|entry| (entry, Priority::Latent))
            })
    }

    /// Whether any priority lane currently has ready work.
    fn has_ready_job(&self) -> bool {
        !self.immediate.is_empty()
            || !self.critical_path.is_empty()
            || !self.render_path.is_empty()
            || !self.latent.is_empty()
    }

    /// Mutable access to a priority lane.
    fn lane_mut(&mut self, priority: Priority) -> &mut VecDeque<JobQueueEntry> {
        match priority {
            Priority::Latent => &mut self.latent,
            Priority::RenderPath => &mut self.render_path,
            Priority::CriticalPath => &mut self.critical_path,
            Priority::Immediate => &mut self.immediate,
        }
    }

    /// Removes every queued job so shutdown can drop them outside the lock.
    fn take_all(&mut self) -> DetachedReadyJobs {
        DetachedReadyJobs {
            _latent: mem::take(&mut self.latent),
            _render_path: mem::take(&mut self.render_path),
            _critical_path: mem::take(&mut self.critical_path),
            _immediate: mem::take(&mut self.immediate),
        }
    }
}

struct DetachedReadyJobs {
    _latent: VecDeque<JobQueueEntry>,
    _render_path: VecDeque<JobQueueEntry>,
    _critical_path: VecDeque<JobQueueEntry>,
    _immediate: VecDeque<JobQueueEntry>,
}

#[cfg(test)]
#[path = "tests/queue_support.rs"]
mod test_support;

// Test bodies live in `src/tests`; the declaration stays here so the unit tests
// remain child modules with access to private queue invariants.
#[cfg(test)]
#[path = "tests/queue.rs"]
mod tests;
