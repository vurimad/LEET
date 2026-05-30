use super::super::{
    NodeGroupId, RenderDependencyId, RenderNodeCommandListUsage, RenderNodeDebugName,
    RenderNodeDependencyKind, RenderNodeId, RenderNodeImplId, RenderNodeKind, RenderNodeRole,
    RenderNodeSubtype,
};

fn accepts_node_id(id: RenderNodeId) -> u32 {
    id.raw()
}

fn accepts_dependency_id(id: RenderDependencyId) -> u32 {
    id.raw()
}

#[test]
fn graph_ids_are_typed_copy_values() {
    let node = RenderNodeId::from_index(7);
    let dependency = RenderDependencyId::from_index(7);
    let impl_id = RenderNodeImplId::from_index(2);
    let group = NodeGroupId::from_index(3);

    assert_eq!(accepts_node_id(node), 7);
    assert_eq!(accepts_dependency_id(dependency), 7);
    assert_eq!(impl_id.raw(), 2);
    assert_eq!(group.raw(), 3);

    let copied = node;
    assert_eq!(copied, node);
}

#[test]
fn invalid_graph_ids_are_distinguishable_from_valid_zero_slot_ids() {
    let zero = RenderNodeId::from_index(0);
    assert!(zero.is_valid());
    assert_eq!(zero.index(), Some(0));

    assert!(!RenderNodeId::INVALID.is_valid());
    assert_eq!(RenderNodeId::INVALID.index(), None);
    assert_eq!(RenderNodeId::default(), RenderNodeId::INVALID);
}

#[test]
fn dependency_kind_indexing_is_stable() {
    assert_eq!(RenderNodeDependencyKind::COUNT, 2);
    assert_eq!(RenderNodeDependencyKind::Cpu.as_index(), 0);
    assert_eq!(RenderNodeDependencyKind::Gpu.as_index(), 1);
    assert_eq!(
        RenderNodeDependencyKind::ALL,
        [RenderNodeDependencyKind::Cpu, RenderNodeDependencyKind::Gpu]
    );
}

#[test]
fn command_list_usage_semantics_are_backend_free() {
    assert!(!RenderNodeCommandListUsage::None.uses_command_list());
    assert!(RenderNodeCommandListUsage::Require.uses_command_list());
    assert!(RenderNodeCommandListUsage::Own.uses_command_list());
    assert!(!RenderNodeCommandListUsage::Sync.uses_command_list());

    assert!(RenderNodeCommandListUsage::Require.requires_command_list());
    assert!(RenderNodeCommandListUsage::Own.owns_command_list());
    assert!(RenderNodeCommandListUsage::Sync.is_sync());
}

#[test]
fn node_kind_and_role_are_independent() {
    let group = NodeGroupId::from_index(4);

    assert_eq!(RenderNodeKind::default(), RenderNodeKind::Stage);
    assert_eq!(
        RenderNodeRole::GroupEntry(group),
        RenderNodeRole::GroupEntry(group)
    );

    let kind = RenderNodeKind::Stage;
    let role = RenderNodeRole::GroupExit(group);

    assert_eq!(kind, RenderNodeKind::Stage);
    assert_ne!(role, RenderNodeRole::Normal);
}

#[test]
fn subtype_and_debug_name_are_not_identity() {
    let subtype = RenderNodeSubtype::new(42);
    let label = RenderNodeDebugName::new("gbuffer");

    assert_eq!(RenderNodeSubtype::default(), RenderNodeSubtype::DEFAULT);
    assert_eq!(subtype.get(), 42);
    assert_eq!(label.as_str(), "gbuffer");
}
