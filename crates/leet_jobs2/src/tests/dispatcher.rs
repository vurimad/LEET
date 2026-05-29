use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use super::*;

fn test_config(worker_count: usize) -> JobSystemConfig {
    JobSystemConfig {
        max_latent_jobs: 32,
        max_critical_path_jobs: 32,
        max_immediate_jobs: 32,
        worker_thread_stack_size: None,
        max_threads: worker_count,
        all_jobs_critical_path: false,
        use_debugger: false,
    }
}

fn wait_for_count(counter: &AtomicUsize, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while counter.load(Ordering::Acquire) != expected {
        assert!(Instant::now() < deadline, "timed out waiting for jobs");
        thread::yield_now();
    }
}

fn wait_for_counter_zero(counter: &Counter) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !counter.is_zero() {
        assert!(Instant::now() < deadline, "timed out waiting for counter");
        thread::yield_now();
    }
}

fn block_workers(jobs: &LeetJobSystem, worker_count: usize) -> Vec<mpsc::Sender<()>> {
    let (started_tx, started_rx) = mpsc::channel();
    let mut release_txs = Vec::new();

    for _ in 0..worker_count {
        let (release_tx, release_rx) = mpsc::channel();
        release_txs.push(release_tx);
        let started_tx = started_tx.clone();
        test_support::dispatch_test_job(jobs, "block worker", Priority::Immediate, move |_ctx| {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        });
    }

    for _ in 0..worker_count {
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    release_txs
}

fn release_workers(release_txs: Vec<mpsc::Sender<()>>) {
    for release_tx in release_txs {
        release_tx.send(()).unwrap();
    }
}

#[test]
fn configured_worker_count_is_respected() {
    let config = test_config(2);
    let expected = crate::config::resolved_worker_thread_count(&config);
    let jobs = LeetJobSystem::new(config);

    assert_eq!(jobs.num_worker_threads(), expected);

    jobs.shutdown();
}

#[test]
fn current_thread_index_is_none_outside_job_system_threads() {
    assert_eq!(LeetJobSystem::current_thread_index(), None);
}

#[test]
fn worker_tls_index_is_visible_inside_worker_run_job() {
    let jobs = LeetJobSystem::new(test_config(1));
    let (tx, rx) = mpsc::channel();

    test_support::dispatch_test_job(&jobs, "tls", Priority::Immediate, move |ctx| {
        tx.send((
            LeetJobSystem::current_thread_index(),
            ctx.thread_index,
            ctx.parallel_for_index,
            ctx.name,
            ctx.continuation.param.priority,
        ))
        .unwrap();
    });

    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (Some(1), 1, -1, "tls", Priority::Immediate)
    );

    jobs.shutdown();
}

#[test]
fn queued_no_dependency_jobs_run_exactly_once() {
    let jobs = LeetJobSystem::new(test_config(2));
    let ran = Arc::new(AtomicUsize::new(0));

    for _ in 0..8 {
        let ran = Arc::clone(&ran);
        test_support::dispatch_test_job(&jobs, "run once", Priority::CriticalPath, move |_ctx| {
            ran.fetch_add(1, Ordering::AcqRel);
        });
    }

    wait_for_count(&ran, 8);
    jobs.shutdown();
    assert_eq!(ran.load(Ordering::Acquire), 8);
}

#[test]
fn shutdown_is_idempotent() {
    let jobs = LeetJobSystem::new(test_config(1));

    jobs.shutdown();
    jobs.shutdown();

    assert_eq!(jobs.num_worker_threads(), 0);
}

#[test]
fn blocked_workers_wake_and_join_on_shutdown() {
    let jobs = LeetJobSystem::new(test_config(2));

    jobs.shutdown();

    assert_eq!(jobs.num_worker_threads(), 0);
}

