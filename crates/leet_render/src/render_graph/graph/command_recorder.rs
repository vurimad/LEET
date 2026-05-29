//! Frame command recording state used by graph execution.

use super::{
    RenderGraphError, RenderGraphResult, RenderNodeDependencyKind, RenderNodeFrameRuntime,
    RenderNodeGraph, RenderViewportRect,
};
use crate::render_graph::resources::{QueueSyncKind, RenderFlowGroup, RenderQueueKind};

/// Stable command recording slot addressed by render flow group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameCommandRecorderSlot(usize);

impl FrameCommandRecorderSlot {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// State of one prepared command recording slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameCommandRecorderState {
    #[default]
    Empty,
    Recording,
    Finished,
    Submitted,
}

/// Kind of pass currently active in a command recorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameCommandPassKind {
    Render,
    Compute,
}

/// Queue sync recorded by command runtime, separate from allocator queue sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameCommandSyncEvent {
    pub flow_group: RenderFlowGroup,
    pub sync: QueueSyncKind,
    pub label: String,
}

/// Submission metadata produced in deterministic GPU graph order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameCommandSubmission {
    pub slot: FrameCommandRecorderSlot,
    pub flow_group: RenderFlowGroup,
    pub queue: RenderQueueKind,
    pub label: String,
}

#[derive(Clone, Debug)]
struct FrameCommandRecorderSlotData {
    flow_group: RenderFlowGroup,
    queue: Option<RenderQueueKind>,
    label: Option<String>,
    state: FrameCommandRecorderState,
    active_pass: Option<FrameCommandPassKind>,
    viewport: Option<RenderViewportRect>,
    debug_markers: Vec<String>,
    sync_events: Vec<FrameCommandSyncEvent>,
}

impl FrameCommandRecorderSlotData {
    fn new(flow_group: RenderFlowGroup) -> Self {
        Self {
            flow_group,
            queue: None,
            label: None,
            state: FrameCommandRecorderState::Empty,
            active_pass: None,
            viewport: None,
            debug_markers: Vec::new(),
            sync_events: Vec::new(),
        }
    }

    fn clear_for_next_frame(&mut self) {
        let flow_group = self.flow_group;
        *self = Self::new(flow_group);
    }
}

/// Frame-scoped command recorder registry.
#[derive(Clone, Debug, Default)]
pub struct FrameCommandRecorders {
    slots: Vec<FrameCommandRecorderSlotData>,
    submissions: Vec<FrameCommandSubmission>,
}

impl FrameCommandRecorders {
    /// Prepares a frame command recorder table with one slot per GPU flow group.
    pub fn prepare(slot_count: usize) -> RenderGraphResult<Self> {
        if slot_count > usize::from(u16::MAX) + 1 {
            return Err(RenderGraphError::InvalidState {
                reason: "frame command recorder slot count exceeded u16 flow group range",
            });
        }

        let mut slots = Vec::with_capacity(slot_count);
        for index in 0..slot_count {
            slots.push(FrameCommandRecorderSlotData::new(RenderFlowGroup::new(
                index as u16,
            )));
        }

        Ok(Self {
            slots,
            submissions: Vec::new(),
        })
    }

    /// Prepares recorder storage for a built graph.
    pub fn prepare_for_graph(graph: &RenderNodeGraph) -> RenderGraphResult<Self> {
        if !graph.is_built() {
            return Err(RenderGraphError::InvalidState {
                reason: "render graph must be built before preparing command recorders",
            });
        }

        Self::prepare(graph.node_count())
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn submissions(&self) -> &[FrameCommandSubmission] {
        &self.submissions
    }

    pub fn state(
        &self,
        flow_group: RenderFlowGroup,
    ) -> RenderGraphResult<FrameCommandRecorderState> {
        Ok(self.slot_data(flow_group)?.state)
    }

    pub fn active_pass(
        &self,
        flow_group: RenderFlowGroup,
    ) -> RenderGraphResult<Option<FrameCommandPassKind>> {
        Ok(self.slot_data(flow_group)?.active_pass)
    }

    pub fn viewport(
        &self,
        flow_group: RenderFlowGroup,
    ) -> RenderGraphResult<Option<RenderViewportRect>> {
        Ok(self.slot_data(flow_group)?.viewport)
    }

    pub fn debug_markers(&self, flow_group: RenderFlowGroup) -> RenderGraphResult<&[String]> {
        Ok(&self.slot_data(flow_group)?.debug_markers)
    }

    pub fn sync_events(
        &self,
        flow_group: RenderFlowGroup,
    ) -> RenderGraphResult<&[FrameCommandSyncEvent]> {
        Ok(&self.slot_data(flow_group)?.sync_events)
    }

    /// Creates an owned command recorder in the selected flow group.
    pub fn create_own_recorder(
        &mut self,
        flow_group: RenderFlowGroup,
        queue: RenderQueueKind,
        label: impl Into<String>,
    ) -> RenderGraphResult<FrameCommandRecorderSlot> {
        if matches!(queue, RenderQueueKind::Copy) {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "create_own_recorder",
                reason: "frame command recorders support graphics or compute queues only",
            });
        }

