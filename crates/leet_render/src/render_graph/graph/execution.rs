//! Render graph node processing core.

use std::collections::{HashMap, VecDeque};

use leet_jobs2::Builder as RenderJobBuilder;

use super::{
    CommandListGroupStore, RenderGlobalBindingMask, RenderGraphError, RenderGraphResult,
    RenderNodeCommandListUsage, RenderNodeDependencyKind, RenderNodeExecutionMetadata,
    RenderNodeFrameRuntime, RenderNodeGraph, RenderNodeId, RenderNodeImpl, RenderNodeImplContext,
    RenderNodeImplContextInit, RenderNodeImplId, RenderNodeImplStore, RenderNodeRole,
};
use crate::render_graph::resources::{
    FrameResourceAllocator, RenderFlowGroup, RenderFlowSpace, ResourceAllocatorPhase,
};

/// Per-executor mutable state reset at the beginning of every node process call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderNodeProcessState {
    current_node: Option<RenderNodeId>,
    node_generation: u64,
    active_global_binding_mod: RenderGlobalBindingMask,
    epilogue_count: u64,
}

impl RenderNodeProcessState {
    /// Creates empty process state for a graph execution walk.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the node currently being processed, if any.
    pub fn current_node(&self) -> Option<RenderNodeId> {
        self.current_node
    }

    /// Returns how many nodes began processing through this state.
    pub fn node_generation(&self) -> u64 {
        self.node_generation
    }

    /// Returns global binding metadata accumulated for the active node.
    pub fn active_global_binding_mod(&self) -> RenderGlobalBindingMask {
        self.active_global_binding_mod
    }

    /// Returns how many command-list epilogues ran through this state.
    pub fn epilogue_count(&self) -> u64 {
        self.epilogue_count
    }

    fn begin_node(&mut self, node: RenderNodeId) {
        self.current_node = Some(node);
        self.node_generation = self.node_generation.saturating_add(1);
        self.active_global_binding_mod = RenderGlobalBindingMask::empty();
    }

    fn note_global_binding_mod(&mut self, mask: RenderGlobalBindingMask) {
        self.active_global_binding_mod = self.active_global_binding_mod.union(mask);
    }

    fn run_epilogue(&mut self) {
        self.epilogue_count = self.epilogue_count.saturating_add(1);
        self.active_global_binding_mod = RenderGlobalBindingMask::empty();
    }

    fn end_node(&mut self) {
        self.current_node = None;
    }
}

/// Result metadata from processing one graph-visible node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderNodeProcessReport {
    pub node: RenderNodeId,
    pub phase: ResourceAllocatorPhase,
    pub flow_group: RenderFlowGroup,
    pub command_list_usage: RenderNodeCommandListUsage,
    pub executed_impls: usize,
    pub command_list_scope_opened: bool,
    pub epilogue_ran: bool,
    pub gpu_scope_allowed: bool,
    pub global_binding_mod: RenderGlobalBindingMask,
    pub global_binding_restore_ran: bool,
    pub state_generation: u64,
    pub worker_index: u32,
}

/// Immutable payload carried by one graph execution job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderGraphJobPayload {
    pub node: RenderNodeId,
    pub metadata: RenderNodeExecutionMetadata,
    pub impl_id: Option<RenderNodeImplId>,
    pub worker_index: u32,
}

/// Runtime state for one dependency-counter graph job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderGraphJobNode {
    pub payload: RenderGraphJobPayload,
    pub cpu_parent_count: usize,
    pub pending_cpu_parents: usize,
    pub cpu_children: Vec<RenderNodeId>,
    pub scheduled: bool,
    pub started: bool,
    pub completed: bool,
    pub terminal: bool,
}

/// Report returned by dependency-counter consume execution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderGraphDependencyExecutionReport {
    pub scheduled_jobs: usize,
    pub completed_jobs: usize,
    pub terminal_nodes: usize,
    pub terminal_completed: bool,
    pub ready_batches: Vec<Vec<RenderNodeId>>,
    pub node_reports: Vec<RenderNodeProcessReport>,
}

