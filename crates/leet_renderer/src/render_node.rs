//! Render nodes and a minimal execution plan.
//!
//! The core split is:
//! - [`RenderRecordTask`]: one parallel-recordable command-list task
//! - [`RenderFrameNodeStep`]: frame-scoped work such as sync/submit
//! - [`RenderExecutionPlan`]: an ordered list of those steps
//!
//! This follows an explicit command-list ownership model:
//! - `Own` nodes root a record task and own a frame command-list slot
//! - `Require` nodes append work to an already-open record task
//! - `Sync` nodes flush finished frame command lists

use crate::frame_command_lists::FrameCommandListIndex;
use crate::render_collector::CollectedRenderScene;
use crate::render_context::{NodeRecordContext, RenderContext};
use leet_core::{Leeror, LeetResult};
use std::sync::Arc;

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
    /// Node does not use a command list directly.
    None,
    /// Node records into a command list that is already active.
    Require,
    /// Node owns a frame command-list slot and roots a record task.
    Own,
    /// Node does not record directly but is responsible for GPU sync/submit.
    Sync,
}

/// GPU queue family targeted by the node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderNodeCommandListType {
    Graphics,
    Compute,
}

/// Common interface for render-graph nodes.
///
/// The executor chooses which method to call based on
/// [`RenderNode::command_list_usage`]:
/// - `Own` and `Require` nodes run through [`RenderNode::record`]
/// - `None` and `Sync` nodes run through [`RenderNode::run`]
pub trait RenderNode: Send + Sync {
    /// Debug/display name for the node.
    fn name(&self) -> &str;

    /// Broad graph role of this node.
    fn node_type(&self) -> RenderNodeType {
        RenderNodeType::Stage
    }

    /// Command-list ownership behaviour of this node.
    fn command_list_usage(&self) -> RenderNodeCommandListUsage;

    /// Queue family targeted by the node.
    fn command_list_type(&self) -> RenderNodeCommandListType {
        RenderNodeCommandListType::Graphics
    }

    /// Execute frame-scoped work that does not directly record commands.
    fn run(&self, _frame: &RenderContext<'_>) -> LeetResult<()> {
        Err(Leeror::Validation(format!(
            "render node '{}' does not support frame execution",
            self.name(),
        )))
    }

    /// Record GPU commands into an already-open encoder.
    fn record(&self, _record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        Err(Leeror::Validation(format!(
            "render node '{}' does not support command recording",
            self.name(),
        )))
    }
}

/// One command-list recording task.
///
/// A record task is the unit that can later be recorded in parallel. It owns a
/// single frame command-list slot, has one `Own` root node, and can append any
/// number of `Require` child nodes that record into the same encoder.
pub struct RenderRecordTask {
    slot: FrameCommandListIndex,
    label: Option<String>,
    root: Box<dyn RenderNode>,
    required_nodes: Vec<Box<dyn RenderNode>>,
}

impl RenderRecordTask {
    /// Create a new record task rooted by an `Own` node.
    pub fn new<N>(slot: FrameCommandListIndex, root: N) -> LeetResult<Self>
    where
        N: RenderNode + 'static,
    {
        Self::new_boxed(slot, Box::new(root))
    }

    /// Create a new record task rooted by an owned node object.
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

    /// Set the command-encoder label used by this task.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Append a `Require` node to the record task.
    pub fn add_required_node<N>(&mut self, node: N) -> LeetResult<()>
    where
        N: RenderNode + 'static,
    {
        self.add_required_boxed(Box::new(node))
    }

    /// Append a `Require` node to the record task.
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

    /// Slot owned by this record task.
    pub fn slot(&self) -> FrameCommandListIndex {
        self.slot
    }

    /// Number of `Require` nodes attached to this record task.
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
    /// Create a new frame step rooted by a `None` or `Sync` node.
    pub fn new<N>(node: N) -> LeetResult<Self>
    where
        N: RenderNode + 'static,
    {
        Self::new_boxed(Box::new(node))
    }

    /// Create a new frame step from an owned node object.
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
    /// Returns the step as a record task when applicable.
    pub fn as_record(&self) -> Option<&RenderRecordTask> {
        match self {
            Self::Record(task) => Some(task),
            Self::Frame(_) => None,
        }
    }

