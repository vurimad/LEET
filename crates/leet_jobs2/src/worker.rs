//! Worker-thread loop and job-system thread-local state.

use std::{
    cell::Cell,
    sync::Arc,
    thread::{self, JoinHandle},
};

use crate::{
    config::{resolved_worker_thread_count, JobSystemConfig},
    dispatcher::Dispatcher,
};

thread_local! {
    static THREAD_INDEX: Cell<Option<u32>> = const { Cell::new(None) };
}

/// Returns the job-system thread index for the current thread.
///
/// Worker threads set this to `Some(1..N)` for the duration of their loop.
/// Threads outside the job system keep the default `None`; the claimed flush
/// thread uses `Some(0)` after `LeetJobSystem::claim_flush_thread()`.
pub(crate) fn current_thread_index() -> Option<u32> {
    THREAD_INDEX.with(Cell::get)
}

/// Updates the thread-local job-system index for the current thread.
///
/// Workers set this when they enter and clear it before exit. The flush thread
/// uses the same slot so public thread-index queries have one source of truth.
pub(crate) fn set_current_thread_index(index: Option<u32>) {
    THREAD_INDEX.with(|thread_index| thread_index.set(index));
}

/// Spawns the configured worker pool and returns their join handles.
///
/// Each worker receives a stable one-based index and uses the dispatcher as the
/// only entry point for executing ready jobs.
pub(crate) fn spawn_workers(
    inner: Arc<Dispatcher>,
    config: &JobSystemConfig,
) -> Vec<JoinHandle<()>> {
    let worker_count = resolved_worker_thread_count(config);

    (1..=worker_count)
        .map(|index| {
            let inner = Arc::clone(&inner);
            let mut builder = thread::Builder::new().name(format!("leet_dispatcher_{index}"));
            if let Some(stack_size) = config.worker_thread_stack_size {
                builder = builder.stack_size(stack_size);
            }

            builder
                .spawn(move || worker_loop(index as u32, inner))
                .unwrap_or_else(|err| panic!("failed to spawn job worker {index}: {err}"))
        })
        .collect()
}

/// Main loop for one worker thread.
fn worker_loop(index: u32, inner: Arc<Dispatcher>) {
    set_current_thread_index(Some(index));

    while let Some((entry, priority)) = inner.pop_ready_job_blocking() {
        if inner.is_shutdown() {
            break;
        }

        inner.run_job_queue_entry(entry, index, priority);
    }

    set_current_thread_index(None);
}
