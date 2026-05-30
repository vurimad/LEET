use super::super::{
    RenderGraphError, RenderNodeDependencyKind, RenderNodeExecutionMetadata, RenderNodeGraph,
    RenderNodeId, RenderNodeParameters,
};

fn add_node(graph: &mut RenderNodeGraph, name: &str) -> RenderNodeId {
    graph
        .add_node(
            RenderNodeParameters::stage(name),
            RenderNodeExecutionMetadata::default(),
        )
        .unwrap()
}

#[test]
fn acyclic_graph_builds_cpu_and_gpu_flow_groups() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");
    let c = add_node(&mut graph, "c");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, b)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, b, c)
        .unwrap();

    graph.build_flow_groups().unwrap();

    assert_eq!(
        graph.flattened_nodes(RenderNodeDependencyKind::Cpu),
        &[a, c, b]
    );
    assert_eq!(
        graph.flattened_nodes(RenderNodeDependencyKind::Gpu),
        &[a, b, c]
    );
    assert_eq!(
        graph
            .node(a)
            .unwrap()
            .metadata()
            .flow_group(RenderNodeDependencyKind::Cpu)
            .unwrap()
            .get(),
        0
    );
    assert_eq!(
        graph
            .node(b)
            .unwrap()
            .metadata()
            .flow_group(RenderNodeDependencyKind::Gpu)
            .unwrap()
            .get(),
        1
    );
}

#[test]
fn cpu_cycle_fails_with_dependency_kind_and_unresolved_nodes() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");
    let c = add_node(&mut graph, "c");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, b)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, b, c)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, c, a)
        .unwrap();

    let error = graph.build_flow_groups().unwrap_err();

    assert!(matches!(
        error,
        RenderGraphError::CycleDetected {
            dependency_kind: "CPU",
            remaining_nodes: 3
        }
    ));
    assert!(!graph.is_built());
}

#[test]
fn gpu_cycle_fails_with_dependency_kind_and_unresolved_nodes() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");

    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, a, b)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, b, a)
        .unwrap();

    let error = graph.build_flow_groups().unwrap_err();

    assert!(matches!(
        error,
        RenderGraphError::CycleDetected {
            dependency_kind: "GPU",
            remaining_nodes: 2
        }
    ));
    assert!(!graph.is_built());
}

#[test]
fn failed_build_does_not_commit_partial_flow_groups() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");
    let c = add_node(&mut graph, "c");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, b)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, b, c)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, c, b)
        .unwrap();

    assert!(graph.build_flow_groups().is_err());

    assert!(graph
        .flattened_nodes(RenderNodeDependencyKind::Cpu)
        .is_empty());
    assert!(graph
        .flattened_nodes(RenderNodeDependencyKind::Gpu)
        .is_empty());
    for node in [a, b, c] {
        assert_eq!(
            graph
                .node(node)
                .unwrap()
                .metadata()
                .flow_group(RenderNodeDependencyKind::Cpu),
            None
        );
        assert_eq!(
            graph
                .node(node)
                .unwrap()
                .metadata()
                .flow_group(RenderNodeDependencyKind::Gpu),
            None
        );
    }
    assert!(!graph.is_built());
}

#[test]
fn isolated_nodes_receive_cpu_and_gpu_flow_groups() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");

    graph.build_flow_groups().unwrap();

    assert_eq!(
        graph.flattened_nodes(RenderNodeDependencyKind::Cpu),
        &[a, b]
    );
    assert_eq!(
        graph.flattened_nodes(RenderNodeDependencyKind::Gpu),
        &[a, b]
    );
    for (expected, node) in [a, b].into_iter().enumerate() {
        assert_eq!(
            graph
                .node(node)
                .unwrap()
                .metadata()
                .flow_group(RenderNodeDependencyKind::Cpu)
                .unwrap()
                .get(),
            expected as u16
        );
        assert_eq!(
            graph
                .node(node)
                .unwrap()
                .metadata()
                .flow_group(RenderNodeDependencyKind::Gpu)
                .unwrap()
                .get(),
            expected as u16
        );
    }
}

#[test]
fn flow_groups_are_dense_unique_per_dependency_kind() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");
    let c = add_node(&mut graph, "c");
    let d = add_node(&mut graph, "d");
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, d)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, b, d)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, c, d)
        .unwrap();

    graph.build_flow_groups().unwrap();

    let cpu_groups = graph
        .flattened_nodes(RenderNodeDependencyKind::Cpu)
        .iter()
        .map(|node| {
            graph
                .node(*node)
                .unwrap()
                .metadata()
                .flow_group(RenderNodeDependencyKind::Cpu)
                .unwrap()
                .get()
        })
        .collect::<Vec<_>>();

    assert_eq!(cpu_groups, vec![0, 1, 2, 3]);
}

#[test]
fn same_level_order_is_deterministic_by_usage_order() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");
    let c = add_node(&mut graph, "c");
    let d = add_node(&mut graph, "d");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, d)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, b, c)
        .unwrap();

    graph.build_flow_groups().unwrap();

    assert_eq!(
        graph.flattened_nodes(RenderNodeDependencyKind::Cpu),
        &[a, b, c, d]
    );
}

#[test]
fn cpu_and_gpu_dependency_graphs_may_differ_legally() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");
    let c = add_node(&mut graph, "c");

    graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, b)
        .unwrap();
    graph
        .add_dependency(RenderNodeDependencyKind::Gpu, b, a)
        .unwrap();

    graph.build_flow_groups().unwrap();

    assert_eq!(
        graph.flattened_nodes(RenderNodeDependencyKind::Cpu),
        &[a, c, b]
    );
    assert_eq!(
        graph.flattened_nodes(RenderNodeDependencyKind::Gpu),
        &[b, c, a]
    );
}

#[test]
fn built_graph_rejects_topology_mutation() {
    let mut graph = RenderNodeGraph::new();
    let a = add_node(&mut graph, "a");
    let b = add_node(&mut graph, "b");

    graph.build_flow_groups().unwrap();
    let error = graph
        .add_dependency(RenderNodeDependencyKind::Cpu, a, b)
        .unwrap_err();

    assert!(graph.is_built());
    assert!(matches!(
        error,
        RenderGraphError::GraphAlreadyBuilt {
            operation: "add_dependency"
        }
    ));
}

#[test]
fn manual_topology_freeze_does_not_mark_graph_built() {
    let mut graph = RenderNodeGraph::new();
    add_node(&mut graph, "a");

    graph.freeze_topology();

    assert!(graph.is_topology_frozen());
    assert!(!graph.is_built());
    assert!(graph
        .flattened_nodes(RenderNodeDependencyKind::Cpu)
        .is_empty());
    assert!(graph
        .flattened_nodes(RenderNodeDependencyKind::Gpu)
        .is_empty());
}

#[test]
fn build_rejects_unfinalized_helper_nodes() {
    let mut graph = RenderNodeGraph::new();
    graph
        .add_node(
            RenderNodeParameters::new(
                super::super::RenderNodeKind::Temporary,
                super::super::RenderNodeRole::Normal,
                super::super::RenderNodeSubtype::DEFAULT,
                None,
                super::super::RenderNodeDebugName::new("temporary"),
            ),
            RenderNodeExecutionMetadata::default(),
        )
        .unwrap();

    let error = graph.build_flow_groups().unwrap_err();

    assert!(matches!(
        error,
        RenderGraphError::InvalidMerge {
            reason: "helper nodes remain after graph finalization"
        }
    ));
}