    /// Returns the step as a frame node when applicable.
    pub fn as_frame(&self) -> Option<&RenderFrameNodeStep> {
        match self {
            Self::Record(_) => None,
            Self::Frame(step) => Some(step),
        }
    }
}

/// Minimal compiled render plan.
///
/// The current executor is serial, but `Record` steps are intentionally shaped
/// as future job-system tasks: each one owns an isolated frame command-list
/// slot and can later be scheduled in parallel before sync/submit steps flush
/// those finished slots.
#[derive(Default)]
pub struct RenderExecutionPlan {
    steps: Vec<RenderExecutionStep>,
}

impl RenderExecutionPlan {
    /// Create an empty plan.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a record task to the plan.
    pub fn push_record_task(&mut self, task: RenderRecordTask) -> &mut Self {
        self.steps.push(RenderExecutionStep::Record(task));
        self
    }

    /// Add a record task and return its step index.
    pub(crate) fn push_record_task_with_index(&mut self, task: RenderRecordTask) -> usize {
        let index = self.steps.len();
        self.steps.push(RenderExecutionStep::Record(task));
        index
    }

    /// Add a frame-scoped node to the plan.
    pub fn push_frame_node<N>(&mut self, node: N) -> LeetResult<&mut Self>
    where
        N: RenderNode + 'static,
    {
        self.steps
            .push(RenderExecutionStep::Frame(RenderFrameNodeStep::new(node)?));
        Ok(self)
    }

    /// Add a frame-scoped step object to the plan.
    pub(crate) fn push_frame_step(&mut self, step: RenderFrameNodeStep) -> &mut Self {
        self.steps.push(RenderExecutionStep::Frame(step));
        self
    }

    /// Mutable access to a record task by step index.
    pub(crate) fn record_task_mut(&mut self, step_index: usize) -> Option<&mut RenderRecordTask> {
        match self.steps.get_mut(step_index) {
            Some(RenderExecutionStep::Record(task)) => Some(task),
            _ => None,
        }
    }

    /// Number of frame command-list slots required to execute this plan.
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

    /// Read-only access to the compiled execution steps.
    pub fn steps(&self) -> &[RenderExecutionStep] {
        &self.steps
    }

    /// Execute the plan against one frame context.
    pub fn execute(&self, frame: &RenderContext<'_>) -> LeetResult<()> {
        for step in &self.steps {
            match step {
                RenderExecutionStep::Record(task) => task.execute(frame)?,
                RenderExecutionStep::Frame(node) => node.execute(frame)?,
            }
        }

        Ok(())
    }
}

/// Node that clears the current backbuffer.
pub struct ClearBackbufferNode {
    name: String,
    color: wgpu::Color,
}

impl ClearBackbufferNode {
    pub fn new(color: wgpu::Color) -> Self {
        Self {
            name: "ClearBackbuffer".to_string(),
            color,
        }
    }

    pub fn named(name: impl Into<String>, color: wgpu::Color) -> Self {
        Self {
            name: name.into(),
            color,
        }
    }
}

impl RenderNode for ClearBackbufferNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Own
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        record.encode_backbuffer_pass(Some(self.name()), wgpu::LoadOp::Clear(self.color), |_| {});
        Ok(())
    }
}

/// Frame-scoped start marker for a render graph.
pub struct StartFrameNode {
    name: String,
}

