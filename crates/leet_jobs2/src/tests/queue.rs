use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use super::*;
use crate::{counter::CounterEntry, job_decl::JobHint};

fn test_queues(capacity_per_lane: usize) -> ReadyQueues {
    ReadyQueues::with_capacities(ReadyQueueCapacities::new(
        capacity_per_lane,
        capacity_per_lane,
        capacity_per_lane,
        capacity_per_lane,
    ))
}

fn entry(name: &'static str) -> JobQueueEntry {
    JobQueueEntry::new(
        JobDecl::new(name, JobHint::None, |_ctx| {}),
        CounterEntry::new(Priority::CriticalPath, "accum"),
    )
}

fn try_push(
    queues: &ReadyQueues,
    entry: JobQueueEntry,
    priority: Priority,
) -> Result<(), QueueFull<JobQueueEntry>> {
    match queues.try_push_if_open(entry, priority) {
        Ok(()) => Ok(()),
        Err(QueuePushError::Full(full)) => Err(full),
        Err(QueuePushError::Shutdown(_entry)) => {
            panic!("cannot push job into a shut down ready queue")
        }
    }
}

fn pop_name(queues: &ReadyQueues) -> (&'static str, Priority) {
    let (entry, priority) = queues.try_pop().expect("expected queued job");
    (entry.job_name(), priority)
}

#[test]
fn capacity_mapping_uses_configured_lane_sizes() {
    let config = JobSystemConfig {
        max_latent_jobs: 2,
        max_critical_path_jobs: 3,
        max_immediate_jobs: 4,
        ..JobSystemConfig::default()
    };
    let queues = ReadyQueues::new(&config);

    assert_eq!(queues.capacities, ReadyQueueCapacities::new(2, 3, 3, 4));
}

#[test]
fn all_critical_path_capacity_mapping_preserves_latent_headroom() {
    let config = JobSystemConfig {
        max_latent_jobs: 1,
        max_critical_path_jobs: 5,
        max_immediate_jobs: 2,
        all_jobs_critical_path: true,
        ..JobSystemConfig::default()
    };
    let queues = ReadyQueues::new(&config);

    assert_eq!(queues.capacities, ReadyQueueCapacities::new(5, 5, 5, 2));
}

#[test]
fn try_pop_returns_none_when_empty() {
    let queues = test_queues(1);

    assert!(queues.try_pop().is_none());
}

#[test]
fn fifo_order_is_preserved_inside_each_lane() {
    let queues = test_queues(4);

    try_push(&queues, entry("first"), Priority::CriticalPath).unwrap();
    try_push(&queues, entry("second"), Priority::CriticalPath).unwrap();

    assert_eq!(pop_name(&queues), ("first", Priority::CriticalPath));
    assert_eq!(pop_name(&queues), ("second", Priority::CriticalPath));
    assert!(queues.try_pop().is_none());
}

#[test]
fn pop_uses_strict_priority_before_fifo_order() {
    let queues = test_queues(4);

    try_push(&queues, entry("latent"), Priority::Latent).unwrap();
    try_push(&queues, entry("render"), Priority::RenderPath).unwrap();
    try_push(&queues, entry("critical"), Priority::CriticalPath).unwrap();
    try_push(&queues, entry("immediate"), Priority::Immediate).unwrap();

    assert_eq!(pop_name(&queues), ("immediate", Priority::Immediate));
    assert_eq!(pop_name(&queues), ("critical", Priority::CriticalPath));
    assert_eq!(pop_name(&queues), ("render", Priority::RenderPath));
    assert_eq!(pop_name(&queues), ("latent", Priority::Latent));
}

#[test]
fn queue_full_returns_the_rejected_entry() {
    let queues = test_queues(1);

    try_push(&queues, entry("kept"), Priority::RenderPath).unwrap();
    let full = try_push(&queues, entry("rejected"), Priority::RenderPath)
        .expect_err("second job should exceed lane capacity");

    assert_eq!(full.priority(), Priority::RenderPath);
    assert_eq!(full.capacity(), 1);
    assert_eq!(full.entry.job_name(), "rejected");
    assert_eq!(full.into_entry().job_name(), "rejected");
    assert_eq!(test_support::lane_len(&queues, Priority::RenderPath), 1);
    assert_eq!(pop_name(&queues), ("kept", Priority::RenderPath));
}

#[test]
fn job_queue_entry_preserves_job_and_counter_ownership() {
    let accum_counter = CounterEntry::new(Priority::Immediate, "owned accum");
    let entry = JobQueueEntry::new(
        JobDecl::new("owned job", JobHint::Large, |_ctx| {}),
        Arc::clone(&accum_counter),
    );

    assert_eq!(entry.job_name(), "owned job");
    assert_eq!(entry.job_hint(), JobHint::Large);
    assert!(Arc::ptr_eq(entry.accum_counter(), &accum_counter));

    let (job, returned_counter) = entry.into_parts();
    assert_eq!(job.name(), "owned job");
    assert!(Arc::ptr_eq(&returned_counter, &accum_counter));
}