#[test]
fn pending_jobs_may_be_dropped_after_shutdown_begins() {
    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    let jobs = LeetJobSystem::new(test_config(1));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let pending_ran = Arc::new(AtomicUsize::new(0));
    let pending_dropped = Arc::new(AtomicUsize::new(0));

    test_support::dispatch_test_job(&jobs, "blocker", Priority::Immediate, move |_ctx| {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let probe = DropProbe(Arc::clone(&pending_dropped));
    let pending_ran_for_job = Arc::clone(&pending_ran);
    test_support::dispatch_test_job(&jobs, "pending", Priority::Immediate, move |_ctx| {
        let _keep_probe_alive_until_drop = &probe;
        pending_ran_for_job.fetch_add(1, Ordering::AcqRel);
    });

    let shutdown_jobs = jobs.clone();
    let shutdown = thread::spawn(move || shutdown_jobs.shutdown());

    wait_for_count(&pending_dropped, 1);
    release_tx.send(()).unwrap();
    shutdown.join().unwrap();

    assert_eq!(pending_ran.load(Ordering::Acquire), 0);
    jobs.shutdown();
}

#[test]
#[should_panic(expected = "cannot dispatch job after shutdown")]
fn dispatch_after_shutdown_panics() {
    let jobs = LeetJobSystem::new(test_config(1));

    jobs.shutdown();
    test_support::dispatch_test_job(&jobs, "late", Priority::Immediate, |_ctx| {});
}

#[test]
#[should_panic(expected = "job system must start at least one worker thread")]
fn zero_worker_configuration_panics() {
    let _ = LeetJobSystem::new(test_config(0));
}

#[test]
fn dropped_handle_does_not_shutdown_workers() {
    let jobs = LeetJobSystem::new(test_config(1));
    let clone = jobs.clone();
    drop(clone);

    let (tx, rx) = mpsc::channel();
    test_support::dispatch_test_job(&jobs, "still alive", Priority::Immediate, move |_ctx| {
        tx.send(()).unwrap();
    });

    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    jobs.shutdown();
}

#[test]
fn shutdown_from_worker_thread_panics_without_stopping_runtime() {
    let jobs = LeetJobSystem::new(test_config(1));
    let shutdown_jobs = jobs.clone();
    let (tx, rx) = mpsc::channel();

    test_support::dispatch_test_job(&jobs, "bad shutdown", Priority::Immediate, move |_ctx| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            shutdown_jobs.shutdown();
        }));
        tx.send(result.is_err()).unwrap();
    });

    assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    assert_eq!(jobs.num_worker_threads(), 1);
    jobs.shutdown();
}

#[test]
fn shutdown_rejects_jobs_that_are_popped_after_exit_request() {
    let jobs = LeetJobSystem::new(test_config(1));
    let ran = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    struct DropProbe(Arc<AtomicUsize>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    let probe = DropProbe(Arc::clone(&dropped));
    let ran_for_job = Arc::clone(&ran);
    test_support::dispatch_test_job(&jobs, "maybe skipped", Priority::Immediate, move |_ctx| {
        let _keep_probe_alive_until_drop = &probe;
        ran_for_job.fetch_add(1, Ordering::AcqRel);
    });

    jobs.shutdown();

    assert!(
        ran.load(Ordering::Acquire) <= 1,
        "a queued job must not run more than once"
    );
    assert_eq!(dropped.load(Ordering::Acquire), 1);
}

#[test]
fn create_counter_maps_priority_through_config() {
    let mut config = test_config(1);
    config.all_jobs_critical_path = true;
    let jobs = LeetJobSystem::new(config);

    let counter = jobs.create_counter(Priority::Latent);

    assert_eq!(counter.entry().priority(), Priority::CriticalPath);
    jobs.shutdown();
}

#[test]
fn job_waiting_on_nonzero_counter_does_not_run_early() {
    let jobs = LeetJobSystem::new(test_config(1));
    let wait_counter = jobs.create_counter(Priority::CriticalPath);
    wait_counter.entry().increment();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let accum_counter = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "waited",
        Priority::CriticalPath,
        JobHint::None,
        Some(&wait_counter),
        move |_ctx| {
            ran_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );

    assert!(!accum_counter.is_zero());
    assert_eq!(ran.load(Ordering::Acquire), 0);

    jobs.inner
        .decrement_counter_entry(Arc::clone(wait_counter.entry()));
    wait_for_count(&ran, 1);
    wait_for_counter_zero(&accum_counter);
    assert!(accum_counter.is_zero());
    jobs.shutdown();
}

#[test]
fn job_waiting_on_zero_counter_queues_immediately() {
    let jobs = LeetJobSystem::new(test_config(1));
    let wait_counter = jobs.create_counter(Priority::CriticalPath);
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let accum_counter = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "ready",
        Priority::CriticalPath,
        JobHint::None,
        Some(&wait_counter),
        move |_ctx| {
            ran_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );

    wait_for_count(&ran, 1);
    wait_for_counter_zero(&accum_counter);
    assert!(accum_counter.is_zero());
    jobs.shutdown();
}

#[test]
fn direct_self_dependency_panics() {
    let jobs = LeetJobSystem::new(test_config(1));
    let counter = jobs.create_counter(Priority::CriticalPath);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        jobs.inner.run_job(
            JobDecl::empty("self dependency"),
            Some(Arc::clone(counter.entry())),
            Arc::clone(counter.entry()),
        );
    }));

    assert!(result.is_err());
    jobs.shutdown();
}