impl StartFrameNode {
    pub fn new() -> Self {
        Self {
            name: "StartFrame".to_string(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Default for StartFrameNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for StartFrameNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> RenderNodeType {
        RenderNodeType::Unique
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn run(&self, _frame: &RenderContext<'_>) -> LeetResult<()> {
        Ok(())
    }
}

/// Root node for the placeholder main pass.
pub struct MainPassRootNode {
    name: String,
    clear_color: wgpu::Color,
    scene: Option<Arc<CollectedRenderScene>>,
}

impl MainPassRootNode {
    pub fn new(clear_color: wgpu::Color) -> Self {
        Self {
            name: "MainPassRoot".to_string(),
            clear_color,
            scene: None,
        }
    }

    pub fn named(name: impl Into<String>, clear_color: wgpu::Color) -> Self {
        Self {
            name: name.into(),
            clear_color,
            scene: None,
        }
    }

    pub fn for_scene(scene: Arc<CollectedRenderScene>, clear_color: wgpu::Color) -> Self {
        Self {
            name: "MainPassRoot".to_string(),
            clear_color,
            scene: Some(scene),
        }
    }

    fn scene_counts(&self) -> (usize, usize) {
        self.scene.as_ref().map_or((0, 0), |scene| {
            (scene.opaque_proxies().len(), scene.sky_proxies().len())
        })
    }
}

impl RenderNode for MainPassRootNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Own
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        let (opaque_count, sky_count) = self.scene_counts();
        let begin_marker = format!(
            "MainPassRoot_Begin opaque={} sky={}",
            opaque_count, sky_count
        );
        record.encoder().insert_debug_marker(&begin_marker);
        let pass_marker = format!(
            "MainPassRoot_Pass opaque={} sky={}",
            opaque_count, sky_count
        );
        record.encode_backbuffer_pass(
            Some(self.name()),
            wgpu::LoadOp::Clear(self.clear_color),
            |pass| {
                pass.insert_debug_marker(&pass_marker);
            },
        );
        let end_marker = format!("MainPassRoot_End total={}", opaque_count + sky_count);
        record.encoder().insert_debug_marker(&end_marker);
        Ok(())
    }
}

/// Placeholder opaque draw list appended into the main pass task.
pub struct OpaqueDrawsNode {
    name: String,
    scene: Option<Arc<CollectedRenderScene>>,
}

impl OpaqueDrawsNode {
    pub fn new() -> Self {
        Self {
            name: "OpaqueDraws".to_string(),
            scene: None,
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scene: None,
        }
    }

    pub fn for_scene(scene: Arc<CollectedRenderScene>) -> Self {
        Self {
            name: "OpaqueDraws".to_string(),
            scene: Some(scene),
        }
    }
}

impl Default for OpaqueDrawsNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for OpaqueDrawsNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Require
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        if let Some(scene) = &self.scene {
            let group_name = format!("OpaqueDraws count={}", scene.opaque_proxies().len());
            record.encoder().push_debug_group(&group_name);
            for proxy in scene.opaque_proxies() {
                let marker = format!(
                    "OpaqueProxy id={} name={} pos=({:.2},{:.2},{:.2})",
                    proxy.id().get(),
                    proxy.name(),
                    proxy.translation().x,
                    proxy.translation().y,
                    proxy.translation().z,
                );
                record.encoder().insert_debug_marker(&marker);
            }
            record.encoder().pop_debug_group();
        } else {
            record.encoder().insert_debug_marker(self.name());
        }
        Ok(())
    }
}

/// Placeholder sky draw list appended into the main pass task.
pub struct SkyDrawsNode {
    name: String,
    scene: Option<Arc<CollectedRenderScene>>,
}

impl SkyDrawsNode {
    pub fn new() -> Self {
        Self {
            name: "SkyDraws".to_string(),
            scene: None,
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scene: None,
        }
    }

    pub fn for_scene(scene: Arc<CollectedRenderScene>) -> Self {
        Self {
            name: "SkyDraws".to_string(),
            scene: Some(scene),
        }
    }
}

impl Default for SkyDrawsNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for SkyDrawsNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Require
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        if let Some(scene) = &self.scene {
            let group_name = format!("SkyDraws count={}", scene.sky_proxies().len());
            record.encoder().push_debug_group(&group_name);
            for proxy in scene.sky_proxies() {
                let marker = format!(
                    "SkyProxy id={} name={} color=({:.2},{:.2},{:.2})",
                    proxy.id().get(),
                    proxy.name(),
                    proxy.debug_color().r,
                    proxy.debug_color().g,
                    proxy.debug_color().b,
                );
                record.encoder().insert_debug_marker(&marker);
            }
            record.encoder().pop_debug_group();
        } else {
            record.encoder().insert_debug_marker(self.name());
        }
        Ok(())
    }
}

