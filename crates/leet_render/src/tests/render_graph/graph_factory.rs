use super::super::{
    RenderGraphError, RenderNodeCommandListUsage, RenderNodeDebugName, RenderNodeDependencyKind,
    RenderNodeGraphFactory, RenderNodeImpl, RenderNodeImplContext, RenderNodeKind,
    RenderNodeParameters, RenderNodeRole, RenderNodeSubtype,
};
use crate::RenderGraphResult;
use leet_jobs2::Builder as RenderJobBuilder;

#[derive(Debug)]
struct TestNode {
    name: &'static str,
    usage: RenderNodeCommandListUsage,
}

impl TestNode {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            usage: RenderNodeCommandListUsage::None,
        }
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

fn create_group_with_node(
    factory: &mut RenderNodeGraphFactory,
    name: &'static str,
) -> (super::super::NodeGroupId, super::super::RenderNodeId) {
    let group = factory.create_group().unwrap();
    let node = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            TestNode::new(name),
        )
        .unwrap();
    (group, node)
}

#[test]
fn normal_node_creation_returns_graph_node_and_impl_id() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();

    let node = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::new(7),
            TestNode::new("gbuffer"),
        )
        .unwrap();

    let view = factory.graph().node(node).unwrap();
    let impl_id = view.impl_id().unwrap();
    assert_eq!(view.debug_name().as_str(), "gbuffer");
    assert_eq!(view.subtype(), RenderNodeSubtype::new(7));
    assert_eq!(view.metadata().group_id, Some(group));
    assert_eq!(factory.impl_store().get(impl_id).unwrap().name(), "gbuffer");
    assert_eq!(factory.created_node_ids(), &[node]);
}

#[test]
fn subnode_creation_without_open_command_list_group_fails() {
    let mut factory = RenderNodeGraphFactory::new();

    let error = factory
        .create_subnode(TestNode::new("subnode"))
        .unwrap_err();

    assert!(matches!(
        error,
        RenderGraphError::InvalidCommandListGroupUsage {
            operation: "create_subnode",
            ..
        }
    ));
    assert_eq!(factory.impl_store().len(), 0);
}

#[test]
fn node_creation_records_group_membership() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let first = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            TestNode::new("first"),
        )
        .unwrap();
    let second = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            TestNode::new("second"),
        )
        .unwrap();

    assert_eq!(factory.group_members(group).unwrap(), &[first, second]);
    assert_eq!(
        factory.graph().node(first).unwrap().metadata().group_id,
        Some(group)
    );
    assert_eq!(
        factory.graph().node(second).unwrap().metadata().group_id,
        Some(group)
    );
}

#[test]
fn direct_links_add_idempotent_dependencies() {
    let mut factory = RenderNodeGraphFactory::new();
    let (_group, parent) = create_group_with_node(&mut factory, "parent");
    let (_group, child) = create_group_with_node(&mut factory, "child");

    factory
        .link_nodes(parent, child, RenderNodeDependencyKind::Gpu)
        .unwrap();
    factory
        .link_nodes(parent, child, RenderNodeDependencyKind::Gpu)
        .unwrap();

    assert_eq!(factory.graph().dependency_count(), 1);
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, parent, child)
        .unwrap());
}

#[test]
fn node_to_group_link_creates_cpu_entry_anchor() {
    let mut factory = RenderNodeGraphFactory::new();
    let (_parent_group, parent) = create_group_with_node(&mut factory, "parent");
    let (child_group, child) = create_group_with_node(&mut factory, "child");

    factory.link_node_to_group(parent, child_group).unwrap();
    let entry = factory.group_entry(child_group).unwrap().unwrap();

    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, parent, entry)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, entry, child)
        .unwrap());
    assert_eq!(
        factory.graph().node(entry).unwrap().role(),
        RenderNodeRole::GroupEntry(child_group)
    );
}