/// CPU-dependency job table used by consume execution.
///
/// The table is prepared up front from a built graph. Root nodes are still
/// gated by an explicit external kickoff so the caller can finish frame setup
/// before any node becomes runnable.
#[derive(Clone, Debug)]
pub struct RenderGraphDependencyCounters {
    nodes: Vec<RenderGraphJobNode>,
    node_to_index: HashMap<RenderNodeId, usize>,
    ready: VecDeque<RenderNodeId>,
    kickoff_released: bool,
    completed_jobs: usize,
    completed_terminal_nodes: usize,
    terminal_completion_marked: bool,
}

impl RenderGraphDependencyCounters {
    /// Builds a dependency-counter table from CPU graph dependencies.
    pub fn prepare(graph: &RenderNodeGraph) -> RenderGraphResult<Self> {
        if !graph.is_built() {
            return Err(RenderGraphError::InvalidState {
                reason: "render graph must be built before dependency-counter execution",
            });
        }

        let node_ids = graph.node_ids().collect::<Vec<_>>();
        let mut nodes = Vec::with_capacity(node_ids.len());
        let mut node_to_index = HashMap::with_capacity(node_ids.len());

        for (index, node) in node_ids.iter().copied().enumerate() {
            node_to_index.insert(node, index);
        }

        for (worker_index, node) in node_ids.iter().copied().enumerate() {
            let node_view = graph.node(node)?;
            let cpu_parent_count = graph
                .parent_nodes(node, RenderNodeDependencyKind::Cpu)?
                .len();
            let cpu_children = graph.child_nodes(node, RenderNodeDependencyKind::Cpu)?;
            nodes.push(RenderGraphJobNode {
                payload: RenderGraphJobPayload {
                    node,
                    metadata: node_view.metadata(),
                    impl_id: node_view.impl_id(),
                    worker_index: u32::try_from(worker_index).map_err(|_| {
                        RenderGraphError::InvalidState {
                            reason: "render graph worker index exceeded u32 range",
                        }
                    })?,
                },
                cpu_parent_count,
                pending_cpu_parents: cpu_parent_count,
                terminal: cpu_children.is_empty(),
                cpu_children,
                scheduled: true,
                started: false,
                completed: false,
            });
        }

        Ok(Self {
            nodes,
            node_to_index,
            ready: VecDeque::new(),
            kickoff_released: false,
            completed_jobs: 0,
            completed_terminal_nodes: 0,
            terminal_completion_marked: false,
        })
    }

    /// Returns the upfront-scheduled job table.
    pub fn jobs(&self) -> &[RenderGraphJobNode] {
        &self.nodes
    }

    /// Returns how many graph jobs were scheduled up front.
    pub fn scheduled_jobs(&self) -> usize {
        self.nodes.iter().filter(|node| node.scheduled).count()
    }

    /// Returns how many jobs have completed.
    pub fn completed_jobs(&self) -> usize {
        self.completed_jobs
    }

    /// Returns how many terminal graph jobs exist.
    pub fn terminal_nodes(&self) -> usize {
        self.nodes.iter().filter(|node| node.terminal).count()
    }

    /// Returns whether the terminal completion handle has resolved.
    pub fn terminal_completed(&self) -> bool {
        self.terminal_completion_marked
    }

    /// Releases the external kickoff gate and queues root nodes.
    pub fn release_external_kickoff(&mut self) {
        if self.kickoff_released {
            return;
        }

        self.kickoff_released = true;
        for job in &self.nodes {
            if job.pending_cpu_parents == 0 && !job.started && !job.completed {
                self.ready.push_back(job.payload.node);
            }
        }
        self.update_terminal_completion();
    }

    /// Returns currently runnable nodes without consuming them.
    pub fn ready_nodes(&self) -> Vec<RenderNodeId> {
        self.ready.iter().copied().collect()
    }

    /// Takes one runnable batch.
    ///
    /// Nodes in the same batch have no CPU dependency between each other and may
    /// be recorded by a parallel executor. This shell still returns the batch to
    /// the caller so frame-owned mutable state can decide how to run it.
    pub fn take_ready_batch(&mut self) -> Vec<RenderNodeId> {
        self.ready.drain(..).collect()
    }