#[test]
fn pop_blocking_wakes_when_a_job_is_pushed() {
    let queues = Arc::new(test_queues(1));
    let worker_queues = Arc::clone(&queues);
    let (started_tx, started_rx) = mpsc::channel();
    let (popped_tx, popped_rx) = mpsc::channel();

    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let (entry, priority) = worker_queues
            .pop_blocking()
            .expect("push should wake blocked pop");
        popped_tx.send((entry.job_name(), priority)).unwrap();
    });

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    try_push(&queues, entry("wake"), Priority::Immediate).unwrap();

    assert_eq!(
        popped_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ("wake", Priority::Immediate)
    );
    worker.join().unwrap();
}

#[test]
fn shutdown_wakes_blocked_poppers() {
    let queues = Arc::new(test_queues(1));
    let worker_queues = Arc::clone(&queues);
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();

    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        done_tx
            .send(worker_queues.pop_blocking().is_none())
            .unwrap();
    });

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    queues.shutdown();

    assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    worker.join().unwrap();
}

#[test]
fn shutdown_drops_pending_jobs_and_makes_try_pop_empty() {
    let queues = test_queues(2);
    try_push(&queues, entry("pending"), Priority::Immediate).unwrap();
    assert_eq!(test_support::total_len(&queues), 1);

    queues.shutdown();

    assert_eq!(test_support::total_len(&queues), 0);
    assert!(queues.try_pop().is_none());
}

#[test]
fn shutdown_is_idempotent() {
    let queues = test_queues(1);

    queues.shutdown();
    queues.shutdown();

    assert!(queues.try_pop().is_none());
}

#[test]
#[should_panic(expected = "cannot push job into a shut down ready queue")]
fn push_after_shutdown_panics() {
    let queues = test_queues(1);

    queues.shutdown();
    let _ = try_push(&queues, entry("late"), Priority::Immediate);
}

#[test]
fn internal_push_after_shutdown_returns_the_entry() {
    let queues = test_queues(1);

    queues.shutdown();
    let err = queues
        .try_push_if_open(entry("late"), Priority::Immediate)
        .expect_err("internal push should report shutdown without dropping entry");

    match err {
        QueuePushError::Shutdown(entry) => assert_eq!(entry.job_name(), "late"),
        QueuePushError::Full(_) => panic!("shutdown should win over capacity checks"),
    }
}

#[test]
fn pop_blocking_returns_queued_work_before_shutdown() {
    let queues = test_queues(1);
    try_push(&queues, entry("ready"), Priority::Latent).unwrap();

    let (entry, priority) = queues.pop_blocking().unwrap();

    assert_eq!(entry.job_name(), "ready");
    assert_eq!(priority, Priority::Latent);
}

#[test]
fn pop_blocking_does_not_wake_without_push_or_shutdown() {
    let queues = Arc::new(test_queues(1));
    let worker_queues = Arc::clone(&queues);
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();

    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        done_tx
            .send(
                worker_queues
                    .pop_blocking()
                    .map(|(entry, _)| entry.job_name()),
            )
            .unwrap();
    });

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        done_rx.recv_timeout(Duration::from_millis(25)),
        Err(RecvTimeoutError::Timeout)
    );

    queues.shutdown();
    assert_eq!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap(), None);
    worker.join().unwrap();
}

#[test]
fn timed_wait_wakes_when_ready_job_is_pushed() {
    let queues = Arc::new(test_queues(1));
    let waiter_queues = Arc::clone(&queues);
    let (started_tx, started_rx) = mpsc::channel();
    let (elapsed_tx, elapsed_rx) = mpsc::channel();

    let waiter = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let started = Instant::now();
        waiter_queues.wait_for_ready_job_timeout(Duration::from_secs(1), || true);
        elapsed_tx.send(started.elapsed()).unwrap();
    });

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    try_push(&queues, entry("wake flush"), Priority::Immediate).unwrap();

    assert!(
        elapsed_rx.recv_timeout(Duration::from_secs(1)).unwrap() < Duration::from_millis(250),
        "ready-queue timed wait did not wake promptly after push"
    );
    waiter.join().unwrap();
    assert_eq!(pop_name(&queues), ("wake flush", Priority::Immediate));
}

#[test]
fn timed_wait_does_not_consume_existing_ready_work() {
    let queues = test_queues(1);
    try_push(&queues, entry("already ready"), Priority::CriticalPath).unwrap();

    let started = Instant::now();
    queues.wait_for_ready_job_timeout(Duration::from_secs(1), || true);

    assert!(started.elapsed() < Duration::from_millis(250));
    assert_eq!(pop_name(&queues), ("already ready", Priority::CriticalPath));
}

