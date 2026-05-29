use std::{
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

use super::*;
use crate::{
    dispatcher::test_support as dispatcher_test_support, job_decl::JobHint, priority::Priority,
};

fn empty_job(name: &'static str) -> JobDecl {
    JobDecl::new(name, JobHint::None, |_ctx| {})
}

fn waiting_job(name: &'static str, accum_counter: Arc<CounterEntry>) -> WaitingJob {
    WaitingJob::new(empty_job(name), accum_counter)
}

#[test]
fn new_counter_entry_starts_at_zero_with_priority_and_name() {
    let counter = CounterEntry::new(Priority::RenderPath, "render work");

    assert!(counter.is_zero());
    assert_eq!(counter.priority(), Priority::RenderPath);
    assert_eq!(counter.name(), "render work");
    assert_eq!(test_support::waiting_len(&counter), 0);
}

#[test]
fn increment_reports_old_zero_and_decrement_reports_new_zero() {
    let counter = CounterEntry::new(Priority::CriticalPath, "counter");

    assert!(counter.increment());
    assert!(!counter.is_zero());
    assert!(!counter.increment());
    assert!(!counter.decrement());
    assert!(!counter.is_zero());
    assert!(counter.decrement());
    assert!(counter.is_zero());
}

#[test]
#[should_panic(expected = "counter value underflow")]
fn decrement_from_zero_panics() {
    let counter = CounterEntry::new(Priority::CriticalPath, "counter");

    counter.decrement();
}

#[test]
fn try_add_to_waiting_returns_job_when_counter_is_zero() {
    let wait_counter = CounterEntry::new(Priority::CriticalPath, "wait");
    let accum_counter = CounterEntry::new(Priority::CriticalPath, "accum");
    let job = waiting_job("parked", Arc::clone(&accum_counter));

    let returned = wait_counter
        .try_add_to_waiting(job)
        .expect_err("zero counter must not accept waiting jobs");

    assert_eq!(returned.job.name(), "parked");
    assert!(Arc::ptr_eq(&returned.accum_counter, &accum_counter));
    assert_eq!(test_support::waiting_len(&wait_counter), 0);
}

#[test]
fn try_add_to_waiting_parks_job_when_counter_is_nonzero() {
    let wait_counter = CounterEntry::new(Priority::CriticalPath, "wait");
    let accum_counter = CounterEntry::new(Priority::CriticalPath, "accum");
    wait_counter.increment();

    assert!(wait_counter
        .try_add_to_waiting(waiting_job("parked", Arc::clone(&accum_counter)))
        .is_ok());

    assert_eq!(test_support::waiting_len(&wait_counter), 1);
}

#[test]
fn flush_waiting_only_drains_when_value_is_still_zero_under_lock() {
    let wait_counter = CounterEntry::new(Priority::CriticalPath, "wait");
    let accum_counter = CounterEntry::new(Priority::CriticalPath, "accum");
    wait_counter.increment();
    assert!(wait_counter
        .try_add_to_waiting(waiting_job("parked", Arc::clone(&accum_counter)))
        .is_ok());

    assert!(wait_counter.flush_waiting().is_empty());
    assert_eq!(test_support::waiting_len(&wait_counter), 1);

    assert!(wait_counter.decrement());
    assert!(wait_counter.increment());
    assert!(wait_counter.flush_waiting().is_empty());
    assert_eq!(test_support::waiting_len(&wait_counter), 1);

    assert!(wait_counter.decrement());
    let released = wait_counter.flush_waiting();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].job.name(), "parked");
    assert!(wait_counter.flush_waiting().is_empty());
}

#[test]
fn concurrent_zero_observer_does_not_flush_reused_counter_waiters() {
    let wait_counter = CounterEntry::new(Priority::CriticalPath, "wait");
    let accum_counter = CounterEntry::new(Priority::CriticalPath, "accum");
    wait_counter.increment();

    let (decremented_tx, decremented_rx) = mpsc::channel();
    let (flush_tx, flush_rx) = mpsc::channel();
    let wait_counter_for_flush = Arc::clone(&wait_counter);
    let flush_thread = thread::spawn(move || {
        assert!(wait_counter_for_flush.decrement());
        decremented_tx.send(()).unwrap();
        flush_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        wait_counter_for_flush.flush_waiting()
    });

    decremented_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(wait_counter.increment());
    assert!(wait_counter
        .try_add_to_waiting(waiting_job("reused", Arc::clone(&accum_counter)))
        .is_ok());
    flush_tx.send(()).unwrap();

    let released_by_old_zero = flush_thread.join().unwrap();
    assert!(released_by_old_zero.is_empty());
    assert_eq!(test_support::waiting_len(&wait_counter), 1);

    assert!(wait_counter.decrement());
    let released_after_real_zero = wait_counter.flush_waiting();
    assert_eq!(released_after_real_zero.len(), 1);
    assert_eq!(released_after_real_zero[0].job.name(), "reused");
}

#[test]
fn waiting_job_can_be_split_back_into_dispatch_parts() {
    let accum_counter = CounterEntry::new(Priority::CriticalPath, "accum");
    let waiting_job = waiting_job("parked", Arc::clone(&accum_counter));

    let (job, returned_accum_counter) = waiting_job.into_parts();

    assert_eq!(job.name(), "parked");
    assert!(Arc::ptr_eq(&returned_accum_counter, &accum_counter));
}

#[test]
fn counter_handle_owns_one_arc_reference_and_exposes_zero_snapshot() {
    let dispatcher = dispatcher_test_support::dispatcher_handle();
    let entry = CounterEntry::new(Priority::CriticalPath, "counter");
    assert_eq!(Arc::strong_count(&entry), 1);

    let counter = Counter::from_entry(dispatcher, Arc::clone(&entry));

    assert_eq!(Arc::strong_count(&entry), 2);
    assert!(counter.is_zero());
    entry.increment();
    assert!(!counter.is_zero());

    drop(counter);
    assert_eq!(Arc::strong_count(&entry), 1);
}

#[test]
fn counter_reset_moves_the_replacement_handle() {
    let first = CounterEntry::new(Priority::Latent, "first");
    let second = CounterEntry::new(Priority::Immediate, "second");
    let mut counter = Counter::from_entry(
        dispatcher_test_support::dispatcher_handle(),
        Arc::clone(&first),
    );
    let replacement = Counter::from_entry(
        dispatcher_test_support::dispatcher_handle(),
        Arc::clone(&second),
    );

    counter.reset(replacement);

    assert!(Arc::ptr_eq(counter.entry(), &second));
    assert!(!Arc::ptr_eq(counter.entry(), &first));
}
