use super::super::{
    AddGraphGroupImport, AddGraphOptions, NodeGroupId, RenderFlowGroup, RenderGraphError,
    RenderNodeDebugName, RenderNodeDependencyKind, RenderNodeExecutionMetadata, RenderNodeGraph,
    RenderNodeId, RenderNodeImplId, RenderNodeKind, RenderNodeParameters, RenderNodeRole,
    RenderNodeSubtype,
};

fn params(kind: RenderNodeKind, subtype: u32, name: &str) -> RenderNodeParameters {
    RenderNodeParameters::new(
        kind,
        RenderNodeRole::Normal,
        RenderNodeSubtype::new(subtype),
        Some(RenderNodeImplId::from_index(subtype)),
        RenderNodeDebugName::new(name),
    )
}

fn stage(name: &str) -> RenderNodeParameters {
    RenderNodeParameters::stage(name)
}

fn add_node(graph: &mut RenderNodeGraph, name: &str) -> RenderNodeId {
    graph
        .add_node(stage(name), RenderNodeExecutionMetadata::default())
        .unwrap()
}

fn add_kind(
    graph: &mut RenderNodeGraph,
    kind: RenderNodeKind,
    subtype: u32,
    name: &str,
) -> RenderNodeId {
    graph
        .add_node(
            params(kind, subtype, name),
            RenderNodeExecutionMetadata::default(),
        )
        .unwrap()
}

#[test]
fn imported_nodes_and_dependencies_receive_remapped_ids() {
    let mut source = RenderNodeGraph::new();
    let a = source
        .add_node(
            params(RenderNodeKind::Stage, 42, "a"),
            RenderNodeExecutionMetadata::default(),
        )
        .unwrap();
    let b = add_node(&mut source, "b");
    let dependency = source
        .add_dependency(RenderNodeDependencyKind::Cpu, a, b)
        .unwrap();

    let mut target = RenderNodeGraph::new();
    let existing = add_node(&mut target, "existing");
    let existing_child = add_node(&mut target, "existing-child");
    target
        .add_dependency(RenderNodeDependencyKind::Cpu, existing, existing_child)
        .unwrap();
    let import_map = target
        .add_graph(
            &source,
            AddGraphOptions {
                merge_special_nodes: false,
                ..Default::default()
            },
        )
        .unwrap();

    let imported_a = import_map.node(a).unwrap();
    let imported_b = import_map.node(b).unwrap();
    let imported_dependency = import_map.dependency(dependency).unwrap();

    assert_ne!(imported_a, a);
    assert_ne!(imported_b, b);
    assert_ne!(imported_dependency, dependency);
    assert_eq!(target.node_count(), 4);
    assert_eq!(target.node_ids().collect::<Vec<_>>()[0], existing);
    assert_eq!(
        target.node(imported_a).unwrap().impl_id(),
        Some(RenderNodeImplId::from_index(42))
    );
    assert!(target
        .has_dependency(RenderNodeDependencyKind::Cpu, imported_a, imported_b)
        .unwrap());
    assert_eq!(
        target.dependency(imported_dependency).unwrap().parent,
        imported_a
    );
    assert_eq!(
        target.dependency(imported_dependency).unwrap().child,
        imported_b
    );
    target.validate_dependency_links().unwrap();
}

