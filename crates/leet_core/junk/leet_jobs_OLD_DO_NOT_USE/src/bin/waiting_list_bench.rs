use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use leet_jobs::{Counter, Dispatcher, JobDecl, JobHint, JobSystemConfig, Priority, ScheduleParam};

#[derive(Default)]
struct BenchState {
    completed: AtomicU64,
}

unsafe fn bench_job(job_data: *mut c_void, _run_context: &leet_jobs::RunContext) {
    // SAFETY: job_data always points to a live BenchState for the whole run.
    let state = unsafe { &*(job_data as *const BenchState) };
    state.completed.fetch_add(1, Ordering::Relaxed);
}

unsafe fn noop_job(_job_data: *mut c_void, _run_context: &leet_jobs::RunContext) {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JobWork {
    Noop,
    SharedAtomic,
}

impl JobWork {
    const ALL: [Self; 2] = [Self::Noop, Self::SharedAtomic];

    fn label(self) -> &'static str {
        match self {
            Self::Noop => "noop",
            Self::SharedAtomic => "shared_atomic",
        }
    }

    fn job_func(self) -> unsafe fn(*mut c_void, &leet_jobs::RunContext) {
        match self {
            Self::Noop => noop_job,
            Self::SharedAtomic => bench_job,
        }
    }

    fn verifies_completed_count(self) -> bool {
        matches!(self, Self::SharedAtomic)
    }
}

#[derive(Clone, Copy)]
struct Sample {
    elapsed_us: u128,
    completed: Option<u64>,
}

fn make_job_decl(state: &Arc<BenchState>, work: JobWork) -> JobDecl {
    JobDecl {
        job_func: Some(work.job_func()),
        job_data: Arc::as_ptr(state) as *mut c_void,
        instrumentation_object: None,
        hint: JobHint::None,
        debug_flags: 0,
    }
}

fn make_counter() -> Counter {
    Counter::new(
        ScheduleParam {
            priority: Priority::CriticalPath,
        },
        "waiting_list_bench",
    )
}

fn run_direct(
    dispatcher: &Dispatcher,
    job_decl: JobDecl,
    work: JobWork,
    jobs_per_round: usize,
    rounds: usize,
) -> Sample {
    let state = unsafe { &*(job_decl.job_data as *const BenchState) };
    state.completed.store(0, Ordering::Relaxed);

    let start = Instant::now();
    for _ in 0..rounds {
        let acc = make_counter();
        let zero_gate = make_counter();
        for _ in 0..jobs_per_round {
            // SAFETY: `job_decl.job_data` points at `state`, which outlives
            // every submitted job in this benchmark run.
            unsafe {
                dispatcher.submit_job_decl(
                    job_decl,
                    Some(&zero_gate),
                    &acc,
                    Priority::CriticalPath,
                );
            }
        }
        dispatcher.flush(&acc);
        assert!(acc.is_zero(), "direct benchmark counter did not reach zero");
    }
    let elapsed = start.elapsed().as_micros();
    Sample {
        elapsed_us: elapsed,
        completed: work
            .verifies_completed_count()
            .then(|| state.completed.load(Ordering::Relaxed)),
    }
}

fn run_gated(
    dispatcher: &Dispatcher,
    job_decl: JobDecl,
    work: JobWork,
    jobs_per_round: usize,
    rounds: usize,
) -> Sample {
    let state = unsafe { &*(job_decl.job_data as *const BenchState) };
    state.completed.store(0, Ordering::Relaxed);

    let start = Instant::now();
    for _ in 0..rounds {
        let acc = make_counter();
        let gate = make_counter();
        let deferral = dispatcher.create_deferral(&gate, "waiting_list_bench_gate");

        for _ in 0..jobs_per_round {
            // SAFETY: `job_decl.job_data` points at `state`, which outlives
            // every submitted job in this benchmark run.
            unsafe {
                dispatcher.submit_job_decl(job_decl, Some(&gate), &acc, Priority::CriticalPath);
            }
        }

        deferral.finish();
        dispatcher.flush(&acc);
        assert!(acc.is_zero(), "gated benchmark counter did not reach zero");
    }
    let elapsed = start.elapsed().as_micros();
    Sample {
        elapsed_us: elapsed,
        completed: work
            .verifies_completed_count()
            .then(|| state.completed.load(Ordering::Relaxed)),
    }
}

