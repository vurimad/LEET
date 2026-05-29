//! Dense live-order storage for graph topology entries.
//!
//! Graph topology entries keep stable physical slots and a separate dense usage order.
//! Graph import, deterministic iteration, and helper-node removal all depend on
//! this usage-order behavior.

use std::marker::PhantomData;

use super::{ids::GraphStorageId, RenderGraphError, RenderGraphResult};

#[allow(dead_code)]
#[derive(Debug)]
struct GraphSlot<T> {
    value: Option<T>,
    usage_index: Option<usize>,
}

/// Slot-stable storage with dense live usage order.
///
/// Ids point at physical slots. Iteration goes through `usage`, not raw slot
/// order. Freed slots are intentionally not reused during this storage lifetime:
/// that keeps stale slot-index ids invalid without introducing generation bits
/// into the id layout.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct GraphStorage<T, Id> {
    slots: Vec<GraphSlot<T>>,
    usage: Vec<u32>,
    _id: PhantomData<fn() -> Id>,
}

impl<T, Id> Default for GraphStorage<T, Id> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            usage: Vec::new(),
            _id: PhantomData,
        }
    }
}

#[allow(dead_code)]
impl<T, Id> GraphStorage<T, Id>
where
    Id: GraphStorageId,
{
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.usage.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.usage.is_empty()
    }

    pub(crate) fn allocate(&mut self, value: T) -> RenderGraphResult<Id> {
        let slot_index =
            u32::try_from(self.slots.len()).map_err(|_| RenderGraphError::InvalidState {
                reason: "render graph storage exceeded u32 slot id range",
            })?;
        let usage_index = self.usage.len();

        self.slots.push(GraphSlot {
            value: Some(value),
            usage_index: Some(usage_index),
        });
        self.usage.push(slot_index);

        Ok(Id::from_index(slot_index))
    }

    pub(crate) fn free(&mut self, id: Id) -> RenderGraphResult<T> {
        let slot_index = self.valid_slot_index(id)?;
        let usage_index =
            self.slots[slot_index]
                .usage_index
                .ok_or_else(|| RenderGraphError::InvalidId {
                    kind: "graph storage item",
                    raw: id.raw(),
                })?;

        let last_slot = self.usage.pop().ok_or(RenderGraphError::InvalidState {
            reason: "graph storage usage order was empty while freeing a live item",
        })?;

        if usage_index < self.usage.len() {
            self.usage[usage_index] = last_slot;
            let moved_slot =
                usize::try_from(last_slot).map_err(|_| RenderGraphError::InvalidState {
                    reason: "graph storage slot id could not fit usize",
                })?;
            self.slots[moved_slot].usage_index = Some(usage_index);
        }

        let slot = &mut self.slots[slot_index];
        slot.usage_index = None;
        slot.value
            .take()
            .ok_or_else(|| RenderGraphError::InvalidId {
                kind: "graph storage item",
                raw: id.raw(),
            })
    }

    pub(crate) fn get(&self, id: Id) -> RenderGraphResult<&T> {
        let slot_index = self.valid_slot_index(id)?;
        self.slots[slot_index]
            .value
            .as_ref()
            .ok_or(RenderGraphError::InvalidId {
                kind: "graph storage item",
                raw: id.raw(),
            })
    }

    pub(crate) fn get_mut(&mut self, id: Id) -> RenderGraphResult<&mut T> {
        let slot_index = self.valid_slot_index(id)?;
        self.slots[slot_index]
            .value
            .as_mut()
            .ok_or(RenderGraphError::InvalidId {
                kind: "graph storage item",
                raw: id.raw(),
            })
    }

    pub(crate) fn is_allocated(&self, id: Id) -> bool {
        self.slot_index(id)
            .and_then(|slot_index| self.slots.get(slot_index))
            .is_some_and(|slot| slot.value.is_some())
    }

    pub(crate) fn usage_index(&self, id: Id) -> Option<usize> {
        self.slot_index(id)
            .and_then(|slot_index| self.slots.get(slot_index))
            .and_then(|slot| slot.usage_index)
    }

    pub(crate) fn id_by_usage_index(&self, usage_index: usize) -> RenderGraphResult<Id> {
        self.usage
            .get(usage_index)
            .copied()
            .map(Id::from_index)
            .ok_or(RenderGraphError::InvalidId {
                kind: "graph storage usage index",
                raw: u32::try_from(usage_index).unwrap_or(u32::MAX),
            })
    }

    pub(crate) fn ids_in_usage_order(&self) -> impl Iterator<Item = Id> + '_ {
        self.usage.iter().copied().map(Id::from_index)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (Id, &T)> + '_ {
        self.usage.iter().copied().map(|slot| {
            let slot_index =
                usize::try_from(slot).expect("graph storage u32 slot id did not fit usize");
            let value = self.slots[slot_index]
                .value
                .as_ref()
                .expect("graph storage usage order referenced a freed slot");
            (Id::from_index(slot), value)
        })
    }

    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.usage.clear();
    }

    fn valid_slot_index(&self, id: Id) -> RenderGraphResult<usize> {
        let slot_index = self.slot_index(id).ok_or(RenderGraphError::InvalidId {
            kind: "graph storage item",
            raw: id.raw(),
        })?;

        if slot_index >= self.slots.len() {
            return Err(RenderGraphError::InvalidId {
                kind: "graph storage item",
                raw: id.raw(),
            });
        }

        Ok(slot_index)
    }

    fn slot_index(&self, id: Id) -> Option<usize> {
        id.index().and_then(|index| usize::try_from(index).ok())
    }
}
