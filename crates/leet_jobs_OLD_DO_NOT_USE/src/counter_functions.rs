use std::sync::Arc;

use crate::builder::RunContext;
use crate::dispatcher_entries::ParallelForSharedCounterEntry;
use crate::dispatcher_thread::current_dispatcher_thread_index;
use crate::job_decl::{JobDecl, JobDeclParallelFor, JobHint};
use crate::priority::Priority;
use crate::{CompletionDeferral, Counter, Dispatcher};

/// Mirror of RED's `RunJob(...)` free function.
pub fn run_job(_job: &JobDecl, _wait_for_zero_counter: &Counter, _accumulate_counter: &Counter) {
    unimplemented!(
        "[leet_jobs] TODO: resolve the dispatcher from the Bevy plugin/resource bridge; use run_job_on for now"
    );
}

/// Explicit-dispatcher variant used by tests and non-global engine bootstrap.
///
/// # Safety
///
/// `job.job_data` must remain valid until the queued job has run or the
/// dispatcher has been dropped, whichever happens first. The pointed-to data
/// must be safe to access from a worker thread, and `job.job_func` must uphold
/// Rust's aliasing and synchronization rules for that data.
pub unsafe fn run_job_on(
    dispatcher: &Dispatcher,
    job: &JobDecl,
    wait_for_zero_counter: &Counter,
    accumulate_counter: &Counter,
) {
    let job = *job;
    let param = accumulate_counter.inner().param;
    // SAFETY: guaranteed by `run_job_on`'s safety contract.
    unsafe {
        dispatcher.submit_job_decl(
            job,
            Some(wait_for_zero_counter),
            accumulate_counter,
            param.priority,
        );
    }
}

/// Mirror of RED's `RunParallelForJob(...)` free function.
pub fn run_parallel_for_job(
    _job: &JobDeclParallelFor,
    _wait_for_zero_counter: &Counter,
    _accumulate_counter: &Counter,
) {
    unimplemented!(
        "[leet_jobs] TODO: resolve the dispatcher from the Bevy plugin/resource bridge; use run_parallel_for_job_on for now"
    );
}

