use std::sync::{Arc, Mutex};

use leet_jobs2::{JobSystemConfig, LeetJobSystem, Priority};

use super::super::{
    execute_graph_dependency_counter_consume, execute_graph_sequential_gpu_order, process_node,
    FrameCommandRecorders, FrameResourceAllocator, RenderGlobalBindingMask,
    RenderGraphDependencyCounters, RenderGraphError, RenderGraphResult, RenderNodeCommandListUsage,
    RenderNodeDependencyKind, RenderNodeGraphFactory, RenderNodeImpl, RenderNodeImplContext,
    RenderNodeKind, RenderNodeProcessState, RenderNodeSubtype, RenderQueueKind,
    ResourceAllocatorPhase, ResourceRequest,
};
use leet_jobs2::Builder as RenderJobBuilder;

#[derive(Clone)]
struct RecordingNode {
    name: &'static str,
    usage: RenderNodeCommandListUsage,
    log: Arc<Mutex<Vec<&'static str>>>,
    binding_mod: RenderGlobalBindingMask,
    allow_gpu_scope: bool,
    consume_only: bool,
    runtime_worker_log: Option<Arc<Mutex<Vec<u32>>>>,
    fail: bool,
}

impl RecordingNode {
    fn new(
        name: &'static str,
        usage: RenderNodeCommandListUsage,
        log: Arc<Mutex<Vec<&'static str>>>,
    ) -> Self {
        Self {
            name,
            usage,
            log,
            binding_mod: RenderGlobalBindingMask::empty(),
            allow_gpu_scope: true,
            consume_only: false,
            runtime_worker_log: None,
            fail: false,
        }
    }

    fn with_binding_mod(mut self, binding_mod: RenderGlobalBindingMask) -> Self {
        self.binding_mod = binding_mod;
        self
    }

    fn with_gpu_scope(mut self, allow_gpu_scope: bool) -> Self {
        self.allow_gpu_scope = allow_gpu_scope;
        self
    }

    fn consume_only(mut self) -> Self {
        self.consume_only = true;
        self
    }

    fn require_frame_runtime(mut self, runtime_worker_log: Arc<Mutex<Vec<u32>>>) -> Self {
        self.runtime_worker_log = Some(runtime_worker_log);
        self
    }

    fn fail(mut self) -> Self {
        self.fail = true;
        self
    }
}

impl RenderNodeImpl for RecordingNode {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        self.usage
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        if self.fail {
            return Err(RenderGraphError::InvalidState {
                reason: "test node failed intentionally",
            });
        }

        if !self.consume_only || rctx.is_consume_phase() {
            self.log.lock().unwrap().push(self.name);
        }
        if let Some(runtime_worker_log) = &self.runtime_worker_log {
            if !rctx.has_frame_runtime() {
                return Err(RenderGraphError::InvalidState {
                    reason: "test node expected explicit frame runtime",
                });
            }
            runtime_worker_log
                .lock()
                .unwrap()
                .push(rctx.dispatcher_thread_index());
        }
        Ok(())
    }

    fn allow_gpu_scope(&self) -> bool {
        self.allow_gpu_scope
    }

    fn global_binding_mod(&self) -> RenderGlobalBindingMask {
        self.binding_mod
    }
}

struct JobHarness {
    jobs: LeetJobSystem,
}

impl JobHarness {
    fn new() -> Self {
        Self {
            jobs: LeetJobSystem::new(JobSystemConfig {
                max_threads: 1,
                ..JobSystemConfig::default()
            }),
        }
    }

    fn builder(&self) -> RenderJobBuilder {
        self.jobs.create_builder(Priority::RenderPath)
    }
}

impl Drop for JobHarness {
    fn drop(&mut self) {
        self.jobs.shutdown();
    }
}

fn transition_to(allocator: &mut FrameResourceAllocator, phase: ResourceAllocatorPhase) {
    match phase {
        ResourceAllocatorPhase::Startup => {}
        ResourceAllocatorPhase::PreConsume => {
            allocator
                .set_phase(ResourceAllocatorPhase::PreConsume)
                .unwrap();
        }
        ResourceAllocatorPhase::Resolve => {
            allocator
                .set_phase(ResourceAllocatorPhase::PreConsume)
                .unwrap();
            allocator
                .set_phase(ResourceAllocatorPhase::Resolve)
                .unwrap();
        }
        ResourceAllocatorPhase::Consume => {
            allocator
                .set_phase(ResourceAllocatorPhase::PreConsume)
                .unwrap();
            allocator
                .set_phase(ResourceAllocatorPhase::Resolve)
                .unwrap();
            allocator
                .set_phase(ResourceAllocatorPhase::Consume)
                .unwrap();
        }
        ResourceAllocatorPhase::Cleanup => {
            allocator
                .set_phase(ResourceAllocatorPhase::PreConsume)
                .unwrap();
            allocator
                .set_phase(ResourceAllocatorPhase::Resolve)
                .unwrap();
            allocator
                .set_phase(ResourceAllocatorPhase::Consume)
                .unwrap();
            allocator
                .set_phase(ResourceAllocatorPhase::Cleanup)
                .unwrap();
        }
    }
}

