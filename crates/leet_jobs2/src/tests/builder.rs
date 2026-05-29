use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use super::*;
use crate::{config::JobSystemConfig, dispatcher::LeetJobSystem, priority::ScheduleParam};

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

fn wait_for_counter_zero(counter: &Counter) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !counter.is_zero() {
        assert!(Instant::now() < deadline, "timed out waiting for counter");
        thread::yield_now();
    }
}

fn wait_for_count(value: &AtomicUsize, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while value.load(Ordering::Acquire) != expected {
        assert!(Instant::now() < deadline, "timed out waiting for count");
        thread::yield_now();
    }
}

fn sorted_ranges(ranges: &Mutex<Vec<(u32, u32)>>) -> Vec<(u32, u32)> {
    let mut ranges = ranges.lock().unwrap().clone();
    ranges.sort_unstable();
    ranges
}

#[test]
fn ordered_builder_dispatch_runs_in_order() {
    let jobs = LeetJobSystem::new(test_config(2));
    jobs.claim_flush_thread();
    let (first_started_tx, first_started_rx) = mpsc::channel();
    let (release_first_tx, release_first_rx) = mpsc::channel();
    let second_ran = Arc::new(AtomicUsize::new(0));
    let second_ran_for_job = Arc::clone(&second_ran);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_job("first", move |_ctx| {
        first_started_tx.send(()).unwrap();
        release_first_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
    });
    builder.dispatch_job("second", move |_ctx| {
        second_ran_for_job.fetch_add(1, Ordering::AcqRel);
    });
    let wait = builder.extract_wait_counter();

    first_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert_eq!(second_ran.load(Ordering::Acquire), 0);
    release_first_tx.send(()).unwrap();
    assert!(jobs.flush_counter(&wait));
    assert_eq!(second_ran.load(Ordering::Acquire), 1);
    jobs.shutdown();
}

