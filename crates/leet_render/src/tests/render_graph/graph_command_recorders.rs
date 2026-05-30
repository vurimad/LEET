use leet_jobs2::Builder as RenderJobBuilder;

use bevy_math::URect;

use super::super::{
    FrameCommandPassKind, FrameCommandRecorderState, FrameCommandRecorders, FrameResourceAllocator,
    QueueSyncKind, RenderFlowGroup, RenderGraphError, RenderGraphResult,
    RenderNodeCommandListUsage, RenderNodeDependencyKind, RenderNodeGraphFactory, RenderNodeImpl,
    RenderNodeImplContext, RenderNodeImplContextInit, RenderNodeKind, RenderNodeSubtype,
    RenderQueueKind, ResourceAllocatorPhase, ResourceRequest,
};

#[derive(Debug)]
struct NoopNode {
    name: &'static str,
}

impl NoopNode {
    fn new(name: &'static str) -> Self {
        Self { name }
    }
}

impl RenderNodeImpl for NoopNode {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        _rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        Ok(())
    }
}

fn flow_group(index: u16) -> RenderFlowGroup {
    RenderFlowGroup::new(index)
}

#[test]
fn command_recording_storage_prepares_slots_for_graph_nodes() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            NoopNode::new("first"),
        )
        .unwrap();
    factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            NoopNode::new("second"),
        )
        .unwrap();
    let built = factory.finish().unwrap();

    let recorders = FrameCommandRecorders::prepare_for_graph(built.graph()).unwrap();

    assert_eq!(recorders.len(), built.graph().node_count());
    assert_eq!(
        recorders.state(flow_group(0)).unwrap(),
        FrameCommandRecorderState::Empty
    );
    assert_eq!(
        recorders.state(flow_group(1)).unwrap(),
        FrameCommandRecorderState::Empty
    );
}

#[test]
fn own_command_usage_creates_and_owns_recording_slot() {
    let mut recorders = FrameCommandRecorders::prepare(1).unwrap();

    let slot = recorders
        .create_own_recorder(flow_group(0), RenderQueueKind::Graphics, "gbuffer")
        .unwrap();

    assert_eq!(slot.get(), 0);
    assert!(recorders.has_command_recorder(flow_group(0)).unwrap());
    assert_eq!(
        recorders.state(flow_group(0)).unwrap(),
        FrameCommandRecorderState::Recording
    );
}

#[test]
fn require_command_usage_fails_without_available_recording_slot() {
    let recorders = FrameCommandRecorders::prepare(1).unwrap();

    let err = recorders.require_recorder(flow_group(0)).unwrap_err();

    assert!(matches!(
        err,
        RenderGraphError::InvalidCommandRecorderUsage {
            operation: "require_recorder",
            ..
        }
    ));
}

#[test]
fn sync_recording_does_not_create_normal_command_recorder() {
    let mut recorders = FrameCommandRecorders::prepare(1).unwrap();

    recorders
        .record_sync(flow_group(0), QueueSyncKind::Fork, "fork shadows")
        .unwrap();

    assert!(!recorders.has_command_recorder(flow_group(0)).unwrap());
    assert_eq!(
        recorders.state(flow_group(0)).unwrap(),
        FrameCommandRecorderState::Empty
    );
    assert_eq!(recorders.sync_events(flow_group(0)).unwrap().len(), 1);
}

#[test]
fn active_pass_state_controls_viewport_and_debug_markers() {
    let mut recorders = FrameCommandRecorders::prepare(1).unwrap();
    let viewport = URect::new(0, 0, 128, 64);

    recorders
        .create_own_recorder(flow_group(0), RenderQueueKind::Graphics, "main")
        .unwrap();
    assert!(matches!(
        recorders.set_viewport(flow_group(0), viewport),
        Err(RenderGraphError::InvalidCommandRecorderUsage {
            operation: "set_viewport",
            ..
        })
    ));

    recorders
        .begin_render_pass(flow_group(0), "main pass")
        .unwrap();
    assert_eq!(
        recorders.active_pass(flow_group(0)).unwrap(),
        Some(FrameCommandPassKind::Render)
    );
    recorders.set_viewport(flow_group(0), viewport).unwrap();
    recorders.end_pass(flow_group(0)).unwrap();

    assert_eq!(recorders.viewport(flow_group(0)).unwrap(), Some(viewport));
    assert_eq!(
        recorders.debug_markers(flow_group(0)).unwrap(),
        &["main pass".to_string()]
    );
}

