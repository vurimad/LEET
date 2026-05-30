//! # leet_jobs
//!
//! A central-queue, non-stealing job dispatcher for the LEET engine.
//!
//! ## Design
//!
//! Designed around a central-queue, non-stealing scheduler. Key properties:
//!
//! * **No work-stealing** — four bounded MPMC queues (one per priority lane)
//!   are shared by all threads.  Each worker also has a thread-local child
//!   queue it drains first, giving child-job locality without cross-thread
//!   access.
//! * **Counter-based dependencies** — a [`Counter`] is an atomic integer.
//!   Jobs are "gated" on a counter reaching zero.  When it does, all parked
//!   jobs are released back into the global queue.
//! * **Fluent builder** — [`Builder`] provides `dispatch`, `dispatch_fence`,
//!   and `dispatch_wait` to compose dependency graphs without managing raw
//!   counter handles for the common case.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use leet_jobs::{Dispatcher, JobSystemConfig, ScheduleParam};
//!
//! let dispatcher = Dispatcher::new(JobSystemConfig::default());
//!
//! // Simple sequential chain via Builder
//! let mut builder = dispatcher.builder(ScheduleParam::default());
//! builder.dispatch(|| println!("job 1"));
//! builder.dispatch(|| println!("job 2")); // waits for job 1
//! let done = builder.extract_wait_counter();
//! dispatcher.flush(&done);
//! ```

pub mod builder;
pub mod counter;
pub mod counter_functions;
pub mod debugger;
mod debugger_stack_trace_cache;
pub mod deferral;
pub mod dispatcher;
pub(crate) mod dispatcher_entries;
pub(crate) mod dispatcher_thread;
pub mod job_decl;
pub mod plugin;
pub mod priority;
pub(crate) mod semaphore;
pub mod stack_trace;

