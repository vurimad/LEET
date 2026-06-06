use std::sync::{Arc, Mutex};

use leet_jobs2::{Builder as RenderJobBuilder, JobSystemConfig, LeetJobSystem, Priority};

use super::super::{
    FinalRenderNodeGraph, FrameCommandRecorderState, RenderFlowGroup, RenderGraphCoreRunner,
    RenderGraphCoreRunnerHooks, RenderGraphCoreRunnerState, RenderGraphError, RenderGraphResult,
    RenderNodeCommandListUsage, RenderNodeDependencyKind, RenderNodeGraphFactory, RenderNodeImpl,
    RenderNodeImplContext, RenderNodeKind, RenderNodeSubtype, ResourceAllocatorPhase,
};

#[derive(Clone)]
struct PhaseLogNode {
    name: &'static str,
    log: Arc<Mutex<Vec<String>>>,
}

impl PhaseLogNode {
    fn new(name: &'static str, log: Arc<Mutex<Vec<String>>>) -> Self {
        Self { name, log }
    }
}

impl RenderNodeImpl for PhaseLogNode {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        self.log.lock().unwrap().push(format!(
            "{}:{:?}:runtime={}",
            self.name,
            rctx.resource_phase(),
            rctx.has_frame_runtime()
        ));
        Ok(())
    }
}

struct LoggingHooks {
    log: Arc<Mutex<Vec<String>>>,
}

impl LoggingHooks {
    fn new(log: Arc<Mutex<Vec<String>>>) -> Self {
        Self { log }
    }
}

impl RenderGraphCoreRunnerHooks for LoggingHooks {
    fn after_graph_build_merge(&mut self, graph: &FinalRenderNodeGraph) -> RenderGraphResult<()> {
        assert!(graph.graph().is_built());
        self.log.lock().unwrap().push("hook:graph".to_owned());
        Ok(())
    }

    fn prepare_frame_data(&mut self, _graph: &FinalRenderNodeGraph) -> RenderGraphResult<()> {
        self.log.lock().unwrap().push("hook:frame".to_owned());
        Ok(())
    }
}

struct JobHarness {
    jobs: LeetJobSystem,
}

impl JobHarness {
    fn new() -> Self {
        Self {
            jobs: LeetJobSystem::new(JobSystemConfig {
                max_threads: 1,
                ..JobSystemConfig::default()
            }),
        }
    }

    fn builder(&self) -> RenderJobBuilder {
        self.jobs.create_builder(Priority::RenderPath)
    }
}

impl Drop for JobHarness {
    fn drop(&mut self) {
        self.jobs.shutdown();
    }
}

fn build_two_node_graph(log: Arc<Mutex<Vec<String>>>) -> (FinalRenderNodeGraph, RenderFlowGroup) {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let first = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            PhaseLogNode::new("first", Arc::clone(&log)),
        )
        .unwrap();
    let second = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            PhaseLogNode::new("second", Arc::clone(&log)),
        )
        .unwrap();
    factory
        .link_nodes(first, second, RenderNodeDependencyKind::Cpu)
        .unwrap();
    factory
        .link_nodes(first, second, RenderNodeDependencyKind::Gpu)
        .unwrap();

    let built = factory.finish().unwrap();
    let first_flow_group = built
        .graph()
        .node(first)
        .unwrap()
        .metadata()
        .flow_group(RenderNodeDependencyKind::Gpu)
        .unwrap();
    (built, first_flow_group)
}

#[test]
fn core_runner_drives_allocator_phases_and_node_passes() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (built, _) = build_two_node_graph(Arc::clone(&log));
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut runner = RenderGraphCoreRunner::new();

    let report = runner.execute_built_graph(&built, &mut builder).unwrap();

    assert_eq!(
        report.phase_order,
        vec![
            ResourceAllocatorPhase::Startup,
            ResourceAllocatorPhase::PreConsume,
            ResourceAllocatorPhase::Resolve,
            ResourceAllocatorPhase::Consume,
            ResourceAllocatorPhase::Cleanup,
        ]
    );
    assert_eq!(
        log.lock().unwrap().as_slice(),
        &[
            "first:PreConsume:runtime=false".to_owned(),
            "second:PreConsume:runtime=false".to_owned(),
            "first:Consume:runtime=true".to_owned(),
            "second:Consume:runtime=true".to_owned(),
        ]
    );
    assert_eq!(report.preconsume_reports.len(), 2);
    assert_eq!(report.consume_report.completed_jobs, 2);
    assert!(report.consume_report.terminal_completed);
    assert!(report.terminal_completed_before_cleanup);
    assert!(report.cleanup_ran);
    assert_eq!(runner.allocator().phase(), ResourceAllocatorPhase::Cleanup);
    assert_eq!(runner.completed_frames(), 1);
}

#[test]
fn core_runner_rejects_overlapping_execution_view() {
    let mut runner = RenderGraphCoreRunner::new();

    runner.begin_execution_view().unwrap();
    assert_eq!(runner.state(), RenderGraphCoreRunnerState::Running);

    assert!(matches!(
        runner.begin_execution_view(),
        Err(RenderGraphError::InvalidState { reason })
            if reason.contains("already active")
    ));

    runner.end_execution_view().unwrap();
    assert_eq!(runner.state(), RenderGraphCoreRunnerState::Finished);
}

#[test]
fn core_runner_prepares_recorders_and_cleans_epilogue_state() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (built, first_flow_group) = build_two_node_graph(log);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut runner = RenderGraphCoreRunner::new();

    let report = runner.execute_built_graph(&built, &mut builder).unwrap();

    assert_eq!(
        report.command_recorder_slots_prepared,
        built.graph().node_count()
    );
    assert!(report.command_submissions.is_empty());
    assert_eq!(runner.command_recorders().len(), built.graph().node_count());
    assert_eq!(
        runner.command_recorders().state(first_flow_group).unwrap(),
        FrameCommandRecorderState::Empty
    );
    assert!(runner.command_recorders().submissions().is_empty());
    assert_eq!(runner.allocator().request_group_count(), 0);
}

#[test]
fn core_runner_hooks_run_before_preconsume() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (built, _) = build_two_node_graph(Arc::clone(&log));
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut runner = RenderGraphCoreRunner::new();
    let mut hooks = LoggingHooks::new(Arc::clone(&log));

    let report = runner
        .execute_built_graph_with_hooks(&built, &mut builder, &mut hooks)
        .unwrap();

    assert!(report.graph_ready_hook_completed);
    assert!(report.frame_data_hook_completed);
    assert_eq!(
        &log.lock().unwrap()[..4],
        &[
            "hook:graph".to_owned(),
            "hook:frame".to_owned(),
            "first:PreConsume:runtime=false".to_owned(),
            "second:PreConsume:runtime=false".to_owned(),
        ]
    );
}
