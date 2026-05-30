//! Render nodes and a minimal execution plan.

use super::frame_command_lists::FrameCommandListIndex;
use super::render_context::{NodeRecordContext, RenderContext};
use leet_core::{Leeror, LeetResult};
use std::ops::Range;

/// High-level role of a node inside the render graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderNodeType {
    Unique,
    Stage,
    SequenceBegin,
    SequenceEnd,
    Temporary,
}

/// Type of dependency between render nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderNodeDependencyType {
    Cpu,
    Gpu,
}

/// How a node interacts with command-list lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderNodeCommandListUsage {
    None,
    Require,
    Own,
    Sync,
}

/// GPU queue family targeted by the node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderNodeCommandListType {
    Graphics,
    Compute,
}

/// Common interface for render-graph nodes.
pub trait RenderNode: Send + Sync {
    fn name(&self) -> &str;

    fn node_type(&self) -> RenderNodeType {
        RenderNodeType::Stage
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage;

    fn command_list_type(&self) -> RenderNodeCommandListType {
        RenderNodeCommandListType::Graphics
    }

    fn run(&self, _frame: &RenderContext<'_>) -> LeetResult<()> {
        Err(Leeror::Validation(format!(
            "render node '{}' does not support frame execution",
            self.name(),
        )))
    }

    fn record(&self, _record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        Err(Leeror::Validation(format!(
            "render node '{}' does not support command recording",
            self.name(),
        )))
    }
}

/// One command-list recording task.
pub struct RenderRecordTask {
    slot: FrameCommandListIndex,
    label: Option<String>,
    root: Box<dyn RenderNode>,
    required_nodes: Vec<Box<dyn RenderNode>>,
}

impl RenderRecordTask {
    pub fn new<N>(slot: FrameCommandListIndex, root: N) -> LeetResult<Self>
    where
        N: RenderNode + 'static,
    {
        Self::new_boxed(slot, Box::new(root))
    }

    pub fn new_boxed(slot: FrameCommandListIndex, root: Box<dyn RenderNode>) -> LeetResult<Self> {
        if root.command_list_usage() != RenderNodeCommandListUsage::Own {
            return Err(Leeror::Validation(format!(
                "record task root '{}' must use RenderNodeCommandListUsage::Own",
                root.name(),
            )));
        }

        Ok(Self {
            slot,
            label: None,
            root,
            required_nodes: Vec::new(),
        })
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn add_required_node<N>(&mut self, node: N) -> LeetResult<()>
    where
        N: RenderNode + 'static,
    {
        self.add_required_boxed(Box::new(node))
    }

    pub fn add_required_boxed(&mut self, node: Box<dyn RenderNode>) -> LeetResult<()> {
        if node.command_list_usage() != RenderNodeCommandListUsage::Require {
            return Err(Leeror::Validation(format!(
                "record task child '{}' must use RenderNodeCommandListUsage::Require",
                node.name(),
            )));
        }

        self.required_nodes.push(node);
        Ok(())
    }

    pub fn slot(&self) -> FrameCommandListIndex {
        self.slot
    }

    pub fn required_node_count(&self) -> usize {
        self.required_nodes.len()
    }

    fn execute(&self, frame: &RenderContext<'_>) -> LeetResult<()> {
        let label = self.label.as_deref().or_else(|| Some(self.root.name()));
        let mut record = frame.begin_recording(self.slot, label)?;

        self.root.record(&mut record)?;
        for node in &self.required_nodes {
            node.record(&mut record)?;
        }

        record.finish()
    }
}

/// Frame-scoped node step for `None` and `Sync` nodes.
pub struct RenderFrameNodeStep {
    node: Box<dyn RenderNode>,
}

impl RenderFrameNodeStep {
    pub fn new<N>(node: N) -> LeetResult<Self>
    where
        N: RenderNode + 'static,
    {
        Self::new_boxed(Box::new(node))
    }

    pub fn new_boxed(node: Box<dyn RenderNode>) -> LeetResult<Self> {
        match node.command_list_usage() {
            RenderNodeCommandListUsage::None | RenderNodeCommandListUsage::Sync => {
                Ok(Self { node })
            }
            usage => Err(Leeror::Validation(format!(
                "frame node '{}' cannot use {:?}",
                node.name(),
                usage,
            ))),
        }
    }