/// Explicit-dispatcher variant used by tests and non-global engine bootstrap.
pub fn run_parallel_for_job_on(
    dispatcher: &Dispatcher,
    job: &JobDeclParallelFor,
    wait_for_zero_counter: &Counter,
    accumulate_counter: &Counter,
) {
    let job_func = job
        .job_func
        .expect("[leet_jobs] JobDeclParallelFor::job_func must be set before dispatch");

    let max_parallel_for_team_size = dispatcher.num_dispatcher_threads() as u32 + 1;
    let team_size = calc_parallel_for_team_size(job.num_elements, 1, max_parallel_for_team_size);
    assert!(
        team_size > 0 || job.num_elements == 0,
        "[leet_jobs] invalid zero team-size calculation"
    );

    let resolved_shared_data = if let Some(init) = job.init_shared_data_callback {
        // SAFETY: mirrors RED's raw callback contract.
        unsafe { init(team_size, job.shared_data) }
    } else {
        job.shared_data
    } as usize;

    let elements = job.elements as usize;
    let num_elements = job.num_elements;
    let max_batch_size = job.max_batch_size;
    let epilogue_func = job.epilogue_func;
    let instrumentation_object = job.instrumentation_object;
    let param = accumulate_counter.inner().param;
    let debug_name = instrumentation_object.unwrap_or(accumulate_counter.inner().debug_name);

    if team_size == 0 {
        if let Some(epilogue_func) = epilogue_func {
            submit_parallel_epilogue_only(
                dispatcher,
                wait_for_zero_counter,
                accumulate_counter,
                param.priority,
                debug_name,
                instrumentation_object,
                resolved_shared_data,
                elements,
                num_elements,
                epilogue_func,
            );
        } else {
            submit_empty_job(
                dispatcher,
                wait_for_zero_counter,
                accumulate_counter,
                param.priority,
            );
        }
        return;
    }

    if team_size == 1 {
        submit_parallel_single_team(
            dispatcher,
            wait_for_zero_counter,
            accumulate_counter,
            param.priority,
            debug_name,
            instrumentation_object,
            resolved_shared_data,
            elements,
            num_elements,
            job_func,
            epilogue_func,
        );
        return;
    }

    let shared_counter = ParallelForSharedCounterEntry::new();
    for team_index in 0..team_size {
        let dispatcher_inner = Arc::clone(dispatcher.inner());
        let continuation_counter = accumulate_counter.clone();
        let shared_counter = Arc::clone(&shared_counter);

        dispatcher.submit_with_hint(
            move || {
                let run_context = RunContext::for_job(
                    &dispatcher_inner,
                    param,
                    debug_name,
                    instrumentation_object,
                    current_dispatcher_thread_index(),
                    team_index as i32,
                    Some(continuation_counter),
                );
                let epilogue_context = RunContext::for_job(
                    &dispatcher_inner,
                    param,
                    debug_name,
                    instrumentation_object,
                    current_dispatcher_thread_index(),
                    -1,
                    None,
                );

                let real_team_size = team_size;
                let effective_team_size = if max_batch_size == 0 {
                    team_size
                } else {
                    (num_elements / max_batch_size).max(1)
                };
                let batch_size = num_elements.div_ceil(effective_team_size);

                loop {
                    let index = shared_counter.next_index();
                    if index < effective_team_size {
                        let element_start_index = index * batch_size;
                        let element_end_index =
                            (element_start_index + batch_size).min(num_elements);
                        // SAFETY: mirrors RED's raw parallel-for callback contract.
                        unsafe {
                            job_func(
                                resolved_shared_data as *mut _,
                                elements as *mut _,
                                element_start_index,
                                element_end_index,
                                &run_context,
                            );
                        }
                    } else {
                        let must_release_counter_now =
                            index == effective_team_size + real_team_size - 1;
                        if must_release_counter_now {
                            if let Some(epilogue_func) = epilogue_func {
                                // SAFETY: mirrors RED's raw epilogue callback contract.
                                unsafe {
                                    epilogue_func(
                                        resolved_shared_data as *mut _,
                                        elements as *mut _,
                                        num_elements,
                                        &epilogue_context,
                                    );
                                }
                            }
                        }
                        break;
                    }
                }
            },
            Some(wait_for_zero_counter),
            accumulate_counter,
            param.priority,
            JobHint::None,
        );
    }
}

/// Mirror of RED's `CreateDeferral(...)` free function.
pub fn create_deferral(
    _debug_name: &'static str,
    _debug_user_data: Option<usize>,
    _counter: &Counter,
) -> CompletionDeferral {
    unimplemented!(
        "[leet_jobs] TODO: resolve the dispatcher from the Bevy plugin/resource bridge; use create_deferral_on for now"
    );
}

/// Explicit-dispatcher variant used by tests and non-global engine bootstrap.
pub fn create_deferral_on(
    dispatcher: &Dispatcher,
    debug_name: &'static str,
    debug_user_data: Option<usize>,
    counter: &Counter,
) -> CompletionDeferral {
    dispatcher.create_deferral_with_debug_user_data(counter, debug_name, debug_user_data)
}

/// Mirror of RED's `FlushCounter(...)` free function.
pub fn flush_counter(
    _counter: &Counter,
    _process_latent: bool,
    _timeout_milliseconds: i32,
) -> bool {
    unimplemented!(
        "[leet_jobs] TODO: resolve the dispatcher from the Bevy plugin/resource bridge; use flush_counter_on for now"
    );
}

/// Explicit-dispatcher variant used by tests and non-global engine bootstrap.
pub fn flush_counter_on(
    dispatcher: &Dispatcher,
    counter: &Counter,
    process_latent: bool,
    timeout_milliseconds: i32,
) -> bool {
    dispatcher.flush_counter(counter, process_latent, timeout_milliseconds)
}

/// Mirror of RED's `FlushCounterOnProcessFrame(...)`.
pub fn flush_counter_on_process_frame(_counter: &Counter) -> bool {
    unimplemented!(
        "[leet_jobs] TODO: resolve the dispatcher from the Bevy plugin/resource bridge; use flush_counter_on_process_frame_on for now"
    );
}

