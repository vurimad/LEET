//! Frame-local execution state for a built render graph.

use super::{
    FrameCommandRecorders, RenderGraphError, RenderGraphResult, RenderNodeFrameContextInit,
    RenderNodeGraph, RenderNodeProcessState,
};
use crate::render_graph::resources::{RenderResourceAllocator, ResourceAllocatorPhase};
use crate::{
    FrameCustomDataPrepareContext, FrameInput, PersistentRenderSceneDataRegistry,
    PreparedFrameSceneData, RenderSceneId,
};
use leet_jobs2::{Builder as RenderJobBuilder, CompletionDeferral, Counter as RenderJobCounter};

struct RenderGraphNodeKickoff {
    deferral: CompletionDeferral,
}

pub struct FrameExecutionRuntime<'a, 'frame> {
    allocator: &'a mut RenderResourceAllocator,
    command_recorders: FrameCommandRecorders,
    process_state: RenderNodeProcessState,
    node_frame_context: Option<RenderNodeFrameContextInit<'frame>>,
    prepared_custom_data: Option<PreparedFrameSceneData>,
    _node_kickoff: Option<RenderGraphNodeKickoff>,
}

impl<'a, 'frame> FrameExecutionRuntime<'a, 'frame> {
    pub fn construct(allocator: &'a mut RenderResourceAllocator) -> Self {
        allocator.reset_for_frame();
        Self {
            allocator,
            command_recorders: FrameCommandRecorders::default(),
            process_state: RenderNodeProcessState::new(),
            node_frame_context: None,
            prepared_custom_data: None,
            _node_kickoff: None,
        }
    }

    pub fn allocator_phase(&self) -> ResourceAllocatorPhase {
        self.allocator.phase()
    }

    pub fn configure_resource_eviction(&mut self, frame: &FrameInput) -> bool {
        let is_blank_frame = frame.cameras.is_empty() || frame.purpose.is_blank();
        let process_eviction = !is_blank_frame;
        self.allocator.set_process_eviction(process_eviction);
        process_eviction
    }

    pub fn execute_graph_preconsume(
        &mut self,
        graph: &RenderNodeGraph,
        builder: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        self.allocator
            .set_phase(ResourceAllocatorPhase::PreConsume)
            .map_err(RenderGraphError::from)?;
        let frame_context = self
            .node_frame_context
            .ok_or(RenderGraphError::InvalidState {
                reason: "node frame context is not initialized",
            })?;
        graph.execute_parallel(frame_context, builder)
    }

    pub fn dispatch_flow_allocator_resolve_to_consume(
        &mut self,
        builder: &mut RenderJobBuilder,
    ) {
        let allocator = self.allocator.clone();
        builder.dispatch_job("FlowAllocator_Resolve", move |run_context| {
            let _dep_builder = run_context.create_builder();
            allocator
                .set_phase(ResourceAllocatorPhase::Resolve)
                .expect("flow allocator failed to enter resolve phase");
            allocator
                .set_phase(ResourceAllocatorPhase::Consume)
                .expect("flow allocator failed to enter consume phase");
        });
    }

    pub fn prepare_command_recorders_for_graph(
        &mut self,
        graph: &super::RenderNodeGraph,
    ) -> super::RenderGraphResult<usize> {
        self.command_recorders = FrameCommandRecorders::prepare_for_graph(graph)?;
        Ok(self.command_recorders.len())
    }

    pub fn prepare_node_kickoff(
        &mut self,
        jobs: &RenderJobBuilder,
        external_wait: Option<&RenderJobCounter>,
    ) -> RenderJobCounter {
        let mut counter = jobs.create_counter("RenderGraph/NodesKickoff");
        if let Some(external_wait) = external_wait {
            counter += external_wait;
        }

        let deferral = counter.create_deferral("RenderGraph/NodesKickoffDeferral");
        self._node_kickoff = Some(RenderGraphNodeKickoff { deferral });
        counter
    }

    pub fn dispatch_finish_node_kickoff(
        &mut self,
        builder: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        let mut kickoff = self
            ._node_kickoff
            .take()
            .ok_or(RenderGraphError::InvalidState {
                reason: "node kickoff deferral is not active",
            })?;

        builder.dispatch_job("FlowAllocator_Resolve_Finish", move |_run_context| {
            kickoff.deferral.finish();
        });
        Ok(())
    }

    pub fn init_node_frame_context(&mut self, init: RenderNodeFrameContextInit<'frame>) {
        debug_assert_ne!(
            init.dispatcher_thread_index,
            u32::MAX,
            "node frame context dispatcher thread index is uninitialized"
        );
        let _ = init.frame.viewport.extent();
        self.node_frame_context = Some(init);
    }

    pub fn process_camera_data(
        &mut self,
        scene_registry: &mut PersistentRenderSceneDataRegistry,
        scene_id: RenderSceneId,
        frame: &FrameInput,
    ) -> RenderGraphResult<bool> {
        if frame.cameras.is_empty() {
            return Ok(false);
        }

        let frame_context = self
            .node_frame_context
            .ok_or(RenderGraphError::InvalidState {
                reason: "node frame context is not initialized",
            })?;
        let ctx = FrameCustomDataPrepareContext {
            scene_id,
            timing: frame.timing,
            mode: frame.mode,
            purpose: frame.purpose,
            debug: frame.debug,
            viewport_extent: frame.viewport.extent(),
            dispatcher_thread_index: frame_context.dispatcher_thread_index,
        };

        self.prepared_custom_data = Some(scene_registry.prepare(scene_id, &ctx, &frame.cameras)?);
        Ok(true)
    }

    pub fn into_epilogue_state(self) -> (FrameCommandRecorders, RenderNodeProcessState) {
        (self.command_recorders, self.process_state)
    }
}