#[test]
fn decrement_to_nonzero_does_not_flush_waiters() {
    let jobs = LeetJobSystem::new(test_config(1));
    let wait_counter = jobs.create_counter(Priority::CriticalPath);
    wait_counter.entry().increment();
    wait_counter.entry().increment();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "waited",
        Priority::CriticalPath,
        JobHint::None,
        Some(&wait_counter),
        move |_ctx| {
            ran_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );

    jobs.inner
        .decrement_counter_entry(Arc::clone(wait_counter.entry()));
    assert_eq!(ran.load(Ordering::Acquire), 0);

    jobs.inner
        .decrement_counter_entry(Arc::clone(wait_counter.entry()));
    wait_for_count(&ran, 1);
    jobs.shutdown();
}

#[test]
fn waiting_jobs_release_lifo_without_second_increment() {
    let jobs = LeetJobSystem::new(test_config(1));
    let wait_counter = jobs.create_counter(Priority::CriticalPath);
    wait_counter.entry().increment();
    let order = Arc::new(Mutex::new(Vec::new()));

    let first_order = Arc::clone(&order);
    let first_accum = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "first",
        Priority::CriticalPath,
        JobHint::None,
        Some(&wait_counter),
        move |_ctx| {
            first_order.lock().unwrap().push("first");
        },
    );
    let second_order = Arc::clone(&order);
    let second_accum = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "second",
        Priority::CriticalPath,
        JobHint::None,
        Some(&wait_counter),
        move |_ctx| {
            second_order.lock().unwrap().push("second");
        },
    );

    jobs.inner
        .decrement_counter_entry(Arc::clone(wait_counter.entry()));

    let deadline = Instant::now() + Duration::from_secs(1);
    while order.lock().unwrap().len() != 2 {
        assert!(Instant::now() < deadline, "timed out waiting for order");
        thread::yield_now();
    }

    wait_for_counter_zero(&first_accum);
    wait_for_counter_zero(&second_accum);
    assert_eq!(*order.lock().unwrap(), vec!["second", "first"]);
    assert!(first_accum.is_zero());
    assert!(second_accum.is_zero());
    jobs.shutdown();
}

#[test]
fn counter_add_assign_preserves_dependency_ordering() {
    let jobs = LeetJobSystem::new(test_config(1));
    let mut first = jobs.create_counter(Priority::CriticalPath);
    let second = jobs.create_counter(Priority::CriticalPath);
    second.entry().increment();

    first += &second;
    assert!(!first.is_zero());

    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);
    let final_counter = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "after composed dependency",
        Priority::CriticalPath,
        JobHint::None,
        Some(&first),
        move |_ctx| {
            ran_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );

    assert_eq!(ran.load(Ordering::Acquire), 0);
    jobs.inner
        .decrement_counter_entry(Arc::clone(second.entry()));
    wait_for_count(&ran, 1);
    wait_for_counter_zero(&final_counter);
    assert!(first.is_zero());
    assert!(final_counter.is_zero());
    jobs.shutdown();
}

