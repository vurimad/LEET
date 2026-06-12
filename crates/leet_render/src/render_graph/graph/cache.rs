//! Built render graph cache.

use std::sync::Arc;

use super::{
    AddGraphOptions, CommandListGroupStore, FinalRenderNodeGraph, RenderGraphError,
    RenderGraphResult, RenderNodeGraph, RenderNodeImplStore,
};

const DEFAULT_MAX_CACHE_ENTRIES: usize = 4;
const FNV64_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV64_PRIME: u64 = 0x100000001b3;

/// Deterministic hash for topology-affecting render graph inputs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RenderGraphShapeHash(u64);

impl RenderGraphShapeHash {
    pub const INVALID: Self = Self(FNV64_OFFSET_BASIS);

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn is_valid(self) -> bool {
        self.0 != Self::INVALID.0
    }
}

/// Incremental deterministic graph-shape hash builder.
#[derive(Clone, Debug)]
pub struct RenderGraphShapeHashBuilder {
    hash: u64,
}

impl Default for RenderGraphShapeHashBuilder {
    fn default() -> Self {
        Self {
            hash: FNV64_OFFSET_BASIS,
        }
    }
}

impl RenderGraphShapeHashBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append_u64(&mut self, value: u64) {
        self.append_bytes(&value.to_le_bytes());
    }

    pub fn append_u32(&mut self, value: u32) {
        self.append_bytes(&value.to_le_bytes());
    }

    pub fn append_usize(&mut self, value: usize) {
        self.append_u64(value as u64);
    }

    pub fn append_bool(&mut self, value: bool) {
        self.append_bytes(&[u8::from(value)]);
    }

    pub fn append_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(FNV64_PRIME);
        }
    }

    pub fn finish(self) -> RenderGraphShapeHash {
        RenderGraphShapeHash::from_raw(self.hash)
    }
}

/// Per-camera graph-build data retained by a cache entry.
///
/// Temporary topology may be cleared after final graph construction, but node
/// implementation storage remains alive for as long as the cache entry does.
pub struct RenderGraphCameraBuildData {
    setup_hash: RenderGraphShapeHash,
    temporary_graph: RenderNodeGraph,
    node_impls: RenderNodeImplStore,
    command_groups: CommandListGroupStore,
}

impl RenderGraphCameraBuildData {
    pub fn new(setup_hash: RenderGraphShapeHash) -> Self {
        Self {
            setup_hash,
            temporary_graph: RenderNodeGraph::new(),
            node_impls: RenderNodeImplStore::new(),
            command_groups: CommandListGroupStore::new(),
        }
    }

    pub fn from_built_graph(
        setup_hash: RenderGraphShapeHash,
        built_graph: FinalRenderNodeGraph,
    ) -> Self {
        let (temporary_graph, node_impls, command_groups) = built_graph.into_parts();
        Self {
            setup_hash,
            temporary_graph,
            node_impls,
            command_groups,
        }
    }

    pub fn setup_hash(&self) -> RenderGraphShapeHash {
        self.setup_hash
    }

    pub fn temporary_graph(&self) -> &RenderNodeGraph {
        &self.temporary_graph
    }

    pub fn temporary_graph_mut(&mut self) -> &mut RenderNodeGraph {
        &mut self.temporary_graph
    }

    pub fn node_impls(&self) -> &RenderNodeImplStore {
        &self.node_impls
    }

    pub fn command_groups(&self) -> &CommandListGroupStore {
        &self.command_groups
    }

    fn clear_temporary_topology(&mut self) {
        self.temporary_graph = RenderNodeGraph::new();
        self.command_groups.clear();
    }
}

impl Default for RenderGraphCameraBuildData {
    fn default() -> Self {
        Self::new(RenderGraphShapeHash::INVALID)
    }
}

/// One reusable cache slot.
pub struct RenderGraphCacheEntry {
    shape_hash: RenderGraphShapeHash,
    last_used_frame: u64,
    final_graph: Option<Arc<FinalRenderNodeGraph>>,
    camera_build_data: Vec<RenderGraphCameraBuildData>,
    rebuild_generation: u64,
}

impl RenderGraphCacheEntry {
    fn new(shape_hash: RenderGraphShapeHash, camera_setup_count: usize, frame_tick: u64) -> Self {
        let mut entry = Self {
            shape_hash,
            last_used_frame: frame_tick,
            final_graph: None,
            camera_build_data: Vec::new(),
            rebuild_generation: 0,
        };
        entry.prepare_for_rebuild(shape_hash, camera_setup_count, frame_tick);
        entry
    }

    pub fn shape_hash(&self) -> RenderGraphShapeHash {
        self.shape_hash
    }

    pub fn camera_setup_count(&self) -> usize {
        self.camera_build_data.len()
    }

    pub fn last_used_frame(&self) -> u64 {
        self.last_used_frame
    }

    pub fn rebuild_generation(&self) -> u64 {
        self.rebuild_generation
    }

    pub fn final_graph(&self) -> Option<Arc<FinalRenderNodeGraph>> {
        self.final_graph.as_ref().map(Arc::clone)
    }

    pub fn final_graph_mut(&mut self) -> Option<&mut FinalRenderNodeGraph> {
        self.final_graph.as_mut().and_then(Arc::get_mut)
    }

    pub fn set_final_graph(&mut self, graph: FinalRenderNodeGraph) {
        self.final_graph = Some(Arc::new(graph));
    }

    pub fn ensure_final_graph(&mut self) -> &mut FinalRenderNodeGraph {
        if self.final_graph.is_none() {
            self.final_graph = Some(Arc::new(FinalRenderNodeGraph::from_parts(
                RenderNodeGraph::new(),
                RenderNodeImplStore::new(),
                CommandListGroupStore::new(),
            )));
        }

        Arc::get_mut(self.final_graph.as_mut().unwrap())
            .expect("final render graph cannot be mutated after it is shared")
    }