#[test]
fn imported_dependency_next_links_preserve_source_traversal_order() {
    let mut source = RenderNodeGraph::new();
    let parent = add_node(&mut source, "parent");
    let first = add_node(&mut source, "first");
    let middle = add_node(&mut source, "middle");
    let last = add_node(&mut source, "last");
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, first)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, middle)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, last)
        .unwrap();

    let mut target = RenderNodeGraph::new();
    let import_map = target
        .add_graph(
            &source,
            AddGraphOptions {
                merge_special_nodes: false,
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(
        target
            .child_nodes(
                import_map.node(parent).unwrap(),
                RenderNodeDependencyKind::Cpu
            )
            .unwrap(),
        vec![
            import_map.node(last).unwrap(),
            import_map.node(middle).unwrap(),
            import_map.node(first).unwrap()
        ]
    );
    target.validate_dependency_links().unwrap();
}

#[test]
fn import_options_force_camera_index_and_can_clear_group_metadata() {
    let mut source = RenderNodeGraph::new();
    let mut metadata = RenderNodeExecutionMetadata::new(Some(1), Some(NodeGroupId::from_index(9)));
    metadata.set_flow_group(RenderNodeDependencyKind::Cpu, RenderFlowGroup::new(3));
    let source_node = source.add_node(stage("camera-node"), metadata).unwrap();

    let mut target = RenderNodeGraph::new();
    let import_map = target
        .add_graph(
            &source,
            AddGraphOptions {
                force_camera_index: Some(77),
                merge_special_nodes: false,
                group_import: AddGraphGroupImport::ClearMetadata,
            },
        )
        .unwrap();

    let imported = target.node(import_map.node(source_node).unwrap()).unwrap();

    assert_eq!(imported.metadata().camera_index, Some(77));
    assert_eq!(imported.metadata().group_id, None);
    assert_eq!(
        imported
            .metadata()
            .flow_group(RenderNodeDependencyKind::Cpu),
        None
    );
}

#[test]
fn unique_merge_copies_duplicate_dependencies_to_survivor() {
    let mut target = RenderNodeGraph::new();
    let survivor = add_kind(&mut target, RenderNodeKind::Unique, 12, "unique-main");
    let source_parent = add_node(&mut target, "target-parent");
    target
        .add_dependency(RenderNodeDependencyKind::Cpu, source_parent, survivor)
        .unwrap();

    let mut source = RenderNodeGraph::new();
    let imported_parent = add_node(&mut source, "import-parent");
    let duplicate = add_kind(&mut source, RenderNodeKind::Unique, 12, "unique-import");
    let imported_child = add_node(&mut source, "import-child");
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, imported_parent, duplicate)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Gpu, duplicate, imported_child)
        .unwrap();

    let import_map = target
        .add_graph(&source, AddGraphOptions::default())
        .unwrap();

    let remapped_parent = import_map.node(imported_parent).unwrap();
    let remapped_child = import_map.node(imported_child).unwrap();
    assert_eq!(target.node_count(), 4);
    assert!(target
        .has_dependency(RenderNodeDependencyKind::Cpu, remapped_parent, survivor)
        .unwrap());
    assert!(target
        .has_dependency(RenderNodeDependencyKind::Gpu, survivor, remapped_child)
        .unwrap());
    assert!(target.node(import_map.node(duplicate).unwrap()).is_err());
    target.validate_dependency_links().unwrap();
}

#[test]
fn broken_sequence_pairs_fail_before_import_mutates_target() {
    let mut source = RenderNodeGraph::new();
    add_kind(&mut source, RenderNodeKind::SequenceBegin, 5, "begin-only");

    let mut target = RenderNodeGraph::new();
    let existing = add_node(&mut target, "existing");
    let error = target
        .add_graph(&source, AddGraphOptions::default())
        .unwrap_err();

    assert!(matches!(
        error,
        RenderGraphError::InvalidMerge {
            reason: "sequence begin/end helper nodes are not paired"
        }
    ));
    assert_eq!(target.node_ids().collect::<Vec<_>>(), vec![existing]);
}

#[test]
fn duplicate_sequence_pairs_inside_one_graph_side_fail_before_import_mutates_target() {
    let mut source = RenderNodeGraph::new();
    add_kind(&mut source, RenderNodeKind::SequenceBegin, 6, "begin-a");
    add_kind(&mut source, RenderNodeKind::SequenceEnd, 6, "end-a");
    add_kind(&mut source, RenderNodeKind::SequenceBegin, 6, "begin-b");
    add_kind(&mut source, RenderNodeKind::SequenceEnd, 6, "end-b");

    let mut target = RenderNodeGraph::new();
    let existing = add_node(&mut target, "existing");
    let error = target
        .add_graph(&source, AddGraphOptions::default())
        .unwrap_err();

    assert!(matches!(
        error,
        RenderGraphError::InvalidMerge {
            reason: "sequence helper nodes must be unique begin/end pairs per graph side"
        }
    ));
    assert_eq!(target.node_ids().collect::<Vec<_>>(), vec![existing]);
}

