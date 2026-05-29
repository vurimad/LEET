//! Command-list group authoring data.

use std::collections::HashMap;

use super::{
    GraphImportMap, RenderGraphError, RenderGraphResult, RenderNodeCommandListUsage,
    RenderNodeDebugName, RenderNodeId, RenderNodeImplId,
};
use crate::render_graph::resources::RenderQueueKind;

/// Graph-visible command-list parent plus its ordered internal subnodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandListGroupNode {
    graph_node: RenderNodeId,
    name: RenderNodeDebugName,
    queue_kind: RenderQueueKind,
    subnodes: Vec<RenderNodeImplId>,
}

impl CommandListGroupNode {
    pub(crate) fn new(
        graph_node: RenderNodeId,
        name: RenderNodeDebugName,
        queue_kind: RenderQueueKind,
    ) -> Self {
        Self {
            graph_node,
            name,
            queue_kind,
            subnodes: Vec::new(),
        }
    }

    /// Returns the graph-visible wrapper node id.
    pub fn graph_node(&self) -> RenderNodeId {
        self.graph_node
    }

    /// Returns the diagnostic group name.
    pub fn name(&self) -> &RenderNodeDebugName {
        &self.name
    }

    /// Returns the queue family requested by this command-list group.
    pub fn queue_kind(&self) -> RenderQueueKind {
        self.queue_kind
    }

    /// Returns the command-list behavior of the graph-visible wrapper node.
    pub fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Own
    }

    /// Returns ordered subnode implementation ids.
    pub fn subnodes(&self) -> &[RenderNodeImplId] {
        &self.subnodes
    }

    pub(crate) fn push_subnode(&mut self, subnode: RenderNodeImplId) {
        self.subnodes.push(subnode);
    }

    fn remapped(&self, import_map: &GraphImportMap) -> RenderGraphResult<Self> {
        let graph_node =
            import_map
                .node(self.graph_node)
                .ok_or(RenderGraphError::InvalidMerge {
                    reason: "imported command-list group referenced a node outside the import map",
                })?;

        Ok(Self {
            graph_node,
            name: self.name.clone(),
            queue_kind: self.queue_kind,
            subnodes: self.subnodes.clone(),
        })
    }
}

/// Lookup table for command-list groups keyed by their graph-visible node id.
#[derive(Clone, Debug, Default)]
pub struct CommandListGroupStore {
    groups: Vec<CommandListGroupNode>,
    group_by_node: HashMap<RenderNodeId, usize>,
}

impl CommandListGroupStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.groups.len()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn groups(&self) -> &[CommandListGroupNode] {
        &self.groups
    }

    pub fn get(&self, graph_node: RenderNodeId) -> RenderGraphResult<&CommandListGroupNode> {
        let index = self.group_index(graph_node)?;
        Ok(&self.groups[index])
    }

    /// Imports command-list group metadata after graph topology import.
    ///
    /// Subnode implementation ids are copied as-is. The caller must use the same
    /// implementation store for the imported graph, matching graph topology import.
    pub fn import_from(
        &mut self,
        source: &CommandListGroupStore,
        import_map: &GraphImportMap,
    ) -> RenderGraphResult<()> {
        for group in source.groups() {
            self.insert(group.remapped(import_map)?)?;
        }

        Ok(())
    }

    pub(crate) fn get_mut(
        &mut self,
        graph_node: RenderNodeId,
    ) -> RenderGraphResult<&mut CommandListGroupNode> {
        let index = self.group_index(graph_node)?;
        Ok(&mut self.groups[index])
    }

    pub(crate) fn insert(&mut self, group: CommandListGroupNode) -> RenderGraphResult<()> {
        if self.group_by_node.contains_key(&group.graph_node) {
            return Err(RenderGraphError::InvalidCommandListGroupUsage {
                operation: "begin_command_list_group",
                reason: "graph node already owns a command-list group",
            });
        }

        let index = self.groups.len();
        self.group_by_node.insert(group.graph_node, index);
        self.groups.push(group);
        Ok(())
    }

    pub(crate) fn clear(&mut self) {
        self.groups.clear();
        self.group_by_node.clear();
    }

    fn group_index(&self, graph_node: RenderNodeId) -> RenderGraphResult<usize> {
        self.group_by_node
            .get(&graph_node)
            .copied()
            .ok_or(RenderGraphError::InvalidId {
                kind: "command-list group node",
                raw: graph_node.raw(),
            })
    }
}