#[test]
fn no_fence_dispatch_allows_parallel_jobs_before_explicit_fence() {
    let jobs = LeetJobSystem::new(test_config(2));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    for _ in 0..2 {
        let started_tx = started_tx.clone();
        let release_rx = Arc::clone(&release_rx);
        builder.dispatch_job_no_fence("parallel", move |_ctx| {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
        });
    }
    builder.dispatch_fence_explicitly();
    let wait = builder.extract_wait_counter();

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn ordered_dispatch_after_no_fence_without_explicit_fence_panics() {
    let jobs = LeetJobSystem::new(test_config(1));
    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_job_no_fence("parallel", |_ctx| {});

    let result = catch_unwind(AssertUnwindSafe(|| {
        builder.dispatch_job("ordered", |_ctx| {});
    }));

    assert!(result.is_err());
    builder.dispatch_fence_explicitly();
    let wait = builder.extract_wait_counter();
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn dispatch_wait_after_no_fence_without_explicit_fence_panics() {
    let jobs = LeetJobSystem::new(test_config(1));
    let external = jobs.create_counter(Priority::CriticalPath);
    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_job_no_fence("parallel", |_ctx| {});

    let result = catch_unwind(AssertUnwindSafe(|| {
        builder.dispatch_wait(&external);
    }));

    assert!(result.is_err());
    builder.dispatch_fence_explicitly();
    let wait = builder.extract_wait_counter();
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn dropping_builder_with_pending_no_fence_work_panics() {
    let jobs = LeetJobSystem::new(test_config(1));
    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_job_no_fence("parallel", |_ctx| {});

    let result = catch_unwind(AssertUnwindSafe(|| drop(builder)));

    assert!(result.is_err());
    jobs.shutdown();
}

#[test]
fn explicit_fence_preserves_external_wait_when_accumulator_is_empty() {
    let jobs = LeetJobSystem::new(test_config(1));
    let external = jobs.create_counter(Priority::CriticalPath);
    external.entry().increment();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_job = Arc::clone(&ran);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_wait(&external);
    builder.dispatch_fence_explicitly();
    builder.dispatch_job("after external", move |_ctx| {
        ran_for_job.fetch_add(1, Ordering::AcqRel);
    });
    let wait = builder.extract_wait_counter();

    assert_eq!(ran.load(Ordering::Acquire), 0);
    jobs.dispatcher_handle()
        .decrement_counter_entry(Arc::clone(external.entry()));
    wait_for_count(&ran, 1);
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn final_sync_discards_empty_accumulator() {
    let jobs = LeetJobSystem::new(test_config(1));
    let external = jobs.create_counter(Priority::CriticalPath);
    external.entry().increment();

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_wait(&external);
    let wait = builder.extract_wait_counter();

    assert!(!wait.is_zero());
    jobs.dispatcher_handle()
        .decrement_counter_entry(Arc::clone(external.entry()));
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn nonempty_accumulator_becomes_final_wait_counter() {
    let jobs = LeetJobSystem::new(test_config(1));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_job_no_fence("work", move |_ctx| {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    });
    builder.dispatch_fence_explicitly();
    let wait = builder.extract_wait_counter();

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(!wait.is_zero());
    release_tx.send(()).unwrap();
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn extract_wait_counter_invalidates_builder() {
    let jobs = LeetJobSystem::new(test_config(1));
    let mut builder = jobs.create_builder(Priority::CriticalPath);
    let wait = builder.extract_wait_counter();

    let result = catch_unwind(AssertUnwindSafe(|| {
        builder.dispatch_job("late", |_ctx| {});
    }));

    assert!(result.is_err());
    assert!(wait.is_zero());
    jobs.shutdown();
}

#[test]
fn continuation_builder_inherits_context_priority() {
    let jobs = LeetJobSystem::new(test_config(1));
    let continuation = jobs.create_counter(Priority::RenderPath);
    let ctx = RunContext {
        name: "ctx",
        thread_index: 0,
        parallel_for_index: -1,
        dispatcher: jobs.dispatcher_handle(),
        continuation: crate::job_decl::ContinuationContext {
            counter: Arc::clone(continuation.entry()),
            param: ScheduleParam {
                priority: Priority::Immediate,
            },
        },
    };

    let builder = Builder::from_context(jobs.dispatcher_handle(), &ctx);

    assert_eq!(builder.priority, Priority::Immediate);
    drop(builder);
    jobs.shutdown();
}

#[test]
fn continuation_builder_drop_extends_parent_job_lifetime() {
    let jobs = LeetJobSystem::new(test_config(2));
    let jobs_for_parent = jobs.clone();
    let (parent_done_tx, parent_done_rx) = mpsc::channel();
    let (child_started_tx, child_started_rx) = mpsc::channel();
    let (release_child_tx, release_child_rx) = mpsc::channel();

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_job("parent", move |ctx| {
        let mut child = jobs_for_parent.create_builder_from_context(ctx);
        child.dispatch_job("child", move |_ctx| {
            child_started_tx.send(()).unwrap();
            release_child_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
        });
        drop(child);
        parent_done_tx.send(()).unwrap();
    });
    let parent_wait = builder.extract_wait_counter();

    parent_done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    child_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert!(!parent_wait.is_zero());
    release_child_tx.send(()).unwrap();
    wait_for_counter_zero(&parent_wait);
    jobs.shutdown();
}

#[test]
fn continuation_builder_extraction_still_links_parent_continuation() {
    let jobs = LeetJobSystem::new(test_config(2));
    let jobs_for_parent = jobs.clone();
    let (extracted_tx, extracted_rx) = mpsc::channel();
    let (parent_done_tx, parent_done_rx) = mpsc::channel();
    let (child_started_tx, child_started_rx) = mpsc::channel();
    let (release_child_tx, release_child_rx) = mpsc::channel();

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_job("parent", move |ctx| {
        let mut child = jobs_for_parent.create_builder_from_context(ctx);
        child.dispatch_job("child", move |_ctx| {
            child_started_tx.send(()).unwrap();
            release_child_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
        });
        let extracted = child.extract_wait_counter();
        extracted_tx.send(extracted).unwrap();
        parent_done_tx.send(()).unwrap();
    });
    let parent_wait = builder.extract_wait_counter();

    let extracted = extracted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    parent_done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    child_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert!(!parent_wait.is_zero());
    assert!(!extracted.is_zero());
    release_child_tx.send(()).unwrap();
    wait_for_counter_zero(&parent_wait);
    wait_for_counter_zero(&extracted);
    jobs.shutdown();
}

#[test]
fn zero_element_parallel_for_without_epilogue_still_queues_empty_work() {
    let jobs = LeetJobSystem::new(test_config(1));
    let (block_started_tx, block_started_rx) = mpsc::channel();
    let (release_block_tx, release_block_rx) = mpsc::channel();

    let mut blocker = jobs.create_builder(Priority::CriticalPath);
    blocker.dispatch_job("block worker", move |_ctx| {
        block_started_tx.send(()).unwrap();
        release_block_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
    });
    let blocker_wait = blocker.extract_wait_counter();
    block_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for("empty", 0, |_start, _end, _ctx| {
        panic!("zero-element parallel-for must not run range work");
    });
    let wait = builder.extract_wait_counter();

    assert!(!wait.is_zero());
    release_block_tx.send(()).unwrap();
    wait_for_counter_zero(&blocker_wait);
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn zero_element_parallel_for_with_epilogue_runs_epilogue_once() {
    let jobs = LeetJobSystem::new(test_config(1));
    let range_calls = Arc::new(AtomicUsize::new(0));
    let epilogue_calls = Arc::new(AtomicUsize::new(0));
    let range_calls_for_job = Arc::clone(&range_calls);
    let epilogue_calls_for_job = Arc::clone(&epilogue_calls);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for_with_epilogue(
        "empty epilogue",
        0,
        move |_start, _end, _ctx| {
            range_calls_for_job.fetch_add(1, Ordering::AcqRel);
        },
        move |ctx| {
            assert_eq!(ctx.parallel_for_index, -1);
            epilogue_calls_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );
    let wait = builder.extract_wait_counter();

    wait_for_counter_zero(&wait);
    assert_eq!(range_calls.load(Ordering::Acquire), 0);
    assert_eq!(epilogue_calls.load(Ordering::Acquire), 1);
    jobs.shutdown();
}

#[test]
fn single_team_parallel_for_covers_full_range() {
    let jobs = LeetJobSystem::new(test_config(1));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_for_job = Arc::clone(&observed);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for("single team", 1, move |start, end, ctx| {
        observed_for_job
            .lock()
            .unwrap()
            .push((start, end, ctx.parallel_for_index));
    });
    let wait = builder.extract_wait_counter();

    wait_for_counter_zero(&wait);
    assert_eq!(*observed.lock().unwrap(), vec![(0, 1, 0)]);
    jobs.shutdown();
}

#[test]
fn multi_team_parallel_for_covers_every_element_once() {
    let jobs = LeetJobSystem::new(test_config(3));
    let seen = Arc::new((0..17).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>());
    let seen_for_job = Arc::clone(&seen);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for("multi team", 17, move |start, end, _ctx| {
        assert!(start < end);
        assert!(end <= 17);
        for index in start..end {
            seen_for_job[index as usize].fetch_add(1, Ordering::AcqRel);
        }
    });
    let wait = builder.extract_wait_counter();

    wait_for_counter_zero(&wait);
    for count in seen.iter() {
        assert_eq!(count.load(Ordering::Acquire), 1);
    }
    jobs.shutdown();
}

#[test]
fn parallel_for_index_is_team_index_not_batch_index() {
    let jobs = LeetJobSystem::new(test_config(1));
    let observed_indices = Arc::new(Mutex::new(Vec::new()));
    let observed_for_job = Arc::clone(&observed_indices);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for_with_max_batch_size(
        "many batches",
        8,
        1,
        move |_start, _end, ctx| {
            observed_for_job
                .lock()
                .unwrap()
                .push(ctx.parallel_for_index);
        },
    );
    let wait = builder.extract_wait_counter();

    wait_for_counter_zero(&wait);
    assert!(!observed_indices.lock().unwrap().is_empty());
    assert!(
        observed_indices
            .lock()
            .unwrap()
            .iter()
            .all(|index| (0..2).contains(index)),
        "team indices must stay below team size even when there are more batches"
    );
    jobs.shutdown();
}

#[test]
fn epilogue_runs_after_chunks_and_before_outer_counter_reaches_zero() {
    let jobs = LeetJobSystem::new(test_config(2));
    let processed = Arc::new(AtomicUsize::new(0));
    let processed_for_job = Arc::clone(&processed);
    let processed_for_epilogue = Arc::clone(&processed);
    let (epilogue_started_tx, epilogue_started_rx) = mpsc::channel();
    let (release_epilogue_tx, release_epilogue_rx) = mpsc::channel();

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for_with_epilogue(
        "epilogue",
        11,
        move |start, end, _ctx| {
            processed_for_job.fetch_add((end - start) as usize, Ordering::AcqRel);
        },
        move |ctx| {
            assert_eq!(ctx.parallel_for_index, -1);
            assert_eq!(processed_for_epilogue.load(Ordering::Acquire), 11);
            epilogue_started_tx.send(()).unwrap();
            release_epilogue_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
        },
    );
    let wait = builder.extract_wait_counter();

    epilogue_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert!(!wait.is_zero());
    release_epilogue_tx.send(()).unwrap();
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn automatic_parallel_for_batching_uses_team_size() {
    let jobs = LeetJobSystem::new(test_config(3));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let ranges_for_job = Arc::clone(&ranges);
    let count = 10;
    let team_size = count.min(jobs.num_worker_threads() as u32 + 1);
    let batch_size = count.div_ceil(team_size);
    let expected = (0..team_size)
        .map(|batch| {
            let start = batch * batch_size;
            let end = (start + batch_size).min(count);
            (start, end)
        })
        .filter(|(start, end)| start < end)
        .collect::<Vec<_>>();

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for("auto batch", count, move |start, end, _ctx| {
        ranges_for_job.lock().unwrap().push((start, end));
    });
    let wait = builder.extract_wait_counter();

    wait_for_counter_zero(&wait);
    assert_eq!(sorted_ranges(&ranges), expected);
    jobs.shutdown();
}

#[test]
fn nonzero_max_batch_size_uses_documented_batch_formula() {
    let jobs = LeetJobSystem::new(test_config(4));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let ranges_for_job = Arc::clone(&ranges);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for_with_max_batch_size(
        "explicit batch",
        10,
        3,
        move |start, end, _ctx| {
            ranges_for_job.lock().unwrap().push((start, end));
        },
    );
    let wait = builder.extract_wait_counter();

    wait_for_counter_zero(&wait);
    assert_eq!(sorted_ranges(&ranges), vec![(0, 4), (4, 8), (8, 10)]);
    jobs.shutdown();
}

#[test]
fn no_fence_parallel_for_participates_in_builder_fence_rules() {
    let jobs = LeetJobSystem::new(test_config(1));
    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for_no_fence("parallel", 1, |_start, _end, _ctx| {});

    let result = catch_unwind(AssertUnwindSafe(|| {
        builder.dispatch_job("ordered", |_ctx| {});
    }));

    assert!(result.is_err());
    builder.dispatch_fence_explicitly();
    let wait = builder.extract_wait_counter();
    wait_for_counter_zero(&wait);
    jobs.shutdown();
}

#[test]
fn no_fence_parallel_for_with_epilogue_runs_after_explicit_fence() {
    let jobs = LeetJobSystem::new(test_config(2));
    let range_count = Arc::new(AtomicUsize::new(0));
    let epilogue_count = Arc::new(AtomicUsize::new(0));
    let range_count_for_job = Arc::clone(&range_count);
    let range_count_for_epilogue = Arc::clone(&range_count);
    let epilogue_count_for_job = Arc::clone(&epilogue_count);

    let mut builder = jobs.create_builder(Priority::CriticalPath);
    builder.dispatch_parallel_for_with_epilogue_no_fence(
        "parallel epilogue no fence",
        7,
        move |start, end, _ctx| {
            range_count_for_job.fetch_add((end - start) as usize, Ordering::AcqRel);
        },
        move |ctx| {
            assert_eq!(ctx.parallel_for_index, -1);
            assert_eq!(range_count_for_epilogue.load(Ordering::Acquire), 7);
            epilogue_count_for_job.fetch_add(1, Ordering::AcqRel);
        },
    );
    builder.dispatch_fence_explicitly();
    let wait = builder.extract_wait_counter();

    wait_for_counter_zero(&wait);
    assert_eq!(range_count.load(Ordering::Acquire), 7);
    assert_eq!(epilogue_count.load(Ordering::Acquire), 1);
    jobs.shutdown();
}
