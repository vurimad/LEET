use super::super::{
    AddGraphOptions, CommandListGroupStore, FrameResourceAllocator, RenderGraphError,
    RenderNodeCommandListUsage, RenderNodeDebugName, RenderNodeDependencyKind,
    RenderNodeExecutionMetadata, RenderNodeGraph, RenderNodeGraphFactory, RenderNodeImpl,
    RenderNodeImplContext, RenderNodeKind, RenderNodeParameters, RenderNodeRole, RenderNodeSubtype,
    RenderQueueKind, ResourceAllocatorPhase, ResourceRequest,
};
use crate::RenderGraphResult;
use leet_jobs2::Builder as RenderJobBuilder;

#[derive(Debug)]
struct TestNode {
    name: &'static str,
    usage: RenderNodeCommandListUsage,
}

impl TestNode {
    fn new(name: &'static str, usage: RenderNodeCommandListUsage) -> Self {
        Self { name, usage }
    }
}

impl RenderNodeImpl for TestNode {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        self.usage
    }

    fn execute(
        &self,
        _rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        Ok(())
    }
}

fn require_node(name: &'static str) -> TestNode {
    TestNode::new(name, RenderNodeCommandListUsage::Require)
}

#[test]
fn command_list_group_creates_one_graph_visible_parent_node() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();

    let parent = factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::new(11),
            "gbuffer bucket",
            RenderQueueKind::Graphics,
        )
        .unwrap();

    let view = factory.graph().node(parent).unwrap();
    let command_group = factory.command_list_group(parent).unwrap();

    assert_eq!(factory.graph().node_count(), 1);
    assert_eq!(factory.created_node_ids(), &[parent]);
    assert_eq!(factory.group_members(group).unwrap(), &[parent]);
    assert_eq!(view.role(), RenderNodeRole::CommandListGroup);
    assert_eq!(view.impl_id(), None);
    assert_eq!(view.debug_name().as_str(), "gbuffer bucket");
    assert_eq!(command_group.graph_node(), parent);
    assert_eq!(command_group.queue_kind(), RenderQueueKind::Graphics);
    assert_eq!(
        command_group.command_list_usage(),
        RenderNodeCommandListUsage::Own
    );
    assert!(command_group.subnodes().is_empty());
}

#[test]
fn subnodes_are_owned_by_group_and_not_graph_visible() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let parent = factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "lighting bucket",
            RenderQueueKind::Compute,
        )
        .unwrap();

    factory.create_subnode(require_node("first")).unwrap();
    factory.create_subnode(require_node("second")).unwrap();

    let command_group = factory.command_list_group(parent).unwrap();
    let subnodes = command_group.subnodes();

    assert_eq!(factory.graph().node_count(), 1);
    assert_eq!(factory.impl_store().len(), 2);
    assert_eq!(subnodes.len(), 2);
    assert_eq!(
        factory.impl_store().get(subnodes[0]).unwrap().name(),
        "first"
    );
    assert_eq!(
        factory.impl_store().get(subnodes[1]).unwrap().name(),
        "second"
    );
}

#[test]
fn subnode_order_is_preserved_across_factory_and_built_graph() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let parent = factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "post bucket",
            RenderQueueKind::Graphics,
        )
        .unwrap();

    factory.create_subnode(require_node("tonemap")).unwrap();
    factory.create_subnode(require_node("bloom")).unwrap();
    factory.end_command_list_group().unwrap();

    let factory_order = factory
        .command_list_group(parent)
        .unwrap()
        .subnodes()
        .to_vec();
    let built = factory.finish().unwrap();
    let built_order = built.command_list_group(parent).unwrap().subnodes();

    assert_eq!(built_order, factory_order.as_slice());
    assert_eq!(
        built.impl_store().get(built_order[0]).unwrap().name(),
        "tonemap"
    );
    assert_eq!(
        built.impl_store().get(built_order[1]).unwrap().name(),
        "bloom"
    );
}

#[test]
fn command_list_group_metadata_imports_with_graph_node_remap() {
    let mut source_factory = RenderNodeGraphFactory::new();
    let group = source_factory.create_group().unwrap();
    let source_parent = source_factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "imported bucket",
            RenderQueueKind::Compute,
        )
        .unwrap();
    source_factory
        .create_subnode(require_node("first"))
        .unwrap();
    source_factory
        .create_subnode(require_node("second"))
        .unwrap();
    source_factory.end_command_list_group().unwrap();

    let source_subnodes = source_factory
        .command_list_group(source_parent)
        .unwrap()
        .subnodes()
        .to_vec();
    let source_built = source_factory.finish().unwrap();
    let (source_graph, _source_impls, source_command_groups) = source_built.into_parts();
    let mut target_graph = RenderNodeGraph::new();
    target_graph
        .add_node(
            RenderNodeParameters::stage("existing target node"),
            RenderNodeExecutionMetadata::default(),
        )
        .unwrap();
    let import_map = target_graph
        .add_graph(
            &source_graph,
            AddGraphOptions {
                merge_special_nodes: false,
                ..Default::default()
            },
        )
        .unwrap();
    let imported_parent = import_map.node(source_parent).unwrap();
    let mut target_command_groups = CommandListGroupStore::new();

    target_command_groups
        .import_from(&source_command_groups, &import_map)
        .unwrap();
    let imported_group = target_command_groups.get(imported_parent).unwrap();

    assert_eq!(imported_group.graph_node(), imported_parent);
    assert_ne!(imported_parent, source_parent);
    assert_eq!(imported_group.name().as_str(), "imported bucket");
    assert_eq!(imported_group.queue_kind(), RenderQueueKind::Compute);
    assert_eq!(imported_group.subnodes(), source_subnodes.as_slice());
    assert!(target_command_groups.get(source_parent).is_err());
}

