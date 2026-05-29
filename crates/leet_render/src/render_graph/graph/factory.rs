//! Authoring API for building render node graphs.

use super::{
    CommandListGroupNode, CommandListGroupStore, NodeGroupId, RenderGraphError, RenderGraphResult,
    RenderNodeCommandListUsage, RenderNodeDebugName, RenderNodeDependencyKind,
    RenderNodeExecutionMetadata, RenderNodeGraph, RenderNodeId, RenderNodeImpl,
    RenderNodeImplStore, RenderNodeKind, RenderNodeParameters, RenderNodeRole, RenderNodeSubtype,
    RenderNodeView,
};
use crate::render_graph::resources::RenderQueueKind;

/// Finished graph package produced by `RenderNodeGraphFactory::finish`.
pub struct BuiltRenderNodeGraph {
    graph: RenderNodeGraph,
    impl_store: RenderNodeImplStore,
    command_groups: CommandListGroupStore,
}

impl BuiltRenderNodeGraph {
    /// Returns the built graph topology.
    pub fn graph(&self) -> &RenderNodeGraph {
        &self.graph
    }

    /// Returns the implementation store referenced by the graph.
    pub fn impl_store(&self) -> &RenderNodeImplStore {
        &self.impl_store
    }

    /// Returns command-list groups in deterministic authoring order.
    pub fn command_groups(&self) -> &[CommandListGroupNode] {
        self.command_groups.groups()
    }

    /// Returns the command-list group store referenced by the graph.
    pub fn command_group_store(&self) -> &CommandListGroupStore {
        &self.command_groups
    }

    /// Returns command-list group data by graph-visible wrapper node id.
    pub fn command_list_group(
        &self,
        graph_node: RenderNodeId,
    ) -> RenderGraphResult<&CommandListGroupNode> {
        self.command_groups.get(graph_node)
    }