    /// Marks a node as started by the consume executor.
    pub fn begin_node(&mut self, node: RenderNodeId) -> RenderGraphResult<RenderGraphJobPayload> {
        if !self.kickoff_released {
            return Err(RenderGraphError::InvalidState {
                reason: "dependency-counter job started before external kickoff",
            });
        }

        let job = self.job_mut(node)?;
        if job.pending_cpu_parents != 0 {
            return Err(RenderGraphError::InvalidState {
                reason: "dependency-counter job started before CPU parents completed",
            });
        }
        if job.started || job.completed {
            return Err(RenderGraphError::InvalidState {
                reason: "dependency-counter job was started more than once",
            });
        }

        job.started = true;
        Ok(job.payload)
    }

    /// Completes a node and releases any CPU children whose wait count reaches zero.
    pub fn complete_node(&mut self, node: RenderNodeId) -> RenderGraphResult<()> {
        let children = {
            let job = self.job_mut(node)?;
            if !job.started || job.completed {
                return Err(RenderGraphError::InvalidState {
                    reason: "dependency-counter job completed without a matching start",
                });
            }

            job.completed = true;
            job.cpu_children.clone()
        };

        self.completed_jobs += 1;
        if self.job(node)?.terminal {
            self.completed_terminal_nodes += 1;
        }

        for child in children {
            let child_job = self.job_mut(child)?;
            if child_job.pending_cpu_parents == 0 {
                return Err(RenderGraphError::InvalidState {
                    reason: "dependency-counter child wait count underflowed",
                });
            }

            child_job.pending_cpu_parents -= 1;
            if child_job.pending_cpu_parents == 0 {
                self.ready.push_back(child);
            }
        }

        self.update_terminal_completion();
        Ok(())
    }

