//! Built graph execution facade.

use std::sync::Arc;

use leet_jobs2::{
    Builder as RenderJobBuilder, Counter as RenderJobCounter, Priority,
    RunContext as RenderJobRunContext,
};

use super::{
    FinalRenderNodeGraph, FrameCommandRecorders, FrameCommandSubmission, FrameExecutionRuntime,
    RenderGraphError, RenderGraphResult, RenderNodeFrameContextInit, RenderNodeProcessReport,
    RenderNodeProcessState,
};
use crate::{
    render_graph::resources::{RenderResourceAllocator, ResourceAllocatorPhase},
    FrameInput, PersistentRenderSceneDataRegistry, RenderSceneId,
};

/// Execution state for the graph-core runner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderGraphExecutorState {
    /// No graph execution is currently active.
    #[default]
    Idle,
    /// A frame execution view is active and cannot be entered again.
    Running,
    /// The previous execution completed or failed and the runner can be reused.
    Finished,
}

/// Optional extension points around the core graph execution lifecycle.
///
/// These hooks are deliberately narrow. They mark where graph build/import/merge
/// integration and camera/frame custom data preparation will attach later while
/// keeping the runner independent from the final frame renderer.
pub trait RenderGraphExecutorHooks {
    /// Called after the graph has been verified built and before recorder prep.
    fn after_graph_build_merge(&mut self, _graph: &FinalRenderNodeGraph) -> RenderGraphResult<()> {
        Ok(())
    }

    /// Called after command recorder storage is prepared and before preconsume.
    fn prepare_frame_data(&mut self, _graph: &FinalRenderNodeGraph) -> RenderGraphResult<()> {
        Ok(())
    }
}

/// No-op runner hooks.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopRenderGraphExecutorHooks;

impl RenderGraphExecutorHooks for NoopRenderGraphExecutorHooks {}

/// Report produced by one graph-core run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderGraphExecutionReport {
    pub phase_order: Vec<ResourceAllocatorPhase>,
    pub command_recorder_slots_prepared: usize,
    pub graph_ready_hook_completed: bool,
    pub frame_data_hook_completed: bool,
    pub camera_data_prepared: bool,
    pub preconsume_reports: Vec<RenderNodeProcessReport>,
    pub consume_report: super::RenderGraphDependencyExecutionReport,
    pub command_submissions: Vec<FrameCommandSubmission>,
    pub terminal_completed_before_cleanup: bool,
    pub cleanup_ran: bool,
}

pub struct RenderGraphExecutionInput<'a> {
    pub graph: Arc<FinalRenderNodeGraph>,
    // Temporary boundary leak: graph execution currently receives the
    // renderer-level FrameInput. This is intentionally not final API. Replace
    // it with a graph-facing frame payload/view once render nodes have a
    // precise frame-data contract.
    pub frame: &'a FrameInput,
    pub dispatcher_thread_index: u32,
    pub allocator: &'a mut RenderResourceAllocator,
    pub scene_registry: &'a mut PersistentRenderSceneDataRegistry,
    pub scene_id: RenderSceneId,
    pub builder: RenderJobBuilder,
    pub external_kickoff_wait_counter: Option<&'a RenderJobCounter>,
}

fn run_render_node_jobs(
    run_context: &RenderJobRunContext,
    _graph: Arc<FinalRenderNodeGraph>,
    _nodes_kickoff_counter: RenderJobCounter,
) {
    let _dep_builder = run_context.create_builder();
    //UNIMPLEMENTED: run render node jobs, respecting the kickoff counter and reporting back to the runtime
}

/// Persistent facade for built render graph execution.
#[derive(Default)]
pub struct RenderGraphExecutor {
    state: RenderGraphExecutorState,
    last_command_recorders: FrameCommandRecorders,
    last_process_state: RenderNodeProcessState,
    completed_frames: u64,
}

impl RenderGraphExecutor {
    /// Creates an idle runner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the runner lifecycle state.
    pub fn state(&self) -> RenderGraphExecutorState {
        self.state
    }

    /// Returns how many frame executions reached the runner epilogue.
    pub fn completed_frames(&self) -> u64 {
        self.completed_frames
    }

    /// Returns command recorder state owned by the last run.
    pub fn command_recorders(&self) -> &FrameCommandRecorders {
        &self.last_command_recorders
    }

    /// Returns node process state owned by the runner.
    pub fn process_state(&self) -> &RenderNodeProcessState {
        &self.last_process_state
    }

