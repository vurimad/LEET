//! Render-graph authoring layer.

use super::frame_command_lists::FrameCommandListIndex;
use super::nodes::SubmitCommandListsNode;
use super::render_node::{
    RenderExecutionPlan, RenderFlowGroup, RenderFrameNodeStep, RenderNode,
    RenderNodeCommandListUsage, RenderNodeDependencyType, RenderRecordTask,
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

struct OrderedRenderGraphNode {
    node_index: usize,
    gpu_level: usize,
    entry: RenderGraphNodeEntry,
}

struct RenderGraphFlowGroupBuild {
    gpu_level: usize,
    nodes: Vec<OrderedRenderGraphNode>,
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
#[derive(Default)]
pub struct RenderGraph {
    nodes: Vec<RenderGraphNodeEntry>,
}

impl RenderGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn add_node<N>(&mut self, node: N) -> RenderGraphNodeId
    where
        N: RenderNode + 'static,
    {
        self.add_boxed_node(Box::new(node))
    }

    pub fn add_boxed_node(&mut self, node: Box<dyn RenderNode>) -> RenderGraphNodeId {
        let id = RenderGraphNodeId::new(self.nodes.len());
        self.nodes.push(RenderGraphNodeEntry {
            payload: RenderGraphNodePayload::Node(node),
            dependencies: Vec::new(),
        });
        id
    }

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

    pub fn add_cpu_dependency(
        &mut self,
        node: RenderGraphNodeId,
        depends_on: RenderGraphNodeId,
    ) -> LeetResult<()> {
        self.add_dependency(node, depends_on, RenderNodeDependencyType::Cpu)
    }

    pub fn add_gpu_dependency(
        &mut self,
        node: RenderGraphNodeId,
        depends_on: RenderGraphNodeId,
    ) -> LeetResult<()> {
        self.add_dependency(node, depends_on, RenderNodeDependencyType::Gpu)
    }

    pub fn compile(self) -> LeetResult<RenderExecutionPlan> {
        let ordered_nodes = Self::topologically_order_nodes(self.nodes)?;
        let flow_groups = Self::build_render_flow_groups(ordered_nodes);
        Self::emit_execution_plan(flow_groups)
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

    fn topologically_order_nodes(
        nodes: Vec<RenderGraphNodeEntry>,
    ) -> LeetResult<Vec<OrderedRenderGraphNode>> {
        let node_count = nodes.len();
        let mut nodes = nodes.into_iter().map(Some).collect::<Vec<_>>();
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

        let mut ordered_nodes = Vec::<OrderedRenderGraphNode>::with_capacity(node_count);

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
            let gpu_level = gpu_levels[node_index];

            ordered_nodes.push(OrderedRenderGraphNode {
                node_index,
                gpu_level,
                entry,
            });

            for &(child, dependency_type) in &outgoing[node_index] {
                let child_level = match dependency_type {
                    RenderNodeDependencyType::Cpu => gpu_level,
                    RenderNodeDependencyType::Gpu => gpu_level + 1,
                };
                gpu_levels[child] = gpu_levels[child].max(child_level);
                indegree[child] -= 1;
                if indegree[child] == 0 {
                    ready.push(child);
                }
            }
        }

        Ok(ordered_nodes)
    }

    /// RED mapping: this is the current equivalent of `BuildRenderFlowGroups()`.
    fn build_render_flow_groups(
        ordered_nodes: Vec<OrderedRenderGraphNode>,
    ) -> Vec<RenderGraphFlowGroupBuild> {
        let mut flow_groups = Vec::<RenderGraphFlowGroupBuild>::new();

        for ordered_node in ordered_nodes {
            if let Some(flow_group) = flow_groups.last_mut() {
                if flow_group.gpu_level == ordered_node.gpu_level {
                    flow_group.nodes.push(ordered_node);
                    continue;
                }
            }

            flow_groups.push(RenderGraphFlowGroupBuild {
                gpu_level: ordered_node.gpu_level,
                nodes: vec![ordered_node],
            });
        }

        flow_groups
    }

    fn emit_execution_plan(
        flow_groups: Vec<RenderGraphFlowGroupBuild>,
    ) -> LeetResult<RenderExecutionPlan> {
        let node_count = flow_groups
            .iter()
            .map(|flow_group| flow_group.nodes.len())
            .sum();
        let group_count = flow_groups.len();
        let mut next_slot = 0usize;
        let mut pending_submit_slot = None::<FrameCommandListIndex>;
        let mut task_bindings = vec![None::<TaskBinding>; node_count];
        let mut plan = RenderExecutionPlan::new();

        for (group_index, flow_group) in flow_groups.into_iter().enumerate() {
            let group_start = plan.step_count();

            for ordered_node in flow_group.nodes {
                let node_index = ordered_node.node_index;
                let dependencies = ordered_node.entry.dependencies;

                match ordered_node.entry.payload {
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
                            let task =
                                plan.record_task_mut(binding.step_index).ok_or_else(|| {
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
                        Self::emit_submit_if_pending(
                            &mut plan,
                            &mut pending_submit_slot,
                            scope_name,
                        )?;
                    }
                }
            }

            let submit_scope_name = if group_index + 1 == group_count {
                "Submit_Final".to_string()
            } else {
                format!("Submit_Level_{}", flow_group.gpu_level)
            };
            Self::emit_submit_if_pending(&mut plan, &mut pending_submit_slot, submit_scope_name)?;
            Self::push_flow_group_from_step_range(&mut plan, flow_group.gpu_level, group_start);
        }

        Ok(plan)
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

    fn push_flow_group_from_step_range(
        plan: &mut RenderExecutionPlan,
        gpu_level: usize,
        group_start: usize,
    ) {
        let group_end = plan.step_count();
        if group_end > group_start {
            plan.push_flow_group(RenderFlowGroup::new(gpu_level, group_start..group_end));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
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
        assert_eq!(plan.flow_groups().len(), 1);
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
        assert_eq!(plan.flow_groups().len(), 2);
        assert_eq!(plan.flow_groups()[0].gpu_level(), 0);
        assert_eq!(plan.flow_groups()[1].gpu_level(), 1);
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
        assert_eq!(plan.flow_groups().len(), 1);
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
        assert_eq!(plan.flow_groups().len(), 2);
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
