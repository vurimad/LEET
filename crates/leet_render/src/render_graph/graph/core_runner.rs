//! Graph-core frame execution runner.
//!
//! This runner proves the built graph lifecycle without becoming the final
//! frame renderer. It owns the small per-frame state needed to drive allocator
//! phases, command recorder preparation, node execution, terminal completion,
//! and cleanup in the expected order.

use leet_jobs2::Builder as RenderJobBuilder;

use super::{
    execute_graph_dependency_counter_consume, execute_graph_sequential_gpu_order,
    FinalRenderNodeGraph, FrameCommandRecorders, FrameCommandSubmission, RenderGraphError,
    RenderGraphResult, RenderNodeProcessReport, RenderNodeProcessState,
};
use crate::render_graph::resources::{FrameResourceAllocator, ResourceAllocatorPhase};

/// Execution state for the graph-core runner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderGraphCoreRunnerState {
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
pub trait RenderGraphCoreRunnerHooks {
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
pub struct NoopRenderGraphCoreRunnerHooks;

impl RenderGraphCoreRunnerHooks for NoopRenderGraphCoreRunnerHooks {}

/// Report produced by one graph-core run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderGraphCoreRunReport {
    pub phase_order: Vec<ResourceAllocatorPhase>,
    pub command_recorder_slots_prepared: usize,
    pub graph_ready_hook_completed: bool,
    pub frame_data_hook_completed: bool,
    pub preconsume_reports: Vec<RenderNodeProcessReport>,
    pub consume_report: super::RenderGraphDependencyExecutionReport,
    pub command_submissions: Vec<FrameCommandSubmission>,
    pub terminal_completed_before_cleanup: bool,
    pub cleanup_ran: bool,
}

/// Small execution harness for built render node graphs.
#[derive(Default)]
pub struct RenderGraphCoreRunner {
    state: RenderGraphCoreRunnerState,
    allocator: FrameResourceAllocator,
    command_recorders: FrameCommandRecorders,
    process_state: RenderNodeProcessState,
    completed_frames: u64,
}

impl RenderGraphCoreRunner {
    /// Creates an idle runner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the runner lifecycle state.
    pub fn state(&self) -> RenderGraphCoreRunnerState {
        self.state
    }

    /// Returns how many frame executions reached the runner epilogue.
    pub fn completed_frames(&self) -> u64 {
        self.completed_frames
    }

    /// Returns the frame resource allocator owned by the last run.
    pub fn allocator(&self) -> &FrameResourceAllocator {
        &self.allocator
    }

    /// Returns command recorder state owned by the last run.
    pub fn command_recorders(&self) -> &FrameCommandRecorders {
        &self.command_recorders
    }

    /// Returns node process state owned by the runner.
    pub fn process_state(&self) -> &RenderNodeProcessState {
        &self.process_state
    }

    /// Begins an exclusive execution view.
    ///
    /// This is exposed for staged callers that need to prove the no-overlap
    /// invariant before the full frame renderer exists.
    pub fn begin_execution_view(&mut self) -> RenderGraphResult<()> {
        if self.state == RenderGraphCoreRunnerState::Running {
            return Err(RenderGraphError::InvalidState {
                reason: "graph-core runner execution view is already active",
            });
        }

        self.state = RenderGraphCoreRunnerState::Running;
        Ok(())
    }

    /// Ends an exclusive execution view.
    pub fn end_execution_view(&mut self) -> RenderGraphResult<()> {
        if self.state != RenderGraphCoreRunnerState::Running {
            return Err(RenderGraphError::InvalidState {
                reason: "graph-core runner execution view is not active",
            });
        }

        self.state = RenderGraphCoreRunnerState::Finished;
        Ok(())
    }

    /// Executes a built graph with no-op lifecycle hooks.
    pub fn execute_built_graph(
        &mut self,
        graph: &FinalRenderNodeGraph,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<RenderGraphCoreRunReport> {
        self.execute_built_graph_with_hooks(graph, jobs, &mut NoopRenderGraphCoreRunnerHooks)
    }

    /// Executes a built graph through the graph-core lifecycle.
    pub fn execute_built_graph_with_hooks(
        &mut self,
        graph: &FinalRenderNodeGraph,
        jobs: &mut RenderJobBuilder,
        hooks: &mut dyn RenderGraphCoreRunnerHooks,
    ) -> RenderGraphResult<RenderGraphCoreRunReport> {
        self.begin_execution_view()?;
        let result = self.execute_built_graph_inner(graph, jobs, hooks);
        self.state = RenderGraphCoreRunnerState::Finished;
        result
    }

    fn execute_built_graph_inner(
        &mut self,
        graph: &FinalRenderNodeGraph,
        jobs: &mut RenderJobBuilder,
        hooks: &mut dyn RenderGraphCoreRunnerHooks,
    ) -> RenderGraphResult<RenderGraphCoreRunReport> {
        if !graph.graph().is_built() {
            return Err(RenderGraphError::InvalidState {
                reason: "render graph must be built before core runner execution",
            });
        }

        self.allocator = FrameResourceAllocator::new();
        self.command_recorders = FrameCommandRecorders::default();
        self.process_state = RenderNodeProcessState::new();

        let mut phase_order = vec![ResourceAllocatorPhase::Startup];

        hooks.after_graph_build_merge(graph)?;
        let graph_ready_hook_completed = true;

        self.command_recorders = FrameCommandRecorders::prepare_for_graph(graph.graph())?;
        let command_recorder_slots_prepared = self.command_recorders.len();

        hooks.prepare_frame_data(graph)?;
        let frame_data_hook_completed = true;

        self.allocator
            .set_phase(ResourceAllocatorPhase::PreConsume)?;
        phase_order.push(ResourceAllocatorPhase::PreConsume);
        let preconsume_reports = execute_graph_sequential_gpu_order(
            graph.graph(),
            graph.impl_store(),
            graph.command_group_store(),
            &mut self.process_state,
            &mut self.allocator,
            jobs,
        )?;

        self.allocator.set_phase(ResourceAllocatorPhase::Resolve)?;
        phase_order.push(ResourceAllocatorPhase::Resolve);

        self.allocator.set_phase(ResourceAllocatorPhase::Consume)?;
        phase_order.push(ResourceAllocatorPhase::Consume);
        let consume_report = execute_graph_dependency_counter_consume(
            graph.graph(),
            graph.impl_store(),
            graph.command_group_store(),
            &mut self.process_state,
            &mut self.allocator,
            jobs,
            Some(&mut self.command_recorders),
        )?;
        let terminal_completed_before_cleanup = consume_report.terminal_completed;

        let command_submissions = self
            .command_recorders
            .submit_finished_in_gpu_order(graph.graph())?
            .to_vec();

        self.allocator.set_phase(ResourceAllocatorPhase::Cleanup)?;
        phase_order.push(ResourceAllocatorPhase::Cleanup);
        self.command_recorders.cleanup();
        self.completed_frames = self.completed_frames.saturating_add(1);

        Ok(RenderGraphCoreRunReport {
            phase_order,
            command_recorder_slots_prepared,
            graph_ready_hook_completed,
            frame_data_hook_completed,
            preconsume_reports,
            consume_report,
            command_submissions,
            terminal_completed_before_cleanup,
            cleanup_ran: true,
        })
    }
}