#[test]
fn queue_wakeup_wait_wakes_on_later_push_without_consuming_ready_work() {
    let queues = Arc::new(test_queues(2));
    try_push(&queues, entry("already ready"), Priority::CriticalPath).unwrap();

    let waiter_queues = Arc::clone(&queues);
    let (started_tx, started_rx) = mpsc::channel();
    let (elapsed_tx, elapsed_rx) = mpsc::channel();

    let waiter = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let started = Instant::now();
        waiter_queues.wait_for_queue_wakeup_timeout(Duration::from_secs(1), || true);
        elapsed_tx.send(started.elapsed()).unwrap();
    });

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    thread::sleep(Duration::from_millis(25));
    try_push(&queues, entry("wake waiter"), Priority::Immediate).unwrap();

    let elapsed = elapsed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        elapsed < Duration::from_millis(250),
        "queue wakeup wait did not wake promptly after a later push"
    );
    waiter.join().unwrap();
    assert_eq!(pop_name(&queues), ("wake waiter", Priority::Immediate));
    assert_eq!(pop_name(&queues), ("already ready", Priority::CriticalPath));
}

#[test]
fn timed_wait_returns_immediately_when_predicate_is_false() {
    let queues = test_queues(1);

    let started = Instant::now();
    queues.wait_for_ready_job_timeout(Duration::from_secs(1), || false);

    assert!(started.elapsed() < Duration::from_millis(250));
}

#[test]
fn explicit_progress_notification_wakes_timed_waiters() {
    let queues = Arc::new(test_queues(1));
    let waiter_queues = Arc::clone(&queues);
    let (started_tx, started_rx) = mpsc::channel();
    let (elapsed_tx, elapsed_rx) = mpsc::channel();

    let waiter = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let started = Instant::now();
        waiter_queues.wait_for_ready_job_timeout(Duration::from_secs(1), || true);
        elapsed_tx.send(started.elapsed()).unwrap();
    });

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    thread::sleep(Duration::from_millis(25));
    queues.notify_all_waiters();

    assert!(
        elapsed_rx.recv_timeout(Duration::from_secs(1)).unwrap() < Duration::from_millis(250),
        "explicit progress notification did not wake timed waiter promptly"
    );
    waiter.join().unwrap();
}

#[test]
fn shutdown_drops_pending_job_payloads_without_running_them() {
    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    let queues = test_queues(1);
    let dropped = Arc::new(AtomicUsize::new(0));
    let ran = Arc::new(AtomicUsize::new(0));
    let probe = DropProbe(Arc::clone(&dropped));
    let ran_for_job = Arc::clone(&ran);

    queues
        .try_push_if_open(
            JobQueueEntry::new(
                JobDecl::new("pending", JobHint::None, move |_ctx| {
                    let _keep_probe_alive_until_drop = &probe;
                    ran_for_job.fetch_add(1, Ordering::Relaxed);
                }),
                CounterEntry::new(Priority::Immediate, "accum"),
            ),
            Priority::Immediate,
        )
        .unwrap();
    assert_eq!(dropped.load(Ordering::Relaxed), 0);

    queues.shutdown();

    assert_eq!(ran.load(Ordering::Relaxed), 0);
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
}

#[test]
fn multiple_producers_and_poppers_do_not_lose_entries() {
    const PRODUCERS: usize = 4;
    const JOBS_PER_PRODUCER: usize = 16;
    const TOTAL_JOBS: usize = PRODUCERS * JOBS_PER_PRODUCER;
    const POPPERS: usize = 4;

    let queues = Arc::new(ReadyQueues::with_capacities(ReadyQueueCapacities::new(
        1, 1, 1, TOTAL_JOBS,
    )));
    let (popped_tx, popped_rx) = mpsc::channel();

    let mut poppers = Vec::new();
    for _ in 0..POPPERS {
        let queues = Arc::clone(&queues);
        let popped_tx = popped_tx.clone();
        poppers.push(thread::spawn(move || {
            while let Some((entry, priority)) = queues.pop_blocking() {
                assert_eq!(priority, Priority::Immediate);
                popped_tx.send(entry.job_name()).unwrap();
            }
        }));
    }
    drop(popped_tx);

    let mut producers = Vec::new();
    for _ in 0..PRODUCERS {
        let queues = Arc::clone(&queues);
        producers.push(thread::spawn(move || {
            for _ in 0..JOBS_PER_PRODUCER {
                try_push(&queues, entry("mpmc"), Priority::Immediate).unwrap();
            }
        }));
    }

    for producer in producers {
        producer.join().unwrap();
    }

    for _ in 0..TOTAL_JOBS {
        assert_eq!(
            popped_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "mpmc"
        );
    }
    assert_eq!(
        popped_rx.recv_timeout(Duration::from_millis(25)),
        Err(RecvTimeoutError::Timeout)
    );

    queues.shutdown();
    for popper in poppers {
        popper.join().unwrap();
    }
}

#[test]
#[should_panic(expected = "immediate ready-queue capacity must be nonzero")]
fn zero_capacity_lanes_are_rejected() {
    let _ = ReadyQueueCapacities::new(1, 1, 1, 0);
}