#[test]
fn flush_counter_runs_jobs_accumulating_into_target_on_flush_thread() {
    let jobs = LeetJobSystem::new(test_config(1));
    jobs.claim_flush_thread();
    let blockers = block_workers(&jobs, 1);
    let (tx, rx) = mpsc::channel();

    let target =
        test_support::dispatch_test_job(&jobs, "target", Priority::CriticalPath, move |ctx| {
            tx.send((ctx.thread_index, LeetJobSystem::current_thread_index()))
                .unwrap();
        });

    assert!(jobs.flush_counter(&target));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (0, Some(0))
    );

    release_workers(blockers);
    jobs.shutdown();
}

#[test]
fn flush_counter_runs_eligible_higher_priority_work_while_waiting() {
    let jobs = LeetJobSystem::new(test_config(1));
    jobs.claim_flush_thread();
    let blockers = block_workers(&jobs, 1);
    let target = jobs.create_counter(Priority::RenderPath);
    target.entry().increment();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let high = test_support::dispatch_test_job(&jobs, "high", Priority::Immediate, move |_ctx| {
        ran_for_job.fetch_add(1, Ordering::AcqRel);
    });

    assert!(!jobs.flush_counter_with_timeout(&target, Duration::from_millis(20)));
    assert_eq!(ran.load(Ordering::Acquire), 1);
    assert!(high.is_zero());

    jobs.inner
        .decrement_counter_entry(Arc::clone(target.entry()));
    release_workers(blockers);
    jobs.shutdown();
}

#[test]
fn flush_counter_runs_trivial_jobs_even_below_priority_threshold() {
    let jobs = LeetJobSystem::new(test_config(1));
    jobs.claim_flush_thread();
    let blockers = block_workers(&jobs, 1);
    let target = jobs.create_counter(Priority::Immediate);
    target.entry().increment();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let trivial = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "trivial",
        Priority::Latent,
        JobHint::Trivial,
        None,
        move |_ctx| {
            ran_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );

    assert!(!jobs.flush_counter_with_timeout(&target, Duration::from_millis(20)));
    assert_eq!(ran.load(Ordering::Acquire), 1);
    assert!(trivial.is_zero());

    jobs.inner
        .decrement_counter_entry(Arc::clone(target.entry()));
    release_workers(blockers);
    jobs.shutdown();
}

#[test]
fn flush_counter_render_frame_skips_large_jobs_when_workers_are_available() {
    let jobs = LeetJobSystem::new(test_config(3));
    jobs.claim_flush_thread();
    let blockers = block_workers(&jobs, 3);
    let target = jobs.create_counter(Priority::RenderPath);
    target.entry().increment();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let large = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "large",
        Priority::RenderPath,
        JobHint::Large,
        None,
        move |_ctx| {
            ran_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );

    let inner = Arc::clone(&jobs.inner);
    let target_entry = Arc::clone(target.entry());
    let release_target = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        inner.decrement_counter_entry(target_entry);
    });

    assert!(jobs.flush_counter_render_frame(&target));
    release_target.join().unwrap();
    assert_eq!(ran.load(Ordering::Acquire), 0);
    assert!(!large.is_zero());

    release_workers(blockers);
    wait_for_count(&ran, 1);
    jobs.shutdown();
}

#[test]
fn flush_counter_render_frame_runs_large_jobs_when_workers_are_scarce() {
    let jobs = LeetJobSystem::new(test_config(1));
    jobs.claim_flush_thread();
    let blockers = block_workers(&jobs, 1);
    let target = jobs.create_counter(Priority::RenderPath);
    target.entry().increment();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let large = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "large",
        Priority::RenderPath,
        JobHint::Large,
        None,
        move |_ctx| {
            ran_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );

    let inner = Arc::clone(&jobs.inner);
    let target_entry = Arc::clone(target.entry());
    let release_target = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        inner.decrement_counter_entry(target_entry);
    });

    assert!(jobs.flush_counter_render_frame(&target));
    release_target.join().unwrap();
    assert_eq!(ran.load(Ordering::Acquire), 1);
    assert!(large.is_zero());

    release_workers(blockers);
    jobs.shutdown();
}

