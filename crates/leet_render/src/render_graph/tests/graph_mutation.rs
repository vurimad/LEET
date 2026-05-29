use super::super::{
    RenderDependencyId, RenderGraphError, RenderNodeData, RenderNodeDebugName,
    RenderNodeDependencyKind, RenderNodeExecutionMetadata, RenderNodeGraph, RenderNodeKind,
    RenderNodeParameters, RenderNodeRole, RenderNodeSubtype,
};

fn node_params(name: &str) -> RenderNodeParameters {
    RenderNodeParameters::new(
        RenderNodeKind::Stage,
        RenderNodeRole::Normal,
        RenderNodeSubtype::DEFAULT,
        None,
        RenderNodeDebugName::new(name),
    )
}

fn add_node(graph: &mut RenderNodeGraph, name: &str) -> super::super::RenderNodeId {
    graph
        .add_node(node_params(name), RenderNodeExecutionMetadata::default())
        .unwrap()
}

#[test]
fn dependency_insertion_links_from_parent_and_child() {
    let mut graph = RenderNodeGraph::new();
    let parent = add_node(&mut graph, "parent");
    let child = add_node(&mut graph, "child");

    let dependency = graph
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, child)
        .unwrap();

    assert_eq!(graph.dependency_count(), 1);
    assert!(graph
        .has_dependency(RenderNodeDependencyKind::Cpu, parent, child)
        .unwrap());
    assert_eq!(
        graph
            .node(parent)
            .unwrap()
            .first_child_dependency(RenderNodeDependencyKind::Cpu),
        Some(dependency)
    );
    assert_eq!(
        graph
            .node(child)
            .unwrap()
            .first_parent_dependency(RenderNodeDependencyKind::Cpu),
        Some(dependency)
    );
    graph.validate_dependency_links().unwrap();
}

#[test]
fn duplicate_dependencies_are_rejected_without_duplicating_storage() {
    let mut graph = RenderNodeGraph::new();
    let parent = add_node(&mut graph, "parent");
    let child = add_node(&mut graph, "child");

    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, parent, child)
        .unwrap();
    let error = graph
        .add_dependency(RenderNodeDependencyKind::Gpu, parent, child)
        .unwrap_err();

    assert_eq!(graph.dependency_count(), 1);
    assert!(matches!(
        error,
        RenderGraphError::DuplicateDependency {
            dependency_kind: "GPU",
            ..
        }
    ));
    graph.validate_dependency_links().unwrap();
}

#[test]
fn self_dependencies_fail() {
    let mut graph = RenderNodeGraph::new();
    let node = add_node(&mut graph, "node");

    let error = graph
        .add_dependency(RenderNodeDependencyKind::Cpu, node, node)
        .unwrap_err();

    assert!(matches!(
        error,
        RenderGraphError::SelfDependency {
            dependency_kind: "CPU",
            ..
        }
    ));
    assert_eq!(graph.dependency_count(), 0);
}

#[test]
fn dependency_removal_unlinks_both_directions() {
    let mut graph = RenderNodeGraph::new();
    let parent = add_node(&mut graph, "parent");
    let child = add_node(&mut graph, "child");
    let dependency = graph
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, child)
        .unwrap();

    graph.remove_dependency(dependency).unwrap();

    assert_eq!(graph.dependency_count(), 0);
    assert!(!graph
        .has_dependency(RenderNodeDependencyKind::Cpu, parent, child)
        .unwrap());
    assert_eq!(
        graph
            .node(parent)
            .unwrap()
            .first_child_dependency(RenderNodeDependencyKind::Cpu),
        None
    );
    assert_eq!(
        graph
            .node(child)
            .unwrap()
            .first_parent_dependency(RenderNodeDependencyKind::Cpu),
        None
    );
    graph.validate_dependency_links().unwrap();
}

#[test]
fn dependency_removal_unlinks_middle_and_tail_entries() {
    let mut graph = RenderNodeGraph::new();
    let parent = add_node(&mut graph, "parent");
    let first = add_node(&mut graph, "first");
    let middle = add_node(&mut graph, "middle");
    let last = add_node(&mut graph, "last");

    let first_dep = graph
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, first)
        .unwrap();
    let middle_dep = graph
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, middle)
        .unwrap();
    let _last_dep = graph
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, last)
        .unwrap();

    graph.remove_dependency(middle_dep).unwrap();

    assert_eq!(
        graph
            .child_nodes(parent, RenderNodeDependencyKind::Cpu)
            .unwrap(),
        vec![last, first]
    );
    assert_eq!(
        graph
            .parent_nodes(middle, RenderNodeDependencyKind::Cpu)
            .unwrap(),
        Vec::new()
    );

    graph.remove_dependency(first_dep).unwrap();

    assert_eq!(
        graph
            .child_nodes(parent, RenderNodeDependencyKind::Cpu)
            .unwrap(),
        vec![last]
    );
    assert_eq!(
        graph
            .parent_nodes(first, RenderNodeDependencyKind::Cpu)
            .unwrap(),
        Vec::new()
    );
    graph.validate_dependency_links().unwrap();
}