        let slot = self.slot(flow_group)?;
        let data = self.slot_data_mut(flow_group)?;
        if data.state != FrameCommandRecorderState::Empty {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "create_own_recorder",
                reason: "command recorder slot is already owned",
            });
        }

        data.queue = Some(queue);
        data.label = Some(label.into());
        data.state = FrameCommandRecorderState::Recording;
        Ok(slot)
    }

    /// Requires an existing active command recorder.
    pub fn require_recorder(
        &self,
        flow_group: RenderFlowGroup,
    ) -> RenderGraphResult<FrameCommandRecorderSlot> {
        let slot = self.slot(flow_group)?;
        let data = self.slot_data(flow_group)?;
        if data.state != FrameCommandRecorderState::Recording {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "require_recorder",
                reason: "command recorder slot is not active",
            });
        }

        Ok(slot)
    }

    pub fn has_command_recorder(&self, flow_group: RenderFlowGroup) -> RenderGraphResult<bool> {
        Ok(matches!(
            self.slot_data(flow_group)?.state,
            FrameCommandRecorderState::Recording | FrameCommandRecorderState::Finished
        ))
    }

    pub fn begin_render_pass(
        &mut self,
        flow_group: RenderFlowGroup,
        label: impl Into<String>,
    ) -> RenderGraphResult<()> {
        self.begin_pass(flow_group, FrameCommandPassKind::Render, label)
    }

    pub fn begin_compute_pass(
        &mut self,
        flow_group: RenderFlowGroup,
        label: impl Into<String>,
    ) -> RenderGraphResult<()> {
        self.begin_pass(flow_group, FrameCommandPassKind::Compute, label)
    }

    pub fn end_pass(&mut self, flow_group: RenderFlowGroup) -> RenderGraphResult<()> {
        let data = self.slot_data_mut(flow_group)?;
        if data.active_pass.is_none() {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "end_pass",
                reason: "no command recorder pass is active",
            });
        }

        data.active_pass = None;
        Ok(())
    }

    pub fn set_viewport(
        &mut self,
        flow_group: RenderFlowGroup,
        viewport: RenderViewportRect,
    ) -> RenderGraphResult<()> {
        let data = self.slot_data_mut(flow_group)?;
        if data.active_pass != Some(FrameCommandPassKind::Render) {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "set_viewport",
                reason: "viewport requires an active render pass",
            });
        }

        data.viewport = Some(viewport);
        Ok(())
    }

    pub fn push_debug_marker(
        &mut self,
        flow_group: RenderFlowGroup,
        label: impl Into<String>,
    ) -> RenderGraphResult<()> {
        let data = self.slot_data_mut(flow_group)?;
        if data.state != FrameCommandRecorderState::Recording {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "push_debug_marker",
                reason: "debug markers require an active command recorder",
            });
        }

        data.debug_markers.push(label.into());
        Ok(())
    }

    pub fn record_sync(
        &mut self,
        flow_group: RenderFlowGroup,
        sync: QueueSyncKind,
        label: impl Into<String>,
    ) -> RenderGraphResult<()> {
        let data = self.slot_data_mut(flow_group)?;
        data.sync_events.push(FrameCommandSyncEvent {
            flow_group,
            sync,
            label: label.into(),
        });
        Ok(())
    }

    pub fn finish_recorder(&mut self, flow_group: RenderFlowGroup) -> RenderGraphResult<()> {
        let data = self.slot_data_mut(flow_group)?;
        if data.state != FrameCommandRecorderState::Recording {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "finish_recorder",
                reason: "command recorder slot is not recording",
            });
        }
        if data.active_pass.is_some() {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "finish_recorder",
                reason: "cannot finish a command recorder while a pass is active",
            });
        }

        data.state = FrameCommandRecorderState::Finished;
        Ok(())
    }

    /// Marks finished command recorders submitted in GPU dependency order.
    pub fn submit_finished_in_gpu_order(
        &mut self,
        graph: &RenderNodeGraph,
    ) -> RenderGraphResult<&[FrameCommandSubmission]> {
        if !graph.is_built() {
            return Err(RenderGraphError::InvalidState {
                reason: "render graph must be built before command submission",
            });
        }

        self.submissions.clear();
        for node in graph
            .flattened_nodes(RenderNodeDependencyKind::Gpu)
            .iter()
            .copied()
        {
            let flow_group = graph
                .node(node)?
                .metadata()
                .flow_group(RenderNodeDependencyKind::Gpu)
                .ok_or(RenderGraphError::InvalidState {
                    reason: "node is missing GPU flow group during command submission",
                })?;
            let data = self.slot_data_mut(flow_group)?;
            match data.state {
                FrameCommandRecorderState::Empty | FrameCommandRecorderState::Submitted => {}
                FrameCommandRecorderState::Recording => {
                    return Err(RenderGraphError::InvalidCommandRecorderUsage {
                        operation: "submit_finished_in_gpu_order",
                        reason: "cannot submit an unfinished command recorder",
                    });
                }
                FrameCommandRecorderState::Finished => {
                    let queue =
                        data.queue
                            .ok_or(RenderGraphError::InvalidCommandRecorderUsage {
                                operation: "submit_finished_in_gpu_order",
                                reason: "finished command recorder is missing queue metadata",
                            })?;
                    let label = data.label.clone().ok_or(
                        RenderGraphError::InvalidCommandRecorderUsage {
                            operation: "submit_finished_in_gpu_order",
                            reason: "finished command recorder is missing debug label",
                        },
                    )?;
                    data.state = FrameCommandRecorderState::Submitted;
                    self.submissions.push(FrameCommandSubmission {
                        slot: FrameCommandRecorderSlot::new(usize::from(flow_group.get())),
                        flow_group,
                        queue,
                        label,
                    });
                }
            }
        }

        Ok(&self.submissions)
    }

    /// Explicitly clears per-frame command recording state.
    pub fn cleanup(&mut self) {
        for slot in &mut self.slots {
            slot.clear_for_next_frame();
        }
        self.submissions.clear();
    }

    fn begin_pass(
        &mut self,
        flow_group: RenderFlowGroup,
        pass: FrameCommandPassKind,
        label: impl Into<String>,
    ) -> RenderGraphResult<()> {
        let data = self.slot_data_mut(flow_group)?;
        if data.state != FrameCommandRecorderState::Recording {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "begin_pass",
                reason: "passes require an active command recorder",
            });
        }
        if data.active_pass.is_some() {
            return Err(RenderGraphError::InvalidCommandRecorderUsage {
                operation: "begin_pass",
                reason: "a command recorder pass is already active",
            });
        }

        data.active_pass = Some(pass);
        data.debug_markers.push(label.into());
        Ok(())
    }

    fn slot(&self, flow_group: RenderFlowGroup) -> RenderGraphResult<FrameCommandRecorderSlot> {
        let index = usize::from(flow_group.get());
        if index >= self.slots.len() {
            return Err(RenderGraphError::InvalidId {
                kind: "frame command recorder slot",
                raw: u32::from(flow_group.get()),
            });
        }

        Ok(FrameCommandRecorderSlot::new(index))
    }

    fn slot_data(
        &self,
        flow_group: RenderFlowGroup,
    ) -> RenderGraphResult<&FrameCommandRecorderSlotData> {
        let slot = self.slot(flow_group)?;
        Ok(&self.slots[slot.get()])
    }

    fn slot_data_mut(
        &mut self,
        flow_group: RenderFlowGroup,
    ) -> RenderGraphResult<&mut FrameCommandRecorderSlotData> {
        let slot = self.slot(flow_group)?;
        Ok(&mut self.slots[slot.get()])
    }
}