    /// Consumes the package and returns graph topology, implementations, and command groups.
    pub fn into_parts(self) -> (RenderNodeGraph, RenderNodeImplStore, CommandListGroupStore) {
        (self.graph, self.impl_store, self.command_groups)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NodeGroupData {
    members: Vec<RenderNodeId>,
    entry: Option<RenderNodeId>,
    exit: Option<RenderNodeId>,
}

/// Factory used while authoring a render node graph.
pub struct RenderNodeGraphFactory {
    graph: RenderNodeGraph,
    impl_store: RenderNodeImplStore,
    command_groups: CommandListGroupStore,
    open_command_group: Option<RenderNodeId>,
    groups: Vec<NodeGroupData>,
    created_node_ids: Vec<RenderNodeId>,
}

impl Default for RenderNodeGraphFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNodeGraphFactory {
    /// Creates an empty graph factory.
    pub fn new() -> Self {
        Self {
            graph: RenderNodeGraph::new(),
            impl_store: RenderNodeImplStore::new(),
            command_groups: CommandListGroupStore::new(),
            open_command_group: None,
            groups: Vec::new(),
            created_node_ids: Vec::new(),
        }
    }

    /// Clears the graph, implementation store, groups, and authoring order.
    ///
    /// This is safe because the factory owns all graph ids that point into the
    /// implementation store. External built graphs should use their own store.
    pub fn reset(&mut self) {
        self.graph = RenderNodeGraph::new();
        self.impl_store.clear();
        self.command_groups.clear();
        self.open_command_group = None;
        self.groups.clear();
        self.created_node_ids.clear();
    }

    /// Returns the graph currently being authored.
    pub fn graph(&self) -> &RenderNodeGraph {
        &self.graph
    }

    /// Returns the implementation store currently being authored.
    pub fn impl_store(&self) -> &RenderNodeImplStore {
        &self.impl_store
    }

    /// Returns command-list groups in deterministic authoring order.
    pub fn command_groups(&self) -> &[CommandListGroupNode] {
        self.command_groups.groups()
    }

    /// Returns command-list group data by graph-visible wrapper node id.
    pub fn command_list_group(
        &self,
        graph_node: RenderNodeId,
    ) -> RenderGraphResult<&CommandListGroupNode> {
        self.command_groups.get(graph_node)
    }

    /// Returns the open command-list group wrapper node, if any.
    pub fn open_command_list_group(&self) -> Option<RenderNodeId> {
        self.open_command_group
    }

    /// Returns graph-visible implementation nodes in creation order.
    pub fn created_node_ids(&self) -> &[RenderNodeId] {
        &self.created_node_ids
    }

    /// Creates an empty authoring group.
    pub fn create_group(&mut self) -> RenderGraphResult<NodeGroupId> {
        let index =
            u32::try_from(self.groups.len()).map_err(|_| RenderGraphError::InvalidState {
                reason: "node group id exceeded u32 range",
            })?;
        self.groups.push(NodeGroupData::default());
        Ok(NodeGroupId::from_index(index))
    }

    /// Returns member nodes registered to a group.
    pub fn group_members(&self, group: NodeGroupId) -> RenderGraphResult<&[RenderNodeId]> {
        Ok(&self.group(group)?.members)
    }

    /// Returns a group's entry anchor, if it has been created.
    pub fn group_entry(&self, group: NodeGroupId) -> RenderGraphResult<Option<RenderNodeId>> {
        Ok(self.group(group)?.entry)
    }

    /// Returns a group's exit anchor, if it has been created.
    pub fn group_exit(&self, group: NodeGroupId) -> RenderGraphResult<Option<RenderNodeId>> {
        Ok(self.group(group)?.exit)
    }

    /// Creates a graph-visible node implementation in `group`.
    pub fn create_node<N: RenderNodeImpl>(
        &mut self,
        group: NodeGroupId,
        kind: RenderNodeKind,
        subtype: RenderNodeSubtype,
        node: N,
    ) -> RenderGraphResult<RenderNodeId> {
        self.create_boxed_node(group, kind, subtype, Box::new(node))
    }

    /// Creates a graph-visible boxed node implementation in `group`.
    pub fn create_boxed_node(
        &mut self,
        group: NodeGroupId,
        kind: RenderNodeKind,
        subtype: RenderNodeSubtype,
        node: Box<dyn RenderNodeImpl>,
    ) -> RenderGraphResult<RenderNodeId> {
        self.create_boxed_node_with_role(group, kind, RenderNodeRole::Normal, subtype, node)
    }

    /// Creates a graph-visible lifecycle/system node implementation in `group`.
    pub fn create_system_node<N: RenderNodeImpl>(
        &mut self,
        group: NodeGroupId,
        kind: RenderNodeKind,
        subtype: RenderNodeSubtype,
        node: N,
    ) -> RenderGraphResult<RenderNodeId> {
        self.create_boxed_node_with_role(
            group,
            kind,
            RenderNodeRole::LifecycleSystem,
            subtype,
            Box::new(node),
        )
    }

    fn create_boxed_node_with_role(
        &mut self,
        group: NodeGroupId,
        kind: RenderNodeKind,
        role: RenderNodeRole,
        subtype: RenderNodeSubtype,
        node: Box<dyn RenderNodeImpl>,
    ) -> RenderGraphResult<RenderNodeId> {
        if self.open_command_group.is_some() {
            return Err(RenderGraphError::InvalidCommandListGroupUsage {
                operation: "create_node",
                reason: "a command-list group is already open",
            });
        }
        self.ensure_group_exists(group)?;

        let debug_name = RenderNodeDebugName::new(node.name().to_owned());
        let impl_id = self.impl_store.insert_boxed(node)?;
        let params = RenderNodeParameters::new(kind, role, subtype, Some(impl_id), debug_name);
        let metadata = RenderNodeExecutionMetadata::new(None, Some(group));
        let node_id = self.graph.add_node(params, metadata)?;

        self.created_node_ids.push(node_id);
        self.add_node_to_group(group, node_id)?;
        Ok(node_id)
    }

    /// Opens a graph-visible command-list group node.
    pub fn begin_command_list_group(
        &mut self,
        group: NodeGroupId,
        kind: RenderNodeKind,
        subtype: RenderNodeSubtype,
        name: impl Into<String>,
        queue_kind: RenderQueueKind,
    ) -> RenderGraphResult<RenderNodeId> {
        if self.open_command_group.is_some() {
            return Err(RenderGraphError::InvalidCommandListGroupUsage {
                operation: "begin_command_list_group",
                reason: "command-list groups cannot be nested",
            });
        }
        validate_command_list_group_queue(queue_kind)?;
        self.ensure_group_exists(group)?;

        let debug_name = RenderNodeDebugName::new(name);
        let params = RenderNodeParameters::new(
            kind,
            RenderNodeRole::CommandListGroup,
            subtype,
            None,
            debug_name.clone(),
        );
        let metadata = RenderNodeExecutionMetadata::new(None, Some(group));
        let node_id = self.graph.add_node(params, metadata)?;

        self.command_groups
            .insert(CommandListGroupNode::new(node_id, debug_name, queue_kind))?;
        self.created_node_ids.push(node_id);
        self.add_node_to_group(group, node_id)?;
        self.open_command_group = Some(node_id);

        Ok(node_id)
    }

    /// Creates a subnode inside an open command-list group.
    pub fn create_subnode<N: RenderNodeImpl>(&mut self, node: N) -> RenderGraphResult<()> {
        self.create_boxed_subnode(Box::new(node))
    }

    /// Creates a boxed subnode inside an open command-list group.
    pub fn create_boxed_subnode(&mut self, node: Box<dyn RenderNodeImpl>) -> RenderGraphResult<()> {
        let Some(group_node) = self.open_command_group else {
            return Err(RenderGraphError::InvalidCommandListGroupUsage {
                operation: "create_subnode",
                reason: "no command-list group is open",
            });
        };

        let impl_id = self.impl_store.insert_boxed(node)?;
        self.command_groups
            .get_mut(group_node)?
            .push_subnode(impl_id);
        Ok(())
    }

    /// Closes the currently open command-list group.
    pub fn end_command_list_group(&mut self) -> RenderGraphResult<()> {
        let Some(_group_node) = self.open_command_group.take() else {
            return Err(RenderGraphError::InvalidCommandListGroupUsage {
                operation: "end_command_list_group",
                reason: "no command-list group is open",
            });
        };

        Ok(())
    }

    /// Adds an idempotent dependency between graph-visible nodes.
    pub fn link_nodes(
        &mut self,
        parent: RenderNodeId,
        child: RenderNodeId,
        kind: RenderNodeDependencyKind,
    ) -> RenderGraphResult<()> {
        if !self.graph.has_dependency(kind, parent, child)? {
            self.graph.add_dependency(kind, parent, child)?;
        }
        Ok(())
    }

    /// Adds an idempotent GPU dependency between graph-visible nodes.
    pub fn link_gpu(&mut self, parent: RenderNodeId, child: RenderNodeId) -> RenderGraphResult<()> {
        self.link_nodes(parent, child, RenderNodeDependencyKind::Gpu)
    }

    /// Adds an idempotent CPU dependency between graph-visible nodes.
    pub fn link_cpu(&mut self, parent: RenderNodeId, child: RenderNodeId) -> RenderGraphResult<()> {
        self.link_nodes(parent, child, RenderNodeDependencyKind::Cpu)
    }

    /// Adds GPU dependencies between graph-visible GPU work in creation order.
    pub fn link_created_order_gpu_chain(&mut self) -> RenderGraphResult<()> {
        let gpu_nodes = self.created_gpu_work_nodes()?;
        for pair in gpu_nodes.windows(2) {
            self.link_gpu(pair[0], pair[1])?;
        }

        Ok(())
    }

    /// Adds CPU dependencies between every graph-visible node in creation order.
    pub fn link_created_order_cpu_chain(&mut self) -> RenderGraphResult<()> {
        let nodes = self.created_node_ids.clone();
        for pair in nodes.windows(2) {
            self.link_cpu(pair[0], pair[1])?;
        }

        Ok(())
    }

    /// Adds CPU dependencies from `cpu_node` to later graph-visible GPU work.
    pub fn link_cpu_to_later_gpu_work(&mut self, cpu_node: RenderNodeId) -> RenderGraphResult<()> {
        let start = self.created_node_position(cpu_node)?;
        let targets = self.created_gpu_work_nodes_after(start)?;
        for target in targets {
            self.link_cpu(cpu_node, target)?;
        }

        Ok(())
    }

    /// Adds CPU dependencies from earlier graph-visible GPU work to `cpu_node`.
    pub fn link_cpu_from_earlier_gpu_work(
        &mut self,
        cpu_node: RenderNodeId,
    ) -> RenderGraphResult<()> {
        let end = self.created_node_position(cpu_node)?;
        let sources = self.created_gpu_work_nodes_before(end)?;
        for source in sources {
            self.link_cpu(source, cpu_node)?;
        }

        Ok(())
    }

    /// Adds dependencies from matching graph-visible nodes to `child`.
    pub fn link_matching_to_node(
        &mut self,
        child: RenderNodeId,
        kind: RenderNodeDependencyKind,
        mut predicate: impl FnMut(RenderNodeView<'_>) -> bool,
    ) -> RenderGraphResult<()> {
        self.graph.node(child)?;
        let nodes = self.collect_matching_nodes(&mut predicate)?;
        for parent in nodes {
            if parent != child {
                self.link_nodes(parent, child, kind)?;
            }
        }

        Ok(())
    }

    /// Adds dependencies from `parent` to matching graph-visible nodes.
    pub fn link_node_to_matching(
        &mut self,
        parent: RenderNodeId,
        kind: RenderNodeDependencyKind,
        mut predicate: impl FnMut(RenderNodeView<'_>) -> bool,
    ) -> RenderGraphResult<()> {
        self.graph.node(parent)?;
        let nodes = self.collect_matching_nodes(&mut predicate)?;
        for child in nodes {
            if parent != child {
                self.link_nodes(parent, child, kind)?;
            }
        }

        Ok(())
    }

    /// Adds a CPU dependency from one node to all nodes in a group.
    pub fn link_node_to_group(
        &mut self,
        parent: RenderNodeId,
        child_group: NodeGroupId,
    ) -> RenderGraphResult<()> {
        let entry = self.ensure_group_entry(child_group)?;
        self.link_nodes(parent, entry, RenderNodeDependencyKind::Cpu)
    }

    /// Adds a CPU dependency from all nodes in a group to one node.
    pub fn link_group_to_node(
        &mut self,
        parent_group: NodeGroupId,
        child: RenderNodeId,
    ) -> RenderGraphResult<()> {
        let exit = self.ensure_group_exit(parent_group)?;
        self.link_nodes(exit, child, RenderNodeDependencyKind::Cpu)
    }

    /// Adds a CPU dependency from all nodes in one group to all nodes in another group.
    pub fn link_group_to_group(
        &mut self,
        parent_group: NodeGroupId,
        child_group: NodeGroupId,
    ) -> RenderGraphResult<()> {
        if parent_group == child_group {
            return Err(RenderGraphError::InvalidState {
                reason: "node group cannot depend on itself",
            });
        }

        let exit = self.ensure_group_exit(parent_group)?;
        let entry = self.ensure_group_entry(child_group)?;
        self.link_nodes(exit, entry, RenderNodeDependencyKind::Cpu)
    }

    /// Builds flow groups and returns the finalized graph package.
    pub fn finish(mut self) -> RenderGraphResult<BuiltRenderNodeGraph> {
        if self.open_command_group.is_some() {
            return Err(RenderGraphError::InvalidCommandListGroupUsage {
                operation: "finish",
                reason: "a command-list group is still open",
            });
        }
        self.graph.build_flow_groups()?;
        Ok(BuiltRenderNodeGraph {
            graph: self.graph,
            impl_store: self.impl_store,
            command_groups: self.command_groups,
        })
    }

    fn add_node_to_group(
        &mut self,
        group: NodeGroupId,
        node: RenderNodeId,
    ) -> RenderGraphResult<()> {
        let index = self.group_index(group)?;
        let entry = self.groups[index].entry;
        let exit = self.groups[index].exit;
        self.groups[index].members.push(node);

        if let Some(entry) = entry {
            self.link_nodes(entry, node, RenderNodeDependencyKind::Cpu)?;
        }
        if let Some(exit) = exit {
            self.link_nodes(node, exit, RenderNodeDependencyKind::Cpu)?;
        }

        Ok(())
    }

    fn ensure_group_entry(&mut self, group: NodeGroupId) -> RenderGraphResult<RenderNodeId> {
        let index = self.group_index(group)?;
        if let Some(entry) = self.groups[index].entry {
            return Ok(entry);
        }

        let entry = self.create_group_anchor(group, RenderNodeRole::GroupEntry(group), "entry")?;
        let members = self.groups[index].members.clone();
        self.groups[index].entry = Some(entry);

        for member in members {
            self.link_nodes(entry, member, RenderNodeDependencyKind::Cpu)?;
        }
        self.link_empty_group_bridge(group)?;

        Ok(entry)
    }

    fn ensure_group_exit(&mut self, group: NodeGroupId) -> RenderGraphResult<RenderNodeId> {
        let index = self.group_index(group)?;
        if let Some(exit) = self.groups[index].exit {
            return Ok(exit);
        }

        let exit = self.create_group_anchor(group, RenderNodeRole::GroupExit(group), "exit")?;
        let members = self.groups[index].members.clone();
        self.groups[index].exit = Some(exit);

        for member in members {
            self.link_nodes(member, exit, RenderNodeDependencyKind::Cpu)?;
        }
        self.link_empty_group_bridge(group)?;

        Ok(exit)
    }

    fn create_group_anchor(
        &mut self,
        group: NodeGroupId,
        role: RenderNodeRole,
        suffix: &str,
    ) -> RenderGraphResult<RenderNodeId> {
        let params = RenderNodeParameters::new(
            RenderNodeKind::Stage,
            role,
            RenderNodeSubtype::DEFAULT,
            None,
            RenderNodeDebugName::new(format!("group_{}_{}", group.raw(), suffix)),
        );
        let metadata = RenderNodeExecutionMetadata::new(None, Some(group));
        self.graph.add_node(params, metadata)
    }

    fn link_empty_group_bridge(&mut self, group: NodeGroupId) -> RenderGraphResult<()> {
        let index = self.group_index(group)?;
        if let (Some(entry), Some(exit)) = (self.groups[index].entry, self.groups[index].exit) {
            self.link_empty_group_bridge_if_needed(group, entry, exit)?;
        }

        Ok(())
    }

    fn link_empty_group_bridge_if_needed(
        &mut self,
        group: NodeGroupId,
        entry: RenderNodeId,
        exit: RenderNodeId,
    ) -> RenderGraphResult<()> {
        if self.group(group)?.members.is_empty() {
            self.link_nodes(entry, exit, RenderNodeDependencyKind::Cpu)?;
        }
        Ok(())
    }

    fn group(&self, group: NodeGroupId) -> RenderGraphResult<&NodeGroupData> {
        let index = self.group_index(group)?;
        Ok(&self.groups[index])
    }

    fn group_index(&self, group: NodeGroupId) -> RenderGraphResult<usize> {
        let Some(index) = group.index().and_then(|index| usize::try_from(index).ok()) else {
            return Err(RenderGraphError::InvalidId {
                kind: "node group",
                raw: group.raw(),
            });
        };

        if index >= self.groups.len() {
            return Err(RenderGraphError::InvalidId {
                kind: "node group",
                raw: group.raw(),
            });
        }

        Ok(index)
    }

    fn ensure_group_exists(&self, group: NodeGroupId) -> RenderGraphResult<()> {
        self.group_index(group).map(|_| ())
    }

    fn created_node_position(&self, node: RenderNodeId) -> RenderGraphResult<usize> {
        self.created_node_ids
            .iter()
            .position(|candidate| *candidate == node)
            .ok_or(RenderGraphError::InvalidId {
                kind: "factory-created node",
                raw: node.raw(),
            })
    }

    fn created_gpu_work_nodes(&self) -> RenderGraphResult<Vec<RenderNodeId>> {
        self.created_gpu_work_nodes_in_range(0, self.created_node_ids.len())
    }

    fn created_gpu_work_nodes_before(&self, end: usize) -> RenderGraphResult<Vec<RenderNodeId>> {
        self.created_gpu_work_nodes_in_range(0, end)
    }

    fn created_gpu_work_nodes_after(&self, start: usize) -> RenderGraphResult<Vec<RenderNodeId>> {
        self.created_gpu_work_nodes_in_range(start + 1, self.created_node_ids.len())
    }

    fn created_gpu_work_nodes_in_range(
        &self,
        start: usize,
        end: usize,
    ) -> RenderGraphResult<Vec<RenderNodeId>> {
        let mut nodes = Vec::new();
        for node in self.created_node_ids[start..end].iter().copied() {
            if self.node_has_gpu_work(node)? {
                nodes.push(node);
            }
        }

        Ok(nodes)
    }

    fn node_has_gpu_work(&self, node: RenderNodeId) -> RenderGraphResult<bool> {
        if self.command_groups.get(node).is_ok() {
            return Ok(true);
        }

        let Some(impl_id) = self.graph.node(node)?.impl_id() else {
            return Ok(false);
        };

        Ok(self.impl_store.get(impl_id)?.command_list_usage() != RenderNodeCommandListUsage::None)
    }

    fn collect_matching_nodes(
        &self,
        predicate: &mut impl FnMut(RenderNodeView<'_>) -> bool,
    ) -> RenderGraphResult<Vec<RenderNodeId>> {
        let mut nodes = Vec::new();
        for node in self.created_node_ids.iter().copied() {
            let view = self.graph.node(node)?;
            if predicate(view) {
                nodes.push(node);
            }
        }

        Ok(nodes)
    }
}

fn validate_command_list_group_queue(queue_kind: RenderQueueKind) -> RenderGraphResult<()> {
    if matches!(
        queue_kind,
        RenderQueueKind::Graphics | RenderQueueKind::Compute
    ) {
        Ok(())
    } else {
        Err(RenderGraphError::InvalidCommandListGroupUsage {
            operation: "begin_command_list_group",
            reason: "command-list groups support graphics or compute queues only",
        })
    }
}