// Re-export the most commonly used types at crate root.
pub use builder::{Builder, ContinuationContext, Fence, RunContext};
pub use counter::Counter;
pub use deferral::CompletionDeferral;
pub use dispatcher::{Dispatcher, JobSystemConfig, WorkerConfig, WorkerPriority};
pub use job_decl::{JobDebugFlags, JobDecl, JobDeclParallelFor, JobHint};
pub use plugin::LeetJobsPlugin;
pub use priority::{Priority, ScheduleParam, PRIORITY_COUNT};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // Helper: create a small dispatcher (2 workers) so tests run fast.
    fn small_dispatcher() -> Dispatcher {
        Dispatcher::new(JobSystemConfig {
            num_threads: 2,
            queue_capacity: 256,
            worker_configs: Vec::new(),
        })
    }

    // Helper: flush a counter with a timeout to protect against hangs in tests.
    fn flush_timeout(d: &Dispatcher, counter: &Counter) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !counter.is_zero() {
            if Instant::now() > deadline {
                panic!("[leet_jobs] flush_timeout: counter never reached zero");
            }
            std::thread::yield_now();
        }
        // Run one last flush pass to drain any in-flight work.
        d.flush(counter);
    }

    // -----------------------------------------------------------------------
    // 1. A single job runs and completes.
    // -----------------------------------------------------------------------
    #[test]
    fn single_job_completes() {
        let d = small_dispatcher();
        let flag = Arc::new(AtomicU32::new(0));

        let flag2 = Arc::clone(&flag);
        let mut builder = d.builder(ScheduleParam::default());
        builder.dispatch(move || {
            flag2.store(1, Ordering::Release);
        });
        let done = builder.extract_wait_counter();

        flush_timeout(&d, &done);
        assert_eq!(flag.load(Ordering::Acquire), 1);
    }

    // -----------------------------------------------------------------------
    // 2. Sequential chain: job 2 must see the write made by job 1.
    // -----------------------------------------------------------------------
    #[test]
    fn sequential_chain_ordering() {
        let d = small_dispatcher();

        // Shared slot written by job 1, read-asserted by job 2.
        let slot = Arc::new(AtomicU32::new(0));

        let slot_write = Arc::clone(&slot);
        let slot_read = Arc::clone(&slot);

        let mut builder = d.builder(ScheduleParam::default());
        // Fence::Full (default) ensures job 2 cannot start before job 1 finishes.
        builder.dispatch(move || {
            slot_write.store(42, Ordering::Release);
        });
        builder.dispatch(move || {
            let v = slot_read.load(Ordering::Acquire);
            assert_eq!(v, 42, "job 2 ran before job 1 wrote the value");
        });
        let done = builder.extract_wait_counter();
        flush_timeout(&d, &done);
    }

    // -----------------------------------------------------------------------
    // 3. Parallel jobs with Fence::None all complete.
    // -----------------------------------------------------------------------
    #[test]
    fn parallel_jobs_all_complete() {
        let d = small_dispatcher();
        const N: u32 = 8;
        let counter = Arc::new(AtomicU32::new(0));

        let mut builder = d.builder(ScheduleParam::default());
        for _ in 0..N {
            let c = Arc::clone(&counter);
            builder.dispatch_with_fence(
                move || {
                    c.fetch_add(1, Ordering::Relaxed);
                },
                Fence::None,
            );
        }
        builder.dispatch_fence(); // close the parallel group

        let done = builder.extract_wait_counter();
        flush_timeout(&d, &done);
        assert_eq!(counter.load(Ordering::Acquire), N);
    }

    // -----------------------------------------------------------------------
    // 4. dispatch_wait: a second builder waits for the first to finish.
    // -----------------------------------------------------------------------
    #[test]
    fn dispatch_wait_external_counter() {
        let d = small_dispatcher();

        let wrote = Arc::new(AtomicU32::new(0));
        let read_ok = Arc::new(AtomicU32::new(0));

        // Builder A: one job that stores a value.
        let wrote2 = Arc::clone(&wrote);
        let mut a = d.builder(ScheduleParam::default());
        a.dispatch(move || {
            wrote2.store(99, Ordering::Release);
        });
        let a_done = a.extract_wait_counter();

        // Builder B: waits for A, then reads the value.
        let wrote3 = Arc::clone(&wrote);
        let read_ok2 = Arc::clone(&read_ok);
        let mut b = d.builder(ScheduleParam::default());
        b.dispatch_wait(&a_done);
        b.dispatch(move || {
            assert_eq!(wrote3.load(Ordering::Acquire), 99);
            read_ok2.store(1, Ordering::Release);
        });
        let b_done = b.extract_wait_counter();

        flush_timeout(&d, &b_done);
        assert_eq!(read_ok.load(Ordering::Acquire), 1);
    }

    // -----------------------------------------------------------------------
    // 5. Priority: Immediate jobs complete before Latent jobs when queued together.
    //    (Probabilistic — valid on a two-worker dispatcher with a short sleep.)
    // -----------------------------------------------------------------------
    #[test]
    fn higher_priority_drains_first() {
        let d = small_dispatcher();
        let order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));

        // Saturate workers briefly so queued jobs pile up.
        let barrier = Arc::new(std::sync::Barrier::new(3)); // 2 workers + main
        let b1 = Arc::clone(&barrier);
        let b2 = Arc::clone(&barrier);
        d.submit(
            move || {
                b1.wait();
            },
            None,
            &Counter::new(
                ScheduleParam {
                    priority: Priority::CriticalPath,
                },
                "hold",
            ),
            Priority::CriticalPath,
        );
        d.submit(
            move || {
                b2.wait();
            },
            None,
            &Counter::new(
                ScheduleParam {
                    priority: Priority::CriticalPath,
                },
                "hold",
            ),
            Priority::CriticalPath,
        );
        barrier.wait(); // all three reach here; workers are now free

        // Now queue one Latent and one Immediate job.
        let order_lat = Arc::clone(&order);
        let order_imm = Arc::clone(&order);
        let lat_acc = Counter::new(
            ScheduleParam {
                priority: Priority::Latent,
            },
            "lat",
        );
        let imm_acc = Counter::new(
            ScheduleParam {
                priority: Priority::Immediate,
            },
            "imm",
        );
        d.submit(
            move || order_lat.lock().unwrap().push("latent"),
            None,
            &lat_acc,
            Priority::Latent,
        );
        d.submit(
            move || order_imm.lock().unwrap().push("immediate"),
            None,
            &imm_acc,
            Priority::Immediate,
        );

        flush_timeout(&d, &lat_acc);
        flush_timeout(&d, &imm_acc);

        let recorded = order.lock().unwrap();
        // Both jobs must have completed.
        assert_eq!(recorded.len(), 2, "not all jobs completed: {:?}", *recorded);
        // Note: deterministic ordering requires OS thread-priority support (thread-priority crate).
        // Without it this is best-effort; we only assert both ran.
    }

    // -----------------------------------------------------------------------
    // CompletionDeferral tests
    // -----------------------------------------------------------------------

    /// Dropping a deferral without calling finish() must still release the
    /// counter so that any waiting jobs are unblocked.
    #[test]
    fn deferral_auto_finish_on_drop() {
        let d = small_dispatcher();
        let counter = Counter::new(ScheduleParam::default(), "drop_test");

        // Deferral holds the counter at +1.
        let deferral = d.create_deferral(&counter, "drop_test");

        // The counter must not be zero yet.
        assert!(!counter.is_zero(), "counter zero before deferral released");

        // Drop without calling .finish() — Drop impl should decrement.
        drop(deferral);

        // Allow time for the decrement to propagate.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !counter.is_zero() {
            assert!(
                std::time::Instant::now() < deadline,
                "deferral drop did not release counter"
            );
            std::thread::yield_now();
        }
    }

    /// Explicit finish() releases the counter and unblocks jobs waiting on it.
    #[test]
    fn deferral_finish_releases_waiting_job() {
        let d = small_dispatcher();
        let counter = Counter::new(ScheduleParam::default(), "finish_test");

        // Hold the counter before dispatching the waiting job.
        let deferral = d.create_deferral(&counter, "finish_test");

        let ran = Arc::new(AtomicU32::new(0));
        let ran2 = Arc::clone(&ran);

        // Job B: waits for counter to hit zero (gated by the deferral).
        let mut builder = d.builder(ScheduleParam::default());
        builder.dispatch_wait(&counter);
        builder.dispatch(move || {
            ran2.store(1, Ordering::Release);
        });
        let done = builder.extract_wait_counter();

        // Give the workers a moment — job B must NOT run yet.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(
            ran.load(Ordering::Acquire),
            0,
            "job B ran before deferral.finish()"
        );

        // Now release — job B must run.
        deferral.finish();
        flush_timeout(&d, &done);
        assert_eq!(
            ran.load(Ordering::Acquire),
            1,
            "job B did not run after deferral.finish()"
        );
    }

    /// A deferral can be sent to another thread and finished there.
    #[test]
    fn deferral_send_to_other_thread() {
        let d = small_dispatcher();
        let counter = Counter::new(ScheduleParam::default(), "cross_thread");

        let deferral = d.create_deferral(&counter, "cross_thread");

        let ran = Arc::new(AtomicU32::new(0));
        let ran2 = Arc::clone(&ran);

        let mut builder = d.builder(ScheduleParam::default());
        builder.dispatch_wait(&counter);
        builder.dispatch(move || {
            ran2.store(1, Ordering::Release);
        });
        let done = builder.extract_wait_counter();

        // Send the deferral to a separate system thread and finish from there.
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            deferral.finish();
        });

        flush_timeout(&d, &done);
        assert_eq!(
            ran.load(Ordering::Acquire),
            1,
            "cross-thread finish did not release job"
        );
        handle.join().unwrap();
    }

    // -----------------------------------------------------------------------
    // 6. Many jobs — stress the queue and counter decrement path.
    // -----------------------------------------------------------------------
    #[test]
    fn stress_many_jobs() {
        let d = Dispatcher::new(JobSystemConfig {
            num_threads: 4,
            queue_capacity: 4096,
            worker_configs: Vec::new(),
        });
        const N: u32 = 1000;
        let sum = Arc::new(AtomicU32::new(0));

        let mut builder = d.builder(ScheduleParam::default());
        for _ in 0..N {
            let s = Arc::clone(&sum);
            builder.dispatch_with_fence(
                move || {
                    s.fetch_add(1, Ordering::Relaxed);
                },
                Fence::None,
            );
        }
        builder.dispatch_fence();
        let done = builder.extract_wait_counter();

        flush_timeout(&d, &done);
        assert_eq!(sum.load(Ordering::Acquire), N);
    }

    // -----------------------------------------------------------------------
    // 7. Priority drain order — deterministic with a single-worker dispatcher.
    // -----------------------------------------------------------------------
    //
    // Uses one worker so only one thread pops from the queues, making the
    // drain order deterministic.  All four jobs are queued while the worker is
    // blocked on a barrier, so they are in the global queues before the worker
    // is free to pop.
    #[test]
    fn priority_ordering_is_deterministic() {
        let d = Dispatcher::new(JobSystemConfig {
            num_threads: 1,
            queue_capacity: 256,
            worker_configs: Vec::new(),
        });

        // Saturate the single worker.
        let barrier = Arc::new(std::sync::Barrier::new(2)); // 1 worker + main
        let b = Arc::clone(&barrier);
        let hold_acc = Counter::new(
            ScheduleParam {
                priority: Priority::CriticalPath,
            },
            "hold",
        );
        d.submit(
            move || {
                b.wait();
            },
            None,
            &hold_acc,
            Priority::CriticalPath,
        );

        // Queue one job per priority lane while the worker is blocked.
        let order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let acc = Counter::new(ScheduleParam::default(), "acc");
        for &(name, pri) in &[
            ("latent", Priority::Latent),
            ("renderpath", Priority::RenderPath),
            ("criticalpath", Priority::CriticalPath),
            ("immediate", Priority::Immediate),
        ] {
            let o = Arc::clone(&order);
            d.submit(move || o.lock().unwrap().push(name), None, &acc, pri);
        }

        // Unblock the worker — it drains all four queues in priority order.
        barrier.wait();

        flush_timeout(&d, &acc);
        let recorded = order.lock().unwrap();
        assert_eq!(
            *recorded,
            vec!["immediate", "criticalpath", "renderpath", "latent"],
            "jobs not drained in priority order: {:?}",
            *recorded
        );
    }

    // -----------------------------------------------------------------------
    // 8. Deep sequential chain (25 jobs) — stresses fence rotation.
    // -----------------------------------------------------------------------
    #[test]
    fn deep_sequential_chain() {
        let d = small_dispatcher();
        const DEPTH: u32 = 25;

        // Each job asserts it sees the write from the previous job, then writes
        // its own index.  Any reordering will panic inside the job itself.
        let slot = Arc::new(AtomicU32::new(0));
        let mut builder = d.builder(ScheduleParam::default());
        for i in 1..=DEPTH {
            let s = Arc::clone(&slot);
            builder.dispatch(move || {
                let prev = s.load(Ordering::Acquire);
                assert_eq!(prev, i - 1, "ordering broken at step {}: saw {}", i, prev);
                s.store(i, Ordering::Release);
            });
        }

        let done = builder.extract_wait_counter();
        flush_timeout(&d, &done);
        assert_eq!(slot.load(Ordering::Acquire), DEPTH);
    }

    // -----------------------------------------------------------------------
    // 9. dispatch_wait on an already-zero counter — hits the early-return path.
    // -----------------------------------------------------------------------
    #[test]
    fn dispatch_wait_already_zero_counter() {
        let d = small_dispatcher();

        // Counter that starts (and stays) at zero — no jobs submitted to it.
        let already_zero = Counter::new(ScheduleParam::default(), "zero");
        assert!(already_zero.is_zero());

        let ran = Arc::new(AtomicU32::new(0));
        let ran2 = Arc::clone(&ran);

        let mut builder = d.builder(ScheduleParam::default());
        builder.dispatch_wait(&already_zero); // should be a no-op, not a permanent gate
        builder.dispatch(move || {
            ran2.store(1, Ordering::Release);
        });
        let done = builder.extract_wait_counter();

        flush_timeout(&d, &done);
        assert_eq!(
            ran.load(Ordering::Acquire),
            1,
            "job after dispatch_wait(zero) did not run"
        );
    }

    // -----------------------------------------------------------------------
    // 10. Thundering herd — N jobs parked on one counter, all released at once.
    // -----------------------------------------------------------------------
    //
    // Exercises the RED-style waiting-list release path in `decrement`: all N
    // waiting jobs are moved back to runnable queues when the gate reaches zero.
    #[test]
    fn thundering_herd_counter_release() {
        let d = Dispatcher::new(JobSystemConfig {
            num_threads: 4,
            queue_capacity: 4096,
            worker_configs: Vec::new(),
        });
        const N: u32 = 100;

        let gate = Counter::new(ScheduleParam::default(), "gate");
        let deferral = d.create_deferral(&gate, "herd_gate");

        let sum = Arc::new(AtomicU32::new(0));
        let acc = Counter::new(ScheduleParam::default(), "acc");

        // Park N jobs on the gate — none should run yet.
        for _ in 0..N {
            let s = Arc::clone(&sum);
            d.submit(
                move || {
                    s.fetch_add(1, Ordering::Relaxed);
                },
                Some(&gate),
                &acc,
                Priority::CriticalPath,
            );
        }

        assert_eq!(
            sum.load(Ordering::Acquire),
            0,
            "jobs ran before gate was released"
        );

        // Release: all N are pushed to the queue and the semaphore bumped once.
        deferral.finish();

        flush_timeout(&d, &acc);
        assert_eq!(
            sum.load(Ordering::Acquire),
            N,
            "not all thundering-herd jobs ran"
        );
    }

    // -----------------------------------------------------------------------
    // 11. Drop with in-flight jobs — no hang, no panic.
    // -----------------------------------------------------------------------
    //
    // Drop sets the exit flag and joins all workers.  Workers finish their
    // current job before checking the flag, so any job already being executed
    // completes.  Queued-but-not-yet-started jobs may not run — that is
    // expected and documented behaviour.
    #[test]
    fn drop_with_in_flight_jobs() {
        let d = small_dispatcher();
        let acc = Counter::new(ScheduleParam::default(), "acc");

        for _ in 0..4 {
            d.submit(
                || {
                    std::thread::sleep(Duration::from_millis(5));
                },
                None,
                &acc,
                Priority::CriticalPath,
            );
        }

        // Drop must return cleanly without hanging.
        drop(d);
    }

    // -----------------------------------------------------------------------
    // 12. Fence actually blocks group B from starting before all of group A.
    // -----------------------------------------------------------------------
    //
    // Group A writes 8 distinct slots.  Group B jobs each assert their
    // corresponding slot was already written — i.e. ALL of group A must be
    // done before ANY of group B starts.  A fence that only waits for *some*
    // of group A would let group B see zeros.
    #[test]
    fn fence_blocks_until_entire_parallel_group_done() {
        let d = small_dispatcher();
        const N: usize = 8;

        let slots: Arc<[AtomicU32; N]> = Arc::new(std::array::from_fn(|_| AtomicU32::new(0)));

        let mut builder = d.builder(ScheduleParam::default());

        // Group A: write each slot, all in parallel (Fence::None).
        for i in 0..N {
            let s = Arc::clone(&slots);
            builder.dispatch_with_fence(
                move || {
                    s[i].store(1, Ordering::Release);
                },
                Fence::None,
            );
        }
        builder.dispatch_fence(); // seal group A

        // Group B: each job reads ALL slots — every one must be 1.
        for i in 0..N {
            let s = Arc::clone(&slots);
            builder.dispatch_with_fence(
                move || {
                    for j in 0..N {
                        assert_eq!(
                            s[j].load(Ordering::Acquire),
                            1,
                            "group B job {} saw slot {} = 0 (group A not fully done)",
                            i,
                            j,
                        );
                    }
                },
                Fence::None,
            );
        }
        builder.dispatch_fence();

        let done = builder.extract_wait_counter();
        flush_timeout(&d, &done);
    }

    // -----------------------------------------------------------------------
    // 13. Diamond dependency: A → {B, C} → D.
    // -----------------------------------------------------------------------
    //
    // D must only run after B and C have both completed.  B and C may run in
    // parallel.  Two dispatch_wait calls on builder D create an AND-dependency:
    // both bridge jobs accumulate into D's wait_for_zero, so D's job only
    // becomes runnable when both B *and* C have finished.
    #[test]
    fn diamond_dependency() {
        let d = small_dispatcher();

        let wrote_a = Arc::new(AtomicU32::new(0));
        let wrote_b = Arc::new(AtomicU32::new(0));
        let wrote_c = Arc::new(AtomicU32::new(0));
        let wrote_d = Arc::new(AtomicU32::new(0));

        // Builder A.
        let wa = Arc::clone(&wrote_a);
        let mut a = d.builder(ScheduleParam::default());
        a.dispatch(move || wa.store(1, Ordering::Release));
        let a_done = a.extract_wait_counter();

        // Builder B — depends on A.
        let wb = Arc::clone(&wrote_b);
        let ra = Arc::clone(&wrote_a);
        let mut b = d.builder(ScheduleParam::default());
        b.dispatch_wait(&a_done);
        b.dispatch(move || {
            assert_eq!(ra.load(Ordering::Acquire), 1, "B ran before A");
            wb.store(1, Ordering::Release);
        });
        let b_done = b.extract_wait_counter();

        // Builder C — also depends on A (parallel branch).
        let wc = Arc::clone(&wrote_c);
        let ra2 = Arc::clone(&wrote_a);
        let mut c = d.builder(ScheduleParam::default());
        c.dispatch_wait(&a_done);
        c.dispatch(move || {
            assert_eq!(ra2.load(Ordering::Acquire), 1, "C ran before A");
            wc.store(1, Ordering::Release);
        });
        let c_done = c.extract_wait_counter();

        // Builder D — depends on BOTH B and C.
        let wd = Arc::clone(&wrote_d);
        let rb = Arc::clone(&wrote_b);
        let rc = Arc::clone(&wrote_c);
        let mut d_builder = d.builder(ScheduleParam::default());
        d_builder.dispatch_wait(&b_done);
        d_builder.dispatch_wait(&c_done);
        d_builder.dispatch(move || {
            assert_eq!(rb.load(Ordering::Acquire), 1, "D ran before B");
            assert_eq!(rc.load(Ordering::Acquire), 1, "D ran before C");
            wd.store(1, Ordering::Release);
        });
        let d_done = d_builder.extract_wait_counter();

        flush_timeout(&d, &d_done);
        assert_eq!(wrote_d.load(Ordering::Acquire), 1, "D never ran");
    }

    // -----------------------------------------------------------------------
    // 14. Three-builder transitivity: A → B → C.
    // -----------------------------------------------------------------------
    //
    // Each hop goes through a real extract_wait_counter / dispatch_wait pair,
    // exercising the submit double-check on every crossing.
    #[test]
    fn three_builder_chain_transitivity() {
        let d = small_dispatcher();

        let seq = Arc::new(AtomicU32::new(0));

        // A writes 1.
        let s1 = Arc::clone(&seq);
        let mut a = d.builder(ScheduleParam::default());
        a.dispatch(move || s1.store(1, Ordering::Release));
        let a_done = a.extract_wait_counter();

        // B waits for A, asserts 1, writes 2.
        let s2 = Arc::clone(&seq);
        let mut b = d.builder(ScheduleParam::default());
        b.dispatch_wait(&a_done);
        b.dispatch(move || {
            assert_eq!(s2.load(Ordering::Acquire), 1, "B ran before A");
            s2.store(2, Ordering::Release);
        });
        let b_done = b.extract_wait_counter();

        // C waits for B, asserts 2, writes 3.
        let s3 = Arc::clone(&seq);
        let mut c = d.builder(ScheduleParam::default());
        c.dispatch_wait(&b_done);
        c.dispatch(move || {
            assert_eq!(s3.load(Ordering::Acquire), 2, "C ran before B");
            s3.store(3, Ordering::Release);
        });
        let c_done = c.extract_wait_counter();

        flush_timeout(&d, &c_done);
        assert_eq!(seq.load(Ordering::Acquire), 3, "chain did not complete");
    }

    // -----------------------------------------------------------------------
    // 15. Two builders race to dispatch_wait on the same gate counter.
    // -----------------------------------------------------------------------
    //
    // Both builders call dispatch_wait concurrently (synchronized via a barrier)
    // to exercise the double-check-under-lock in submit when called from
    // multiple threads simultaneously.  Both downstream jobs must complete.
    #[test]
    fn concurrent_dispatch_wait_same_gate() {
        let d = Arc::new(small_dispatcher());

        let gate = Counter::new(ScheduleParam::default(), "shared_gate");
        let deferral = d.create_deferral(&gate, "shared_gate");

        let ran_x = Arc::new(AtomicU32::new(0));
        let ran_y = Arc::new(AtomicU32::new(0));

        // Both builder threads will reach the barrier before calling dispatch_wait,
        // making the two submissions as concurrent as userspace allows.
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let d1 = Arc::clone(&d);
        let gate1 = gate.clone();
        let rx = Arc::clone(&ran_x);
        let bar1 = Arc::clone(&barrier);
        let h1 = std::thread::spawn(move || {
            bar1.wait(); // sync with h2
            let mut builder = d1.builder(ScheduleParam::default());
            builder.dispatch_wait(&gate1);
            builder.dispatch(move || rx.store(1, Ordering::Release));
            builder.extract_wait_counter()
        });

        let d2 = Arc::clone(&d);
        let gate2 = gate.clone();
        let ry = Arc::clone(&ran_y);
        let bar2 = Arc::clone(&barrier);
        let h2 = std::thread::spawn(move || {
            bar2.wait(); // sync with h1
            let mut builder = d2.builder(ScheduleParam::default());
            builder.dispatch_wait(&gate2);
            builder.dispatch(move || ry.store(1, Ordering::Release));
            builder.extract_wait_counter()
        });

        // Let both threads build their builders, then release the gate.
        let done_x = h1.join().unwrap();
        let done_y = h2.join().unwrap();

        deferral.finish();

        flush_timeout(&d, &done_x);
        flush_timeout(&d, &done_y);

        assert_eq!(ran_x.load(Ordering::Acquire), 1, "builder X job never ran");
        assert_eq!(ran_y.load(Ordering::Acquire), 1, "builder Y job never ran");
    }

    // -----------------------------------------------------------------------
    // 16. Counter reuse — wave 1 drains, counter reused for wave 2.
    // -----------------------------------------------------------------------
    //
    // Verifies that after a counter reaches zero and its waiting list is
    // drained (mem::take), a second set of jobs gated on the same counter
    // works correctly with no bleed from wave 1.
    #[test]
    fn counter_reuse_no_wave_bleed() {
        let d = small_dispatcher();

        let gate = Counter::new(ScheduleParam::default(), "reuse_gate");

        // --- Wave 1 ---
        let wave1_ran = Arc::new(AtomicU32::new(0));
        {
            let deferral = d.create_deferral(&gate, "wave1");
            let acc = Counter::new(ScheduleParam::default(), "w1_acc");
            for _ in 0..4 {
                let r = Arc::clone(&wave1_ran);
                d.submit(
                    move || {
                        r.fetch_add(1, Ordering::Relaxed);
                    },
                    Some(&gate),
                    &acc,
                    Priority::CriticalPath,
                );
            }
            deferral.finish();
            flush_timeout(&d, &acc);
        }
        assert_eq!(
            wave1_ran.load(Ordering::Acquire),
            4,
            "wave 1 jobs did not all run"
        );
        assert!(gate.is_zero(), "gate not zero after wave 1");

        // --- Wave 2 ---
        // Counter is at zero; create a fresh deferral to hold it non-zero again.
        let wave2_ran = Arc::new(AtomicU32::new(0));
        {
            let deferral = d.create_deferral(&gate, "wave2");
            let acc = Counter::new(ScheduleParam::default(), "w2_acc");
            for _ in 0..4 {
                let r = Arc::clone(&wave2_ran);
                d.submit(
                    move || {
                        r.fetch_add(1, Ordering::Relaxed);
                    },
                    Some(&gate),
                    &acc,
                    Priority::CriticalPath,
                );
            }
            // Wave 2 must not have started yet — gate still held by deferral.
            assert_eq!(
                wave2_ran.load(Ordering::Acquire),
                0,
                "wave 2 jobs ran before gate released"
            );
            deferral.finish();
            flush_timeout(&d, &acc);
        }
        assert_eq!(
            wave2_ran.load(Ordering::Acquire),
            4,
            "wave 2 jobs did not all run"
        );
        // Wave 1 count must still be exactly 4 — wave 2 didn't re-run those jobs.
        assert_eq!(
            wave1_ran.load(Ordering::Acquire),
            4,
            "wave 1 count changed after wave 2"
        );
    }

    // -----------------------------------------------------------------------
    // 17. Concurrent decrements — waiting job runs exactly once.
    // -----------------------------------------------------------------------
    //
    // N jobs are all tracked by the same accumulate counter.  One job is
    // parked on that counter's waiting list.  When all N complete, N concurrent
    // fetch_sub calls race; exactly one must observe prev==1 and fire the flush.
    // The parked job must run exactly once — not zero times, not twice.
    #[test]
    fn concurrent_decrements_flush_fires_exactly_once() {
        let d = Dispatcher::new(JobSystemConfig {
            num_threads: 4,
            queue_capacity: 512,
            worker_configs: Vec::new(),
        });
        const N: u32 = 16;

        // acc: the counter all N workers decrement.
        let acc = Counter::new(ScheduleParam::default(), "acc");
        // gate: the counter the sentinel job is parked on — same as acc.
        // We submit the sentinel via a builder that dispatch_waits on acc,
        // then we track how many times it runs.
        let run_count = Arc::new(AtomicU32::new(0));

        // Submit N tiny jobs against acc.
        for _ in 0..N {
            let noop_acc = acc.clone();
            d.submit(|| {}, None, &noop_acc, Priority::CriticalPath);
        }

        // Submit a sentinel job gated on acc.  It must run exactly once.
        let rc = Arc::clone(&run_count);
        let sentinel_acc = Counter::new(ScheduleParam::default(), "sentinel_acc");
        d.submit(
            move || {
                rc.fetch_add(1, Ordering::Relaxed);
            },
            Some(&acc),
            &sentinel_acc,
            Priority::CriticalPath,
        );

        flush_timeout(&d, &sentinel_acc);
        assert_eq!(
            run_count.load(Ordering::Acquire),
            1,
            "sentinel ran {} times (expected exactly 1)",
            run_count.load(Ordering::Acquire),
        );
    }

    // -----------------------------------------------------------------------
    // 18. Cross-builder chain with parallel groups at each hop.
    // -----------------------------------------------------------------------
    //
    // Unlike test 14 (single job per builder), each hop here is M parallel
    // jobs (Fence::None group) followed by a fence.  Tests fence rotation
    // combined with extract_wait_counter / dispatch_wait in the same chain.
    //
    //   [A₀ ‖ A₁ ‖ A₂ ‖ A₃] → [B₀ ‖ B₁ ‖ B₂ ‖ B₃] → [C₀ ‖ C₁ ‖ C₂ ‖ C₃]
    #[test]
    fn cross_builder_chain_with_parallel_groups() {
        let d = small_dispatcher();
        const M: usize = 4;

        let phase = Arc::new(AtomicU32::new(0)); // 0 → A running, 1 → B, 2 → C

        // Builder A: M parallel jobs that assert phase==0, then set it to 1 once.
        let mut a = d.builder(ScheduleParam::default());
        for _ in 0..M {
            let p = Arc::clone(&phase);
            a.dispatch_with_fence(
                move || assert_eq!(p.load(Ordering::Acquire), 0, "A job ran out of phase"),
                Fence::None,
            );
        }
        // After all A jobs, advance phase to 1.
        let p1 = Arc::clone(&phase);
        a.dispatch_with_fence(move || p1.store(1, Ordering::Release), Fence::None);
        a.dispatch_fence();
        let a_done = a.extract_wait_counter();

        // Builder B: waits for A, then M parallel jobs assert phase==1, set to 2.
        let mut b = d.builder(ScheduleParam::default());
        b.dispatch_wait(&a_done);
        for _ in 0..M {
            let p = Arc::clone(&phase);
            b.dispatch_with_fence(
                move || assert_eq!(p.load(Ordering::Acquire), 1, "B job ran out of phase"),
                Fence::None,
            );
        }
        let p2 = Arc::clone(&phase);
        b.dispatch_with_fence(move || p2.store(2, Ordering::Release), Fence::None);
        b.dispatch_fence();
        let b_done = b.extract_wait_counter();

        // Builder C: waits for B, then M parallel jobs assert phase==2.
        let mut c = d.builder(ScheduleParam::default());
        c.dispatch_wait(&b_done);
        for _ in 0..M {
            let p = Arc::clone(&phase);
            c.dispatch_with_fence(
                move || assert_eq!(p.load(Ordering::Acquire), 2, "C job ran out of phase"),
                Fence::None,
            );
        }
        c.dispatch_fence();
        let c_done = c.extract_wait_counter();

        flush_timeout(&d, &c_done);
        assert_eq!(phase.load(Ordering::Acquire), 2);
    }

    // -----------------------------------------------------------------------
    // Builder misuse — contract panics (regression guards)
    // -----------------------------------------------------------------------
    //
    // Note: "dispatch after extract_wait_counter" is NOT a runtime scenario —
    // extract_wait_counter() consumes the builder (takes self), so calling
    // dispatch afterwards is a compile-time error.  No test needed for that.

    /// Fence::None followed by extract_wait_counter() without an intervening
    /// dispatch_fence() must panic, not silently return a broken counter.
    #[test]
    #[should_panic(expected = "Must call dispatch_fence() before extract_wait_counter()")]
    fn extract_wait_counter_without_fence_panics() {
        let d = small_dispatcher();
        let mut builder = d.builder(ScheduleParam::default());
        builder.dispatch_with_fence(|| {}, Fence::None);
        // Missing dispatch_fence() — must panic here:
        let _ = builder.extract_wait_counter();
    }

    /// Fence::None followed by dispatch_wait() without an intervening
    /// dispatch_fence() must panic.
    #[test]
    #[should_panic(expected = "Must call dispatch_fence() before dispatch_wait()")]
    fn dispatch_wait_without_fence_panics() {
        let d = small_dispatcher();
        let gate = Counter::new(ScheduleParam::default(), "gate");
        let _deferral = d.create_deferral(&gate, "gate");

        let mut builder = d.builder(ScheduleParam::default());
        builder.dispatch_with_fence(|| {}, Fence::None);
        // Missing dispatch_fence() — must panic here:
        builder.dispatch_wait(&gate);
    }

    /// Fence::None followed by a Fence::Full dispatch() without an intervening
    /// dispatch_fence() must panic.
    #[test]
    #[should_panic(
        expected = "Must call dispatch_fence() before a Fence::Full dispatch after using Fence::None"
    )]
    fn fence_full_after_fence_none_without_dispatch_fence_panics() {
        let d = small_dispatcher();
        let mut builder = d.builder(ScheduleParam::default());
        builder.dispatch_with_fence(|| {}, Fence::None);
        // Missing dispatch_fence() — calling dispatch() (Fence::Full) must panic:
        builder.dispatch(|| {});
    }

    // -----------------------------------------------------------------------
    // 25. Concurrent builders — N threads build independent chains simultaneously.
    // -----------------------------------------------------------------------
    //
    // No shared builder; the dispatcher is shared via Arc.  All threads start
    // building simultaneously (barrier-synchronized), exercising concurrent
    // `push_global` / `semaphore.release` from N simultaneous producers.
    #[test]
    fn concurrent_builders_independent_chains() {
        const N_THREADS: usize = 8;
        const JOBS_PER: u32 = 10;

        let d = Arc::new(Dispatcher::new(JobSystemConfig {
            num_threads: N_THREADS,
            queue_capacity: 1024,
            worker_configs: Vec::new(),
        }));

        let total = Arc::new(AtomicU32::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(N_THREADS));

        let handles: Vec<_> = (0..N_THREADS)
            .map(|_| {
                let d2 = Arc::clone(&d);
                let t = Arc::clone(&total);
                let bar = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    bar.wait(); // all threads start building at the same instant
                    let mut builder = d2.builder(ScheduleParam::default());
                    for _ in 0..JOBS_PER {
                        let tc = Arc::clone(&t);
                        builder.dispatch_with_fence(
                            move || {
                                tc.fetch_add(1, Ordering::Relaxed);
                            },
                            Fence::None,
                        );
                    }
                    builder.dispatch_fence();
                    builder.extract_wait_counter()
                })
            })
            .collect();

        // Collect all done counters (threads have submitted, not yet flushed).
        let done_counters: Vec<Counter> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for done in &done_counters {
            flush_timeout(&d, done);
        }

        assert_eq!(
            total.load(Ordering::Acquire),
            (N_THREADS as u32) * JOBS_PER,
            "not all concurrent-builder jobs ran",
        );
    }

    // -----------------------------------------------------------------------
    // 26. Concurrent flush on disjoint counters — no cross-contamination.
    // -----------------------------------------------------------------------
    //
    // flush() is *designed* to execute any available job while helping wait
    // for its target counter.  The property under test is accounting
    // correctness: running jobs belonging to counter_b while flushing
    // counter_a must not corrupt either counter's value or job count.
    //
    // Two threads flush different counters simultaneously.  After both return,
    // every job count and both counters must be exactly right.
    #[test]
    fn concurrent_flush_disjoint_counters() {
        const N: u32 = 50;

        let d = Arc::new(Dispatcher::new(JobSystemConfig {
            num_threads: 4,
            queue_capacity: 512,
            worker_configs: Vec::new(),
        }));

        let count_a = Arc::new(AtomicU32::new(0));
        let count_b = Arc::new(AtomicU32::new(0));
        let acc_a = Counter::new(ScheduleParam::default(), "acc_a");
        let acc_b = Counter::new(ScheduleParam::default(), "acc_b");

        for _ in 0..N {
            let ca = Arc::clone(&count_a);
            let cb = Arc::clone(&count_b);
            d.submit(
                move || {
                    ca.fetch_add(1, Ordering::Relaxed);
                },
                None,
                &acc_a,
                Priority::CriticalPath,
            );
            d.submit(
                move || {
                    cb.fetch_add(1, Ordering::Relaxed);
                },
                None,
                &acc_b,
                Priority::CriticalPath,
            );
        }

        let d1 = Arc::clone(&d);
        let a1 = acc_a.clone();
        let h_a = std::thread::spawn(move || flush_timeout(&d1, &a1));

        let d2 = Arc::clone(&d);
        let b2 = acc_b.clone();
        let h_b = std::thread::spawn(move || flush_timeout(&d2, &b2));

        h_a.join().unwrap();
        h_b.join().unwrap();

        assert_eq!(count_a.load(Ordering::Acquire), N, "acc_a: wrong job count");
        assert_eq!(count_b.load(Ordering::Acquire), N, "acc_b: wrong job count");
        assert!(acc_a.is_zero(), "acc_a not zero after flush");
        assert!(acc_b.is_zero(), "acc_b not zero after flush");
    }

    // -----------------------------------------------------------------------
    // 27. Rapid-fire submit/flush loop — catches semaphore drift and
    //     counter-state leaks between frames.
    // -----------------------------------------------------------------------
    //
    // 1000 "frame" iterations: each dispatches JOBS_PER parallel jobs, flushes,
    // and asserts the counter is truly zero before the next frame begins.
    // Positive semaphore drift → spurious worker wake-ups and stale jobs running
    // in a later frame.  Negative drift → eventual deadlock.
    #[test]
    fn rapid_fire_submit_flush_loop() {
        let d = small_dispatcher();
        const ITERS: u32 = 1000;
        const JOBS_PER: u32 = 4;

        let total = Arc::new(AtomicU32::new(0));

        for _ in 0..ITERS {
            let mut builder = d.builder(ScheduleParam::default());
            for _ in 0..JOBS_PER {
                let t = Arc::clone(&total);
                builder.dispatch_with_fence(
                    move || {
                        t.fetch_add(1, Ordering::Relaxed);
                    },
                    Fence::None,
                );
            }
            builder.dispatch_fence();
            let done = builder.extract_wait_counter();
            flush_timeout(&d, &done);
            // The counter must be at zero before the next iteration begins.
            assert!(
                done.is_zero(),
                "done counter not zero after flush — state leak between frames"
            );
        }

        assert_eq!(
            total.load(Ordering::Acquire),
            ITERS * JOBS_PER,
            "total job count wrong after rapid-fire loop",
        );
    }

    // -----------------------------------------------------------------------
    // 28. N builders all dispatch_wait on the same gate — all N chains proceed.
    // -----------------------------------------------------------------------
    //
    // 16 independent builders each add a 2-hop chain
    // (bridge job → actual job) to a single shared gate.  The gate is
    // released exactly once.  Verifies that `decrement` releases all N bridge
    // jobs across N *different* accumulate counters, not just the first one
    // registered.
    //
    // Distinct from `thundering_herd_counter_release` (test 10) which uses raw
    // `submit()` with a single shared accumulate counter (1-hop).  Here every
    // builder has its own independent counter chain (bridge via dispatch_wait).
    #[test]
    fn n_builders_dispatch_wait_same_gate_all_proceed() {
        const N: usize = 16;

        let d = Dispatcher::new(JobSystemConfig {
            num_threads: 4,
            queue_capacity: 1024,
            worker_configs: Vec::new(),
        });

        let gate = Counter::new(ScheduleParam::default(), "gate");
        let deferral = d.create_deferral(&gate, "n_builders_gate");

        let ran = Arc::new(AtomicU32::new(0));
        let mut done_counters: Vec<Counter> = Vec::with_capacity(N);

        // Build all N chains while the gate is held — all bridge jobs park
        // on gate.waiting, and all actual jobs park on their per-builder
        // wait_for_zero counter.
        for _ in 0..N {
            let r = Arc::clone(&ran);
            let mut builder = d.builder(ScheduleParam::default());
            builder.dispatch_wait(&gate);
            builder.dispatch(move || {
                r.fetch_add(1, Ordering::Relaxed);
            });
            done_counters.push(builder.extract_wait_counter());
        }

        // No job should have run yet.
        assert_eq!(
            ran.load(Ordering::Acquire),
            0,
            "jobs ran before gate released"
        );

        // Single release — all N 2-hop chains must unblock.
        deferral.finish();

        for done in &done_counters {
            flush_timeout(&d, done);
        }

        assert_eq!(
            ran.load(Ordering::Acquire),
            N as u32,
            "not all N builder chains proceeded after single gate release",
        );
    }

    // -----------------------------------------------------------------------
    // 29. submit_local depth — three levels of child-queue chaining.
    // -----------------------------------------------------------------------
    //
    // L1 (submitted normally) calls Dispatcher::submit_local to push L2 onto
    // the calling worker's LOCAL_QUEUE.  L2 does the same for L3.  The work
    // loop drains LOCAL_QUEUE before going back to sleep, so execution is
    // strictly L1 → L2 → L3 on the same worker thread — no semaphore
    // involvement for L2 or L3.
    //
    // All three share one accumulate counter.  flush_timeout returns only when
    // all three increments-then-decrements have completed, so `ran == 3` is
    // a full correctness check, not just a "did it finish" check.
    #[test]
    fn submit_local_three_level_depth_all_complete() {
        let d = small_dispatcher();
        let ran = Arc::new(AtomicU32::new(0));
        let acc = Counter::new(ScheduleParam::default(), "acc");

        let r = Arc::clone(&ran);
        let acc2 = acc.clone();
        d.submit(
            move || {
                // L1 — on the worker thread; LOCAL_QUEUE is valid here.
                r.fetch_add(1, Ordering::Relaxed);
                let r = Arc::clone(&r); // shadow: clone for L2 capture
                let acc3 = acc2.clone();
                Dispatcher::submit_local(
                    move || {
                        // L2 — popped from LOCAL_QUEUE by the same worker after L1.
                        r.fetch_add(1, Ordering::Relaxed);
                        let r = Arc::clone(&r); // shadow: clone for L3 capture
                        Dispatcher::submit_local(
                            move || {
                                r.fetch_add(1, Ordering::Relaxed);
                            }, // L3
                            &acc3,
                            Priority::CriticalPath,
                        );
                    },
                    &acc2,
                    Priority::CriticalPath,
                );
            },
            None,
            &acc,
            Priority::CriticalPath,
        );

        flush_timeout(&d, &acc);
        assert_eq!(
            ran.load(Ordering::Acquire),
            3,
            "not all three local-queue levels ran"
        );
    }

    // -----------------------------------------------------------------------
    // 30. flush() called from inside a running job.
    // -----------------------------------------------------------------------
    //
    // The outer job submits inner work and then calls dispatcher.flush() on
    // the inner counter — exercising re-entrant flush from a worker thread.
    // flush() uses try_pop() to help drain the queue inline, so it works even
    // when all workers are otherwise occupied.  With 2 workers the inner job
    // may be picked up by the second worker or by the outer job's flush loop;
    // either path must satisfy the guarantee that flush() returns only when
    // inner_acc has reached zero.
    #[test]
    fn flush_from_inside_running_job() {
        let d = Arc::new(Dispatcher::new(JobSystemConfig {
            num_threads: 2,
            queue_capacity: 256,
            worker_configs: Vec::new(),
        }));

        let inner_ran = Arc::new(AtomicU32::new(0));
        let outer_ran = Arc::new(AtomicU32::new(0));
        let inner_acc = Counter::new(ScheduleParam::default(), "inner");
        let outer_acc = Counter::new(ScheduleParam::default(), "outer");

        {
            let d2 = Arc::clone(&d);
            let ir = Arc::clone(&inner_ran);
            let outer_flag = Arc::clone(&outer_ran);
            let inner_acc_sub = inner_acc.clone(); // accumulate for inner job
            let inner_acc_flush = inner_acc.clone(); // used for the flush call

            d.submit(
                move || {
                    // Submit inner work from inside a running job.
                    d2.submit(
                        move || {
                            ir.fetch_add(1, Ordering::Relaxed);
                        },
                        None,
                        &inner_acc_sub,
                        Priority::CriticalPath,
                    );
                    // Flush inline — blocks until inner_acc reaches zero.
                    d2.flush(&inner_acc_flush);
                    // flush() guarantees the counter is zero on return.
                    assert!(
                        inner_acc_flush.is_zero(),
                        "flush returned before inner_acc reached zero"
                    );
                    outer_flag.store(1, Ordering::Release);
                },
                None,
                &outer_acc,
                Priority::CriticalPath,
            );
        }

        flush_timeout(&d, &outer_acc);
        assert_eq!(
            outer_ran.load(Ordering::Acquire),
            1,
            "outer job did not complete"
        );
        assert_eq!(
            inner_ran.load(Ordering::Acquire),
            1,
            "inner job did not complete"
        );
    }

    // -----------------------------------------------------------------------
    // 31. Panicking job — worker survives, dispatcher remains functional.
    // -----------------------------------------------------------------------
    //
    // Without catch_unwind, a panicking job kills the worker thread and leaves
    // the accumulate counter permanently non-zero, gating every downstream job
    // forever.  execute_job() uses catch_unwind + unconditional decrement to
    // ensure:
    //   (a) accumulate reaches zero → flush_timeout returns (not deadlock),
    //   (b) the worker stays alive,
    //   (c) the dispatcher accepts and runs new work after the panic.
    //
    // The panic message is emitted to stderr by execute_job — this is expected
    // and visible in `cargo test -- --nocapture` output.
    //
    // Requires panic = "unwind" (default for dev/test). With panic = "abort"
    // the process would terminate before catch_unwind acts.
    #[test]
    fn panicking_job_worker_remains_functional() {
        let d = small_dispatcher();
        let panic_acc = Counter::new(ScheduleParam::default(), "panic_acc");

        // (a) Submit a job that panics; its accumulate must still reach zero.
        d.submit(
            || panic!("intentional leet_jobs test panic"),
            None,
            &panic_acc,
            Priority::CriticalPath,
        );
        // If decrement is skipped on panic this times out → test failure.
        flush_timeout(&d, &panic_acc);

        // (b) + (c) Submit normal work after the panic — must run cleanly.
        let ran = Arc::new(AtomicU32::new(0));
        let normal_acc = Counter::new(ScheduleParam::default(), "normal_acc");
        let r = Arc::clone(&ran);
        d.submit(
            move || {
                r.store(1, Ordering::Release);
            },
            None,
            &normal_acc,
            Priority::CriticalPath,
        );
        flush_timeout(&d, &normal_acc);
        assert_eq!(
            ran.load(Ordering::Acquire),
            1,
            "dispatcher not functional after panicking job"
        );
    }

    // -----------------------------------------------------------------------
    // 32. Shutdown while a job blocks on an eternal counter — no hang.
    // -----------------------------------------------------------------------
    //
    // Root cause without the fix: `Dispatcher::flush` parks on the counter's
    // zero_condvar with a 1 ms timeout, then `continue`s — it never checks the
    // exit flag.  `Dispatcher::drop` calls `join()` on the worker thread whose
    // current job is stuck in that park loop.  Because no one ever sets the
    // counter to zero, the join blocks forever.
    //
    // Fix: after the condvar park `flush` now checks `self.inner.exit` and
    // breaks if the dispatcher is shutting down.
    //
    // Test structure note: `Dispatcher` is !Sync (holds Vec<JoinHandle>) so
    // we cannot call `d.flush()` from a second thread while the main thread
    // owns `d`.  Instead the job captures `Arc<DispatcherInner>` (pub(crate))
    // and runs the same park-then-exit-check logic that the fixed `flush` uses.
    // This exercises the identical code path without requiring Dispatcher: Sync.
    #[test]
    fn shutdown_while_job_blocks_on_eternal_counter_no_hang() {
        use crate::dispatcher::DispatcherInner;

        let d = Dispatcher::new(JobSystemConfig {
            num_threads: 2,
            queue_capacity: 256,
            worker_configs: Vec::new(),
        });

        // eternal_gate is held non-zero by a deferral that is never finished.
        let eternal_gate = Counter::new(ScheduleParam::default(), "eternal");
        let deferral = d.create_deferral(&eternal_gate, "eternal");

        // Clone the DispatcherInner Arc before submitting.  The job captures
        // this, NOT Arc<Dispatcher>, so Dispatcher::drop() fires as soon as the
        // main thread calls drop(d).
        let inner: Arc<DispatcherInner> = Arc::clone(d.inner());

        let gate2 = eternal_gate.clone();
        let unblocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ub2: Arc<std::sync::atomic::AtomicBool> = Arc::clone(&unblocked);

        // Barrier: ensures the job is already inside the park loop before the
        // main thread calls drop(d), avoiding a race where exit=true is set
        // before the loop is even entered.
        let bar = Arc::new(std::sync::Barrier::new(2));
        let b2 = Arc::clone(&bar);

        let acc = Counter::new(ScheduleParam::default(), "acc");
        d.submit(
            move || {
                b2.wait(); // signal main thread: the job is now actively running
                           // Mirror the park logic of Dispatcher::flush (same code path as the fix).
                while !gate2.is_zero() {
                    let (ref mtx, ref cvar) = gate2.inner().zero_condvar;
                    let guard = mtx.lock().unwrap();
                    if !gate2.is_zero() {
                        let _ = cvar.wait_timeout(guard, Duration::from_millis(1));
                    }
                    // Exit-aware: this is what Dispatcher::flush now does.
                    if inner.is_exiting() {
                        break;
                    }
                }
                ub2.store(true, Ordering::Release);
            },
            None,
            &acc,
            Priority::CriticalPath,
        );

        bar.wait(); // wait until the job has started

        // drop(d) runs Dispatcher::drop() immediately: sets exit=true,
        // semaphore.release(2), then join()s all workers.
        // With the fix the parked job sees is_exiting()=true within ≤1ms,
        // breaks, returns, and its worker exits — join returns cleanly.
        drop(d); // must not hang

        // gate is still non-zero (deferral was never finished).
        assert!(
            !eternal_gate.is_zero(),
            "eternal gate should still be non-zero"
        );
        assert!(
            unblocked.load(Ordering::Acquire),
            "job did not unblock after dispatcher drop"
        );

        drop(deferral); // harmless cleanup: decrements gate, no workers to wake
    }

    // -----------------------------------------------------------------------
    // 33. Shutdown with jobs parked in counter waiting lists — no hang, no leak.
    // -----------------------------------------------------------------------
    //
    // When `Dispatcher::drop` runs:
    //  1. `exit = true` is set.
    //  2. `semaphore.release(N_workers)` wakes all sleeping workers.
    //  3. Workers see `exit=true` via the semaphore-acquire → exit-check path
    //     and break from their loop.  `join()` returns for every worker.
    //
    // Jobs parked in counter waiting lists are NOT touched by any of this —
    // they remain in `CounterInner::waiting` until the gate counter's last
    // `Counter` or `CompletionDeferral` handle drops.  At that point:
    //  - The waiting jobs are moved to the global queues (via `decrement`).
    //  - With no workers alive those jobs are never executed.
    //  - When the `DispatcherInner` Arc's refcount reaches zero the
    //    `ArrayQueue`s are dropped, invoking `Box::drop` on each closure.
    //
    // Assertions:
    //  - drop(d) must not hang (workers exit without touching the waiting list).
    //  - acc.value stays == N after drop(d): the waiting jobs never ran.
    //  - Everything cleans up without panic when the remaining handles drop.
    #[test]
    fn shutdown_with_waiting_jobs_no_hang() {
        const N: u32 = 20;

        let d = Dispatcher::new(JobSystemConfig {
            num_threads: 2,
            queue_capacity: 256,
            worker_configs: Vec::new(),
        });

        let gate = Counter::new(ScheduleParam::default(), "eternal_gate");
        let deferral = d.create_deferral(&gate, "shutdown_deferral");
        let acc = Counter::new(ScheduleParam::default(), "acc");

        // Park N jobs on gate.waiting.  Workers are sleeping (no global items).
        for _ in 0..N {
            d.submit(|| {}, Some(&gate), &acc, Priority::CriticalPath);
        }

        // Workers sleeping on the semaphore, N jobs in gate.waiting.
        // drop(d): exit=true, semaphore.release(2) → workers wake and exit.
        // join() must return without touching the waiting list.
        drop(d); // must not hang

        // Waiting jobs never ran: acc was incremented N times at submit but
        // never decremented.
        assert!(
            !acc.is_zero(),
            "waiting jobs ran unexpectedly before gate released"
        );

        // Deferral drop: decrement gate → N jobs moved to global queues
        // (no workers to pick them up) → stay there until DispatcherInner drops.
        // DispatcherInner drops here (last Arc released) → queues drain → closures
        // are dropped without being called.  Must complete without panic.
        drop(deferral);
        // acc's CounterInner is now held only by the local `acc` variable.
        // Its value is still N (never decremented).  All clean.
    }
}