impl RenderNodeFrameRuntime for FrameCommandRecorders {
    fn create_command_recorder(
        &mut self,
        flow_group: RenderFlowGroup,
        queue: RenderQueueKind,
        label: &str,
    ) -> RenderGraphResult<()> {
        self.create_own_recorder(flow_group, queue, label)?;
        Ok(())
    }

    fn has_command_recorder(&self, flow_group: RenderFlowGroup) -> RenderGraphResult<bool> {
        FrameCommandRecorders::has_command_recorder(self, flow_group)
    }

    fn set_command_recorder_active(
        &mut self,
        flow_group: RenderFlowGroup,
        active: bool,
    ) -> RenderGraphResult<()> {
        if active {
            self.require_recorder(flow_group)?;
            Ok(())
        } else {
            self.finish_recorder(flow_group)
        }
    }

    fn set_viewport(
        &mut self,
        flow_group: RenderFlowGroup,
        viewport: RenderViewportRect,
    ) -> RenderGraphResult<()> {
        FrameCommandRecorders::set_viewport(self, flow_group, viewport)
    }

    fn record_command_sync(
        &mut self,
        flow_group: RenderFlowGroup,
        sync: QueueSyncKind,
        label: &str,
    ) -> RenderGraphResult<()> {
        FrameCommandRecorders::record_sync(self, flow_group, sync, label)
    }
}