#[test]
fn group_to_node_link_creates_cpu_exit_anchor() {
    let mut factory = RenderNodeGraphFactory::new();
    let (parent_group, parent) = create_group_with_node(&mut factory, "parent");
    let (_child_group, child) = create_group_with_node(&mut factory, "child");

    factory.link_group_to_node(parent_group, child).unwrap();
    let exit = factory.group_exit(parent_group).unwrap().unwrap();

    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, parent, exit)
        .unwrap());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, exit, child)
        .unwrap());
    assert_eq!(
        factory.graph().node(exit).unwrap().role(),
        RenderNodeRole::GroupExit(parent_group)
    );
}

#[test]
fn group_to_group_link_is_cpu_only_and_reuses_stable_anchors() {
    let mut factory = RenderNodeGraphFactory::new();
    let (parent_group, _parent) = create_group_with_node(&mut factory, "parent");
    let (child_group, _child) = create_group_with_node(&mut factory, "child");

    factory
        .link_group_to_group(parent_group, child_group)
        .unwrap();
    factory
        .link_group_to_group(parent_group, child_group)
        .unwrap();
    let exit = factory.group_exit(parent_group).unwrap().unwrap();
    let entry = factory.group_entry(child_group).unwrap().unwrap();

    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, exit, entry)
        .unwrap());
    assert!(!factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Gpu, exit, entry)
        .unwrap());
    assert_eq!(factory.graph().dependency_count(), 3);
}

#[test]
fn empty_group_entry_and_exit_are_bridged() {
    let mut factory = RenderNodeGraphFactory::new();
    let parent = factory.create_group().unwrap();
    let child = factory.create_group().unwrap();
    let (_before_group, before) = create_group_with_node(&mut factory, "before");
    let (_after_group, after) = create_group_with_node(&mut factory, "after");

    factory.link_node_to_group(before, parent).unwrap();
    factory.link_group_to_node(parent, after).unwrap();
    let entry = factory.group_entry(parent).unwrap().unwrap();
    let exit = factory.group_exit(parent).unwrap().unwrap();
    factory.link_group_to_group(parent, child).unwrap();

    assert!(factory.group_members(parent).unwrap().is_empty());
    assert!(factory
        .graph()
        .has_dependency(RenderNodeDependencyKind::Cpu, entry, exit)
        .unwrap());
    assert!(factory.group_entry(child).unwrap().is_some());
}

#[test]
fn finish_builds_flow_groups_only_when_called_explicitly() {
    let mut factory = RenderNodeGraphFactory::new();
    let (_group, node) = create_group_with_node(&mut factory, "node");

    assert!(!factory.graph().is_built());
    let built = factory.finish().unwrap();

    assert!(built.graph().is_built());
    assert_eq!(built.impl_store().len(), 1);
    assert_eq!(
        built
            .graph()
            .node(node)
            .unwrap()
            .metadata()
            .flow_group(RenderNodeDependencyKind::Cpu)
            .unwrap()
            .get(),
        0
    );
}

#[test]
fn reset_clears_graph_groups_and_implementation_store() {
    let mut factory = RenderNodeGraphFactory::new();
    let (_group, _node) = create_group_with_node(&mut factory, "node");

    factory.reset();

    assert_eq!(factory.graph().node_count(), 0);
    assert_eq!(factory.impl_store().len(), 0);
    assert!(factory.created_node_ids().is_empty());
    assert!(factory
        .group_members(super::super::NodeGroupId::from_index(0))
        .is_err());
}

#[test]
fn group_anchors_use_structural_stage_nodes_without_impls() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let (_other_group, node) = create_group_with_node(&mut factory, "node");

    factory.link_group_to_node(group, node).unwrap();
    let exit = factory.group_exit(group).unwrap().unwrap();
    let view = factory.graph().node(exit).unwrap();

    assert_eq!(view.kind(), RenderNodeKind::Stage);
    assert_eq!(view.role(), RenderNodeRole::GroupExit(group));
    assert_eq!(view.impl_id(), None);
    assert_eq!(
        view.params(),
        &RenderNodeParameters::new(
            RenderNodeKind::Stage,
            RenderNodeRole::GroupExit(group),
            RenderNodeSubtype::DEFAULT,
            None,
            RenderNodeDebugName::new(format!("group_{}_exit", group.raw())),
        )
    );
}