    pub fn import_camera_setup_graph_to_final(
        &mut self,
        camera_index: usize,
        force_camera_index: u32,
    ) -> RenderGraphResult<()> {
        let Some(camera_data) = self.camera_build_data.get(camera_index) else {
            return Err(RenderGraphError::InvalidId {
                kind: "camera graph build data",
                raw: camera_index as u32,
            });
        };

        if self.final_graph.is_none() {
            self.final_graph = Some(Arc::new(FinalRenderNodeGraph::from_parts(
                RenderNodeGraph::new(),
                RenderNodeImplStore::new(),
                CommandListGroupStore::new(),
            )));
        }

        let source_graph = camera_data.temporary_graph();
        let source_command_groups = camera_data.command_groups();
        let final_graph = Arc::get_mut(self.final_graph.as_mut().unwrap())
            .expect("final render graph cannot be mutated after it is shared");
        let import_map = final_graph.graph_mut().add_graph(
            source_graph,
            AddGraphOptions {
                force_camera_index: Some(force_camera_index),
                merge_special_nodes: true,
                ..AddGraphOptions::default()
            },
        )?;

        final_graph
            .command_group_store_mut()
            .import_from(source_command_groups, &import_map)
    }

    pub fn take_final_graph(&mut self) -> Option<FinalRenderNodeGraph> {
        self.final_graph.take().map(|graph| {
            Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("final render graph is still shared"))
        })
    }

    pub fn camera_build_data(&self) -> &[RenderGraphCameraBuildData] {
        &self.camera_build_data
    }

    pub fn camera_build_data_mut(&mut self) -> &mut [RenderGraphCameraBuildData] {
        &mut self.camera_build_data
    }

    pub fn set_camera_build_data(
        &mut self,
        camera_index: usize,
        data: RenderGraphCameraBuildData,
    ) -> RenderGraphResult<()> {
        let Some(slot) = self.camera_build_data.get_mut(camera_index) else {
            return Err(RenderGraphError::InvalidId {
                kind: "camera graph build data",
                raw: camera_index as u32,
            });
        };

        *slot = data;
        Ok(())
    }

    /// Clears temporary camera topology while retaining implementation storage.
    pub fn post_build_clear(&mut self) {
        for camera_data in &mut self.camera_build_data {
            camera_data.clear_temporary_topology();
        }
    }

    /// Graph cache entries intentionally do not own transient frame resources.
    pub fn has_transient_gpu_resources(&self) -> bool {
        false
    }

    fn is_hit(&self, shape_hash: RenderGraphShapeHash, camera_setup_count: usize) -> bool {
        self.shape_hash == shape_hash
            && self.camera_build_data.len() == camera_setup_count
            && self.final_graph.is_some()
    }

    fn mark_used(&mut self, frame_tick: u64) {
        self.last_used_frame = frame_tick;
    }

    fn prepare_for_rebuild(
        &mut self,
        shape_hash: RenderGraphShapeHash,
        camera_setup_count: usize,
        frame_tick: u64,
    ) {
        self.shape_hash = shape_hash;
        self.last_used_frame = frame_tick;
        self.final_graph = None;
        self.camera_build_data.clear();
        self.camera_build_data
            .resize_with(camera_setup_count, RenderGraphCameraBuildData::default);
        self.rebuild_generation = self.rebuild_generation.saturating_add(1);
    }
}

/// Result of a cache lookup.
pub struct RenderGraphCacheLookup<'a> {
    pub entry_index: usize,
    pub needs_rebuild: bool,
    pub entry: &'a mut RenderGraphCacheEntry,
}

/// Small cache for finalized graph topology and retained node storage.
pub struct RenderGraphCache {
    entries: Vec<RenderGraphCacheEntry>,
    max_entries: usize,
}

impl Default for RenderGraphCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderGraphCache {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_CACHE_ENTRIES)
    }

    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max_entries.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn entries(&self) -> &[RenderGraphCacheEntry] {
        &self.entries
    }

    /// Finds a matching graph entry or reuses the oldest cache slot.
    pub fn get_graph(
        &mut self,
        shape_hash: RenderGraphShapeHash,
        camera_setup_count: usize,
        frame_tick: u64,
        force_clear: bool,
    ) -> RenderGraphResult<RenderGraphCacheLookup<'_>> {
        if !shape_hash.is_valid() {
            return Err(RenderGraphError::InvalidState {
                reason: "render graph cache lookup requires a valid graph-shape hash",
            });
        }

        if !force_clear {
            if let Some(index) = self
                .entries
                .iter()
                .position(|entry| entry.is_hit(shape_hash, camera_setup_count))
            {
                let entry = &mut self.entries[index];
                entry.mark_used(frame_tick);
                return Ok(RenderGraphCacheLookup {
                    entry_index: index,
                    needs_rebuild: false,
                    entry,
                });
            }
        }

        let index = if self.entries.len() < self.max_entries {
            self.entries.push(RenderGraphCacheEntry::new(
                shape_hash,
                camera_setup_count,
                frame_tick,
            ));
            self.entries.len() - 1
        } else {
            let oldest_index = self.oldest_entry_index();
            self.entries[oldest_index].prepare_for_rebuild(
                shape_hash,
                camera_setup_count,
                frame_tick,
            );
            oldest_index
        };

        Ok(RenderGraphCacheLookup {
            entry_index: index,
            needs_rebuild: true,
            entry: &mut self.entries[index],
        })
    }

    fn oldest_entry_index(&self) -> usize {
        self.entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.last_used_frame)
            .map(|(index, _)| index)
            .unwrap_or(0)
    }
}