#[test]
fn ordered_submission_follows_gpu_dependency_order() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let first = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            NoopNode::new("first"),
        )
        .unwrap();
    let second = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            NoopNode::new("second"),
        )
        .unwrap();
    let third = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            NoopNode::new("third"),
        )
        .unwrap();

    factory
        .link_nodes(second, first, RenderNodeDependencyKind::Gpu)
        .unwrap();
    factory
        .link_nodes(first, third, RenderNodeDependencyKind::Gpu)
        .unwrap();

    let built = factory.finish().unwrap();
    let second_group = built
        .graph()
        .node(second)
        .unwrap()
        .metadata()
        .flow_group(RenderNodeDependencyKind::Gpu)
        .unwrap();
    let first_group = built
        .graph()
        .node(first)
        .unwrap()
        .metadata()
        .flow_group(RenderNodeDependencyKind::Gpu)
        .unwrap();
    let third_group = built
        .graph()
        .node(third)
        .unwrap()
        .metadata()
        .flow_group(RenderNodeDependencyKind::Gpu)
        .unwrap();
    let mut recorders = FrameCommandRecorders::prepare_for_graph(built.graph()).unwrap();

    recorders
        .create_own_recorder(third_group, RenderQueueKind::Graphics, "third")
        .unwrap();
    recorders.finish_recorder(third_group).unwrap();
    recorders
        .create_own_recorder(second_group, RenderQueueKind::Graphics, "second")
        .unwrap();
    recorders.finish_recorder(second_group).unwrap();
    recorders
        .create_own_recorder(first_group, RenderQueueKind::Graphics, "first")
        .unwrap();
    recorders.finish_recorder(first_group).unwrap();

    let submissions = recorders
        .submit_finished_in_gpu_order(built.graph())
        .unwrap();
    let labels = submissions
        .iter()
        .map(|submission| submission.label.as_str())
        .collect::<Vec<_>>();

    assert_eq!(labels, ["second", "first", "third"]);
}

#[test]
fn allocator_queue_sync_does_not_own_command_recording_state() {
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    let mut recorders = FrameCommandRecorders::prepare(1).unwrap();
    recorders
        .record_sync(flow_group(0), QueueSyncKind::Barrier, "command barrier")
        .unwrap();

    {
        let mut rctx = RenderNodeImplContext::new_with_runtime(
            &mut allocator,
            &mut recorders,
            RenderNodeImplContextInit::unique_node(flow_group(0)),
        );
        rctx.queue_sync(QueueSyncKind::Barrier).unwrap();
    }

    let requests = allocator.request_group(flow_group(0)).unwrap().requests();
    assert!(matches!(
        requests[0],
        ResourceRequest::QueueSync {
            sync: QueueSyncKind::Barrier
        }
    ));
    assert!(!recorders.has_command_recorder(flow_group(0)).unwrap());
}

#[test]
fn recorder_cleanup_is_explicit_not_context_drop() {
    let mut allocator = FrameResourceAllocator::new();
    let mut recorders = FrameCommandRecorders::prepare(1).unwrap();
    recorders
        .create_own_recorder(flow_group(0), RenderQueueKind::Compute, "compute")
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::new_with_runtime(
            &mut allocator,
            &mut recorders,
            RenderNodeImplContextInit::unique_node(flow_group(0)),
        );
        assert!(rctx.has_command_recorder().unwrap());
    }

    assert_eq!(
        recorders.state(flow_group(0)).unwrap(),
        FrameCommandRecorderState::Recording
    );

    recorders.cleanup();

    assert_eq!(
        recorders.state(flow_group(0)).unwrap(),
        FrameCommandRecorderState::Empty
    );
}
