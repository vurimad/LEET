use std::sync::{atomic::Ordering, Arc};

use super::*;
use crate::{
    counter::Counter,
    job_decl::{JobDecl, JobHint},
};

pub(crate) fn inert_dispatcher() -> Arc<Dispatcher> {
    let config = JobSystemConfig {
        max_threads: 1,
        ..JobSystemConfig::default()
    };
    let inner = Arc::new(Dispatcher::new(config));
    inner.shutdown.store(true, Ordering::Release);
    inner
}

pub(crate) fn dispatcher_handle() -> DispatcherHandle {
    DispatcherHandle {
        inner: inert_dispatcher(),
    }
}

pub(crate) fn dispatch_test_job<F>(
    jobs: &LeetJobSystem,
    name: &'static str,
    priority: Priority,
    f: F,
) -> Counter
where
    F: FnOnce(&RunContext) + Send + 'static,
{
    dispatch_test_job_with_hint_and_wait(jobs, name, priority, JobHint::None, None, f)
}

pub(crate) fn dispatch_test_job_with_hint_and_wait<F>(
    jobs: &LeetJobSystem,
    name: &'static str,
    priority: Priority,
    hint: JobHint,
    wait_counter: Option<&Counter>,
    f: F,
) -> Counter
where
    F: FnOnce(&RunContext) + Send + 'static,
{
    let accum_counter = jobs.create_counter(priority);
    let wait_counter = wait_counter.map(|counter| Arc::clone(counter.entry()));
    jobs.inner.run_job(
        JobDecl::new(name, hint, f),
        wait_counter,
        Arc::clone(accum_counter.entry()),
    );
    accum_counter
}