fn parse_arg<T: std::str::FromStr>(args: &[String], index: usize, default: T) -> T {
    args.get(index)
        .and_then(|s| s.parse::<T>().ok())
        .unwrap_or(default)
}

fn average_us(samples: &[Sample]) -> f64 {
    samples
        .iter()
        .map(|sample| sample.elapsed_us as f64)
        .sum::<f64>()
        / samples.len().max(1) as f64
}

fn best_us(samples: &[Sample]) -> u128 {
    samples
        .iter()
        .map(|sample| sample.elapsed_us)
        .min()
        .unwrap_or(0)
}

fn median_us(samples: &[Sample]) -> u128 {
    let mut values: Vec<_> = samples.iter().map(|sample| sample.elapsed_us).collect();
    values.sort_unstable();
    values
        .get(values.len().saturating_sub(1) / 2)
        .copied()
        .unwrap_or(0)
}

fn assert_completed(samples: &[Sample], expected: u64, label: &str) {
    for sample in samples {
        if let Some(completed) = sample.completed {
            assert_eq!(completed, expected, "{label} benchmark job count mismatch");
        }
    }
}

fn print_summary(label: &str, samples: &[Sample]) {
    println!(
        "{label}: best {:.3} ms | median {:.3} ms | avg {:.3} ms",
        best_us(samples) as f64 / 1000.0,
        median_us(samples) as f64 / 1000.0,
        average_us(samples) / 1000.0,
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let jobs_per_round = parse_arg(&args, 1, 32_768usize);
    let rounds = parse_arg(&args, 2, 16usize);
    let threads = parse_arg(&args, 3, 4usize);
    let queue_capacity = parse_arg(&args, 4, jobs_per_round + 1_024);
    let samples = parse_arg(&args, 5, 5usize);
    let warmups = parse_arg(&args, 6, 1usize);

    let dispatcher = Dispatcher::new(JobSystemConfig {
        num_threads: threads,
        queue_capacity,
        worker_configs: Vec::new(),
    });

    let total_jobs = (jobs_per_round * rounds) as u64;
    println!("leet_jobs waiting-list benchmark");
    println!("threads: {}", threads);
    println!("queue capacity: {}", queue_capacity);
    println!("jobs/round: {}", jobs_per_round);
    println!("rounds: {}", rounds);
    println!("warmups: {}", warmups);
    println!("samples: {}", samples);
    println!("expected jobs: {}", total_jobs);

    for work in JobWork::ALL {
        let direct_state = Arc::new(BenchState::default());
        let gated_state = Arc::new(BenchState::default());
        let direct_job = make_job_decl(&direct_state, work);
        let gated_job = make_job_decl(&gated_state, work);

        for _ in 0..warmups {
            let _ = run_direct(&dispatcher, direct_job, work, jobs_per_round, rounds);
            let _ = run_gated(&dispatcher, gated_job, work, jobs_per_round, rounds);
        }

        let mut direct_samples = Vec::with_capacity(samples);
        let mut gated_samples = Vec::with_capacity(samples);
        for sample_index in 0..samples {
            if sample_index % 2 == 0 {
                direct_samples.push(run_direct(
                    &dispatcher,
                    direct_job,
                    work,
                    jobs_per_round,
                    rounds,
                ));
                gated_samples.push(run_gated(
                    &dispatcher,
                    gated_job,
                    work,
                    jobs_per_round,
                    rounds,
                ));
            } else {
                gated_samples.push(run_gated(
                    &dispatcher,
                    gated_job,
                    work,
                    jobs_per_round,
                    rounds,
                ));
                direct_samples.push(run_direct(
                    &dispatcher,
                    direct_job,
                    work,
                    jobs_per_round,
                    rounds,
                ));
            }
        }

        assert_completed(&direct_samples, total_jobs, "direct");
        assert_completed(&gated_samples, total_jobs, "gated");

        let direct_best = best_us(&direct_samples);
        let gated_best = best_us(&gated_samples);

        println!();
        println!("work: {}", work.label());
        print_summary("direct", &direct_samples);
        print_summary("gated ", &gated_samples);
        println!(
            "gated/direct best: {:.2}x",
            gated_best as f64 / direct_best.max(1) as f64
        );
    }
}
