use std::sync::{Arc, Mutex};

use leet_jobs2::{Builder as RenderJobBuilder, JobSystemConfig, LeetJobSystem, Priority};

use crate::{
    FrameCaptureIntent, FrameDebugIntent, FrameGpuScene, FrameInput, FrameOutput, FramePurpose,
    FrameRenderingMode, FrameTiming, PersistentRenderSceneDataRegistry, PreparedFrameViews,
    PresentationIntent, RenderGraphExecutionInput, RenderSceneId, RenderViewport,
};

use super::super::{
    FinalRenderNodeGraph, RenderFlowGroup, RenderGraphError, RenderGraphExecutor,
    RenderGraphExecutorHooks, RenderGraphExecutorState, RenderGraphResult,
    RenderNodeCommandListUsage, RenderNodeDependencyKind, RenderNodeGraphFactory, RenderNodeImpl,
    RenderNodeImplContext, RenderNodeKind, RenderNodeSubtype, RenderResourceAllocator,
    ResourceAllocatorPhase,
};
use bevy_math::UVec2;

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

impl RenderGraphExecutorHooks for LoggingHooks {
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

fn test_frame() -> FrameInput {
    FrameInput {
        viewport: RenderViewport::targetless(
            UVec2::new(128, 72),
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ),
        output: FrameOutput::Targetless,
        scene_id: RenderSceneId::default(),
        cameras: PreparedFrameViews::default(),
        scene: FrameGpuScene::empty(),
        timing: FrameTiming::default(),
        mode: FrameRenderingMode::Blank,
        purpose: FramePurpose::Blank,
        presentation: PresentationIntent::NoPresent,
        capture: FrameCaptureIntent::None,
        debug: FrameDebugIntent::default(),
    }
}

#[test]
fn graph_executor_constructs_frame_runtime_without_running_graph_work() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (built, _) = build_two_node_graph(Arc::clone(&log));
    let jobs = JobHarness::new();
    let builder = jobs.builder();
    let mut runner = RenderGraphExecutor::new();
    let mut allocator = RenderResourceAllocator::new();
    let mut scene_registry = PersistentRenderSceneDataRegistry::new();
    let frame = test_frame();

    let report = runner
        .execute(RenderGraphExecutionInput {
            graph: Arc::new(built),
            frame: &frame,
            dispatcher_thread_index: 0,
            allocator: &mut allocator,
            scene_registry: &mut scene_registry,
            scene_id: frame.scene_id,
            builder,
            external_kickoff_wait_counter: None,
        })
        .unwrap();

    assert_eq!(
        report.phase_order,
        vec![
            ResourceAllocatorPhase::Startup,
            ResourceAllocatorPhase::PreConsume,
            ResourceAllocatorPhase::Resolve,
            ResourceAllocatorPhase::Consume
        ]
    );
    assert!(log.lock().unwrap().is_empty());
    assert!(report.preconsume_reports.is_empty());
    assert_eq!(report.consume_report.completed_jobs, 0);
    assert!(!report.consume_report.terminal_completed);
    assert!(!report.terminal_completed_before_cleanup);
    assert!(!report.cleanup_ran);
    assert_eq!(allocator.phase(), ResourceAllocatorPhase::Consume);
    assert_eq!(runner.completed_frames(), 1);
}

#[test]
fn graph_executor_rejects_overlapping_execution_view() {
    let mut runner = RenderGraphExecutor::new();

    runner.begin_execution_view().unwrap();
    assert_eq!(runner.state(), RenderGraphExecutorState::Running);

    assert!(matches!(
        runner.begin_execution_view(),
        Err(RenderGraphError::InvalidState { reason })
            if reason.contains("already active")
    ));

    runner.end_execution_view().unwrap();
    assert_eq!(runner.state(), RenderGraphExecutorState::Finished);
}

#[test]
fn graph_executor_keeps_empty_epilogue_state_until_lifecycle_steps_are_added() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (built, _) = build_two_node_graph(log);
    let jobs = JobHarness::new();
    let builder = jobs.builder();
    let mut runner = RenderGraphExecutor::new();
    let mut allocator = RenderResourceAllocator::new();
    let mut scene_registry = PersistentRenderSceneDataRegistry::new();
    let frame = test_frame();

    let report = runner
        .execute(RenderGraphExecutionInput {
            graph: Arc::new(built),
            frame: &frame,
            dispatcher_thread_index: 0,
            allocator: &mut allocator,
            scene_registry: &mut scene_registry,
            scene_id: frame.scene_id,
            builder,
            external_kickoff_wait_counter: None,
        })
        .unwrap();

    assert_eq!(report.command_recorder_slots_prepared, 2);
    assert!(report.command_submissions.is_empty());
    assert_eq!(runner.command_recorders().len(), 2);
    assert!(runner.command_recorders().submissions().is_empty());
    assert_eq!(allocator.request_group_count(), 0);
}

#[test]
fn graph_executor_hooks_are_deferred_until_matching_lifecycle_steps_exist() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (built, _) = build_two_node_graph(Arc::clone(&log));
    let jobs = JobHarness::new();
    let builder = jobs.builder();
    let mut runner = RenderGraphExecutor::new();
    let mut hooks = LoggingHooks::new(Arc::clone(&log));
    let mut allocator = RenderResourceAllocator::new();
    let mut scene_registry = PersistentRenderSceneDataRegistry::new();
    let frame = test_frame();

    let report = runner
        .execute_with_hooks(
            RenderGraphExecutionInput {
                graph: Arc::new(built),
                frame: &frame,
                dispatcher_thread_index: 0,
                allocator: &mut allocator,
                scene_registry: &mut scene_registry,
                scene_id: frame.scene_id,
                builder,
                external_kickoff_wait_counter: None,
            },
            &mut hooks,
        )
        .unwrap();

    assert!(!report.graph_ready_hook_completed);
    assert!(!report.frame_data_hook_completed);
    assert!(log.lock().unwrap().is_empty());
}