#[test]
fn flush_requeues_ineligible_jobs_without_second_increment() {
    let jobs = LeetJobSystem::new(test_config(1));
    jobs.claim_flush_thread();
    let blockers = block_workers(&jobs, 1);
    let target = jobs.create_counter(Priority::Immediate);
    target.entry().increment();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let skipped = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "skipped",
        Priority::Latent,
        JobHint::Large,
        None,
        move |_ctx| {
            ran_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );

    assert!(!jobs.flush_counter_with_timeout(&target, Duration::from_millis(20)));
    assert_eq!(ran.load(Ordering::Acquire), 0);
    assert!(!skipped.is_zero());

    jobs.inner
        .decrement_counter_entry(Arc::clone(target.entry()));
    release_workers(blockers);
    wait_for_count(&ran, 1);
    wait_for_counter_zero(&skipped);
    assert!(skipped.is_zero());
    jobs.shutdown();
}

#[test]
fn flush_counter_rejects_reentrant_use() {
    let jobs = LeetJobSystem::new(test_config(1));
    jobs.claim_flush_thread();
    let blockers = block_workers(&jobs, 1);
    let nested_jobs = jobs.clone();
    let nested_counter = jobs.create_counter(Priority::CriticalPath);
    let (tx, rx) = mpsc::channel();

    let outer =
        test_support::dispatch_test_job(&jobs, "outer", Priority::CriticalPath, move |_ctx| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                nested_jobs.flush_counter(&nested_counter);
            }));
            tx.send(result.is_err()).unwrap();
        });

    assert!(jobs.flush_counter(&outer));
    assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    release_workers(blockers);
    jobs.shutdown();
}

#[test]
fn flush_counter_rejects_wrong_thread_in_debug_builds() {
    let jobs = LeetJobSystem::new(test_config(1));
    jobs.claim_flush_thread();
    let counter = jobs.create_counter(Priority::CriticalPath);
    let flush_jobs = jobs.clone();

    let result = thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            flush_jobs.flush_counter(&counter);
        }))
        .is_err()
    })
    .join()
    .unwrap();

    assert!(result);
    jobs.shutdown();
}

#[test]
fn large_jobs_route_to_latent_when_all_jobs_are_critical_path() {
    let mut config = test_config(1);
    config.all_jobs_critical_path = true;
    let jobs = LeetJobSystem::new(config);
    let blockers = block_workers(&jobs, 1);

    let large = test_support::dispatch_test_job_with_hint_and_wait(
        &jobs,
        "large",
        Priority::Immediate,
        JobHint::Large,
        None,
        |_ctx| {},
    );

    assert_eq!(
        jobs.inner
            .ready_queues
            .try_pop()
            .map(|(_entry, priority)| priority),
        Some(Priority::Latent)
    );
    assert!(!large.is_zero());

    release_workers(blockers);
    jobs.shutdown();
}

#[test]
fn many_counter_chains_complete_in_dependency_order() {
    let jobs = LeetJobSystem::new(test_config(4));
    jobs.claim_flush_thread();
    let observed_next = Arc::new(AtomicUsize::new(0));
    let ordering_failures = Arc::new(AtomicUsize::new(0));
    let mut previous_counter = None;

    for step in 0..96 {
        let observed_next_for_job = Arc::clone(&observed_next);
        let ordering_failures_for_job = Arc::clone(&ordering_failures);
        let mut builder = jobs.create_builder(Priority::CriticalPath);
        if let Some(previous_counter) = &previous_counter {
            builder.dispatch_wait(previous_counter);
        }

        builder.dispatch_job("chain step", move |_ctx| {
            let observed = observed_next_for_job.fetch_add(1, Ordering::AcqRel);
            if observed != step {
                ordering_failures_for_job.fetch_add(1, Ordering::AcqRel);
            }
        });
        previous_counter = Some(builder.extract_wait_counter());
    }

    let final_counter = previous_counter.expect("chain must produce a final counter");
    assert!(jobs.flush_counter(&final_counter));
    assert_eq!(observed_next.load(Ordering::Acquire), 96);
    assert_eq!(ordering_failures.load(Ordering::Acquire), 0);
    jobs.shutdown();
}