#[test]
fn nested_command_list_groups_fail() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "outer",
            RenderQueueKind::Graphics,
        )
        .unwrap();

    let err = factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "inner",
            RenderQueueKind::Graphics,
        )
        .unwrap_err();

    assert!(matches!(
        err,
        RenderGraphError::InvalidCommandListGroupUsage {
            operation: "begin_command_list_group",
            ..
        }
    ));
}

#[test]
fn command_list_group_rejects_unsupported_queue_kind() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();

    let err = factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "copy bucket",
            RenderQueueKind::Copy,
        )
        .unwrap_err();

    assert!(matches!(
        err,
        RenderGraphError::InvalidCommandListGroupUsage {
            operation: "begin_command_list_group",
            ..
        }
    ));
    assert_eq!(factory.graph().node_count(), 0);
}

#[test]
fn create_node_while_command_list_group_is_open_fails() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "bucket",
            RenderQueueKind::Graphics,
        )
        .unwrap();

    let err = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            require_node("normal"),
        )
        .unwrap_err();

    assert!(matches!(
        err,
        RenderGraphError::InvalidCommandListGroupUsage {
            operation: "create_node",
            ..
        }
    ));
}

#[test]
fn create_subnode_and_end_group_require_open_group() {
    let mut factory = RenderNodeGraphFactory::new();

    let subnode_err = factory.create_subnode(require_node("orphan")).unwrap_err();
    let end_err = factory.end_command_list_group().unwrap_err();

    assert!(matches!(
        subnode_err,
        RenderGraphError::InvalidCommandListGroupUsage {
            operation: "create_subnode",
            ..
        }
    ));
    assert!(matches!(
        end_err,
        RenderGraphError::InvalidCommandListGroupUsage {
            operation: "end_command_list_group",
            ..
        }
    ));
}

#[test]
fn direct_dependencies_target_command_list_group_parent_node() {
    let mut factory = RenderNodeGraphFactory::new();
    let command_group = factory.create_group().unwrap();
    let normal_group = factory.create_group().unwrap();
    let parent = factory
        .begin_command_list_group(
            command_group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "gbuffer",
            RenderQueueKind::Graphics,
        )
        .unwrap();
    factory.create_subnode(require_node("draw opaque")).unwrap();
    factory.end_command_list_group().unwrap();
    let child = factory
        .create_node(
            normal_group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            TestNode::new("resolve", RenderNodeCommandListUsage::None),
        )
        .unwrap();

    factory
        .link_nodes(parent, child, RenderNodeDependencyKind::Gpu)
        .unwrap();

    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, parent, child)
        .unwrap());
}

#[test]
fn finish_fails_while_command_list_group_is_open() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "bucket",
            RenderQueueKind::Graphics,
        )
        .unwrap();

    let err = match factory.finish() {
        Ok(_) => panic!("finish unexpectedly succeeded with an open command-list group"),
        Err(err) => err,
    };

    assert!(matches!(
        err,
        RenderGraphError::InvalidCommandListGroupUsage {
            operation: "finish",
            ..
        }
    ));
}

#[test]
fn reset_clears_open_command_list_group_state() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "bucket",
            RenderQueueKind::Graphics,
        )
        .unwrap();
    factory.create_subnode(require_node("draw")).unwrap();

    factory.reset();

    assert!(factory.open_command_list_group().is_none());
    assert!(factory.command_groups().is_empty());
    assert_eq!(factory.impl_store().len(), 0);
    assert_eq!(factory.graph().node_count(), 0);
}

#[test]
fn command_group_parent_uses_explicit_parameters() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();

    let parent = factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Unique,
            RenderNodeSubtype::new(44),
            "shadow bucket",
            RenderQueueKind::Graphics,
        )
        .unwrap();

    assert_eq!(
        factory.graph().node(parent).unwrap().params(),
        &super::super::RenderNodeParameters::new(
            RenderNodeKind::Unique,
            RenderNodeRole::CommandListGroup,
            RenderNodeSubtype::new(44),
            None,
            RenderNodeDebugName::new("shadow bucket"),
        )
    );
}

#[test]
fn context_queue_scope_wrappers_record_begin_and_end_requests() {
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();

    {
        let mut rctx = RenderNodeImplContext::unique_node(
            &mut allocator,
            super::super::FrameResourceFlowGroup::new(0),
        );
        rctx.begin_queue(RenderQueueKind::Compute).unwrap();
        rctx.end_queue().unwrap();
    }

    let requests = allocator
        .request_group(super::super::FrameResourceFlowGroup::new(0))
        .unwrap()
        .requests();

    assert!(matches!(
        requests[0],
        ResourceRequest::BeginQueue {
            queue: RenderQueueKind::Compute
        }
    ));
    assert!(matches!(requests[1], ResourceRequest::EndQueue));
}
