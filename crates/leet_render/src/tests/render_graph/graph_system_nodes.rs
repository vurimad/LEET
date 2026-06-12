use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use leet_jobs2::{Builder as RenderJobBuilder, JobSystemConfig, LeetJobSystem, Priority};
use wgpu::{BufferDescriptor, BufferUsages};

use super::super::{
    execute_graph_dependency_counter_consume, execute_graph_sequential_gpu_order, process_node,
    process_node_with_runtime, FrameBufferDesc, FrameCommandRecorders, QueueSyncKind,
    RenderGraphResult, RenderNodeBeginRenderTargets, RenderNodeDeclareResources,
    RenderNodeEndFrame, RenderNodeEndRender, RenderNodeEndRenderTargets, RenderNodeGraphFactory,
    RenderNodeKind, RenderNodeResourceDeclaration, RenderNodeStartRender, RenderNodeSubtype,
    RenderNodeSynchronize, RenderResourceAllocator, ResourceAllocatorPhase, ResourceRequest,
    ResourceUsage,
};

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

fn test_buffer_desc(label: &'static str) -> FrameBufferDesc {
    FrameBufferDesc::new(BufferDescriptor {
        label: Some(label),
        size: 64,
        usage: BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn transition_to(allocator: &mut RenderResourceAllocator, phase: ResourceAllocatorPhase) {
    match phase {
        ResourceAllocatorPhase::Startup => {}
        ResourceAllocatorPhase::PreConsume => {
            allocator
                .set_phase(ResourceAllocatorPhase::PreConsume)
                .unwrap();
        }
        ResourceAllocatorPhase::Resolve => {
            transition_to(allocator, ResourceAllocatorPhase::PreConsume);
            allocator
                .set_phase(ResourceAllocatorPhase::Resolve)
                .unwrap();
        }
        ResourceAllocatorPhase::Consume => {
            transition_to(allocator, ResourceAllocatorPhase::Resolve);
            allocator
                .set_phase(ResourceAllocatorPhase::Consume)
                .unwrap();
        }
        ResourceAllocatorPhase::Cleanup => {
            transition_to(allocator, ResourceAllocatorPhase::Consume);
            allocator
                .set_phase(ResourceAllocatorPhase::Cleanup)
                .unwrap();
        }
    }
}

#[test]
fn lifecycle_nodes_participate_in_dependencies_like_normal_nodes() {
    let start_counter = Arc::new(AtomicU64::new(0));
    let end_counter = Arc::new(AtomicU64::new(0));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let start = factory
        .create_system_node(
            group,
            RenderNodeKind::Unique,
            RenderNodeSubtype::new(1),
            RenderNodeStartRender::new().with_consume_counter(Arc::clone(&start_counter)),
        )
        .unwrap();
    let end = factory
        .create_system_node(
            group,
            RenderNodeKind::Unique,
            RenderNodeSubtype::new(2),
            RenderNodeEndRender::new().with_consume_counter(Arc::clone(&end_counter)),
        )
        .unwrap();
    factory.link_cpu(start, end).unwrap();
    factory.link_gpu(start, end).unwrap();
    let built = factory.finish().unwrap();
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = super::super::RenderNodeProcessState::new();
    let mut allocator = RenderResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let preconsume_reports = execute_graph_sequential_gpu_order(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    let report = execute_graph_dependency_counter_consume(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        &mut state,
        &mut allocator,
        &mut builder,
        None,
    )
    .unwrap();

    assert_eq!(
        preconsume_reports
            .iter()
            .map(|report| report.node)
            .collect::<Vec<_>>(),
        vec![start, end]
    );
    assert_eq!(report.ready_batches, vec![vec![start], vec![end]]);
    assert_eq!(start_counter.load(Ordering::Relaxed), 1);
    assert_eq!(end_counter.load(Ordering::Relaxed), 1);
}

#[test]
fn declaration_system_nodes_use_no_command_list_and_record_declarations() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_system_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RenderNodeDeclareResources::new(
                "DeclareCommon",
                vec![RenderNodeResourceDeclaration::buffer(
                    "common_buffer",
                    test_buffer_desc("common"),
                )],
            ),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let mut allocator = RenderResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = super::super::RenderNodeProcessState::new();

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

    let request_group = allocator.request_group(report.flow_group).unwrap();
    let requests = request_group.requests();
    assert_eq!(
        report.command_list_usage,
        super::super::RenderNodeCommandListUsage::None
    );
    assert!(matches!(requests, [ResourceRequest::Declare { .. }]));
}

#[test]
fn sync_node_records_allocator_and_command_sync() {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_system_node(
            group,
            RenderNodeKind::Stage,
            RenderNodeSubtype::DEFAULT,
            RenderNodeSynchronize::new(QueueSyncKind::Barrier, "sync_barrier"),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let jobs = JobHarness::new();
    let mut state = super::super::RenderNodeProcessState::new();
    let mut allocator = RenderResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);
    let mut preconsume_builder = jobs.builder();

    let preconsume_report = process_node(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        node,
        &mut state,
        &mut allocator,
        &mut preconsume_builder,
    )
    .unwrap();
    assert!(matches!(
        allocator
            .request_group(preconsume_report.flow_group)
            .unwrap()
            .requests(),
        [ResourceRequest::QueueSync {
            sync: QueueSyncKind::Barrier
        }]
    ));

    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    let mut recorders = FrameCommandRecorders::prepare_for_graph(built.graph()).unwrap();
    let mut consume_builder = jobs.builder();
    process_node_with_runtime(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        node,
        &mut state,
        &mut allocator,
        &mut consume_builder,
        &mut recorders,
    )
    .unwrap();

    assert_eq!(
        recorders
            .sync_events(preconsume_report.flow_group)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        recorders.sync_events(preconsume_report.flow_group).unwrap()[0].label,
        "sync_barrier"
    );
}

#[test]
fn end_frame_lifecycle_node_does_not_run_allocator_cleanup() {
    let counter = Arc::new(AtomicU64::new(0));
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group().unwrap();
    let node = factory
        .create_system_node(
            group,
            RenderNodeKind::Unique,
            RenderNodeSubtype::new(3),
            RenderNodeEndFrame::new().with_consume_counter(Arc::clone(&counter)),
        )
        .unwrap();
    let built = factory.finish().unwrap();
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = super::super::RenderNodeProcessState::new();
    let mut allocator = RenderResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::Consume);

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

    assert_eq!(allocator.phase(), ResourceAllocatorPhase::Consume);
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

#[test]
fn render_target_markers_record_begin_and_end_use_requests() -> RenderGraphResult<()> {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group()?;
    let declare = factory.create_system_node(
        group,
        RenderNodeKind::Stage,
        RenderNodeSubtype::DEFAULT,
        RenderNodeDeclareResources::new(
            "DeclareTarget",
            vec![RenderNodeResourceDeclaration::buffer(
                "main_target",
                test_buffer_desc("target"),
            )],
        ),
    )?;
    let begin = factory.create_system_node(
        group,
        RenderNodeKind::Stage,
        RenderNodeSubtype::DEFAULT,
        RenderNodeBeginRenderTargets::new("BeginTarget", "main_target", ResourceUsage::WRITE),
    )?;
    let end = factory.create_system_node(
        group,
        RenderNodeKind::Stage,
        RenderNodeSubtype::DEFAULT,
        RenderNodeEndRenderTargets::new("EndTarget", "main_target"),
    )?;
    factory.link_gpu(declare, begin)?;
    factory.link_gpu(begin, end)?;
    let built = factory.finish()?;
    let jobs = JobHarness::new();
    let mut builder = jobs.builder();
    let mut state = super::super::RenderNodeProcessState::new();
    let mut allocator = RenderResourceAllocator::new();
    transition_to(&mut allocator, ResourceAllocatorPhase::PreConsume);

    let reports = super::super::execute_graph_sequential_gpu_order(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        &mut state,
        &mut allocator,
        &mut builder,
    )?;
    assert!(matches!(
        allocator
            .request_group(reports[0].flow_group)
            .unwrap()
            .requests(),
        [ResourceRequest::Declare { .. }]
    ));
    assert!(matches!(
        allocator
            .request_group(reports[1].flow_group)
            .unwrap()
            .requests(),
        [ResourceRequest::UseBegin { .. }]
    ));
    assert!(matches!(
        allocator
            .request_group(reports[2].flow_group)
            .unwrap()
            .requests(),
        [ResourceRequest::UseEnd { .. }]
    ));
    Ok(())
}
