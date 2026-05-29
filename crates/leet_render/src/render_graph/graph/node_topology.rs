//! Stored node topology data.

use super::{
    NodeGroupId, RenderDependencyId, RenderNodeDebugName, RenderNodeDependencyKind, RenderNodeId,
    RenderNodeImplId, RenderNodeKind, RenderNodeRole, RenderNodeSubtype,
};
use crate::render_graph::resources::RenderFlowGroup;

/// Static identity for a graph node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderNodeParameters {
    pub kind: RenderNodeKind,
    pub role: RenderNodeRole,
    pub subtype: RenderNodeSubtype,
    pub impl_id: Option<RenderNodeImplId>,
    pub debug_name: RenderNodeDebugName,
}

impl RenderNodeParameters {
    pub fn new(
        kind: RenderNodeKind,
        role: RenderNodeRole,
        subtype: RenderNodeSubtype,
        impl_id: Option<RenderNodeImplId>,
        debug_name: RenderNodeDebugName,
    ) -> Self {
        Self {
            kind,
            role,
            subtype,
            impl_id,
            debug_name,
        }
    }

    pub fn stage(debug_name: impl Into<String>) -> Self {
        Self {
            kind: RenderNodeKind::Stage,
            role: RenderNodeRole::Normal,
            subtype: RenderNodeSubtype::DEFAULT,
            impl_id: None,
            debug_name: RenderNodeDebugName::new(debug_name),
        }
    }
}

impl Default for RenderNodeParameters {
    fn default() -> Self {
        Self {
            kind: RenderNodeKind::Stage,
            role: RenderNodeRole::Normal,
            subtype: RenderNodeSubtype::DEFAULT,
            impl_id: None,
            debug_name: RenderNodeDebugName::default(),
        }
    }
}

/// Graph-owned per-node execution metadata.
///
/// This is not the node-facing implementation context. It stores graph-computed
/// data that will later be used to configure a fresh `RenderNodeImplContext` for
/// each node execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderNodeExecutionMetadata {
    pub camera_index: Option<u32>,
    pub group_id: Option<NodeGroupId>,
    flow_groups: [Option<RenderFlowGroup>; RenderNodeDependencyKind::COUNT],
}

impl RenderNodeExecutionMetadata {
    pub fn new(camera_index: Option<u32>, group_id: Option<NodeGroupId>) -> Self {
        Self {
            camera_index,
            group_id,
            flow_groups: [None; RenderNodeDependencyKind::COUNT],
        }
    }

    pub fn flow_group(self, kind: RenderNodeDependencyKind) -> Option<RenderFlowGroup> {
        self.flow_groups[kind.as_index()]
    }

    pub fn set_flow_group(&mut self, kind: RenderNodeDependencyKind, flow_group: RenderFlowGroup) {
        self.flow_groups[kind.as_index()] = Some(flow_group);
    }

    pub fn clear_flow_groups(&mut self) {
        self.flow_groups = [None; RenderNodeDependencyKind::COUNT];
    }
}

/// Stored graph node topology entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderNodeData {
    pub params: RenderNodeParameters,
    pub metadata: RenderNodeExecutionMetadata,
    first_parent_dep: [Option<RenderDependencyId>; RenderNodeDependencyKind::COUNT],
    first_child_dep: [Option<RenderDependencyId>; RenderNodeDependencyKind::COUNT],
}

impl RenderNodeData {
    pub fn new(params: RenderNodeParameters, metadata: RenderNodeExecutionMetadata) -> Self {
        Self {
            params,
            metadata,
            first_parent_dep: [None; RenderNodeDependencyKind::COUNT],
            first_child_dep: [None; RenderNodeDependencyKind::COUNT],
        }
    }

    pub fn first_parent_dependency(
        &self,
        kind: RenderNodeDependencyKind,
    ) -> Option<RenderDependencyId> {
        self.first_parent_dep[kind.as_index()]
    }

    pub fn set_first_parent_dependency(
        &mut self,
        kind: RenderNodeDependencyKind,
        dependency: Option<RenderDependencyId>,
    ) {
        self.first_parent_dep[kind.as_index()] = dependency;
    }

    pub fn first_child_dependency(
        &self,
        kind: RenderNodeDependencyKind,
    ) -> Option<RenderDependencyId> {
        self.first_child_dep[kind.as_index()]
    }

    pub fn set_first_child_dependency(
        &mut self,
        kind: RenderNodeDependencyKind,
        dependency: Option<RenderDependencyId>,
    ) {
        self.first_child_dep[kind.as_index()] = dependency;
    }

    pub fn clear_dependency_heads(&mut self) {
        self.first_parent_dep = [None; RenderNodeDependencyKind::COUNT];
        self.first_child_dep = [None; RenderNodeDependencyKind::COUNT];
    }
}

impl Default for RenderNodeData {
    fn default() -> Self {
        Self::new(
            RenderNodeParameters::default(),
            RenderNodeExecutionMetadata::default(),
        )
    }
}

/// Stored dependency edge topology entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderDependencyData {
    pub kind: RenderNodeDependencyKind,
    pub parent: RenderNodeId,
    pub child: RenderNodeId,
    pub next_from_parent: Option<RenderDependencyId>,
    pub next_from_child: Option<RenderDependencyId>,
}

impl RenderDependencyData {
    pub fn new(kind: RenderNodeDependencyKind, parent: RenderNodeId, child: RenderNodeId) -> Self {
        Self {
            kind,
            parent,
            child,
            next_from_parent: None,
            next_from_child: None,
        }
    }
}

/// Read-only view over graph node topology data.
#[derive(Clone, Copy, Debug)]
pub struct RenderNodeView<'a> {
    id: RenderNodeId,
    data: &'a RenderNodeData,
}

impl<'a> RenderNodeView<'a> {
    #[allow(dead_code)]
    pub(crate) fn new(id: RenderNodeId, data: &'a RenderNodeData) -> Self {
        Self { id, data }
    }

    pub fn id(self) -> RenderNodeId {
        self.id
    }

    pub fn params(self) -> &'a RenderNodeParameters {
        &self.data.params
    }

    pub fn metadata(self) -> RenderNodeExecutionMetadata {
        self.data.metadata
    }

    pub fn kind(self) -> RenderNodeKind {
        self.data.params.kind
    }

    pub fn role(self) -> RenderNodeRole {
        self.data.params.role
    }

    pub fn subtype(self) -> RenderNodeSubtype {
        self.data.params.subtype
    }

    pub fn impl_id(self) -> Option<RenderNodeImplId> {
        self.data.params.impl_id
    }

    pub fn debug_name(self) -> &'a RenderNodeDebugName {
        &self.data.params.debug_name
    }

    pub fn first_parent_dependency(
        self,
        kind: RenderNodeDependencyKind,
    ) -> Option<RenderDependencyId> {
        self.data.first_parent_dependency(kind)
    }

    pub fn first_child_dependency(
        self,
        kind: RenderNodeDependencyKind,
    ) -> Option<RenderDependencyId> {
        self.data.first_child_dependency(kind)
    }
}
