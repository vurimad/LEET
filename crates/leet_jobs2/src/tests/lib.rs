use std::time::Duration;

use super::{
    Builder, CompletionDeferral, Counter, Fence, JobHint, JobSystemConfig, LeetJobSystem, Priority,
    RunContext, ScheduleParam,
};

#[test]
fn crate_root_reexports_public_types() {
    let _config = JobSystemConfig::default();
    let _param = ScheduleParam::default();
    let _priority = Priority::CriticalPath;
    let _hint = JobHint::None;
    let _fence = Fence::Full;
    let _current_thread = LeetJobSystem::current_thread_index();

    fn accepts_builder_type(_: Option<&Builder>) {}
    fn accepts_deferral_type(_: Option<&CompletionDeferral>) {}
    fn accepts_counter_type(_: Option<&Counter>) {}
    fn accepts_run_context_type(_: Option<&RunContext>) {}
    accepts_builder_type(None);
    accepts_deferral_type(None);
    accepts_counter_type(None);
    accepts_run_context_type(None);
}

#[test]
fn public_api_surface_matches_v1_scope() {
    type JobFn = fn(&RunContext);
    type RangeFn = fn(u32, u32, &RunContext);

    let _: fn() -> JobSystemConfig = JobSystemConfig::default;
    let _: fn() -> JobSystemConfig = JobSystemConfig::editor;
    let _: fn() -> JobSystemConfig = JobSystemConfig::tool;

    let _: fn(JobSystemConfig) -> LeetJobSystem = LeetJobSystem::new;
    let _: fn(&LeetJobSystem) = LeetJobSystem::shutdown;
    let _: fn(&LeetJobSystem) = LeetJobSystem::claim_flush_thread;
    let _: fn(&LeetJobSystem) -> usize = LeetJobSystem::num_worker_threads;
    let _: fn() -> Option<u32> = LeetJobSystem::current_thread_index;
    let _: fn(&LeetJobSystem, Priority) -> Counter = LeetJobSystem::create_counter;
    let _: fn(&LeetJobSystem, Priority) -> Builder = LeetJobSystem::create_builder;
    let _: fn(&LeetJobSystem, &RunContext) -> Builder = LeetJobSystem::create_builder_from_context;
    let _: fn(&LeetJobSystem, &Counter) -> bool = LeetJobSystem::flush_counter;
    let _: fn(&LeetJobSystem, &Counter, Duration) -> bool =
        LeetJobSystem::flush_counter_with_timeout;
    let _: fn(&LeetJobSystem, &Counter) -> bool = LeetJobSystem::flush_counter_render_frame;

    let _: fn(&mut Builder, &'static str, JobFn) = Builder::dispatch_job::<JobFn>;
    let _: fn(&mut Builder, &'static str, JobHint, JobFn) =
        Builder::dispatch_job_with_hint::<JobFn>;
    let _: fn(&mut Builder, &'static str, JobFn) = Builder::dispatch_job_no_fence::<JobFn>;
    let _: fn(&mut Builder, &'static str, u32, RangeFn) = Builder::dispatch_parallel_for::<RangeFn>;
    let _: fn(&mut Builder, &'static str, u32, RangeFn) =
        Builder::dispatch_parallel_for_no_fence::<RangeFn>;
    let _: fn(&mut Builder, &'static str, u32, u32, RangeFn) =
        Builder::dispatch_parallel_for_with_max_batch_size::<RangeFn>;
    let _: fn(&mut Builder, &'static str, u32, RangeFn, JobFn) =
        Builder::dispatch_parallel_for_with_epilogue::<RangeFn, JobFn>;
    let _: fn(&mut Builder, &'static str, u32, RangeFn, JobFn) =
        Builder::dispatch_parallel_for_with_epilogue_no_fence::<RangeFn, JobFn>;
    let _: fn(&mut Builder, &Counter) = Builder::dispatch_wait;
    let _: fn(&mut Builder) = Builder::dispatch_fence_explicitly;
    let _: fn(&mut Builder) -> Counter = Builder::extract_wait_counter;

    let _: fn(&Counter, &'static str) -> CompletionDeferral = Counter::create_deferral;
    let _: fn(&mut Counter, Counter) = Counter::reset;
    let _: fn(&Counter) -> bool = Counter::is_zero;

    let _: fn(&mut CompletionDeferral) = CompletionDeferral::finish;
    let _: fn(&CompletionDeferral) -> &'static str = CompletionDeferral::name;

    fn assert_job_hint_scope(hint: JobHint) -> &'static str {
        match hint {
            JobHint::None => "none",
            JobHint::Trivial => "trivial",
            JobHint::Large => "large",
        }
    }

    fn assert_priority_scope(priority: Priority) -> u8 {
        match priority {
            Priority::Latent => 0,
            Priority::RenderPath => 1,
            Priority::CriticalPath => 2,
            Priority::Immediate => 3,
        }
    }

    assert_eq!(assert_job_hint_scope(JobHint::Large), "large");
    assert_eq!(assert_priority_scope(Priority::Immediate), 3);
}