#[test]
fn node_removal_removes_all_attached_dependencies() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");
    let c = add_node(&mut graph, "c");
    let d = add_node(&mut graph, "d");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, b)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, b, c)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, c, d)
        .unwrap();

    graph.remove_node(b).unwrap();

    assert_eq!(graph.node_count(), 3);
    assert_eq!(graph.dependency_count(), 1);
    assert!(!graph
        .has_dependency(RenderNodeDependencyKind::Cpu, a, c)
        .unwrap());
    assert!(graph
        .has_dependency(RenderNodeDependencyKind::Cpu, c, d)
        .unwrap());
    graph.validate_dependency_links().unwrap();
}

#[test]
fn bridge_removal_skips_existing_edges_and_self_edges() {
    let mut graph = RenderNodeGraph::new();
    let parent = add_node(&mut graph, "parent");
    let gate = add_node(&mut graph, "gate");
    let child = add_node(&mut graph, "child");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, gate)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, gate, child)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, child)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, parent, gate)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, gate, parent)
        .unwrap();

    graph.remove_node_and_bridge_dependencies(gate).unwrap();

    assert_eq!(graph.dependency_count(), 1);
    assert!(graph
        .has_dependency(RenderNodeDependencyKind::Cpu, parent, child)
        .unwrap());
    graph.validate_dependency_links().unwrap();
}

#[test]
fn bridge_removal_connects_parents_to_children_by_same_dependency_kind() {
    let mut graph = RenderNodeGraph::new();
    let cpu_parent = add_node(&mut graph, "cpu-parent");
    let gpu_parent = add_node(&mut graph, "gpu-parent");
    let gate = add_node(&mut graph, "gate");
    let cpu_child = add_node(&mut graph, "cpu-child");
    let gpu_child = add_node(&mut graph, "gpu-child");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, cpu_parent, gate)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, gate, cpu_child)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, gpu_parent, gate)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, gate, gpu_child)
        .unwrap();

    graph.remove_node_and_bridge_dependencies(gate).unwrap();

    assert!(graph
        .has_dependency(RenderNodeDependencyKind::Cpu, cpu_parent, cpu_child)
        .unwrap());
    assert!(graph
        .has_dependency(RenderNodeDependencyKind::Gpu, gpu_parent, gpu_child)
        .unwrap());
    assert!(!graph
        .has_dependency(RenderNodeDependencyKind::Cpu, cpu_parent, gpu_child)
        .unwrap());
    assert!(!graph
        .has_dependency(RenderNodeDependencyKind::Gpu, gpu_parent, cpu_child)
        .unwrap());
    graph.validate_dependency_links().unwrap();
}

#[test]
fn copy_dependency_helpers_preserve_direction_and_skip_duplicates() {
    let mut graph = RenderNodeGraph::new();
    let parent = add_node(&mut graph, "parent");
    let source = add_node(&mut graph, "source");
    let destination = add_node(&mut graph, "destination");
    let child = add_node(&mut graph, "child");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, parent, source)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, source, child)
        .unwrap();

    graph
        .copy_parent_dependencies(source, destination, RenderNodeDependencyKind::Cpu)
        .unwrap();
    graph
        .copy_child_dependencies(source, destination, RenderNodeDependencyKind::Gpu)
        .unwrap();
    graph.copy_all_dependencies(source, destination).unwrap();

    assert!(graph
        .has_dependency(RenderNodeDependencyKind::Cpu, parent, destination)
        .unwrap());
    assert!(graph
        .has_dependency(RenderNodeDependencyKind::Gpu, destination, child)
        .unwrap());
    assert_eq!(graph.dependency_count(), 4);
    graph.validate_dependency_links().unwrap();
}

#[test]
fn graph_rejects_mutation_after_topology_freeze() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");

    graph.freeze_topology();

    let error = graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, b)
        .unwrap_err();

    assert!(graph.is_topology_frozen());
    assert!(matches!(
        error,
        RenderGraphError::GraphAlreadyBuilt {
            operation: "add_dependency"
        }
    ));
}

#[test]
fn raw_node_data_insertion_clears_stale_dependency_heads() {
    let mut graph = RenderNodeGraph::new();
    let mut node = RenderNodeData::new(
        node_params("raw-node"),
        RenderNodeExecutionMetadata::default(),
    );
    node.set_first_parent_dependency(
        RenderNodeDependencyKind::Cpu,
        Some(RenderDependencyId::from_raw(99)),
    );
    node.set_first_child_dependency(
        RenderNodeDependencyKind::Gpu,
        Some(RenderDependencyId::from_raw(100)),
    );

    let node_id = graph.add_node_data(node).unwrap();
    let inserted = graph.node(node_id).unwrap();

    assert_eq!(
        inserted.first_parent_dependency(RenderNodeDependencyKind::Cpu),
        None
    );
    assert_eq!(
        inserted.first_child_dependency(RenderNodeDependencyKind::Gpu),
        None
    );
    graph.validate_dependency_links().unwrap();
}
