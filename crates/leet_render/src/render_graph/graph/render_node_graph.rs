//! Mutable render graph topology.

use std::collections::{HashMap, VecDeque};

use leet_jobs2::Builder as RenderJobBuilder;

use super::{
    storage::GraphStorage, RenderDependencyData, RenderDependencyId, RenderGraphError,
    RenderGraphResult, RenderNodeData, RenderNodeDependencyKind, RenderNodeExecutionMetadata,
    RenderNodeFrameContextInit, RenderNodeId, RenderNodeKind, RenderNodeParameters,
    RenderNodeSubtype, RenderNodeView,
};
use crate::render_graph::resources::RenderFlowGroup;

/// Controls how group metadata is copied during graph import.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddGraphGroupImport {
    /// Preserve group metadata from the imported graph.
    #[default]
    Preserve,
    /// Clear graph-owned group membership metadata on imported nodes.
    ///
    /// This does not rewrite structural node roles. It only drops the
    /// graph-computed `group_id` field so a later grouping pass can rebuild it.
    ClearMetadata,
}

/// Options for importing another graph into this graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddGraphOptions {
    pub force_camera_index: Option<u32>,
    pub merge_special_nodes: bool,
    pub group_import: AddGraphGroupImport,
}

impl Default for AddGraphOptions {
    fn default() -> Self {
        Self {
            force_camera_index: None,
            merge_special_nodes: true,
            group_import: AddGraphGroupImport::Preserve,
        }
    }
}

/// Source-to-destination ids produced by graph import.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphImportMap {
    pub nodes: Vec<(RenderNodeId, RenderNodeId)>,
    pub dependencies: Vec<(RenderDependencyId, RenderDependencyId)>,
    node_lookup: HashMap<RenderNodeId, RenderNodeId>,
    dependency_lookup: HashMap<RenderDependencyId, RenderDependencyId>,
}

impl GraphImportMap {
    pub fn node(&self, source: RenderNodeId) -> Option<RenderNodeId> {
        self.node_lookup.get(&source).copied()
    }

    pub fn dependency(&self, source: RenderDependencyId) -> Option<RenderDependencyId> {
        self.dependency_lookup.get(&source).copied()
    }

    fn insert_node(&mut self, source: RenderNodeId, destination: RenderNodeId) {
        self.nodes.push((source, destination));
        self.node_lookup.insert(source, destination);
    }

    fn insert_dependency(&mut self, source: RenderDependencyId, destination: RenderDependencyId) {
        self.dependencies.push((source, destination));
        self.dependency_lookup.insert(source, destination);
    }
}

/// Node and dependency topology for a render graph.
///
/// This layer owns the linked dependency lists attached to each node. It does
/// not build flow groups, merge imported graphs, own node implementations, or
/// execute work; later passes build those systems on top of these mutation
/// primitives.
#[derive(Debug, Default)]
pub struct RenderNodeGraph {
    nodes: GraphStorage<RenderNodeData, RenderNodeId>,
    dependencies: GraphStorage<RenderDependencyData, RenderDependencyId>,
    flattened_nodes: [Vec<RenderNodeId>; RenderNodeDependencyKind::COUNT],
    topology_frozen: bool,
    flow_groups_built: bool,
}

impl RenderNodeGraph {
    /// Creates an empty mutable graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether topology mutation is locked.
    pub fn is_topology_frozen(&self) -> bool {
        self.topology_frozen
    }

    /// Returns whether flow groups were built and topology is frozen for execution.
    pub fn is_built(&self) -> bool {
        self.flow_groups_built
    }

    /// Freezes topology mutation without computing execution flow groups.
    ///
    /// `build_flow_groups` is the normal transition into an executable graph.
    pub fn freeze_topology(&mut self) {
        self.topology_frozen = true;
    }

    /// Returns the number of live graph nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns the number of live dependency edges.
    pub fn dependency_count(&self) -> usize {
        self.dependencies.len()
    }

    /// Adds a graph node from static parameters and graph-owned execution metadata.
    pub fn add_node(
        &mut self,
        params: RenderNodeParameters,
        metadata: RenderNodeExecutionMetadata,
    ) -> RenderGraphResult<RenderNodeId> {
        self.ensure_mutable("add_node")?;
        self.nodes.allocate(RenderNodeData::new(params, metadata))
    }

    /// Adds a fully constructed node topology entry.
    ///
    /// Dependency heads are cleared before insertion. Edges must be added
    /// through graph mutation APIs so the parent and child lists stay paired.
    pub fn add_node_data(&mut self, mut node: RenderNodeData) -> RenderGraphResult<RenderNodeId> {
        self.ensure_mutable("add_node_data")?;
        node.clear_dependency_heads();
        self.nodes.allocate(node)
    }

