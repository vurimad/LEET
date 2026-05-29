use super::super::{
    AddGraphOptions, RenderGlobalBindingMask, RenderNodeCommandListUsage, RenderNodeDebugName,
    RenderNodeExecutionMetadata, RenderNodeGraph, RenderNodeImpl, RenderNodeImplContext,
    RenderNodeImplId, RenderNodeImplStore, RenderNodeKind, RenderNodeParameters, RenderNodeRole,
    RenderNodeSubtype,
};
use crate::RenderGraphResult;
use leet_jobs2::Builder as RenderJobBuilder;

#[derive(Debug)]
struct TestNodeImpl {
    name: &'static str,
    command_list_usage: RenderNodeCommandListUsage,
    uses_child_jobs: bool,
    allow_gpu_scope: bool,
    binds_render_targets: bool,
    global_binding_mod: RenderGlobalBindingMask,
}

impl TestNodeImpl {
    fn named(name: &'static str) -> Self {
        Self {
            name,
            command_list_usage: RenderNodeCommandListUsage::None,
            uses_child_jobs: false,
            allow_gpu_scope: true,
            binds_render_targets: false,
            global_binding_mod: RenderGlobalBindingMask::empty(),
        }
    }
}

impl RenderNodeImpl for TestNodeImpl {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        self.command_list_usage
    }

    fn execute(
        &self,
        _rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        Ok(())
    }

    fn uses_child_jobs(&self) -> bool {
        self.uses_child_jobs
    }

    fn allow_gpu_scope(&self) -> bool {
        self.allow_gpu_scope
    }

    fn binds_render_targets(&self) -> bool {
        self.binds_render_targets
    }

    fn global_binding_mod(&self) -> RenderGlobalBindingMask {
        self.global_binding_mod
    }
}

#[derive(Debug)]
struct DefaultNodeImpl;

impl RenderNodeImpl for DefaultNodeImpl {
    fn name(&self) -> &str {
        "default-node"
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

fn params_with_impl(name: &str, impl_id: Option<RenderNodeImplId>) -> RenderNodeParameters {
    RenderNodeParameters::new(
        RenderNodeKind::Stage,
        RenderNodeRole::Normal,
        RenderNodeSubtype::DEFAULT,
        impl_id,
        RenderNodeDebugName::new(name),
    )
}

#[test]
fn implementation_store_inserts_and_returns_stable_ids() {
    let mut store = RenderNodeImplStore::new();

    let first = store.insert(TestNodeImpl::named("first")).unwrap();
    let second = store.insert(TestNodeImpl::named("second")).unwrap();

    assert_ne!(first, second);
    assert_eq!(store.len(), 2);
    assert!(store.contains(first));
    assert_eq!(store.get(first).unwrap().name(), "first");
    assert_eq!(store.get(second).unwrap().name(), "second");
    assert_eq!(store.ids().collect::<Vec<_>>(), vec![first, second]);
}

#[test]
fn structural_graph_nodes_can_have_no_implementation() {
    let mut graph = RenderNodeGraph::new();

    let structural = graph
        .add_node(
            params_with_impl("structural", None),
            RenderNodeExecutionMetadata::default(),
        )
        .unwrap();

    assert_eq!(graph.node(structural).unwrap().impl_id(), None);
}

#[test]
fn imported_graph_nodes_keep_valid_shared_implementation_ids() {
    let mut store = RenderNodeImplStore::new();
    let impl_id = store.insert(TestNodeImpl::named("shared-node")).unwrap();
    let mut source = RenderNodeGraph::new();
    let source_node = source
        .add_node(
            params_with_impl("source", Some(impl_id)),
            RenderNodeExecutionMetadata::default(),
        )
        .unwrap();
    let mut target = RenderNodeGraph::new();

    let import_map = target
        .add_graph(&source, AddGraphOptions::default())
        .unwrap();
    let imported_node = import_map.node(source_node).unwrap();

    assert_eq!(target.node(imported_node).unwrap().impl_id(), Some(impl_id));
    assert_eq!(store.get(impl_id).unwrap().name(), "shared-node");
}

#[test]
fn node_impl_trait_metadata_defaults_are_stable() {
    let node = DefaultNodeImpl;

    assert_eq!(node.name(), "default-node");
    assert_eq!(node.command_list_usage(), RenderNodeCommandListUsage::None);
    assert!(!node.uses_child_jobs());
    assert!(node.allow_gpu_scope());
    assert!(!node.binds_render_targets());
    assert_eq!(node.global_binding_mod(), RenderGlobalBindingMask::empty());
}

#[test]
fn global_binding_mask_tracks_slots_and_rejects_out_of_range_slots() {
    let mut mask = RenderGlobalBindingMask::empty();

    mask.insert_slot(3).unwrap();
    mask.insert_slot(63).unwrap();
    let combined = mask.union(RenderGlobalBindingMask::from_bits(1));

    assert!(!mask.is_empty());
    assert!(mask.contains_slot(3));
    assert!(mask.contains_slot(63));
    assert!(!mask.contains_slot(64));
    assert_eq!(combined.bits(), (1u64 << 63) | (1u64 << 3) | 1);
    assert!(mask.insert_slot(64).is_err());
}

#[test]
fn implementation_metadata_overrides_are_visible_through_store() {
    let mut mask = RenderGlobalBindingMask::empty();
    mask.insert_slot(7).unwrap();
    let mut store = RenderNodeImplStore::new();

    let impl_id = store
        .insert(TestNodeImpl {
            name: "metadata-node",
            command_list_usage: RenderNodeCommandListUsage::Own,
            uses_child_jobs: true,
            allow_gpu_scope: false,
            binds_render_targets: true,
            global_binding_mod: mask,
        })
        .unwrap();

    let node = store.get(impl_id).unwrap();
    assert_eq!(node.name(), "metadata-node");
    assert_eq!(node.command_list_usage(), RenderNodeCommandListUsage::Own);
    assert!(node.uses_child_jobs());
    assert!(!node.allow_gpu_scope());
    assert!(node.binds_render_targets());
    assert!(node.global_binding_mod().contains_slot(7));
}