    /// Begins an exclusive execution view.
    ///
    /// This is exposed for staged callers that need to prove the no-overlap
    /// invariant before the full frame renderer exists.
    pub fn begin_execution_view(&mut self) -> RenderGraphResult<()> {
        if self.state == RenderGraphExecutorState::Running {
            return Err(RenderGraphError::InvalidState {
                reason: "graph-core runner execution view is already active",
            });
        }

        self.state = RenderGraphExecutorState::Running;
        Ok(())
    }

    /// Ends an exclusive execution view.
    pub fn end_execution_view(&mut self) -> RenderGraphResult<()> {
        if self.state != RenderGraphExecutorState::Running {
            return Err(RenderGraphError::InvalidState {
                reason: "graph-core runner execution view is not active",
            });
        }

        self.state = RenderGraphExecutorState::Finished;
        Ok(())
    }

    /// Executes a built graph through one frame execution boundary.
    pub fn execute(
        &mut self,
        input: RenderGraphExecutionInput<'_>,
    ) -> RenderGraphResult<RenderGraphExecutionReport> {
        self.execute_with_hooks(input, &mut NoopRenderGraphExecutorHooks)
    }

    /// Executes a built graph through one frame execution boundary.
    pub fn execute_with_hooks(
        &mut self,
        input: RenderGraphExecutionInput<'_>,
        hooks: &mut dyn RenderGraphExecutorHooks,
    ) -> RenderGraphResult<RenderGraphExecutionReport> {
        self.begin_execution_view()?;
        let result = self.execute_inner(input, hooks);
        self.state = RenderGraphExecutorState::Finished;
        result
    }

    fn execute_inner(
        &mut self,
        input: RenderGraphExecutionInput<'_>,
        hooks: &mut dyn RenderGraphExecutorHooks,
    ) -> RenderGraphResult<RenderGraphExecutionReport> {
        let graph = input.graph;
        let frame = input.frame;
        let dispatcher_thread_index = input.dispatcher_thread_index;
        let allocator = input.allocator;
        let scene_registry = input.scene_registry;
        let scene_id = input.scene_id;
        let mut builder = input.builder;
        let _hooks = hooks;

        if !graph.graph().is_built() {
            return Err(RenderGraphError::InvalidState {
                reason: "render graph must be built before core runner execution",
            });
        }

        let mut runtime = FrameExecutionRuntime::construct(allocator);
        let mut phase_order = vec![runtime.allocator_phase()];
        let nodes_kickoff_counter =
            runtime.prepare_node_kickoff(&builder, input.external_kickoff_wait_counter);
        let exclusive_update = FinalRenderNodeGraph::acquire_exclusive_update_flag(&graph)?;

        let dispatcher = builder.dispatcher();
        let mut render_node_jobs_builder = dispatcher.create_builder(Priority::RenderPath);
        let job_graph = Arc::clone(&graph);

        {
            render_node_jobs_builder.dispatch_job("RunRenderNodeJobs", move |run_context| {
                run_render_node_jobs(run_context, job_graph, nodes_kickoff_counter);
            });
        }
        let render_node_jobs_wait_counter = render_node_jobs_builder.extract_wait_counter();

        runtime.configure_resource_eviction(frame);
        let command_recorder_slots_prepared =
            runtime.prepare_command_recorders_for_graph(graph.graph())?;
        runtime.init_node_frame_context(RenderNodeFrameContextInit::new(
            frame,
            dispatcher_thread_index,
        ));
        let camera_data_prepared = runtime.process_camera_data(scene_registry, scene_id, frame)?;
        runtime.execute_graph_preconsume(graph.graph(), &mut builder)?;
        phase_order.push(runtime.allocator_phase());
        runtime.dispatch_flow_allocator_resolve_to_consume(&mut builder);
        phase_order.push(ResourceAllocatorPhase::Resolve);
        phase_order.push(ResourceAllocatorPhase::Consume);
        runtime.dispatch_finish_node_kickoff(&mut builder)?;

        builder.dispatch_wait(&render_node_jobs_wait_counter);
        builder.dispatch_job("RunRenderNodeJobs/Cleanup", move |_run_context| {
            drop(exclusive_update);
        });

        let (command_recorders, process_state) = runtime.into_epilogue_state();
        self.last_command_recorders = command_recorders;
        self.last_process_state = process_state;
        self.completed_frames = self.completed_frames.saturating_add(1);

        Ok(RenderGraphExecutionReport {
            phase_order,
            command_recorder_slots_prepared,
            graph_ready_hook_completed: false,
            frame_data_hook_completed: false,
            camera_data_prepared,
            preconsume_reports: Vec::new(),
            consume_report: super::RenderGraphDependencyExecutionReport::default(),
            command_submissions: Vec::new(),
            terminal_completed_before_cleanup: false,
            cleanup_ran: false,
        })
    }
}
