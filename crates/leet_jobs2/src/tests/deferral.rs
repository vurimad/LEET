use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{config::JobSystemConfig, dispatcher::LeetJobSystem, priority::Priority};

fn test_config() -> JobSystemConfig {
    JobSystemConfig {
        max_latent_jobs: 16,
        max_critical_path_jobs: 16,
        max_immediate_jobs: 16,
        worker_thread_stack_size: None,
        max_threads: 1,
        all_jobs_critical_path: false,
        use_debugger: false,
    }
}

fn wait_until_zero(counter: &crate::counter::Counter) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !counter.is_zero() {
        assert!(Instant::now() < deadline, "timed out waiting for counter");
        thread::yield_now();
    }
}

#[test]
fn finish_decrements_deferral_once() {
    let jobs = LeetJobSystem::new(test_config());
    let counter = jobs.create_counter(Priority::CriticalPath);

    let mut deferral = counter.create_deferral("external work");
    assert_eq!(deferral.name(), "external work");
    assert!(!counter.is_zero());

    deferral.finish();

    wait_until_zero(&counter);
    jobs.shutdown();
}

#[test]
fn double_finish_panics() {
    let jobs = LeetJobSystem::new(test_config());
    let counter = jobs.create_counter(Priority::CriticalPath);
    let mut deferral = counter.create_deferral("external work");

    deferral.finish();
    let result = catch_unwind(AssertUnwindSafe(|| deferral.finish()));

    assert!(result.is_err());
    jobs.shutdown();
}

#[test]
fn drop_auto_finishes_unfinished_deferral() {
    let jobs = LeetJobSystem::new(test_config());
    let counter = jobs.create_counter(Priority::CriticalPath);

    {
        let _deferral = counter.create_deferral("drop work");
        assert!(!counter.is_zero());
    }

    wait_until_zero(&counter);
    jobs.shutdown();
}

#[test]
fn finished_deferral_drop_does_not_panic_or_decrement_again() {
    let jobs = LeetJobSystem::new(test_config());
    let counter = jobs.create_counter(Priority::CriticalPath);

    {
        let mut deferral = counter.create_deferral("finished work");
        deferral.finish();
    }

    wait_until_zero(&counter);
    jobs.shutdown();
}

#[test]
fn dropped_deferral_releases_waiting_jobs() {
    let jobs = LeetJobSystem::new(test_config());
    jobs.claim_flush_thread();
    let counter = jobs.create_counter(Priority::CriticalPath);
    let deferral = counter.create_deferral("gate");
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_wait(&counter);
    builder.dispatch_job("after deferral", move |_ctx| {
        ran_for_job.fetch_add(1, Ordering::AcqRel);
    });
    let wait = builder.extract_wait_counter();

    assert_eq!(ran.load(Ordering::Acquire), 0);
    drop(deferral);
    assert!(jobs.flush_counter(&wait));
    assert_eq!(ran.load(Ordering::Acquire), 1);
    jobs.shutdown();
}
