//! Render node implementation trait and shared implementation store.

use leet_jobs2::Builder as RenderJobBuilder;

use super::{
    storage::GraphStorage, RenderGraphError, RenderGraphResult, RenderNodeCommandListUsage,
    RenderNodeImplContext, RenderNodeImplId,
};

/// Bit mask of global binding slots modified by a node implementation.
///
/// This is process-wrapper metadata. It is not graph identity and does not
/// create dependency edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct RenderGlobalBindingMask(u64);

impl RenderGlobalBindingMask {
    pub const EMPTY: Self = Self(0);

    pub const fn empty() -> Self {
        Self::EMPTY
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains_slot(self, slot: u8) -> bool {
        if slot >= 64 {
            false
        } else {
            (self.0 & (1u64 << slot)) != 0
        }
    }

    pub fn insert_slot(&mut self, slot: u8) -> RenderGraphResult<()> {
        if slot >= 64 {
            return Err(RenderGraphError::InvalidState {
                reason: "global binding slot index exceeded u64 mask range",
            });
        }

        self.0 |= 1u64 << slot;
        Ok(())
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Executable render graph node implementation.
///
/// The graph stores ids to implementations. The store owns the actual trait
/// objects so imported graphs can preserve implementation ids when they share
/// the same store.
pub trait RenderNodeImpl: Send + Sync + 'static {
    /// Human-readable implementation name used by diagnostics and profiling.
    fn name(&self) -> &str;

    /// Declares how this node interacts with command recording.
    fn command_list_usage(&self) -> RenderNodeCommandListUsage;

    /// Executes node work through the graph process wrapper.
    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()>;

    /// Returns true if this node may dispatch child jobs inside `execute`.
    fn uses_child_jobs(&self) -> bool {
        false
    }

    /// Returns whether the process wrapper may create an outer GPU debug scope.
    fn allow_gpu_scope(&self) -> bool {
        true
    }

    /// Returns whether this node binds or changes render targets.
    fn binds_render_targets(&self) -> bool {
        false
    }

    /// Returns which global binding slots this node modifies.
    fn global_binding_mod(&self) -> RenderGlobalBindingMask {
        RenderGlobalBindingMask::empty()
    }
}

/// Shared arena for render node implementations.
#[derive(Default)]
pub struct RenderNodeImplStore {
    nodes: GraphStorage<Box<dyn RenderNodeImpl>, RenderNodeImplId>,
}

impl RenderNodeImplStore {
    /// Creates an empty implementation store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of live implementations.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns whether the store has no live implementations.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Inserts a concrete implementation and returns its stable id.
    pub fn insert(&mut self, node: impl RenderNodeImpl) -> RenderGraphResult<RenderNodeImplId> {
        self.insert_boxed(Box::new(node))
    }

    /// Inserts an already boxed implementation and returns its stable id.
    pub fn insert_boxed(
        &mut self,
        node: Box<dyn RenderNodeImpl>,
    ) -> RenderGraphResult<RenderNodeImplId> {
        self.nodes.allocate(node)
    }

    /// Returns whether an implementation id is live in this store.
    pub fn contains(&self, id: RenderNodeImplId) -> bool {
        self.nodes.is_allocated(id)
    }

    /// Returns an implementation by id.
    pub fn get(&self, id: RenderNodeImplId) -> RenderGraphResult<&dyn RenderNodeImpl> {
        Ok(self.nodes.get(id)?.as_ref())
    }

    /// Returns a mutable implementation by id.
    pub fn get_mut(&mut self, id: RenderNodeImplId) -> RenderGraphResult<&mut dyn RenderNodeImpl> {
        Ok(self.nodes.get_mut(id)?.as_mut())
    }

    /// Returns implementation ids in deterministic usage order.
    pub fn ids(&self) -> impl Iterator<Item = RenderNodeImplId> + '_ {
        self.nodes.ids_in_usage_order()
    }

    /// Clears all implementations from this store.
    pub fn clear(&mut self) {
        self.nodes.clear();
    }
}
