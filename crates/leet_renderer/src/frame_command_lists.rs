//! Frame-scoped command-list registry.
//!
//! This mirrors an engine-side frame command-list registry at a Rust/wgpu level:
//! recording tasks store finished command buffers into numbered slots, and the
//! frame submits contiguous ranges in dependency order.

use leet_core::{Leeror, LeetResult};
use std::sync::Mutex;

/// Stable slot index for a frame command list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameCommandListIndex(usize);

impl FrameCommandListIndex {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

#[derive(Default)]
struct FrameCommandListsState {
    command_lists: Vec<Option<wgpu::CommandBuffer>>,
    next_flush_start: usize,
}

/// Stores the finished command buffers generated during one frame.
///
/// The registry is internally synchronized so independent recording tasks can
/// finish into it without exclusive access to the parent frame context.
pub struct FrameCommandLists {
    state: Mutex<FrameCommandListsState>,
}

impl FrameCommandLists {
    /// Prepare an empty command-list table for a new frame.
    pub fn prepare_for_frame(num_command_lists: usize) -> Self {
        let mut command_lists = Vec::with_capacity(num_command_lists);
        command_lists.resize_with(num_command_lists, || None);

        Self {
            state: Mutex::new(FrameCommandListsState {
                command_lists,
                next_flush_start: 0,
            }),
        }
    }

    /// Number of command-list slots reserved for this frame.
    pub fn len(&self) -> LeetResult<usize> {
        Ok(self.lock_state()?.command_lists.len())
    }

    /// Returns `true` when the table has no reserved slots.
    pub fn is_empty(&self) -> LeetResult<bool> {
        Ok(self.len()? == 0)
    }

    /// Returns `true` when a finished command buffer has been stored at `index`.
    pub fn has_command_list(&self, index: FrameCommandListIndex) -> LeetResult<bool> {
        let state = self.lock_state()?;
        let slot = Self::slot(&state, index)?;
        Ok(state.command_lists[slot].is_some())
    }

    /// Stores a finished command buffer in a frame slot.
    pub fn set_command_list(
        &self,
        index: FrameCommandListIndex,
        command_buffer: wgpu::CommandBuffer,
    ) -> LeetResult<()> {
        let mut state = self.lock_state()?;
        let slot = Self::slot(&state, index)?;

        if state.command_lists[slot].is_some() {
            return Err(Leeror::Validation(format!(
                "frame command list slot {} was already set",
                slot,
            )));
        }

        state.command_lists[slot] = Some(command_buffer);
        Ok(())
    }

    /// Submit the contiguous command-list range from the current flush cursor
    /// through `up_to_index`, then advance the cursor.
    pub fn submit(
        &self,
        scope_name: &str,
        up_to_index: FrameCommandListIndex,
        queue: &wgpu::Queue,
    ) -> LeetResult<()> {
        let mut state = self.lock_state()?;
        let slot = Self::slot(&state, up_to_index)?;

        if slot < state.next_flush_start {
            return Err(Leeror::Validation(format!(
                "submit range for '{scope_name}' is behind the current flush cursor",
            )));
        }

        let mut command_buffers = Vec::with_capacity(slot - state.next_flush_start + 1);
        for index in state.next_flush_start..=slot {
            let command_buffer = state.command_lists[index].take().ok_or_else(|| {
                Leeror::Runtime(format!(
                    "frame command list slot {} is missing before submit '{}'",
                    index, scope_name,
                ))
            })?;
            command_buffers.push(command_buffer);
        }

        state.next_flush_start = slot + 1;
        drop(state);

        queue.submit(command_buffers);
        Ok(())
    }

    fn lock_state(&self) -> LeetResult<std::sync::MutexGuard<'_, FrameCommandListsState>> {
        self.state
            .lock()
            .map_err(|_| Leeror::Runtime("frame command-list registry was poisoned".to_string()))
    }

    fn slot(state: &FrameCommandListsState, index: FrameCommandListIndex) -> LeetResult<usize> {
        let slot = index.get();
        if slot >= state.command_lists.len() {
            return Err(Leeror::Validation(format!(
                "frame command list slot {} is out of range for {} reserved slots",
                slot,
                state.command_lists.len(),
            )));
        }
        Ok(slot)
    }
}