#[test]
fn sequence_merge_preserves_order_and_removes_helpers() {
    let mut target = RenderNodeGraph::new();
    let begin_a = add_kind(&mut target, RenderNodeKind::SequenceBegin, 22, "begin-a");
    let a = add_node(&mut target, "a");
    let end_a = add_kind(&mut target, RenderNodeKind::SequenceEnd, 22, "end-a");
    let after = add_node(&mut target, "after");
    target
        .add_dependency(RenderNodeDependencyKind::Cpu, begin_a, a)
        .unwrap();
    target
        .add_dependency(RenderNodeDependencyKind::Cpu, a, end_a)
        .unwrap();
    target
        .add_dependency(RenderNodeDependencyKind::Cpu, end_a, after)
        .unwrap();

    let mut source = RenderNodeGraph::new();
    let before_import = add_node(&mut source, "before-import");
    let begin_b = add_kind(&mut source, RenderNodeKind::SequenceBegin, 22, "begin-b");
    let b = add_node(&mut source, "b");
    let end_b = add_kind(&mut source, RenderNodeKind::SequenceEnd, 22, "end-b");
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, before_import, begin_b)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, begin_b, b)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, b, end_b)
        .unwrap();

    let import_map = target
        .add_graph(&source, AddGraphOptions::default())
        .unwrap();
    let imported_before = import_map.node(before_import).unwrap();
    let imported_b = import_map.node(b).unwrap();

    target.validate_no_helper_nodes().unwrap();
    assert!(target
        .has_dependency(RenderNodeDependencyKind::Cpu, imported_before, a)
        .unwrap());
    assert!(target
        .has_dependency(RenderNodeDependencyKind::Cpu, a, imported_b)
        .unwrap());
    assert!(target
        .has_dependency(RenderNodeDependencyKind::Cpu, imported_b, after)
        .unwrap());
    target.validate_dependency_links().unwrap();
}

#[test]
fn helper_removal_handles_multiple_helpers_and_usage_order_mutation() {
    let mut source = RenderNodeGraph::new();
    let parent = add_node(&mut source, "parent");
    let helper_a = add_kind(&mut source, RenderNodeKind::Temporary, 0, "temp-a");
    let middle = add_node(&mut source, "middle");
    let helper_b = add_kind(&mut source, RenderNodeKind::Temporary, 1, "temp-b");
    let child = add_node(&mut source, "child");
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, helper_a)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, helper_a, middle)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, middle, helper_b)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Cpu, helper_b, child)
        .unwrap();

    let mut target = RenderNodeGraph::new();
    let import_map = target
        .add_graph(&source, AddGraphOptions::default())
        .unwrap();

    let imported_parent = import_map.node(parent).unwrap();
    let imported_middle = import_map.node(middle).unwrap();
    let imported_child = import_map.node(child).unwrap();

    assert!(target.node(import_map.node(helper_a).unwrap()).is_err());
    assert!(target.node(import_map.node(helper_b).unwrap()).is_err());
    assert!(target
        .has_dependency(
            RenderNodeDependencyKind::Cpu,
            imported_parent,
            imported_middle
        )
        .unwrap());
    assert!(target
        .has_dependency(
            RenderNodeDependencyKind::Cpu,
            imported_middle,
            imported_child
        )
        .unwrap());
    target.validate_no_helper_nodes().unwrap();
}

#[test]
fn temporary_helper_removal_bridges_dependencies() {
    let mut source = RenderNodeGraph::new();
    let parent = add_node(&mut source, "parent");
    let helper = add_kind(&mut source, RenderNodeKind::Temporary, 0, "temp");
    let child = add_node(&mut source, "child");
    source
        .add_dependency(RenderNodeDependencyKind::Gpu, parent, helper)
        .unwrap();
    source
        .add_dependency(RenderNodeDependencyKind::Gpu, helper, child)
        .unwrap();

    let mut target = RenderNodeGraph::new();
    let import_map = target
        .add_graph(&source, AddGraphOptions::default())
        .unwrap();

    let imported_parent = import_map.node(parent).unwrap();
    let imported_child = import_map.node(child).unwrap();
    assert!(target.node(import_map.node(helper).unwrap()).is_err());
    assert!(target
        .has_dependency(
            RenderNodeDependencyKind::Gpu,
            imported_parent,
            imported_child
        )
        .unwrap());
    target.validate_no_helper_nodes().unwrap();
}