    /// Debug-only invariant check for the terminal completion handle.
    pub fn debug_validate_terminal_completion(&self) -> RenderGraphResult<()> {
        if self.terminal_completion_marked && self.completed_jobs != self.nodes.len() {
            return Err(RenderGraphError::InvalidState {
                reason: "terminal graph completion resolved before all jobs completed",
            });
        }

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn debug_force_terminal_completion_for_test(&mut self) {
        self.terminal_completion_marked = true;
    }

    fn update_terminal_completion(&mut self) {
        if self.completed_jobs == self.nodes.len()
            && self.completed_terminal_nodes == self.terminal_nodes()
        {
            self.terminal_completion_marked = true;
        }
    }

    fn job(&self, node: RenderNodeId) -> RenderGraphResult<&RenderGraphJobNode> {
        let index = self
            .node_to_index
            .get(&node)
            .copied()
            .ok_or(RenderGraphError::InvalidId {
                kind: "dependency-counter graph job",
                raw: node.raw(),
            })?;
        Ok(&self.nodes[index])
    }

    fn job_mut(&mut self, node: RenderNodeId) -> RenderGraphResult<&mut RenderGraphJobNode> {
        let index = self
            .node_to_index
            .get(&node)
            .copied()
            .ok_or(RenderGraphError::InvalidId {
                kind: "dependency-counter graph job",
                raw: node.raw(),
            })?;
        Ok(&mut self.nodes[index])
    }
}

/// Processes one graph-visible node through the runtime wrapper.
pub fn process_node(
    graph: &RenderNodeGraph,
    impl_store: &RenderNodeImplStore,
    command_groups: &CommandListGroupStore,
    node: RenderNodeId,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
) -> RenderGraphResult<RenderNodeProcessReport> {
    process_node_core(
        graph,
        impl_store,
        command_groups,
        node,
        state,
        allocator,
        jobs,
        None,
        u32::MAX,
    )
}

/// Processes one graph-visible node with explicit frame runtime hooks.
pub fn process_node_with_runtime(
    graph: &RenderNodeGraph,
    impl_store: &RenderNodeImplStore,
    command_groups: &CommandListGroupStore,
    node: RenderNodeId,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
    frame_runtime: &mut dyn RenderNodeFrameRuntime,
) -> RenderGraphResult<RenderNodeProcessReport> {
    process_node_core(
        graph,
        impl_store,
        command_groups,
        node,
        state,
        allocator,
        jobs,
        Some(frame_runtime),
        u32::MAX,
    )
}

fn process_node_core(
    graph: &RenderNodeGraph,
    impl_store: &RenderNodeImplStore,
    command_groups: &CommandListGroupStore,
    node: RenderNodeId,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
    frame_runtime: Option<&mut dyn RenderNodeFrameRuntime>,
    worker_index: u32,
) -> RenderGraphResult<RenderNodeProcessReport> {
    let node_view = graph.node(node)?;
    let flow_group = execution_flow_group(node_view.metadata())?;
    let init = context_init_for_node(node_view.metadata(), flow_group)?
        .with_dispatcher_thread_index(worker_index);
    let phase = allocator.phase();

    state.begin_node(node);

    let body_result: RenderGraphResult<ProcessBodyReport> = match frame_runtime {
        Some(frame_runtime) => {
            if matches!(node_view.role(), RenderNodeRole::CommandListGroup) {
                process_command_list_group_with_runtime(
                    command_groups,
                    impl_store,
                    node,
                    init,
                    state,
                    allocator,
                    jobs,
                    frame_runtime,
                )
            } else if let Some(impl_id) = node_view.impl_id() {
                let implementation = impl_store.get(impl_id)?;
                process_single_impl_with_runtime(
                    implementation,
                    init,
                    state,
                    allocator,
                    jobs,
                    frame_runtime,
                )
            } else {
                Ok(ProcessBodyReport::empty())
            }
        }
        None => {
            if matches!(node_view.role(), RenderNodeRole::CommandListGroup) {
                process_command_list_group(
                    command_groups,
                    impl_store,
                    node,
                    init,
                    state,
                    allocator,
                    jobs,
                )
            } else if let Some(impl_id) = node_view.impl_id() {
                let implementation = impl_store.get(impl_id)?;
                process_single_impl(implementation, init, state, allocator, jobs)
            } else {
                Ok(ProcessBodyReport::empty())
            }
        }
    };
    let report = match body_result {
        Ok(report) => report,
        Err(error) => {
            state.end_node();
            return Err(error);
        }
    };

    let epilogue_ran = phase.is_consume() && report.command_list_usage.uses_command_list();
    let global_binding_restore_ran = epilogue_ran && !report.global_binding_mod.is_empty();
    if epilogue_ran {
        state.run_epilogue();
    }

    let result = RenderNodeProcessReport {
        node,
        phase,
        flow_group,
        command_list_usage: report.command_list_usage,
        executed_impls: report.executed_impls,
        command_list_scope_opened: report.command_list_scope_opened,
        epilogue_ran,
        gpu_scope_allowed: report.gpu_scope_allowed,
        global_binding_mod: report.global_binding_mod,
        global_binding_restore_ran,
        state_generation: state.node_generation(),
        worker_index,
    };

    state.end_node();
    Ok(result)
}

/// Executes graph nodes during consume using CPU dependency counters.
///
/// This is the core scheduling shell: it builds the full job table up front,
/// releases an explicit kickoff gate, runs currently ready CPU batches, and
/// marks a terminal completion handle only after every terminal node finishes.
pub fn execute_graph_dependency_counter_consume(
    graph: &RenderNodeGraph,
    impl_store: &RenderNodeImplStore,
    command_groups: &CommandListGroupStore,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
    mut frame_runtime: Option<&mut dyn RenderNodeFrameRuntime>,
) -> RenderGraphResult<RenderGraphDependencyExecutionReport> {
    if !allocator.phase().is_consume() {
        return Err(RenderGraphError::InvalidState {
            reason: "dependency-counter graph execution is a consume-phase operation",
        });
    }

    let mut counters = RenderGraphDependencyCounters::prepare(graph)?;
    counters.release_external_kickoff();

    let mut report = RenderGraphDependencyExecutionReport {
        scheduled_jobs: counters.scheduled_jobs(),
        terminal_nodes: counters.terminal_nodes(),
        ..RenderGraphDependencyExecutionReport::default()
    };

    while counters.completed_jobs() < counters.scheduled_jobs() {
        let batch = counters.take_ready_batch();
        if batch.is_empty() {
            return Err(RenderGraphError::InvalidState {
                reason: "dependency-counter graph execution stalled before terminal completion",
            });
        }

        report.ready_batches.push(batch.clone());
        for node in batch {
            let payload = counters.begin_node(node)?;
            let node_report = if let Some(frame_runtime) = frame_runtime.as_deref_mut() {
                process_node_core(
                    graph,
                    impl_store,
                    command_groups,
                    payload.node,
                    state,
                    allocator,
                    jobs,
                    Some(frame_runtime),
                    payload.worker_index,
                )?
            } else {
                process_node_core(
                    graph,
                    impl_store,
                    command_groups,
                    payload.node,
                    state,
                    allocator,
                    jobs,
                    None,
                    payload.worker_index,
                )?
            };
            report.node_reports.push(node_report);
            counters.complete_node(node)?;
        }
    }

    counters.debug_validate_terminal_completion()?;
    report.completed_jobs = counters.completed_jobs();
    report.terminal_completed = counters.terminal_completed();
    Ok(report)
}

/// Executes every graph-visible node in built GPU flow order.
pub fn execute_graph_sequential_gpu_order(
    graph: &RenderNodeGraph,
    impl_store: &RenderNodeImplStore,
    command_groups: &CommandListGroupStore,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
) -> RenderGraphResult<Vec<RenderNodeProcessReport>> {
    if !graph.is_built() {
        return Err(RenderGraphError::InvalidState {
            reason: "render graph must be built before execution",
        });
    }

    let mut reports = Vec::with_capacity(graph.node_count());
    for node in graph
        .flattened_nodes(RenderNodeDependencyKind::Gpu)
        .iter()
        .copied()
    {
        reports.push(process_node(
            graph,
            impl_store,
            command_groups,
            node,
            state,
            allocator,
            jobs,
        )?);
    }

    Ok(reports)
}

#[derive(Clone, Copy, Debug)]
struct ProcessBodyReport {
    command_list_usage: RenderNodeCommandListUsage,
    executed_impls: usize,
    command_list_scope_opened: bool,
    gpu_scope_allowed: bool,
    global_binding_mod: RenderGlobalBindingMask,
}

impl ProcessBodyReport {
    fn empty() -> Self {
        Self {
            command_list_usage: RenderNodeCommandListUsage::None,
            executed_impls: 0,
            command_list_scope_opened: false,
            gpu_scope_allowed: true,
            global_binding_mod: RenderGlobalBindingMask::empty(),
        }
    }
}

fn process_single_impl(
    implementation: &dyn RenderNodeImpl,
    init: RenderNodeImplContextInit,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
) -> RenderGraphResult<ProcessBodyReport> {
    let global_binding_mod = implementation.global_binding_mod();
    state.note_global_binding_mod(global_binding_mod);

    let mut rctx = RenderNodeImplContext::new(allocator, init);
    implementation.execute(&mut rctx, jobs)?;

    Ok(ProcessBodyReport {
        command_list_usage: implementation.command_list_usage(),
        executed_impls: 1,
        command_list_scope_opened: false,
        gpu_scope_allowed: implementation.allow_gpu_scope(),
        global_binding_mod,
    })
}

fn process_single_impl_with_runtime(
    implementation: &dyn RenderNodeImpl,
    init: RenderNodeImplContextInit,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
    frame_runtime: &mut dyn RenderNodeFrameRuntime,
) -> RenderGraphResult<ProcessBodyReport> {
    let global_binding_mod = implementation.global_binding_mod();
    state.note_global_binding_mod(global_binding_mod);

    let mut rctx = RenderNodeImplContext::new_with_runtime(allocator, frame_runtime, init);
    implementation.execute(&mut rctx, jobs)?;

    Ok(ProcessBodyReport {
        command_list_usage: implementation.command_list_usage(),
        executed_impls: 1,
        command_list_scope_opened: false,
        gpu_scope_allowed: implementation.allow_gpu_scope(),
        global_binding_mod,
    })
}

fn process_command_list_group(
    command_groups: &CommandListGroupStore,
    impl_store: &RenderNodeImplStore,
    node: RenderNodeId,
    init: RenderNodeImplContextInit,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
) -> RenderGraphResult<ProcessBodyReport> {
    let group = command_groups.get(node)?;
    let mut executed_impls = 0usize;
    let mut global_binding_mod = RenderGlobalBindingMask::empty();
    let mut gpu_scope_allowed = true;

    let mut rctx = RenderNodeImplContext::new(allocator, init);
    rctx.begin_queue(group.queue_kind())?;

    let mut execution_error = None;
    for subnode in group.subnodes() {
        let implementation = impl_store.get(*subnode)?;
        let subnode_binding_mod = implementation.global_binding_mod();
        state.note_global_binding_mod(subnode_binding_mod);
        global_binding_mod = global_binding_mod.union(subnode_binding_mod);
        gpu_scope_allowed &= implementation.allow_gpu_scope();
        executed_impls += 1;

        if let Err(error) = implementation.execute(&mut rctx, jobs) {
            execution_error = Some(error);
            break;
        }
    }

    let end_result = rctx.end_queue();
    if let Some(error) = execution_error {
        return Err(error);
    }
    end_result?;

    Ok(ProcessBodyReport {
        command_list_usage: RenderNodeCommandListUsage::Own,
        executed_impls,
        command_list_scope_opened: true,
        gpu_scope_allowed,
        global_binding_mod,
    })
}

fn process_command_list_group_with_runtime(
    command_groups: &CommandListGroupStore,
    impl_store: &RenderNodeImplStore,
    node: RenderNodeId,
    init: RenderNodeImplContextInit,
    state: &mut RenderNodeProcessState,
    allocator: &mut FrameResourceAllocator,
    jobs: &mut RenderJobBuilder,
    frame_runtime: &mut dyn RenderNodeFrameRuntime,
) -> RenderGraphResult<ProcessBodyReport> {
    let group = command_groups.get(node)?;
    let mut executed_impls = 0usize;
    let mut global_binding_mod = RenderGlobalBindingMask::empty();
    let mut gpu_scope_allowed = true;
    let owns_command_recorder = allocator.is_consume_phase();

    if owns_command_recorder {
        frame_runtime.create_command_recorder(
            init.flow_group(),
            group.queue_kind(),
            group.name().as_str(),
        )?;
    }

    let mut rctx = RenderNodeImplContext::new_with_runtime(allocator, frame_runtime, init);
    rctx.begin_queue(group.queue_kind())?;

    let mut execution_error = None;
    for subnode in group.subnodes() {
        let implementation = impl_store.get(*subnode)?;
        let subnode_binding_mod = implementation.global_binding_mod();
        state.note_global_binding_mod(subnode_binding_mod);
        global_binding_mod = global_binding_mod.union(subnode_binding_mod);
        gpu_scope_allowed &= implementation.allow_gpu_scope();
        executed_impls += 1;

        if let Err(error) = implementation.execute(&mut rctx, jobs) {
            execution_error = Some(error);
            break;
        }
    }

    let end_result = rctx.end_queue();
    if let Some(error) = execution_error {
        return Err(error);
    }
    end_result?;
    if owns_command_recorder {
        rctx.set_command_recorder_active(false)?;
    }

    Ok(ProcessBodyReport {
        command_list_usage: RenderNodeCommandListUsage::Own,
        executed_impls,
        command_list_scope_opened: true,
        gpu_scope_allowed,
        global_binding_mod,
    })
}

fn execution_flow_group(
    metadata: super::RenderNodeExecutionMetadata,
) -> RenderGraphResult<RenderFlowGroup> {
    metadata
        .flow_group(RenderNodeDependencyKind::Gpu)
        .ok_or(RenderGraphError::InvalidState {
            reason: "node is missing GPU flow group; build_flow_groups must run before execution",
        })
}

fn context_init_for_node(
    metadata: super::RenderNodeExecutionMetadata,
    flow_group: RenderFlowGroup,
) -> RenderGraphResult<RenderNodeImplContextInit> {
    if let Some(camera_index) = metadata.camera_index {
        let flow_space_index =
            u8::try_from(camera_index).map_err(|_| RenderGraphError::InvalidState {
                reason: "camera render-flow space exceeded u8 range",
            })?;
        Ok(RenderNodeImplContextInit::camera_node(
            flow_group,
            RenderFlowSpace::new(flow_space_index),
        ))
    } else {
        Ok(RenderNodeImplContextInit::unique_node(flow_group))
    }
}