/// Placeholder post-process pass that runs in a later GPU level.
pub struct BloomNode {
    name: String,
    scene: Option<Arc<CollectedRenderScene>>,
}

impl BloomNode {
    pub fn new() -> Self {
        Self {
            name: "Bloom".to_string(),
            scene: None,
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scene: None,
        }
    }

    pub fn for_scene(scene: Arc<CollectedRenderScene>) -> Self {
        Self {
            name: "Bloom".to_string(),
            scene: Some(scene),
        }
    }
}

impl Default for BloomNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for BloomNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Own
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        let total_inputs = self
            .scene
            .as_ref()
            .map_or(0, |scene| scene.total_proxy_count());
        let begin_marker = format!("Bloom_Begin inputs={}", total_inputs);
        record.encoder().insert_debug_marker(&begin_marker);
        let pass_marker = format!("Bloom_Pass inputs={}", total_inputs);
        record.encode_backbuffer_pass(Some(self.name()), wgpu::LoadOp::Load, |pass| {
            pass.insert_debug_marker(&pass_marker);
        });
        let end_marker = format!("Bloom_End inputs={}", total_inputs);
        record.encoder().insert_debug_marker(&end_marker);
        Ok(())
    }
}

/// Frame-scoped end marker for a render graph.
pub struct EndFrameNode {
    name: String,
}

impl EndFrameNode {
    pub fn new() -> Self {
        Self {
            name: "EndFrame".to_string(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Default for EndFrameNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for EndFrameNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> RenderNodeType {
        RenderNodeType::Unique
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn run(&self, _frame: &RenderContext<'_>) -> LeetResult<()> {
        Ok(())
    }
}

/// Node that submits all frame command lists through a target slot.
pub struct SubmitCommandListsNode {
    scope_name: String,
    up_to_slot: FrameCommandListIndex,
}

impl SubmitCommandListsNode {
    pub fn new(scope_name: impl Into<String>, up_to_slot: FrameCommandListIndex) -> Self {
        Self {
            scope_name: scope_name.into(),
            up_to_slot,
        }
    }
}

impl RenderNode for SubmitCommandListsNode {
    fn name(&self) -> &str {
        &self.scope_name
    }

    fn node_type(&self) -> RenderNodeType {
        RenderNodeType::Unique
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Sync
    }

    fn run(&self, frame: &RenderContext<'_>) -> LeetResult<()> {
        frame.submit_command_lists(&self.scope_name, self.up_to_slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRequireNode;
    struct TestOwnNode;
    struct TestSyncNode;

    impl RenderNode for TestRequireNode {
        fn name(&self) -> &str {
            "Require"
        }

        fn command_list_usage(&self) -> RenderNodeCommandListUsage {
            RenderNodeCommandListUsage::Require
        }
    }

    impl RenderNode for TestOwnNode {
        fn name(&self) -> &str {
            "Own"
        }

        fn command_list_usage(&self) -> RenderNodeCommandListUsage {
            RenderNodeCommandListUsage::Own
        }
    }

    impl RenderNode for TestSyncNode {
        fn name(&self) -> &str {
            "Sync"
        }

        fn command_list_usage(&self) -> RenderNodeCommandListUsage {
            RenderNodeCommandListUsage::Sync
        }
    }

    #[test]
    fn record_task_rejects_non_own_root() {
        let result = RenderRecordTask::new(FrameCommandListIndex::new(0), TestRequireNode);
        assert!(result.is_err());
    }

    #[test]
    fn record_task_rejects_non_require_child() {
        let mut task = RenderRecordTask::new(FrameCommandListIndex::new(0), TestOwnNode).unwrap();
        let result = task.add_required_node(TestSyncNode);
        assert!(result.is_err());
    }

    #[test]
    fn execution_plan_counts_slots() {
        let mut plan = RenderExecutionPlan::new();
        let task0 = RenderRecordTask::new(FrameCommandListIndex::new(0), TestOwnNode).unwrap();
        let task3 = RenderRecordTask::new(FrameCommandListIndex::new(3), TestOwnNode).unwrap();

        plan.push_record_task(task0);
        plan.push_record_task(task3);

        assert_eq!(plan.command_list_count(), 4);
    }
}