#[test]
fn process_wrapper_calls_execute_once() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("node", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut allocator = FrameResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = RenderNodeProcessState::new();

    let report = process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        node,
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), &["node"]);
    assert_eq!(report.executed_impls, 1);
    assert_eq!(report.command_list_usage, RenderNodeCommandListUsage::None);
    assert_eq!(state.current_node(), None);
}

#[test]
fn sequential_gpu_execution_uses_process_wrapper_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let first = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("first", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let second = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("second", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    factory
        .link_nodes(first, second, RenderNodeDependencyKind::Gpu)
        .unwrap();
    let built = factory.finish().unwrap();
    let mut allocator = FrameResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = RenderNodeProcessState::new();

    let reports = execute_graph_sequential_gpu_order(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), &["first", "second"]);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].node, first);
    assert_eq!(reports[1].node, second);
}

#[test]
fn begin_node_state_resets_for_every_node() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let first = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("first", RenderNodeCommandListUsage::None, Arc::clone(&log))
                .with_binding_mod(RenderGlobalBindingMask::from_bits(0b10)),
        )
        .unwrap();
    let second = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("second", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut allocator = FrameResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = RenderNodeProcessState::new();

    let first_report = process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        first,
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();
    assert_eq!(
        first_report.global_binding_mod,
        RenderGlobalBindingMask::from_bits(0b10)
    );

    let second_report = process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        second,
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();

    assert_eq!(
        second_report.global_binding_mod,
        RenderGlobalBindingMask::empty()
    );
    assert_eq!(state.node_generation(), 2);
    assert_eq!(
        state.active_global_binding_mod(),
        RenderGlobalBindingMask::empty()
    );
}

#[test]
fn process_wrapper_resets_current_node_after_execute_error() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("fail", RenderNodeCommandListUsage::None, Arc::clone(&log)).fail(),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut allocator = FrameResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = RenderNodeProcessState::new();

    assert!(matches!(
        process_node(
            built.graph(),
            built.impl_store(),
            built.command_group_store(),
            node,
            &mut state,
            &mut allocator,
            &mut builder,
        ),
        Err(RenderGraphError::InvalidState { reason })
            if reason == "test node failed intentionally"
    ));
    assert_eq!(state.current_node(), None);
}

#[test]
fn epilogue_runs_for_command_list_nodes_during_consume() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new(
                "draw",
                RenderNodeCommandListUsage::Require,
                Arc::clone(&log),
            )
            .with_binding_mod(RenderGlobalBindingMask::from_bits(0b1)),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let jobs = JobHarness::new();
    let mut state = RenderNodeProcessState::new();

    let mut preconsume_allocator = FrameResourceAllocator::new();
    transition_to(
        &mut preconsume_allocator,
        ResourceAllocatorPhase::PreConsume,
    );
    let mut preconsume_builder = jobs.builder();
    let preconsume_report = process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        node,
        &mut state,
        &mut preconsume_allocator,
        &mut preconsume_builder,
    )
    .unwrap();

    assert!(!preconsume_report.epilogue_ran);

    let mut consume_allocator = preconsume_allocator;
    consume_allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    consume_allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    let mut consume_builder = jobs.builder();
    let consume_report = process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        node,
        &mut state,
        &mut consume_allocator,
        &mut consume_builder,
    )
    .unwrap();

    assert!(consume_report.epilogue_ran);
    assert!(consume_report.global_binding_restore_ran);
    assert_eq!(state.epilogue_count(), 1);
}

#[test]
fn command_list_usage_none_nodes_still_execute_during_preconsume() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new(
                "declare",
                RenderNodeCommandListUsage::None,
                Arc::clone(&log),
            ),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut allocator = FrameResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = RenderNodeProcessState::new();

    process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        node,
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), &["declare"]);
}

#[test]
fn consume_only_side_effects_can_be_guarded_by_context_phase() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new(
                "consume",
                RenderNodeCommandListUsage::None,
                Arc::clone(&log),
            )
            .consume_only(),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let jobs = JobHarness::new();
    let mut state = RenderNodeProcessState::new();

    let mut allocator = FrameResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let mut preconsume_builder = jobs.builder();
    process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        node,
        &mut state,
        &mut allocator,
        &mut preconsume_builder,
    )
    .unwrap();

    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    let mut consume_builder = jobs.builder();
    process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        node,
        &mut state,
        &mut allocator,
        &mut consume_builder,
    )
    .unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), &["consume"]);
}

