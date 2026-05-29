use super::super::{
    graph::storage::GraphStorage, NodeGroupId, RenderDependencyData, RenderDependencyId,
    RenderFlowGroup, RenderNodeData, RenderNodeDebugName, RenderNodeDependencyKind,
    RenderNodeExecutionMetadata, RenderNodeId, RenderNodeImplId, RenderNodeKind,
    RenderNodeParameters, RenderNodeRole, RenderNodeSubtype, RenderNodeView,
};

#[test]
fn node_topology_data_initializes_with_invalid_flow_groups_and_empty_dependency_heads() {
    let metadata = RenderNodeExecutionMetadata::new(Some(7), Some(NodeGroupId::from_index(2)));
    let node = RenderNodeData::new(
        RenderNodeParameters::new(
            RenderNodeKind::Unique,
            RenderNodeRole::Normal,
            RenderNodeSubtype::new(11),
            Some(RenderNodeImplId::from_index(5)),
            RenderNodeDebugName::new("visibility"),
        ),
        metadata,
    );

    assert_eq!(node.metadata.camera_index, Some(7));
    assert_eq!(node.metadata.group_id, Some(NodeGroupId::from_index(2)));
    assert_eq!(
        node.metadata.flow_group(RenderNodeDependencyKind::Cpu),
        None
    );
    assert_eq!(
        node.metadata.flow_group(RenderNodeDependencyKind::Gpu),
        None
    );
    assert_eq!(
        node.first_parent_dependency(RenderNodeDependencyKind::Cpu),
        None
    );
    assert_eq!(
        node.first_child_dependency(RenderNodeDependencyKind::Gpu),
        None
    );
}

#[test]
fn node_execution_metadata_is_graph_owned_and_configurable() {
    let mut metadata = RenderNodeExecutionMetadata::default();

    metadata.set_flow_group(RenderNodeDependencyKind::Cpu, RenderFlowGroup::new(3));
    metadata.set_flow_group(RenderNodeDependencyKind::Gpu, RenderFlowGroup::new(9));

    assert_eq!(
        metadata.flow_group(RenderNodeDependencyKind::Cpu),
        Some(RenderFlowGroup::new(3))
    );
    assert_eq!(
        metadata.flow_group(RenderNodeDependencyKind::Gpu),
        Some(RenderFlowGroup::new(9))
    );

    metadata.clear_flow_groups();

    assert_eq!(metadata.flow_group(RenderNodeDependencyKind::Cpu), None);
    assert_eq!(metadata.flow_group(RenderNodeDependencyKind::Gpu), None);
}

#[test]
fn dependency_topology_data_preserves_parent_child_kind_and_next_links() {
    let parent = RenderNodeId::from_index(1);
    let child = RenderNodeId::from_index(4);
    let mut dependency = RenderDependencyData::new(RenderNodeDependencyKind::Gpu, parent, child);

    dependency.next_from_parent = Some(RenderDependencyId::from_index(8));
    dependency.next_from_child = Some(RenderDependencyId::from_index(9));

    assert_eq!(dependency.kind, RenderNodeDependencyKind::Gpu);
    assert_eq!(dependency.parent, parent);
    assert_eq!(dependency.child, child);
    assert_eq!(
        dependency.next_from_parent,
        Some(RenderDependencyId::from_index(8))
    );
    assert_eq!(
        dependency.next_from_child,
        Some(RenderDependencyId::from_index(9))
    );
}

#[test]
fn read_only_view_exposes_node_metadata_without_mutation() {
    let mut node = RenderNodeData::new(
        RenderNodeParameters::new(
            RenderNodeKind::Stage,
            RenderNodeRole::GroupEntry(NodeGroupId::from_index(1)),
            RenderNodeSubtype::new(2),
            None,
            RenderNodeDebugName::new("group-in"),
        ),
        RenderNodeExecutionMetadata::new(None, Some(NodeGroupId::from_index(1))),
    );
    node.set_first_child_dependency(
        RenderNodeDependencyKind::Cpu,
        Some(RenderDependencyId::from_index(3)),
    );

    let view = RenderNodeView::new(RenderNodeId::from_index(12), &node);

    assert_eq!(view.id(), RenderNodeId::from_index(12));
    assert_eq!(view.kind(), RenderNodeKind::Stage);
    assert_eq!(
        view.role(),
        RenderNodeRole::GroupEntry(NodeGroupId::from_index(1))
    );
    assert_eq!(view.subtype(), RenderNodeSubtype::new(2));
    assert_eq!(view.impl_id(), None);
    assert_eq!(view.debug_name().as_str(), "group-in");
    assert_eq!(
        view.first_child_dependency(RenderNodeDependencyKind::Cpu),
        Some(RenderDependencyId::from_index(3))
    );
}

#[test]
fn node_topology_data_can_be_stored_by_typed_graph_storage() {
    let mut storage = GraphStorage::<RenderNodeData, RenderNodeId>::new();
    let node_id = storage.allocate(RenderNodeData::default()).unwrap();

    let view = RenderNodeView::new(node_id, storage.get(node_id).unwrap());

    assert_eq!(view.id(), node_id);
    assert_eq!(view.kind(), RenderNodeKind::Stage);
    assert_eq!(
        storage.ids_in_usage_order().collect::<Vec<_>>(),
        vec![node_id]
    );
}