#[test]
fn repeated_flushes_do_not_reenter_or_leak_jobs() {
    let jobs = LeetJobSystem::new(test_config(1));
    jobs.claim_flush_thread();
    let blockers = block_workers(&jobs, 1);
    let ran = Arc::new(AtomicUsize::new(0));

    for _ in 0..32 {
        let ran_for_job = Arc::clone(&ran);
        let counter = test_support::dispatch_test_job(
            &jobs,
            "repeated flush target",
            Priority::CriticalPath,
            move |ctx| {
                assert_eq!(ctx.thread_index, 0);
                ran_for_job.fetch_add(1, Ordering::AcqRel);
            },
        );

        assert!(jobs.flush_counter(&counter));
        assert!(counter.is_zero());
    }

    assert_eq!(ran.load(Ordering::Acquire), 32);
    release_workers(blockers);
    jobs.shutdown();
}

#[test]
fn concurrent_producers_dispatch_without_losing_work() {
    const PRODUCERS: usize = 6;
    const JOBS_PER_PRODUCER: usize = 24;

    let jobs = LeetJobSystem::new(test_config(4));
    jobs.claim_flush_thread();
    let ran = Arc::new(AtomicUsize::new(0));

    let producers = (0..PRODUCERS)
        .map(|_| {
            let jobs = jobs.clone();
            let ran = Arc::clone(&ran);
            thread::spawn(move || {
                let mut builder = jobs.create_builder(Priority::CriticalPath);
                for _ in 0..JOBS_PER_PRODUCER {
                    let ran_for_job = Arc::clone(&ran);
                    builder.dispatch_job_no_fence("producer job", move |_ctx| {
                        ran_for_job.fetch_add(1, Ordering::AcqRel);
                    });
                }
                builder.dispatch_fence_explicitly();
                builder.extract_wait_counter()
            })
        })
        .collect::<Vec<_>>();

    let counters = producers
        .into_iter()
        .map(|producer| producer.join().unwrap())
        .collect::<Vec<_>>();
    for counter in &counters {
        assert!(jobs.flush_counter(counter));
    }

    assert_eq!(ran.load(Ordering::Acquire), PRODUCERS * JOBS_PER_PRODUCER);
    jobs.shutdown();
}

#[test]
fn shutdown_with_worker_backlog_drops_queued_work_and_joins_cleanly() {
    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    const WORKERS: usize = 4;
    const PENDING_JOBS: usize = 96;

    let mut config = test_config(WORKERS);
    config.max_latent_jobs = PENDING_JOBS + WORKERS;
    let jobs = LeetJobSystem::new(config);
    let blockers = block_workers(&jobs, WORKERS);
    let ran = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    for _ in 0..PENDING_JOBS {
        let probe = DropProbe(Arc::clone(&dropped));
        let ran_for_job = Arc::clone(&ran);
        test_support::dispatch_test_job(
            &jobs,
            "pending during shutdown",
            Priority::Latent,
            move |_ctx| {
                let _keep_probe_alive_until_drop = &probe;
                ran_for_job.fetch_add(1, Ordering::AcqRel);
            },
        );
    }

    let shutdown_jobs = jobs.clone();
    let shutdown = thread::spawn(move || shutdown_jobs.shutdown());

    wait_for_count(&dropped, PENDING_JOBS);
    assert_eq!(ran.load(Ordering::Acquire), 0);
    release_workers(blockers);
    shutdown.join().unwrap();

    assert_eq!(jobs.num_worker_threads(), 0);
    jobs.shutdown();
}