#[test]
fn command_list_group_processes_subnodes_inside_queue_scope() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let parent = factory
        .begin_command_list_group(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            "bucket",
            RenderQueueKind::Compute,
        )
        .unwrap();
    factory
        .create_subnode(
            RecordingNode::new(
                "first",
                RenderNodeCommandListUsage::Require,
                Arc::clone(&log),
            )
            .with_gpu_scope(false),
        )
        .unwrap();
    factory
        .create_subnode(RecordingNode::new(
            "second",
            RenderNodeCommandListUsage::Require,
            Arc::clone(&log),
        ))
        .unwrap();
    factory.end_command_list_group().unwrap();
    let built = factory.finish().unwrap();
    let mut allocator = FrameResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = RenderNodeProcessState::new();

    let report = process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        parent,
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();
    let requests = allocator
        .request_group(report.flow_group)
        .unwrap()
        .requests();

    assert_eq!(log.lock().unwrap().as_slice(), &["first", "second"]);
    assert_eq!(report.executed_impls, 2);
    assert_eq!(report.command_list_usage, RenderNodeCommandListUsage::Own);
    assert!(report.command_list_scope_opened);
    assert!(!report.gpu_scope_allowed);
    assert!(matches!(
        requests.first(),
        Some(ResourceRequest::BeginQueue {
            queue: RenderQueueKind::Compute
        })
    ));
    assert!(matches!(requests.last(), Some(ResourceRequest::EndQueue)));
}

#[test]
fn dependency_counter_cpu_parent_completes_before_child_starts() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let parent = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("parent", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let child = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("child", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    factory
        .link_nodes(parent, child, RenderNodeDependencyKind::Cpu)
        .unwrap();
    let built = factory.finish().unwrap();
    let mut counters = RenderGraphDependencyCounters::prepare(built.graph()).unwrap();

    assert!(counters.ready_nodes().is_empty());
    counters.release_external_kickoff();
    assert_eq!(counters.take_ready_batch(), vec![parent]);

    counters.begin_node(parent).unwrap();
    counters.complete_node(parent).unwrap();
    assert_eq!(counters.take_ready_batch(), vec![child]);
}

#[test]
fn dependency_counter_external_kickoff_gates_root_nodes() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("root", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut counters = RenderGraphDependencyCounters::prepare(built.graph()).unwrap();

    assert!(matches!(
        counters.begin_node(node),
        Err(RenderGraphError::InvalidState { reason })
            if reason.contains("external kickoff")
    ));

    counters.release_external_kickoff();
    assert_eq!(counters.begin_node(node).unwrap().node, node);
}

#[test]
fn dependency_counter_independent_nodes_share_ready_batch() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let first = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("first", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let second = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("second", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut counters = RenderGraphDependencyCounters::prepare(built.graph()).unwrap();

    counters.release_external_kickoff();
    assert_eq!(counters.take_ready_batch(), vec![first, second]);
}

#[test]
fn dependency_counter_gpu_only_dependency_does_not_gate_cpu_jobs() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let first = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("first", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let second = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("second", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    factory
        .link_nodes(first, second, RenderNodeDependencyKind::Gpu)
        .unwrap();
    let built = factory.finish().unwrap();
    let mut counters = RenderGraphDependencyCounters::prepare(built.graph()).unwrap();

    counters.release_external_kickoff();
    assert_eq!(counters.take_ready_batch(), vec![first, second]);
}

#[test]
fn dependency_counter_terminal_completion_waits_for_terminal_nodes() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let parent = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("parent", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let child = factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("child", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    factory
        .link_nodes(parent, child, RenderNodeDependencyKind::Cpu)
        .unwrap();
    let built = factory.finish().unwrap();
    let mut counters = RenderGraphDependencyCounters::prepare(built.graph()).unwrap();

    counters.release_external_kickoff();
    counters.begin_node(parent).unwrap();
    counters.complete_node(parent).unwrap();
    assert!(!counters.terminal_completed());

    counters.begin_node(child).unwrap();
    counters.complete_node(child).unwrap();
    assert!(counters.terminal_completed());
}

#[test]
fn dependency_counter_premature_terminal_debug_check_catches_invalid_state() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new("node", RenderNodeCommandListUsage::None, Arc::clone(&log)),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut counters = RenderGraphDependencyCounters::prepare(built.graph()).unwrap();

    counters.release_external_kickoff();
    counters.debug_force_terminal_completion_for_test();
    assert!(matches!(
        counters.debug_validate_terminal_completion(),
        Err(RenderGraphError::InvalidState { reason })
            if reason.contains("terminal graph completion")
    ));
}

#[test]
fn dependency_counter_consume_passes_frame_runtime_and_worker_index() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let runtime_workers = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    factory
        .create_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RecordingNode::new(
                "runtime",
                RenderNodeCommandListUsage::None,
                Arc::clone(&log),
            )
            .require_frame_runtime(Arc::clone(&runtime_workers)),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut allocator = FrameResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::Consume);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = RenderNodeProcessState::new();
    let mut recorders = FrameCommandRecorders::prepare_for_graph(built.graph()).unwrap();

    let report = execute_graph_dependency_counter_consume(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        &mut state,
        &mut allocator,
        &mut builder,
        Some(&mut recorders),
    )
    .unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), &["runtime"]);
    assert_eq!(runtime_workers.lock().unwrap().as_slice(), &[0]);
    assert_eq!(report.scheduled_jobs, 1);
    assert_eq!(report.completed_jobs, 1);
    assert_eq!(report.terminal_nodes, 1);
    assert!(report.terminal_completed);
    assert_eq!(report.node_reports[0].worker_index, 0);
}