    /// Returns a read-only node view.
    pub fn node(&self, id: RenderNodeId) -> RenderGraphResult<RenderNodeView<'_>> {
        Ok(RenderNodeView::new(id, self.nodes.get(id)?))
    }

    /// Returns dependency topology data by id.
    pub fn dependency(&self, id: RenderDependencyId) -> RenderGraphResult<&RenderDependencyData> {
        self.dependencies.get(id)
    }

    /// Returns live node ids in deterministic usage order.
    pub fn node_ids(&self) -> impl Iterator<Item = RenderNodeId> + '_ {
        self.nodes.ids_in_usage_order()
    }

    /// Returns live dependency ids in deterministic usage order.
    pub fn dependency_ids(&self) -> impl Iterator<Item = RenderDependencyId> + '_ {
        self.dependencies.ids_in_usage_order()
    }

    /// Returns graph nodes flattened for one dependency kind.
    pub fn flattened_nodes(&self, kind: RenderNodeDependencyKind) -> &[RenderNodeId] {
        &self.flattened_nodes[kind.as_index()]
    }

    /// Builds per-kind flow groups and flattened node order, then freezes topology.
    pub fn build_flow_groups(&mut self) -> RenderGraphResult<()> {
        self.ensure_mutable("build_flow_groups")?;
        self.validate_no_helper_nodes()?;
        self.validate_dependency_links()?;

        let cpu_flattened = self.build_flattening_for_kind(RenderNodeDependencyKind::Cpu)?;
        let gpu_flattened = self.build_flattening_for_kind(RenderNodeDependencyKind::Gpu)?;

        validate_flow_group_capacity(cpu_flattened.len())?;
        validate_flow_group_capacity(gpu_flattened.len())?;

        for node in self.node_ids().collect::<Vec<_>>() {
            self.nodes.get_mut(node)?.metadata.clear_flow_groups();
        }

        for (kind, flattened) in [
            (RenderNodeDependencyKind::Cpu, cpu_flattened.as_slice()),
            (RenderNodeDependencyKind::Gpu, gpu_flattened.as_slice()),
        ] {
            for (position, node) in flattened.iter().copied().enumerate() {
                self.nodes
                    .get_mut(node)?
                    .metadata
                    .set_flow_group(kind, RenderFlowGroup::new(position as u16));
            }
        }

        self.flattened_nodes[RenderNodeDependencyKind::Cpu.as_index()] = cpu_flattened;
        self.flattened_nodes[RenderNodeDependencyKind::Gpu.as_index()] = gpu_flattened;

        self.validate_flattening(RenderNodeDependencyKind::Cpu)?;
        self.validate_flattening(RenderNodeDependencyKind::Gpu)?;
        self.freeze_topology();
        self.flow_groups_built = true;

        Ok(())
    }

    pub fn execute_parallel(
        &self,
        _frame_context: RenderNodeFrameContextInit<'_>,
        _builder: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        Ok(())
    }

    /// Validates that a built flattened order satisfies all dependencies of one kind.
    ///
    /// This is a build/result validator. Calling it before `build_flow_groups`
    /// intentionally fails because no executable order has been produced yet.
    pub fn validate_flattening(&self, kind: RenderNodeDependencyKind) -> RenderGraphResult<()> {
        let flattened = &self.flattened_nodes[kind.as_index()];
        if flattened.len() != self.nodes.len() {
            return Err(RenderGraphError::InvalidState {
                reason: "flattened graph order does not contain every live node",
            });
        }

        let mut positions = vec![usize::MAX; self.nodes.len()];
        for (position, node) in flattened.iter().copied().enumerate() {
            let usage_index = self
                .nodes
                .usage_index(node)
                .ok_or(RenderGraphError::InvalidId {
                    kind: "node",
                    raw: node.raw(),
                })?;
            if positions[usage_index] != usize::MAX {
                return Err(RenderGraphError::InvalidState {
                    reason: "flattened graph order contains a duplicate node",
                });
            }
            positions[usage_index] = position;
        }

        for node in self.node_ids() {
            let usage_index = self
                .nodes
                .usage_index(node)
                .ok_or(RenderGraphError::InvalidId {
                    kind: "node",
                    raw: node.raw(),
                })?;
            if positions[usage_index] == usize::MAX {
                return Err(RenderGraphError::InvalidState {
                    reason: "flattened graph order omitted a live node",
                });
            }
        }

        for (_, dependency) in self.dependencies.iter() {
            if dependency.kind != kind {
                continue;
            }

            let parent_usage =
                self.nodes
                    .usage_index(dependency.parent)
                    .ok_or(RenderGraphError::InvalidId {
                        kind: "node",
                        raw: dependency.parent.raw(),
                    })?;
            let child_usage =
                self.nodes
                    .usage_index(dependency.child)
                    .ok_or(RenderGraphError::InvalidId {
                        kind: "node",
                        raw: dependency.child.raw(),
                    })?;

            if positions[parent_usage] >= positions[child_usage] {
                return Err(RenderGraphError::InvalidState {
                    reason: "flattened graph order violates dependency direction",
                });
            }
        }

        Ok(())
    }

    /// Imports another graph into this graph, remapping all node and dependency ids.
    ///
    /// Node implementation ids are copied as-is. The graph builder must use a
    /// shared implementation store when importing graphs.
    pub fn add_graph(
        &mut self,
        source: &RenderNodeGraph,
        options: AddGraphOptions,
    ) -> RenderGraphResult<GraphImportMap> {
        self.ensure_mutable("add_graph")?;
        self.validate_dependency_links()?;
        source.validate_dependency_links()?;
        if options.merge_special_nodes {
            self.validate_importable_sequence_pairs()?;
            source.validate_importable_sequence_pairs()?;
        }

        let mut import_map = GraphImportMap::default();

        for (source_node_id, source_node) in source.nodes.iter() {
            let mut imported_node = source_node.clone();

            if let Some(camera_index) = options.force_camera_index {
                imported_node.metadata.camera_index = Some(camera_index);
            }
            if matches!(options.group_import, AddGraphGroupImport::ClearMetadata) {
                imported_node.metadata.group_id = None;
            }
            imported_node.metadata.clear_flow_groups();

            let destination_node_id = self.nodes.allocate(imported_node)?;
            import_map.insert_node(source_node_id, destination_node_id);
        }

        for (source_dependency_id, source_dependency) in source.dependencies.iter() {
            let mut imported_dependency = *source_dependency;
            imported_dependency.parent = map_imported_node(&import_map, source_dependency.parent)?;
            imported_dependency.child = map_imported_node(&import_map, source_dependency.child)?;
            imported_dependency.next_from_parent = None;
            imported_dependency.next_from_child = None;

            let destination_dependency_id = self.dependencies.allocate(imported_dependency)?;
            import_map.insert_dependency(source_dependency_id, destination_dependency_id);
        }

        for index in 0..import_map.dependencies.len() {
            let (source_dependency_id, destination_dependency_id) = import_map.dependencies[index];
            let source_dependency = source.dependencies.get(source_dependency_id)?;
            let destination_dependency = self.dependencies.get_mut(destination_dependency_id)?;
            destination_dependency.next_from_parent =
                map_optional_imported_dependency(&import_map, source_dependency.next_from_parent)?;
            destination_dependency.next_from_child =
                map_optional_imported_dependency(&import_map, source_dependency.next_from_child)?;
        }

        for index in 0..import_map.nodes.len() {
            let (source_node_id, destination_node_id) = import_map.nodes[index];
            let source_node = source.nodes.get(source_node_id)?;
            let destination_node = self.nodes.get_mut(destination_node_id)?;
            for kind in RenderNodeDependencyKind::ALL {
                destination_node.set_first_parent_dependency(
                    kind,
                    map_optional_imported_dependency(
                        &import_map,
                        source_node.first_parent_dependency(kind),
                    )?,
                );
                destination_node.set_first_child_dependency(
                    kind,
                    map_optional_imported_dependency(
                        &import_map,
                        source_node.first_child_dependency(kind),
                    )?,
                );
            }
        }

        self.validate_dependency_links()?;

        if options.merge_special_nodes {
            self.merge_special_nodes()?;
        }

        Ok(import_map)
    }

    /// Adds a dependency edge from `parent` to `child` on the selected dependency track.
    pub fn add_dependency(
        &mut self,
        kind: RenderNodeDependencyKind,
        parent: RenderNodeId,
        child: RenderNodeId,
    ) -> RenderGraphResult<RenderDependencyId> {
        self.ensure_mutable("add_dependency")?;
        self.ensure_node_exists(parent)?;
        self.ensure_node_exists(child)?;

        if parent == child {
            return Err(RenderGraphError::SelfDependency {
                dependency_kind: dependency_kind_name(kind),
                node: parent.raw(),
            });
        }

        if self.has_dependency(kind, parent, child)? {
            return Err(RenderGraphError::DuplicateDependency {
                dependency_kind: dependency_kind_name(kind),
                parent: parent.raw(),
                child: child.raw(),
            });
        }

        let next_from_parent = self.nodes.get(parent)?.first_child_dependency(kind);
        let next_from_child = self.nodes.get(child)?.first_parent_dependency(kind);

        let mut dependency = RenderDependencyData::new(kind, parent, child);
        dependency.next_from_parent = next_from_parent;
        dependency.next_from_child = next_from_child;

        let dependency_id = self.dependencies.allocate(dependency)?;
        self.nodes
            .get_mut(parent)?
            .set_first_child_dependency(kind, Some(dependency_id));
        self.nodes
            .get_mut(child)?
            .set_first_parent_dependency(kind, Some(dependency_id));

        Ok(dependency_id)
    }

    /// Returns whether an exact dependency edge already exists.
    pub fn has_dependency(
        &self,
        kind: RenderNodeDependencyKind,
        parent: RenderNodeId,
        child: RenderNodeId,
    ) -> RenderGraphResult<bool> {
        self.ensure_node_exists(parent)?;
        self.ensure_node_exists(child)?;

        let mut cursor = self.nodes.get(parent)?.first_child_dependency(kind);
        while let Some(dependency_id) = cursor {
            let dependency = self.dependencies.get(dependency_id)?;
            if dependency.kind != kind || dependency.parent != parent {
                return Err(RenderGraphError::InvalidState {
                    reason: "dependency was linked from the wrong parent list",
                });
            }
            if dependency.child == child {
                return Ok(true);
            }
            cursor = dependency.next_from_parent;
        }

        Ok(false)
    }

    /// Removes one dependency edge and unlinks it from both endpoint nodes.
    pub fn remove_dependency(
        &mut self,
        dependency_id: RenderDependencyId,
    ) -> RenderGraphResult<()> {
        self.ensure_mutable("remove_dependency")?;
        let dependency = *self.dependencies.get(dependency_id)?;

        self.unlink_from_parent_list(dependency_id, dependency)?;
        self.unlink_from_child_list(dependency_id, dependency)?;
        self.dependencies.free(dependency_id)?;

        Ok(())
    }

    /// Removes a node and all dependencies attached to it.
    pub fn remove_node(&mut self, node: RenderNodeId) -> RenderGraphResult<RenderNodeData> {
        self.ensure_mutable("remove_node")?;
        self.ensure_node_exists(node)?;

        let dependencies = self.attached_dependencies(node)?;
        for dependency in dependencies {
            if self.dependencies.is_allocated(dependency) {
                self.remove_dependency(dependency)?;
            }
        }

        self.nodes.free(node)
    }

    /// Removes a node, connecting each incoming parent to each outgoing child by kind.
    ///
    /// Existing edges are preserved and not duplicated. Edges that would become
    /// self-dependencies are skipped, which matters when a node is used as a
    /// temporary structural gate inside a larger chain.
    pub fn remove_node_and_bridge_dependencies(
        &mut self,
        node: RenderNodeId,
    ) -> RenderGraphResult<RenderNodeData> {
        self.ensure_mutable("remove_node_and_bridge_dependencies")?;
        self.ensure_node_exists(node)?;

        let incoming = self.parent_nodes_by_kind(node)?;
        let outgoing = self.child_nodes_by_kind(node)?;
        let removed = self.remove_node(node)?;

        for kind in RenderNodeDependencyKind::ALL {
            for parent in &incoming[kind.as_index()] {
                if !self.nodes.is_allocated(*parent) {
                    continue;
                }
                for child in &outgoing[kind.as_index()] {
                    if !self.nodes.is_allocated(*child) || parent == child {
                        continue;
                    }
                    if !self.has_dependency(kind, *parent, *child)? {
                        self.add_dependency(kind, *parent, *child)?;
                    }
                }
            }
        }

        Ok(removed)
    }

    /// Copies incoming dependencies from `source` so they also target `destination`.
    pub fn copy_parent_dependencies(
        &mut self,
        source: RenderNodeId,
        destination: RenderNodeId,
        kind: RenderNodeDependencyKind,
    ) -> RenderGraphResult<()> {
        self.ensure_mutable("copy_parent_dependencies")?;
        self.ensure_node_exists(source)?;
        self.ensure_node_exists(destination)?;

        let parents = self.parent_nodes(source, kind)?;
        for parent in parents {
            if parent != destination && !self.has_dependency(kind, parent, destination)? {
                self.add_dependency(kind, parent, destination)?;
            }
        }

        Ok(())
    }

    /// Copies outgoing dependencies from `source` so they also start at `destination`.
    pub fn copy_child_dependencies(
        &mut self,
        source: RenderNodeId,
        destination: RenderNodeId,
        kind: RenderNodeDependencyKind,
    ) -> RenderGraphResult<()> {
        self.ensure_mutable("copy_child_dependencies")?;
        self.ensure_node_exists(source)?;
        self.ensure_node_exists(destination)?;

        let children = self.child_nodes(source, kind)?;
        for child in children {
            if child != destination && !self.has_dependency(kind, destination, child)? {
                self.add_dependency(kind, destination, child)?;
            }
        }

        Ok(())
    }

    /// Copies all incoming and outgoing dependencies from `source` to `destination`.
    pub fn copy_all_dependencies(
        &mut self,
        source: RenderNodeId,
        destination: RenderNodeId,
    ) -> RenderGraphResult<()> {
        for kind in RenderNodeDependencyKind::ALL {
            self.copy_parent_dependencies(source, destination, kind)?;
            self.copy_child_dependencies(source, destination, kind)?;
        }

        Ok(())
    }

    /// Returns parent nodes for one dependency kind.
    pub fn parent_nodes(
        &self,
        node: RenderNodeId,
        kind: RenderNodeDependencyKind,
    ) -> RenderGraphResult<Vec<RenderNodeId>> {
        self.ensure_node_exists(node)?;
        let mut parents = Vec::new();
        let mut cursor = self.nodes.get(node)?.first_parent_dependency(kind);

        while let Some(dependency_id) = cursor {
            let dependency = self.dependencies.get(dependency_id)?;
            if dependency.kind != kind || dependency.child != node {
                return Err(RenderGraphError::InvalidState {
                    reason: "dependency was linked from the wrong child list",
                });
            }
            parents.push(dependency.parent);
            cursor = dependency.next_from_child;
        }

        Ok(parents)
    }

    /// Returns child nodes for one dependency kind.
    pub fn child_nodes(
        &self,
        node: RenderNodeId,
        kind: RenderNodeDependencyKind,
    ) -> RenderGraphResult<Vec<RenderNodeId>> {
        self.ensure_node_exists(node)?;
        let mut children = Vec::new();
        let mut cursor = self.nodes.get(node)?.first_child_dependency(kind);

        while let Some(dependency_id) = cursor {
            let dependency = self.dependencies.get(dependency_id)?;
            if dependency.kind != kind || dependency.parent != node {
                return Err(RenderGraphError::InvalidState {
                    reason: "dependency was linked from the wrong parent list",
                });
            }
            children.push(dependency.child);
            cursor = dependency.next_from_parent;
        }

        Ok(children)
    }

    /// Verifies dependency ids are reachable from both endpoint linked lists.
    pub fn validate_dependency_links(&self) -> RenderGraphResult<()> {
        for (dependency_id, dependency) in self.dependencies.iter() {
            self.ensure_node_exists(dependency.parent)?;
            self.ensure_node_exists(dependency.child)?;

            if !self.parent_list_contains(dependency_id, *dependency)? {
                return Err(RenderGraphError::InvalidState {
                    reason: "dependency was not reachable from its parent node",
                });
            }
            if !self.child_list_contains(dependency_id, *dependency)? {
                return Err(RenderGraphError::InvalidState {
                    reason: "dependency was not reachable from its child node",
                });
            }
        }

        Ok(())
    }

    /// Merges special construction nodes and removes helper nodes.
    pub fn merge_special_nodes(&mut self) -> RenderGraphResult<()> {
        self.ensure_mutable("merge_special_nodes")?;
        self.validate_sequence_pairs()?;
        self.merge_unique_nodes()?;
        self.merge_sequence_nodes()?;
        self.remove_helper_nodes()?;
        self.validate_no_helper_nodes()
    }

    /// Validates that helper nodes have been removed.
    pub fn validate_no_helper_nodes(&self) -> RenderGraphResult<()> {
        if self.node_ids().any(|node| {
            self.nodes
                .get(node)
                .is_ok_and(|data| is_helper_kind(data.params.kind))
        }) {
            return Err(RenderGraphError::InvalidMerge {
                reason: "helper nodes remain after graph finalization",
            });
        }

        Ok(())
    }

    fn merge_unique_nodes(&mut self) -> RenderGraphResult<()> {
        for group in self.unique_node_groups()? {
            for duplicate in group.duplicates {
                if self.nodes.is_allocated(duplicate) {
                    self.copy_all_dependencies(duplicate, group.survivor)?;
                    self.remove_node(duplicate)?;
                }
            }
        }

        Ok(())
    }

    fn build_flattening_for_kind(
        &self,
        kind: RenderNodeDependencyKind,
    ) -> RenderGraphResult<Vec<RenderNodeId>> {
        let node_ids = self.node_ids().collect::<Vec<_>>();
        let node_count = node_ids.len();
        let mut incoming_counts = vec![0usize; node_count];
        let mut outgoing = vec![Vec::<usize>::new(); node_count];
        let mut levels = vec![0usize; node_count];

        for (_, dependency) in self.dependencies.iter() {
            if dependency.kind != kind {
                continue;
            }

            let parent_index =
                self.nodes
                    .usage_index(dependency.parent)
                    .ok_or(RenderGraphError::InvalidId {
                        kind: "node",
                        raw: dependency.parent.raw(),
                    })?;
            let child_index =
                self.nodes
                    .usage_index(dependency.child)
                    .ok_or(RenderGraphError::InvalidId {
                        kind: "node",
                        raw: dependency.child.raw(),
                    })?;

            outgoing[parent_index].push(child_index);
            incoming_counts[child_index] += 1;
        }

        let mut ready = VecDeque::new();
        for index in 0..node_count {
            if incoming_counts[index] == 0 {
                ready.push_back(index);
            }
        }

        let mut processed = 0usize;
        while let Some(parent_index) = ready.pop_front() {
            processed += 1;
            let child_level = levels[parent_index] + 1;
            for child_index in outgoing[parent_index].iter().copied() {
                levels[child_index] = levels[child_index].max(child_level);
                incoming_counts[child_index] -= 1;
                if incoming_counts[child_index] == 0 {
                    ready.push_back(child_index);
                }
            }
        }

        if processed != node_count {
            return Err(RenderGraphError::CycleDetected {
                dependency_kind: dependency_kind_name(kind),
                remaining_nodes: node_count - processed,
            });
        }

        let max_level = levels.iter().copied().max().unwrap_or(0);
        let mut level_buckets = vec![Vec::new(); max_level + 1];
        for usage_index in 0..node_count {
            // Usage indices are assigned in stable graph insertion/import order,
            // so nodes at the same dependency level flatten deterministically.
            level_buckets[levels[usage_index]].push(usage_index);
        }

        let mut flattened = Vec::with_capacity(node_count);
        for bucket in level_buckets {
            for usage_index in bucket {
                let node = node_ids[usage_index];
                flattened.push(node);
            }
        }

        Ok(flattened)
    }

    fn merge_sequence_nodes(&mut self) -> RenderGraphResult<()> {
        for group in self.sequence_pair_groups()? {
            if group.begins.len() > 1 {
                let survivor_begin = group.begins[0];
                let mut current_end = group.ends[0];
                for index in 1..group.begins.len() {
                    // Chain duplicate pairs after the current survivor tail:
                    // survivor_begin -> survivor_end -> duplicate_begin -> duplicate_end.
                    // The duplicate end then becomes the tail for the next pair.
                    self.stitch_sequence_pair(SequenceMerge {
                        survivor_begin,
                        survivor_end: current_end,
                        duplicate_begin: group.begins[index],
                        duplicate_end: group.ends[index],
                    })?;
                    current_end = group.ends[index];
                }
            }
        }

        Ok(())
    }

    fn stitch_sequence_pair(&mut self, sequence: SequenceMerge) -> RenderGraphResult<()> {
        for kind in RenderNodeDependencyKind::ALL {
            let imported_begin_parents = self.parent_nodes(sequence.duplicate_begin, kind)?;
            for parent in imported_begin_parents {
                if parent != sequence.survivor_begin
                    && !self.has_dependency(kind, parent, sequence.survivor_begin)?
                {
                    self.add_dependency(kind, parent, sequence.survivor_begin)?;
                }
            }

            let original_end_children = self.child_nodes(sequence.survivor_end, kind)?;
            for child in original_end_children {
                if let Some(dependency) =
                    self.find_dependency(kind, sequence.survivor_end, child)?
                {
                    self.remove_dependency(dependency)?;
                }
                if child != sequence.duplicate_end
                    && !self.has_dependency(kind, sequence.duplicate_end, child)?
                {
                    self.add_dependency(kind, sequence.duplicate_end, child)?;
                }
            }

            if !self.has_dependency(kind, sequence.survivor_end, sequence.duplicate_begin)? {
                self.add_dependency(kind, sequence.survivor_end, sequence.duplicate_begin)?;
            }
        }

        Ok(())
    }

    fn remove_helper_nodes(&mut self) -> RenderGraphResult<()> {
        while let Some(helper) = self.find_next_helper_node()? {
            self.remove_node_and_bridge_dependencies(helper)?;
        }

        Ok(())
    }

    fn validate_sequence_pairs(&self) -> RenderGraphResult<()> {
        for group in self.sequence_pair_groups()? {
            if group.begins.len() != group.ends.len() {
                return Err(RenderGraphError::InvalidMerge {
                    reason: "sequence begin/end helper nodes are not paired",
                });
            }
        }

        Ok(())
    }

    fn validate_importable_sequence_pairs(&self) -> RenderGraphResult<()> {
        for group in self.sequence_pair_groups()? {
            if group.begins.len() != group.ends.len() {
                return Err(RenderGraphError::InvalidMerge {
                    reason: "sequence begin/end helper nodes are not paired",
                });
            }

            if group.begins.len() > 1 {
                return Err(RenderGraphError::InvalidMerge {
                    reason: "sequence helper nodes must be unique begin/end pairs per graph side",
                });
            }
        }

        Ok(())
    }

    fn unique_node_groups(&self) -> RenderGraphResult<Vec<UniqueNodeGroup>> {
        let mut groups = Vec::<UniqueNodeGroup>::new();
        let mut group_by_subtype = HashMap::<RenderNodeSubtype, usize>::new();

        for node in self.node_ids() {
            let params = &self.nodes.get(node)?.params;
            if params.kind != RenderNodeKind::Unique {
                continue;
            }

            if let Some(group_index) = group_by_subtype.get(&params.subtype).copied() {
                groups[group_index].duplicates.push(node);
            } else {
                let group_index = groups.len();
                groups.push(UniqueNodeGroup {
                    survivor: node,
                    duplicates: Vec::new(),
                });
                group_by_subtype.insert(params.subtype, group_index);
            }
        }

        groups.retain(|group| !group.duplicates.is_empty());
        Ok(groups)
    }

    fn find_next_helper_node(&self) -> RenderGraphResult<Option<RenderNodeId>> {
        for node in self.node_ids() {
            if is_helper_kind(self.nodes.get(node)?.params.kind) {
                return Ok(Some(node));
            }
        }

        Ok(None)
    }

    fn sequence_pair_groups(&self) -> RenderGraphResult<Vec<SequencePairGroup>> {
        let mut groups = Vec::<SequencePairGroup>::new();
        let mut group_by_subtype = HashMap::<RenderNodeSubtype, usize>::new();

        for node in self.node_ids() {
            let params = &self.nodes.get(node)?.params;
            let is_begin = params.kind == RenderNodeKind::SequenceBegin;
            let is_end = params.kind == RenderNodeKind::SequenceEnd;
            if !is_begin && !is_end {
                continue;
            }

            let group_index = if let Some(index) = group_by_subtype.get(&params.subtype).copied() {
                index
            } else {
                let index = groups.len();
                groups.push(SequencePairGroup {
                    begins: Vec::new(),
                    ends: Vec::new(),
                });
                group_by_subtype.insert(params.subtype, index);
                index
            };

            if is_begin {
                groups[group_index].begins.push(node);
            } else {
                groups[group_index].ends.push(node);
            }
        }

        Ok(groups)
    }

    fn find_dependency(
        &self,
        kind: RenderNodeDependencyKind,
        parent: RenderNodeId,
        child: RenderNodeId,
    ) -> RenderGraphResult<Option<RenderDependencyId>> {
        self.ensure_node_exists(parent)?;
        self.ensure_node_exists(child)?;

        let mut cursor = self.nodes.get(parent)?.first_child_dependency(kind);
        while let Some(dependency_id) = cursor {
            let dependency = self.dependencies.get(dependency_id)?;
            if dependency.kind != kind || dependency.parent != parent {
                return Err(RenderGraphError::InvalidState {
                    reason: "dependency was linked from the wrong parent list",
                });
            }
            if dependency.child == child {
                return Ok(Some(dependency_id));
            }
            cursor = dependency.next_from_parent;
        }

        Ok(None)
    }

    fn parent_nodes_by_kind(
        &self,
        node: RenderNodeId,
    ) -> RenderGraphResult<[Vec<RenderNodeId>; RenderNodeDependencyKind::COUNT]> {
        Ok([
            self.parent_nodes(node, RenderNodeDependencyKind::Cpu)?,
            self.parent_nodes(node, RenderNodeDependencyKind::Gpu)?,
        ])
    }

    fn child_nodes_by_kind(
        &self,
        node: RenderNodeId,
    ) -> RenderGraphResult<[Vec<RenderNodeId>; RenderNodeDependencyKind::COUNT]> {
        Ok([
            self.child_nodes(node, RenderNodeDependencyKind::Cpu)?,
            self.child_nodes(node, RenderNodeDependencyKind::Gpu)?,
        ])
    }

    fn attached_dependencies(
        &self,
        node: RenderNodeId,
    ) -> RenderGraphResult<Vec<RenderDependencyId>> {
        let mut attached = Vec::new();
        for kind in RenderNodeDependencyKind::ALL {
            let node_data = self.nodes.get(node)?;
            collect_dependency_ids(
                &self.dependencies,
                node_data.first_parent_dependency(kind),
                |dependency| dependency.next_from_child,
                &mut attached,
            )?;
            collect_dependency_ids(
                &self.dependencies,
                node_data.first_child_dependency(kind),
                |dependency| dependency.next_from_parent,
                &mut attached,
            )?;
        }

        // A self-edge is rejected, but defensive dedup keeps this safe if a
        // corrupted dependency list references the same edge from both paths.
        attached.sort_unstable();
        attached.dedup();
        Ok(attached)
    }

    fn unlink_from_parent_list(
        &mut self,
        dependency_id: RenderDependencyId,
        dependency: RenderDependencyData,
    ) -> RenderGraphResult<()> {
        let mut cursor = self
            .nodes
            .get(dependency.parent)?
            .first_child_dependency(dependency.kind);
        let mut previous = None;

        while let Some(current_id) = cursor {
            let current = *self.dependencies.get(current_id)?;
            if current_id == dependency_id {
                if let Some(previous_id) = previous {
                    self.dependencies.get_mut(previous_id)?.next_from_parent =
                        current.next_from_parent;
                } else {
                    self.nodes
                        .get_mut(dependency.parent)?
                        .set_first_child_dependency(dependency.kind, current.next_from_parent);
                }
                return Ok(());
            }

            previous = Some(current_id);
            cursor = current.next_from_parent;
        }

        Err(RenderGraphError::InvalidState {
            reason: "dependency was not linked from its parent before removal",
        })
    }

    fn unlink_from_child_list(
        &mut self,
        dependency_id: RenderDependencyId,
        dependency: RenderDependencyData,
    ) -> RenderGraphResult<()> {
        let mut cursor = self
            .nodes
            .get(dependency.child)?
            .first_parent_dependency(dependency.kind);
        let mut previous = None;

        while let Some(current_id) = cursor {
            let current = *self.dependencies.get(current_id)?;
            if current_id == dependency_id {
                if let Some(previous_id) = previous {
                    self.dependencies.get_mut(previous_id)?.next_from_child =
                        current.next_from_child;
                } else {
                    self.nodes
                        .get_mut(dependency.child)?
                        .set_first_parent_dependency(dependency.kind, current.next_from_child);
                }
                return Ok(());
            }

            previous = Some(current_id);
            cursor = current.next_from_child;
        }

        Err(RenderGraphError::InvalidState {
            reason: "dependency was not linked from its child before removal",
        })
    }

    fn parent_list_contains(
        &self,
        dependency_id: RenderDependencyId,
        dependency: RenderDependencyData,
    ) -> RenderGraphResult<bool> {
        let mut cursor = self
            .nodes
            .get(dependency.parent)?
            .first_child_dependency(dependency.kind);

        while let Some(current_id) = cursor {
            if current_id == dependency_id {
                return Ok(true);
            }
            cursor = self.dependencies.get(current_id)?.next_from_parent;
        }

        Ok(false)
    }

    fn child_list_contains(
        &self,
        dependency_id: RenderDependencyId,
        dependency: RenderDependencyData,
    ) -> RenderGraphResult<bool> {
        let mut cursor = self
            .nodes
            .get(dependency.child)?
            .first_parent_dependency(dependency.kind);

        while let Some(current_id) = cursor {
            if current_id == dependency_id {
                return Ok(true);
            }
            cursor = self.dependencies.get(current_id)?.next_from_child;
        }

        Ok(false)
    }

    fn ensure_mutable(&self, operation: &'static str) -> RenderGraphResult<()> {
        if self.topology_frozen {
            return Err(RenderGraphError::GraphAlreadyBuilt { operation });
        }

        Ok(())
    }

    fn ensure_node_exists(&self, node: RenderNodeId) -> RenderGraphResult<()> {
        if self.nodes.is_allocated(node) {
            Ok(())
        } else {
            Err(RenderGraphError::InvalidId {
                kind: "node",
                raw: node.raw(),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SequenceMerge {
    survivor_begin: RenderNodeId,
    survivor_end: RenderNodeId,
    duplicate_begin: RenderNodeId,
    duplicate_end: RenderNodeId,
}

#[derive(Debug)]
struct UniqueNodeGroup {
    survivor: RenderNodeId,
    duplicates: Vec<RenderNodeId>,
}

#[derive(Debug)]
struct SequencePairGroup {
    begins: Vec<RenderNodeId>,
    ends: Vec<RenderNodeId>,
}

fn collect_dependency_ids(
    dependencies: &GraphStorage<RenderDependencyData, RenderDependencyId>,
    mut cursor: Option<RenderDependencyId>,
    next: impl Fn(RenderDependencyData) -> Option<RenderDependencyId>,
    out: &mut Vec<RenderDependencyId>,
) -> RenderGraphResult<()> {
    while let Some(dependency_id) = cursor {
        let dependency = *dependencies.get(dependency_id)?;
        out.push(dependency_id);
        cursor = next(dependency);
    }

    Ok(())
}

fn map_imported_node(
    import_map: &GraphImportMap,
    source: RenderNodeId,
) -> RenderGraphResult<RenderNodeId> {
    import_map
        .node(source)
        .ok_or(RenderGraphError::InvalidMerge {
            reason: "imported dependency referenced a node outside the imported graph",
        })
}

fn map_optional_imported_dependency(
    import_map: &GraphImportMap,
    source: Option<RenderDependencyId>,
) -> RenderGraphResult<Option<RenderDependencyId>> {
    source
        .map(|dependency| {
            import_map
                .dependency(dependency)
                .ok_or(RenderGraphError::InvalidMerge {
                    reason:
                        "imported dependency link referenced an edge outside the imported graph",
                })
        })
        .transpose()
}

fn validate_flow_group_capacity(node_count: usize) -> RenderGraphResult<()> {
    if node_count > usize::from(u16::MAX) + 1 {
        return Err(RenderGraphError::InvalidState {
            reason: "render graph flow group index exceeded u16 range",
        });
    }

    Ok(())
}

const fn is_helper_kind(kind: RenderNodeKind) -> bool {
    matches!(
        kind,
        RenderNodeKind::SequenceBegin | RenderNodeKind::SequenceEnd | RenderNodeKind::Temporary
    )
}

const fn dependency_kind_name(kind: RenderNodeDependencyKind) -> &'static str {
    match kind {
        RenderNodeDependencyKind::Cpu => "CPU",
        RenderNodeDependencyKind::Gpu => "GPU",
    }
}
