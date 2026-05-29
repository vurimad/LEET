use super::super::{
    RenderNodeCommandListUsage, RenderNodeDependencyKind, RenderNodeGraphFactory, RenderNodeImpl,
    RenderNodeImplContext, RenderNodeKind, RenderNodeSubtype, RenderQueueKind,
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

fn create_node(
    factory: &mut RenderNodeGraphFactory,
    name: &'static str,
    usage: RenderNodeCommandListUsage,
) -> super::super::RenderNodeId {
    let group = factory.create_group().unwrap();
    factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            TestNode::new(name, usage),
        )
        .unwrap()
}

fn create_command_group(
    factory: &mut RenderNodeGraphFactory,
    name: &'static str,
) -> super::super::RenderNodeId {
    let group = factory.create_group().unwrap();
    let parent = factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            name,
            RenderQueueKind::Graphics,
        )
        .unwrap();
    factory
        .create_subnode(TestNode::new(
            "subnode",
            RenderNodeCommandListUsage::Require,
        ))
        .unwrap();
    factory.end_command_list_group().unwrap();
    parent
}

#[test]
fn creation_order_gpu_links_connect_graph_visible_gpu_work_only() {
    let mut factory = RenderNodeGraphFactory::new();
    let cpu_only = create_node(&mut factory, "cpu", RenderNodeCommandListUsage::None);
    let first_gpu = create_node(
        &mut factory,
        "first_gpu",
        RenderNodeCommandListUsage::Require,
    );
    let command_group = create_command_group(&mut factory, "bucket");
    let second_gpu = create_node(&mut factory, "sync", RenderNodeCommandListUsage::Sync);

    factory.link_created_order_gpu_chain().unwrap();
    factory.link_created_order_gpu_chain().unwrap();

    assert!(!factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, cpu_only, first_gpu)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, first_gpu, command_group)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, command_group, second_gpu)
        .unwrap());
    assert_eq!(factory.graph().dependency_count(), 2);
}

#[test]
fn creation_order_cpu_chain_links_all_graph_visible_created_nodes() {
    let mut factory = RenderNodeGraphFactory::new();
    let first = create_node(&mut factory, "first", RenderNodeCommandListUsage::None);
    let command_group = create_command_group(&mut factory, "bucket");
    let last = create_node(&mut factory, "last", RenderNodeCommandListUsage::None);

    factory.link_created_order_cpu_chain().unwrap();

    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, first, command_group)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, command_group, last)
        .unwrap());
}

#[test]
fn cpu_to_later_gpu_work_creates_cpu_dependencies_only() {
    let mut factory = RenderNodeGraphFactory::new();
    let cpu = create_node(&mut factory, "cpu", RenderNodeCommandListUsage::None);
    let ignored = create_node(&mut factory, "ignored", RenderNodeCommandListUsage::None);
    let gpu = create_node(&mut factory, "gpu", RenderNodeCommandListUsage::Require);
    let command_group = create_command_group(&mut factory, "bucket");

    factory.link_cpu_to_later_gpu_work(cpu).unwrap();

    assert!(!factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, cpu, ignored)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, cpu, gpu)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, cpu, command_group)
        .unwrap());
    assert!(!factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, cpu, gpu)
        .unwrap());
}

#[test]
fn cpu_from_earlier_gpu_work_creates_cpu_dependencies_only() {
    let mut factory = RenderNodeGraphFactory::new();
    let command_group = create_command_group(&mut factory, "bucket");
    let gpu = create_node(&mut factory, "gpu", RenderNodeCommandListUsage::Require);
    let ignored = create_node(&mut factory, "ignored", RenderNodeCommandListUsage::None);
    let cpu = create_node(&mut factory, "cpu", RenderNodeCommandListUsage::None);

    factory.link_cpu_from_earlier_gpu_work(cpu).unwrap();

    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, command_group, cpu)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, gpu, cpu)
        .unwrap());
    assert!(!factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, ignored, cpu)
        .unwrap());
    assert!(!factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, gpu, cpu)
        .unwrap());
}

#[test]
fn predicate_linking_collects_matches_before_adding_edges() {
    let mut factory = RenderNodeGraphFactory::new();
    let shadow = create_node(&mut factory, "shadow", RenderNodeCommandListUsage::Require);
    let gbuffer = create_node(&mut factory, "gbuffer", RenderNodeCommandListUsage::Require);
    let resolve = create_node(&mut factory, "resolve", RenderNodeCommandListUsage::None);

    factory
        .link_matching_to_node(resolve, RenderNodeDependencyKind::Gpu, |node| {
            node.debug_name().as_str().contains("buffer")
        })
        .unwrap();
    factory
        .link_node_to_matching(shadow, RenderNodeDependencyKind::Cpu, |node| {
            node.debug_name().as_str().starts_with("resolve")
        })
        .unwrap();

    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, gbuffer, resolve)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, shadow, resolve)
        .unwrap());
    assert!(!factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, shadow, resolve)
        .unwrap());
}

#[test]
fn bridge_helpers_reject_nodes_not_created_by_factory() {
    let mut factory = RenderNodeGraphFactory::new();
    let alien = super::super::RenderNodeId::from_index(99);

    assert!(factory.link_cpu_to_later_gpu_work(alien).is_err());
    assert!(factory.link_cpu_from_earlier_gpu_work(alien).is_err());
}

#[test]
fn predicate_helpers_reject_invalid_endpoint_even_without_matches() {
    let mut factory = RenderNodeGraphFactory::new();
    let node = create_node(&mut factory, "node", RenderNodeCommandListUsage::None);
    let invalid = super::super::RenderNodeId::from_index(99);

    assert!(factory
        .link_matching_to_node(invalid, RenderNodeDependencyKind::Cpu, |_| false)
        .is_err());
    assert!(factory
        .link_node_to_matching(invalid, RenderNodeDependencyKind::Cpu, |_| false)
        .is_err());
    assert!(factory
        .link_matching_to_node(node, RenderNodeDependencyKind::Cpu, |_| false)
        .is_ok());
}
