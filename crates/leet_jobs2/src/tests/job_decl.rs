use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

use super::*;

#[test]
fn job_decl_stores_and_runs_mixed_closure_types() {
    let ctx = test_support::run_context("ctx");
    let count = Arc::new(AtomicU32::new(0));
    let first_count = Arc::clone(&count);
    let second_count = Arc::clone(&count);

    let first = JobDecl::new("first", JobHint::None, move |ctx| {
        assert_eq!(ctx.name, "ctx");
        first_count.fetch_add(1, Ordering::Relaxed);
    });

    let captured_value = 41;
    let second = JobDecl::new("second", JobHint::Trivial, move |_ctx| {
        second_count.fetch_add(captured_value + 1, Ordering::Relaxed);
    });

    assert_eq!(first.name(), "first");
    assert_eq!(first.hint(), JobHint::None);
    assert_eq!(second.name(), "second");
    assert_eq!(second.hint(), JobHint::Trivial);

    first.run(&ctx);
    second.run(&ctx);

    assert_eq!(count.load(Ordering::Relaxed), 43);
}

#[test]
fn take_once_epilogue_runs_at_most_once() {
    let ctx = test_support::run_context("epilogue");
    let ran = Arc::new(AtomicU32::new(0));
    let ran_for_epilogue = Arc::clone(&ran);
    let epilogue = TakeOnceEpilogue::new(move |_ctx| {
        ran_for_epilogue.fetch_add(1, Ordering::Relaxed);
    });

    assert!(epilogue.run_once(&ctx));
    assert!(!epilogue.run_once(&ctx));
    assert_eq!(ran.load(Ordering::Relaxed), 1);
}

#[test]
fn parallel_for_job_stores_range_func_and_epilogue() {
    let ctx = test_support::run_context("parallel");
    let observed_start = Arc::new(AtomicU32::new(0));
    let observed_end = Arc::new(AtomicU32::new(0));
    let epilogue_ran = Arc::new(AtomicBool::new(false));

    let start_for_job = Arc::clone(&observed_start);
    let end_for_job = Arc::clone(&observed_end);
    let epilogue_for_job = Arc::clone(&epilogue_ran);

    let job = ParallelForJob::with_epilogue(
        "range",
        JobHint::Large,
        8,
        3,
        move |start, end, ctx| {
            assert_eq!(ctx.name, "parallel");
            start_for_job.store(start, Ordering::Relaxed);
            end_for_job.store(end, Ordering::Relaxed);
        },
        move |_ctx| {
            epilogue_for_job.store(true, Ordering::Relaxed);
        },
    );

    assert_eq!(job.name(), "range");
    assert_eq!(job.hint(), JobHint::Large);
    assert_eq!(job.num_elements(), 8);
    assert_eq!(job.max_batch_size(), 3);
    assert!(job.has_epilogue());

    job.run_range(2, 5, &ctx);
    assert_eq!(observed_start.load(Ordering::Relaxed), 2);
    assert_eq!(observed_end.load(Ordering::Relaxed), 5);

    assert!(job.run_epilogue_once(&ctx));
    assert!(!job.run_epilogue_once(&ctx));
    assert!(epilogue_ran.load(Ordering::Relaxed));
}

#[test]
fn parallel_for_job_can_omit_epilogue() {
    let ctx = test_support::run_context("parallel");
    let ran = Arc::new(AtomicBool::new(false));
    let ran_for_job = Arc::clone(&ran);
    let job = ParallelForJob::new("range", JobHint::None, 1, 0, move |_start, _end, _ctx| {
        ran_for_job.store(true, Ordering::Relaxed);
    });

    assert!(!job.has_epilogue());
    job.run_range(0, 1, &ctx);
    assert!(ran.load(Ordering::Relaxed));
    assert!(!job.run_epilogue_once(&ctx));
}
