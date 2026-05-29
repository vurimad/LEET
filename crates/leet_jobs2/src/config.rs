use std::thread;

const DEFAULT_MAX_LATENT_JOBS: usize = 32 * 1024;
const DEFAULT_MAX_CRITICAL_PATH_JOBS: usize = 16 * 1024;
const DEFAULT_MAX_IMMEDIATE_JOBS: usize = 2 * 1024;
const DEFAULT_WORKER_THREAD_STACK_SIZE: usize = 1024 * 1024;
const EDITOR_QUEUE_MULTIPLIER: usize = 4;
const TOOL_MAX_LATENT_JOBS: usize = 1;
const TOOL_MAX_CRITICAL_PATH_JOBS: usize = 4 * 1024 * 1024;
const TOOL_MAX_IMMEDIATE_JOBS: usize = 1;

/// Startup configuration for a `LeetJobSystem` instance.
///
/// The job system is owned explicitly, so these values are captured when that
/// instance is created rather than read from global state. Queue capacities are
/// fixed-size limits; later dispatch code must treat exhaustion as an explicit
/// policy decision instead of silently growing memory behind the caller's back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSystemConfig {
    /// Capacity of the low-priority lane used for work that can wait.
    pub max_latent_jobs: usize,
    /// Capacity shared by the render-path and critical-path priority lanes.
    pub max_critical_path_jobs: usize,
    /// Capacity of the highest-priority lane.
    pub max_immediate_jobs: usize,
    /// Worker stack size in bytes. `None` means use the platform default.
    pub worker_thread_stack_size: Option<usize>,
    /// Upper bound on worker threads for this job-system instance.
    ///
    /// Startup resolves the actual worker count by clamping this cap to the
    /// machine's available parallelism minus one, with a minimum of one worker.
    /// Small values are therefore useful for deterministic tests and tools,
    /// while large values cannot accidentally oversubscribe the machine.
    pub max_threads: usize,
    /// Maps ordinary work onto the critical path while preserving explicit
    /// large-job handling in the dispatcher.
    pub all_jobs_critical_path: bool,
    /// Reserved hook for debugger/profiling integration. It has no v1 runtime
    /// behavior unless a later feature-gated implementation is added.
    pub use_debugger: bool,
}

impl JobSystemConfig {
    /// Editor-oriented preset with extra queue headroom for bursty tooling work.
    pub fn editor() -> Self {
        let mut config = Self::default();
        config.max_latent_jobs *= EDITOR_QUEUE_MULTIPLIER;
        config.max_critical_path_jobs *= EDITOR_QUEUE_MULTIPLIER;
        config.max_immediate_jobs *= EDITOR_QUEUE_MULTIPLIER;
        config
    }

    /// Tool-oriented preset that funnels ordinary work into a very large
    /// critical-path queue and leaves the other lanes intentionally tiny.
    pub fn tool() -> Self {
        Self {
            max_latent_jobs: TOOL_MAX_LATENT_JOBS,
            max_critical_path_jobs: TOOL_MAX_CRITICAL_PATH_JOBS,
            max_immediate_jobs: TOOL_MAX_IMMEDIATE_JOBS,
            all_jobs_critical_path: true,
            ..Self::default()
        }
    }
}

impl Default for JobSystemConfig {
    fn default() -> Self {
        Self {
            max_latent_jobs: DEFAULT_MAX_LATENT_JOBS,
            max_critical_path_jobs: DEFAULT_MAX_CRITICAL_PATH_JOBS,
            max_immediate_jobs: DEFAULT_MAX_IMMEDIATE_JOBS,
            worker_thread_stack_size: Some(DEFAULT_WORKER_THREAD_STACK_SIZE),
            max_threads: default_max_worker_threads(),
            all_jobs_critical_path: false,
            use_debugger: false,
        }
    }
}

/// Default worker cap based on available CPU parallelism.
///
/// One thread is reserved for the caller/flush side when possible, while
/// single-core environments still receive one worker so the runtime remains
/// usable in tests and tools.
fn default_max_worker_threads() -> usize {
    thread::available_parallelism()
        .map(|parallelism| hardware_worker_count(parallelism.get()))
        .unwrap_or(1)
}

/// Resolves the configured worker cap into the actual worker count to spawn.
pub(crate) fn resolved_worker_thread_count(config: &JobSystemConfig) -> usize {
    resolve_worker_thread_count(config.max_threads, default_max_worker_threads())
}

/// Applies the user cap to the hardware-derived worker count.
fn resolve_worker_thread_count(max_threads: usize, hardware_workers: usize) -> usize {
    assert!(
        max_threads > 0,
        "job system must start at least one worker thread"
    );

    max_threads.min(hardware_workers.max(1))
}

/// Converts total available parallelism into worker-thread capacity.
fn hardware_worker_count(available_parallelism: usize) -> usize {
    available_parallelism.saturating_sub(1).max(1)
}

// Test bodies live in `src/tests`; the declaration stays here so the unit tests
// remain close to the private helpers they validate.
#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