/// Explicit-dispatcher variant used by tests and non-global engine bootstrap.
pub fn flush_counter_on_process_frame_on(dispatcher: &Dispatcher, counter: &Counter) -> bool {
    let process_large_jobs = dispatcher.num_dispatcher_threads() < 3;
    dispatcher.flush_with_priority(counter, Priority::RenderPath, -1, process_large_jobs)
}

fn submit_parallel_epilogue_only(
    dispatcher: &Dispatcher,
    wait_for_zero_counter: &Counter,
    accumulate_counter: &Counter,
    priority: Priority,
    debug_name: &'static str,
    instrumentation_object: Option<&'static str>,
    resolved_shared_data: usize,
    elements: usize,
    num_elements: u32,
    epilogue_func: crate::job_decl::EpilogueFunc,
) {
    let dispatcher_inner = Arc::clone(dispatcher.inner());
    let param = accumulate_counter.inner().param;
    let continuation_counter = accumulate_counter.clone();
    dispatcher.submit_with_hint(
        move || {
            let run_context = RunContext::for_job(
                &dispatcher_inner,
                param,
                debug_name,
                instrumentation_object,
                current_dispatcher_thread_index(),
                -1,
                Some(continuation_counter),
            );
            // SAFETY: mirrors RED's raw epilogue callback contract.
            unsafe {
                epilogue_func(
                    resolved_shared_data as *mut _,
                    elements as *mut _,
                    num_elements,
                    &run_context,
                );
            }
        },
        Some(wait_for_zero_counter),
        accumulate_counter,
        priority,
        JobHint::None,
    );
}

fn submit_parallel_single_team(
    dispatcher: &Dispatcher,
    wait_for_zero_counter: &Counter,
    accumulate_counter: &Counter,
    priority: Priority,
    debug_name: &'static str,
    instrumentation_object: Option<&'static str>,
    resolved_shared_data: usize,
    elements: usize,
    num_elements: u32,
    job_func: crate::job_decl::ParallelForJobFunc,
    epilogue_func: Option<crate::job_decl::EpilogueFunc>,
) {
    let dispatcher_inner = Arc::clone(dispatcher.inner());
    let param = accumulate_counter.inner().param;
    let continuation_counter = accumulate_counter.clone();
    dispatcher.submit_with_hint(
        move || {
            let run_context = RunContext::for_job(
                &dispatcher_inner,
                param,
                debug_name,
                instrumentation_object,
                current_dispatcher_thread_index(),
                0,
                Some(continuation_counter),
            );
            // SAFETY: mirrors RED's raw parallel-for callback contract.
            unsafe {
                job_func(
                    resolved_shared_data as *mut _,
                    elements as *mut _,
                    0,
                    num_elements,
                    &run_context,
                );
            }

            if let Some(epilogue_func) = epilogue_func {
                let epilogue_context = RunContext::for_job(
                    &dispatcher_inner,
                    param,
                    debug_name,
                    instrumentation_object,
                    current_dispatcher_thread_index(),
                    -1,
                    None,
                );
                // SAFETY: mirrors RED's raw epilogue callback contract.
                unsafe {
                    epilogue_func(
                        resolved_shared_data as *mut _,
                        elements as *mut _,
                        num_elements,
                        &epilogue_context,
                    );
                }
            }
        },
        Some(wait_for_zero_counter),
        accumulate_counter,
        priority,
        JobHint::None,
    );
}

fn submit_empty_job(
    dispatcher: &Dispatcher,
    wait_for_zero_counter: &Counter,
    accumulate_counter: &Counter,
    priority: Priority,
) {
    dispatcher.submit_with_hint(
        || {},
        Some(wait_for_zero_counter),
        accumulate_counter,
        priority,
        JobHint::Trivial,
    );
}

fn calc_parallel_for_team_size(
    num_elements: u32,
    num_elements_per_batch: u32,
    num_workers: u32,
) -> u32 {
    let num_batches = num_elements.div_ceil(num_elements_per_batch);
    num_batches.min(num_workers)
}