    fn execute(&self, frame: &RenderContext<'_>) -> LeetResult<()> {
        self.node.run(frame)
    }
}

/// Ordered execution step inside a compiled render plan.
pub enum RenderExecutionStep {
    Record(RenderRecordTask),
    Frame(RenderFrameNodeStep),
}

impl RenderExecutionStep {
    pub fn as_record(&self) -> Option<&RenderRecordTask> {
        match self {
            Self::Record(task) => Some(task),
            Self::Frame(_) => None,
        }
    }

    pub fn as_frame(&self) -> Option<&RenderFrameNodeStep> {
        match self {
            Self::Record(_) => None,
            Self::Frame(step) => Some(step),
        }
    }
}

/// One GPU-ordered flow group produced by graph compilation.
///
/// This is the first RED-style layer above the flat step list: all steps in a
/// group can be considered part of the same GPU dependency wave, and a later
/// group must not execute until the previous group's GPU work is submitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderFlowGroup {
    gpu_level: usize,
    step_range: Range<usize>,
}

impl RenderFlowGroup {
    pub fn new(gpu_level: usize, step_range: Range<usize>) -> Self {
        Self {
            gpu_level,
            step_range,
        }
    }

    pub fn gpu_level(&self) -> usize {
        self.gpu_level
    }

    pub fn step_range(&self) -> Range<usize> {
        self.step_range.clone()
    }

    pub fn step_count(&self) -> usize {
        self.step_range.len()
    }
}

/// Compiled render plan.
#[derive(Default)]
pub struct RenderExecutionPlan {
    steps: Vec<RenderExecutionStep>,
    flow_groups: Vec<RenderFlowGroup>,
}

impl RenderExecutionPlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_record_task(&mut self, task: RenderRecordTask) -> &mut Self {
        self.steps.push(RenderExecutionStep::Record(task));
        self
    }

    pub(crate) fn push_record_task_with_index(&mut self, task: RenderRecordTask) -> usize {
        let index = self.steps.len();
        self.steps.push(RenderExecutionStep::Record(task));
        index
    }

    pub fn push_frame_node<N>(&mut self, node: N) -> LeetResult<&mut Self>
    where
        N: RenderNode + 'static,
    {
        self.steps
            .push(RenderExecutionStep::Frame(RenderFrameNodeStep::new(node)?));
        Ok(self)
    }

    pub(crate) fn push_frame_step(&mut self, step: RenderFrameNodeStep) -> &mut Self {
        self.steps.push(RenderExecutionStep::Frame(step));
        self
    }

    pub(crate) fn push_flow_group(&mut self, group: RenderFlowGroup) -> &mut Self {
        self.flow_groups.push(group);
        self
    }

    pub(crate) fn record_task_mut(&mut self, step_index: usize) -> Option<&mut RenderRecordTask> {
        match self.steps.get_mut(step_index) {
            Some(RenderExecutionStep::Record(task)) => Some(task),
            _ => None,
        }
    }

    pub(crate) fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub fn command_list_count(&self) -> usize {
        self.steps
            .iter()
            .filter_map(|step| match step {
                RenderExecutionStep::Record(task) => Some(task.slot().get() + 1),
                RenderExecutionStep::Frame(_) => None,
            })
            .max()
            .unwrap_or(0)
    }

    pub fn steps(&self) -> &[RenderExecutionStep] {
        &self.steps
    }

    pub fn flow_groups(&self) -> &[RenderFlowGroup] {
        &self.flow_groups
    }

    pub fn execute(&self, frame: &RenderContext<'_>) -> LeetResult<()> {
        if self.flow_groups.is_empty() {
            for step in &self.steps {
                match step {
                    RenderExecutionStep::Record(task) => task.execute(frame)?,
                    RenderExecutionStep::Frame(node) => node.execute(frame)?,
                }
            }
            return Ok(());
        }

        for group in &self.flow_groups {
            for step in &self.steps[group.step_range()] {
                match step {
                    RenderExecutionStep::Record(task) => task.execute(frame)?,
                    RenderExecutionStep::Frame(node) => node.execute(frame)?,
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/render_graph/badgraph/render_node.rs"]
mod tests;
