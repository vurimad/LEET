//! Render-graph authoring layer.
//!
//! [`RenderGraph`] stores nodes plus dependency edges. It is compiled into a
//! [`crate::render_node::RenderExecutionPlan`], which is the executable frame
//! schedule.

use crate::frame_command_lists::FrameCommandListIndex;
use crate::render_node::{
    RenderExecutionPlan, RenderFrameNodeStep, RenderNode, RenderNodeCommandListUsage,
    RenderNodeDependencyType, RenderRecordTask, SubmitCommandListsNode,
};
use leet_core::{Leeror, LeetResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TaskBinding {
    step_index: usize,
}

enum RenderGraphNodePayload {
    Node(Box<dyn RenderNode>),
    SubmitBarrier { scope_name: String },
}

impl RenderGraphNodePayload {
    fn priority(&self) -> usize {
        match self {
            Self::Node(node) => match node.command_list_usage() {
                RenderNodeCommandListUsage::Require => 0,
                RenderNodeCommandListUsage::Own => 1,
                RenderNodeCommandListUsage::None | RenderNodeCommandListUsage::Sync => 2,
            },
            Self::SubmitBarrier { .. } => 3,
        }
    }
}

struct RenderGraphNodeEntry {
    payload: RenderGraphNodePayload,
    dependencies: Vec<RenderGraphDependency>,
}

/// Stable node identifier inside a render graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RenderGraphNodeId(usize);

impl RenderGraphNodeId {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// One incoming dependency edge for a graph node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderGraphDependency {
    pub depends_on: RenderGraphNodeId,
    pub dependency_type: RenderNodeDependencyType,
}

impl RenderGraphDependency {
    pub const fn new(
        depends_on: RenderGraphNodeId,
        dependency_type: RenderNodeDependencyType,
    ) -> Self {
        Self {
            depends_on,
            dependency_type,
        }
    }
}

/// Declarative render-graph authoring structure.
///
/// The current compiler intentionally supports a conservative subset:
/// - `Own` nodes become new record tasks and receive a fresh slot
/// - `Require` nodes must depend on exactly one prior CPU node in the same task
/// - `None` and raw `Sync` nodes become frame steps
/// - submit barriers flush all pending command lists compiled before them
#[derive(Default)]
pub struct RenderGraph {
    nodes: Vec<RenderGraphNodeEntry>,
}

impl RenderGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of nodes in the graph.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` when the graph contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Add a render node to the graph.
    pub fn add_node<N>(&mut self, node: N) -> RenderGraphNodeId
    where
        N: RenderNode + 'static,
    {
        self.add_boxed_node(Box::new(node))
    }

    /// Add an owned render node object to the graph.
    pub fn add_boxed_node(&mut self, node: Box<dyn RenderNode>) -> RenderGraphNodeId {
        let id = RenderGraphNodeId::new(self.nodes.len());
        self.nodes.push(RenderGraphNodeEntry {
            payload: RenderGraphNodePayload::Node(node),
            dependencies: Vec::new(),
        });
        id
    }

    /// Add a submit barrier that flushes all pending command lists compiled
    /// before this point in the graph.
    pub fn add_submit_barrier(&mut self, scope_name: impl Into<String>) -> RenderGraphNodeId {
        let id = RenderGraphNodeId::new(self.nodes.len());
        self.nodes.push(RenderGraphNodeEntry {
            payload: RenderGraphNodePayload::SubmitBarrier {
                scope_name: scope_name.into(),
            },
            dependencies: Vec::new(),
        });
        id
    }

    /// Add a dependency edge to a node.
    pub fn add_dependency(
        &mut self,
        node: RenderGraphNodeId,
        depends_on: RenderGraphNodeId,
        dependency_type: RenderNodeDependencyType,
    ) -> LeetResult<()> {
        self.validate_node_id(node)?;
        self.validate_node_id(depends_on)?;

        if node == depends_on {
            return Err(Leeror::Validation(format!(
                "render graph node {} cannot depend on itself",
                node.get(),
            )));
        }

        let dependencies = &mut self.nodes[node.get()].dependencies;
        let duplicate = dependencies.iter().any(|dependency| {
            dependency.depends_on == depends_on && dependency.dependency_type == dependency_type
        });
        if duplicate {
            return Err(Leeror::Validation(format!(
                "render graph node {} already has a {:?} dependency on node {}",
                node.get(),
                dependency_type,
                depends_on.get(),
            )));
        }

        dependencies.push(RenderGraphDependency::new(depends_on, dependency_type));
        Ok(())
    }

    /// Add a CPU dependency edge.
    pub fn add_cpu_dependency(
        &mut self,
        node: RenderGraphNodeId,
        depends_on: RenderGraphNodeId,
    ) -> LeetResult<()> {
        self.add_dependency(node, depends_on, RenderNodeDependencyType::Cpu)
    }

    /// Add a GPU dependency edge.
    pub fn add_gpu_dependency(
        &mut self,
        node: RenderGraphNodeId,
        depends_on: RenderGraphNodeId,
    ) -> LeetResult<()> {
        self.add_dependency(node, depends_on, RenderNodeDependencyType::Gpu)
    }

    /// Compile the declarative graph into an executable frame plan.
    pub fn compile(self) -> LeetResult<RenderExecutionPlan> {
        let node_count = self.nodes.len();
        let mut nodes = self.nodes.into_iter().map(Some).collect::<Vec<_>>();
        let mut indegree = vec![0usize; node_count];
        let mut gpu_levels = vec![0usize; node_count];
        let mut outgoing = vec![Vec::<(usize, RenderNodeDependencyType)>::new(); node_count];

        for (index, entry) in nodes.iter().enumerate() {
            let entry = entry.as_ref().expect("graph node entries should exist");
            indegree[index] = entry.dependencies.len();
            for dependency in &entry.dependencies {
                let parent = dependency.depends_on.get();
                outgoing[parent].push((index, dependency.dependency_type));
            }
        }

        let mut ready = (0..node_count)
            .filter(|&index| indegree[index] == 0)
            .collect::<Vec<_>>();

        let mut ordered_nodes =
            Vec::<(usize, usize, RenderGraphNodeEntry)>::with_capacity(node_count);

        while ordered_nodes.len() < node_count {
            if ready.is_empty() {
                return Err(Leeror::Validation(
                    "render graph contains a cycle or unresolved dependency".to_string(),
                ));
            }

            let ready_position = Self::pick_ready_node(&ready, &gpu_levels, &nodes);
            let node_index = ready.remove(ready_position);
            let entry = nodes[node_index]
                .take()
                .expect("ready queue should only contain live graph nodes");
            let node_level = gpu_levels[node_index];

            ordered_nodes.push((node_index, node_level, entry));

            for &(child, dependency_type) in &outgoing[node_index] {
                let child_level = match dependency_type {
                    RenderNodeDependencyType::Cpu => node_level,
                    RenderNodeDependencyType::Gpu => node_level + 1,
                };
                gpu_levels[child] = gpu_levels[child].max(child_level);
                indegree[child] -= 1;
                if indegree[child] == 0 {
                    ready.push(child);
                }
            }
        }

        let mut next_slot = 0usize;
        let mut pending_submit_slot = None::<FrameCommandListIndex>;
        let mut current_level = None::<usize>;
        let mut task_bindings = vec![None::<TaskBinding>; node_count];
        let mut plan = RenderExecutionPlan::new();

        for (node_index, node_level, entry) in ordered_nodes {
            if let Some(active_level) = current_level {
                if node_level > active_level {
                    Self::emit_submit_if_pending(
                        &mut plan,
                        &mut pending_submit_slot,
                        format!("Submit_Level_{active_level}"),
                    )?;
                }
            }
            current_level = Some(node_level);

            let dependencies = entry.dependencies;

            match entry.payload {
                RenderGraphNodePayload::Node(node) => match node.command_list_usage() {
                    RenderNodeCommandListUsage::Own => {
                        let slot = FrameCommandListIndex::new(next_slot);
                        next_slot += 1;
                        pending_submit_slot = Some(slot);

                        let task = RenderRecordTask::new_boxed(slot, node)?;
                        let step_index = plan.push_record_task_with_index(task);
                        task_bindings[node_index] = Some(TaskBinding { step_index });
                    }
                    RenderNodeCommandListUsage::Require => {
                        let binding = Self::resolve_require_parent(
                            node.name(),
                            &dependencies,
                            &task_bindings,
                        )?;
                        let task = plan.record_task_mut(binding.step_index).ok_or_else(|| {
                            Leeror::Runtime(format!(
                                "compiled task binding for node {} no longer points to a record task",
                                node_index,
                            ))
                        })?;

                        task.add_required_boxed(node)?;
                        task_bindings[node_index] = Some(binding);
                    }
                    RenderNodeCommandListUsage::None | RenderNodeCommandListUsage::Sync => {
                        plan.push_frame_step(RenderFrameNodeStep::new_boxed(node)?);
                    }
                },
                RenderGraphNodePayload::SubmitBarrier { scope_name } => {
                    Self::emit_submit_if_pending(&mut plan, &mut pending_submit_slot, scope_name)?;
                }
            }
        }

        Self::emit_submit_if_pending(&mut plan, &mut pending_submit_slot, "Submit_Final")?;

        Ok(plan)
    }

    fn validate_node_id(&self, node: RenderGraphNodeId) -> LeetResult<()> {
        if node.get() >= self.nodes.len() {
            return Err(Leeror::Validation(format!(
                "render graph node {} is out of range for {} nodes",
                node.get(),
                self.nodes.len(),
            )));
        }

        Ok(())
    }

    fn pick_ready_node(
        ready: &[usize],
        gpu_levels: &[usize],
        nodes: &[Option<RenderGraphNodeEntry>],
    ) -> usize {
        let mut best_position = 0usize;
        let mut best_level = usize::MAX;
        let mut best_priority = usize::MAX;
        let mut best_index = usize::MAX;

        for (position, &node_index) in ready.iter().enumerate() {
            let level = gpu_levels[node_index];
            let priority = nodes[node_index]
                .as_ref()
                .expect("ready node should exist")
                .payload
                .priority();

            if level < best_level
                || (level == best_level
                    && (priority < best_priority
                        || (priority == best_priority && node_index < best_index)))
            {
                best_level = level;
                best_priority = priority;
                best_index = node_index;
                best_position = position;
            }
        }

        best_position
    }

    fn resolve_require_parent(
        node_name: &str,
        dependencies: &[RenderGraphDependency],
        task_bindings: &[Option<TaskBinding>],
    ) -> LeetResult<TaskBinding> {
        if dependencies.len() != 1 {
            return Err(Leeror::Validation(format!(
                "require node '{}' must have exactly one dependency",
                node_name,
            )));
        }

        let dependency = dependencies[0];
        if dependency.dependency_type != RenderNodeDependencyType::Cpu {
            return Err(Leeror::Validation(format!(
                "require node '{}' must chain from a CPU dependency",
                node_name,
            )));
        }

        task_bindings[dependency.depends_on.get()].ok_or_else(|| {
            Leeror::Validation(format!(
                "require node '{}' must depend on a prior own/require task node",
                node_name,
            ))
        })
    }

    fn emit_submit_if_pending(
        plan: &mut RenderExecutionPlan,
        pending_submit_slot: &mut Option<FrameCommandListIndex>,
        scope_name: impl Into<String>,
    ) -> LeetResult<()> {
        let Some(slot) = *pending_submit_slot else {
            return Ok(());
        };

        let step = RenderFrameNodeStep::new(SubmitCommandListsNode::new(scope_name, slot))?;
        plan.push_frame_step(step);
        *pending_submit_slot = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_node::{
        BloomNode, ClearBackbufferNode, EndFrameNode, MainPassRootNode, OpaqueDrawsNode,
        RenderExecutionStep, SkyDrawsNode, StartFrameNode,
    };

    struct TestOwnNode;
    struct TestRequireNode;

    impl RenderNode for TestOwnNode {
        fn name(&self) -> &str {
            "Own"
        }

        fn command_list_usage(&self) -> RenderNodeCommandListUsage {
            RenderNodeCommandListUsage::Own
        }
    }

    impl RenderNode for TestRequireNode {
        fn name(&self) -> &str {
            "Require"
        }

        fn command_list_usage(&self) -> RenderNodeCommandListUsage {
            RenderNodeCommandListUsage::Require
        }
    }

    #[test]
    fn compile_blank_graph() {
        let mut graph = RenderGraph::new();
        let clear = graph.add_node(ClearBackbufferNode::new(wgpu::Color::BLACK));
        let submit = graph.add_submit_barrier("Submit_Clear");
        graph.add_cpu_dependency(submit, clear).unwrap();

        let plan = graph.compile().unwrap();

        assert_eq!(plan.command_list_count(), 1);
        assert_eq!(plan.steps().len(), 2);
        assert!(matches!(plan.steps()[0], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[1], RenderExecutionStep::Frame(_)));
    }

    #[test]
    fn compile_chains_require_nodes_into_one_task() {
        let mut graph = RenderGraph::new();
        let own = graph.add_node(TestOwnNode);
        let require = graph.add_node(TestRequireNode);
        let submit = graph.add_submit_barrier("Submit_Test");

        graph.add_cpu_dependency(require, own).unwrap();
        graph.add_cpu_dependency(submit, require).unwrap();

        let plan = graph.compile().unwrap();

        assert_eq!(plan.command_list_count(), 1);
        assert_eq!(plan.steps().len(), 2);

        match &plan.steps()[0] {
            RenderExecutionStep::Record(task) => assert_eq!(task.required_node_count(), 1),
            RenderExecutionStep::Frame(_) => panic!("expected record task"),
        }
    }

    #[test]
    fn compile_gpu_edge_inserts_intermediate_submit() {
        let mut graph = RenderGraph::new();
        let first = graph.add_node(TestOwnNode);
        let second = graph.add_node(TestOwnNode);
        graph.add_gpu_dependency(second, first).unwrap();

        let plan = graph.compile().unwrap();

        assert_eq!(plan.command_list_count(), 2);
        assert_eq!(plan.steps().len(), 4);
        assert!(matches!(plan.steps()[0], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[1], RenderExecutionStep::Frame(_)));
        assert!(matches!(plan.steps()[2], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[3], RenderExecutionStep::Frame(_)));
    }

    #[test]
    fn compile_cpu_edge_keeps_tasks_in_same_submit_wave() {
        let mut graph = RenderGraph::new();
        let first = graph.add_node(TestOwnNode);
        let second = graph.add_node(TestOwnNode);
        graph.add_cpu_dependency(second, first).unwrap();

        let plan = graph.compile().unwrap();

        assert_eq!(plan.command_list_count(), 2);
        assert_eq!(plan.steps().len(), 3);
        assert!(matches!(plan.steps()[0], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[1], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[2], RenderExecutionStep::Frame(_)));
    }

    #[test]
    fn compile_mixed_example_graph() {
        let mut graph = RenderGraph::new();
        let start = graph.add_node(StartFrameNode::new());
        let main = graph.add_node(MainPassRootNode::new(wgpu::Color::BLACK));
        let opaque = graph.add_node(OpaqueDrawsNode::new());
        let sky = graph.add_node(SkyDrawsNode::new());
        let bloom = graph.add_node(BloomNode::new());
        let end = graph.add_node(EndFrameNode::new());

        graph.add_cpu_dependency(main, start).unwrap();
        graph.add_cpu_dependency(opaque, main).unwrap();
        graph.add_cpu_dependency(sky, main).unwrap();
        graph.add_gpu_dependency(bloom, main).unwrap();
        graph.add_cpu_dependency(end, bloom).unwrap();

        let plan = graph.compile().unwrap();

        assert_eq!(plan.command_list_count(), 2);
        assert_eq!(plan.steps().len(), 6);
        assert!(matches!(plan.steps()[0], RenderExecutionStep::Frame(_)));
        assert!(matches!(plan.steps()[1], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[2], RenderExecutionStep::Frame(_)));
        assert!(matches!(plan.steps()[3], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[4], RenderExecutionStep::Frame(_)));
        assert!(matches!(plan.steps()[5], RenderExecutionStep::Frame(_)));

        match &plan.steps()[1] {
            RenderExecutionStep::Record(task) => assert_eq!(task.required_node_count(), 2),
            RenderExecutionStep::Frame(_) => panic!("expected main-pass record task"),
        }
    }

    #[test]
    fn compile_rejects_require_without_single_cpu_parent() {
        let mut graph = RenderGraph::new();
        graph.add_node(TestRequireNode);

        let error = match graph.compile() {
            Ok(_) => panic!("expected graph compilation to fail"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("must have exactly one dependency"));
    }
}
