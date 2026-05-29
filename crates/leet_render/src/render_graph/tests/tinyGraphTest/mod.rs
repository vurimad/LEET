//! Test-only graph-shaped driver for allocator/rctx integration.
//!
//! This module is intentionally not a production render graph. It only proves
//! that a sequence of node-like callbacks can drive the frame resource allocator
//! through pre-consume, resolve, consume, and cleanup without bypassing
//! `RenderNodeImplContext`.

use super::super::{
    execute_graph_sequential_gpu_order, AddGraphOptions, BuiltRenderNodeGraph,
    ExternalFrameResourceId, FrameBufferDesc, FrameCommandSubmission, FrameResourceAllocationId,
    FrameResourceAllocator, FrameResourceDesc, FrameResourceOwnership, FrameResourceResult,
    FrameTextureDesc, QueueSyncKind, RenderFlowGroup, RenderFlowSpace, RenderGraphCache,
    RenderGraphCoreRunner, RenderGraphResult, RenderGraphShapeHash, RenderGraphShapeHashBuilder,
    RenderNodeBeginRenderTargets, RenderNodeCommandListUsage, RenderNodeDebugName,
    RenderNodeDeclareResources, RenderNodeDependencyKind, RenderNodeEndRenderTargets,
    RenderNodeExecutionMetadata, RenderNodeGraph, RenderNodeGraphFactory, RenderNodeId,
    RenderNodeImpl, RenderNodeImplContext, RenderNodeImplContextInit, RenderNodeKind,
    RenderNodeParameters, RenderNodeResourceDeclaration, RenderNodeRole, RenderNodeStartRender,
    RenderNodeSubtype, RenderNodeSynchronize, RenderQueueKind, ResourceAllocatorPhase,
    ResourceRequest, ResourceUsage,
};
use crate::{RenderDevice, RenderPlugin};
use bevy_app::App;
use leet_jobs2::{Builder as RenderJobBuilder, JobSystemConfig, LeetJobSystem, Priority};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use wgpu::{
    BufferDescriptor, BufferUsages, Extent3d, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages,
};

type ObservedAllocations = Rc<RefCell<Vec<FrameResourceAllocationId>>>;

fn flow_group(index: u16) -> RenderFlowGroup {
    RenderFlowGroup::new(index)
}

fn texture_desc(width: u32) -> FrameTextureDesc {
    FrameTextureDesc::new(TextureDescriptor {
        label: None,
        size: Extent3d {
            width,
            height: 32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn buffer_desc(size: u64) -> FrameBufferDesc {
    FrameBufferDesc::new(BufferDescriptor {
        label: None,
        size,
        usage: BufferUsages::COPY_DST | BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

fn graph_hash(value: u64) -> RenderGraphShapeHash {
    let mut builder = RenderGraphShapeHashBuilder::new();
    builder.append_u64(value);
    builder.finish()
}

fn render_device() -> RenderDevice {
    let mut app = App::new();
    app.add_plugins(RenderPlugin);
    app.world().resource::<RenderDevice>().clone()
}

fn create_external_texture(
    render_device: &RenderDevice,
    desc: &FrameTextureDesc,
) -> (wgpu::Texture, wgpu::TextureView) {
    let descriptor = desc
        .concrete_descriptor_for_shape(desc.max_capacity_shape())
        .unwrap();
    let texture = render_device.0.create_texture(&descriptor);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_external_buffer(render_device: &RenderDevice, desc: &FrameBufferDesc) -> wgpu::Buffer {
    let descriptor = desc
        .concrete_descriptor_for_shape(desc.max_capacity_shape())
        .unwrap();
    render_device.0.create_buffer(&descriptor)
}

trait TinyGraphNode {
    fn init(&self, flow_group: RenderFlowGroup) -> RenderNodeImplContextInit {
        RenderNodeImplContextInit::unique_node(flow_group)
    }

    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()>;
}

struct TinyGraph {
    allocator: FrameResourceAllocator,
    render_device: RenderDevice,
    flow_group: RenderFlowGroup,
    nodes: Vec<Box<dyn TinyGraphNode>>,
}

impl TinyGraph {
    fn new() -> Self {
        Self {
            allocator: FrameResourceAllocator::new(),
            render_device: render_device(),
            flow_group: flow_group(0),
            nodes: Vec::new(),
        }
    }

    fn add_node<N>(&mut self, node: N)
    where
        N: TinyGraphNode + 'static,
    {
        self.nodes.push(Box::new(node));
    }

    fn execute(&mut self) -> FrameResourceResult<()> {
        self.execute_with_external_setup(|_, _| Ok(()))
    }

    fn execute_with_external_setup<F>(&mut self, setup: F) -> FrameResourceResult<()>
    where
        F: FnOnce(&mut FrameResourceAllocator, &RenderDevice) -> FrameResourceResult<()>,
    {
        self.execute_with_hooks(setup, |_, _| Ok(()))
    }

    fn execute_with_hooks<F, G>(
        &mut self,
        setup: F,
        after_materialize: G,
    ) -> FrameResourceResult<()>
    where
        F: FnOnce(&mut FrameResourceAllocator, &RenderDevice) -> FrameResourceResult<()>,
        G: FnOnce(&FrameResourceAllocator, &RenderDevice) -> FrameResourceResult<()>,
    {
        if self.allocator.phase() == ResourceAllocatorPhase::Cleanup {
            self.allocator.set_phase(ResourceAllocatorPhase::Startup)?;
        }

        self.allocator
            .set_phase(ResourceAllocatorPhase::PreConsume)?;
        self.run_nodes()?;

        self.allocator.set_phase(ResourceAllocatorPhase::Resolve)?;
        setup(&mut self.allocator, &self.render_device)?;
        self.allocator
            .resolve_frame_resources(&self.render_device)?;
        after_materialize(&self.allocator, &self.render_device)?;

        self.allocator.set_phase(ResourceAllocatorPhase::Consume)?;
        self.run_nodes()?;

        self.allocator.set_phase(ResourceAllocatorPhase::Cleanup)?;
        Ok(())
    }

    fn run_nodes(&mut self) -> FrameResourceResult<()> {
        for node in &mut self.nodes {
            let init = node.init(self.flow_group);
            let mut rctx = RenderNodeImplContext::new(&mut self.allocator, init);
            node.run(&mut rctx)?;
        }

        Ok(())
    }
}

struct TextureProducerNode {
    name: &'static str,
    desc: FrameTextureDesc,
    observed: Option<ObservedAllocations>,
}

impl TinyGraphNode for TextureProducerNode {
    fn init(&self, flow_group: RenderFlowGroup) -> RenderNodeImplContextInit {
        RenderNodeImplContextInit::camera_node(flow_group, RenderFlowSpace::new(3))
    }

    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        let color = rctx.rt_name_tag(self.name);
        rctx.declare_resource(color, FrameResourceDesc::Texture(self.desc.clone()))?;
        rctx.use_begin(color, ResourceUsage::WRITE)?;
        if rctx.is_consume_phase() {
            if let Some(observed) = &self.observed {
                observed.borrow_mut().push(
                    rctx.resource_allocator()
                        .resolved_allocation_id(color)?
                        .unwrap(),
                );
            }
        }
        rctx.use_end(color)
    }
}

struct TextureConsumerNode {
    name: &'static str,
    observed: Option<ObservedAllocations>,
}

impl TinyGraphNode for TextureConsumerNode {
    fn init(&self, flow_group: RenderFlowGroup) -> RenderNodeImplContextInit {
        RenderNodeImplContextInit::camera_node(flow_group, RenderFlowSpace::new(3))
    }

    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        let color = rctx.rt_name_tag(self.name);
        rctx.use_begin(color, ResourceUsage::READ)?;
        if rctx.is_consume_phase() {
            assert!(rctx.get_texture(color).is_ok());
            assert!(rctx.try_get_texture(color)?.is_some());
            assert!(rctx.get_buffer(color).is_err());
            if let Some(observed) = &self.observed {
                observed.borrow_mut().push(
                    rctx.resource_allocator()
                        .resolved_allocation_id(color)?
                        .unwrap(),
                );
            }
        }
        rctx.use_end(color)
    }
}

struct BufferProducerNode {
    name: &'static str,
    desc: FrameBufferDesc,
    observed: Option<ObservedAllocations>,
}

impl TinyGraphNode for BufferProducerNode {
    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        let buffer = rctx.rt_name_tag(self.name);
        rctx.declare_resource(buffer, FrameResourceDesc::Buffer(self.desc.clone()))?;
        rctx.use_begin(buffer, ResourceUsage::WRITE)?;
        if rctx.is_consume_phase() {
            if let Some(observed) = &self.observed {
                observed.borrow_mut().push(
                    rctx.resource_allocator()
                        .resolved_allocation_id(buffer)?
                        .unwrap(),
                );
            }
        }
        rctx.use_end(buffer)
    }
}

struct BufferConsumerNode {
    name: &'static str,
    observed: Option<ObservedAllocations>,
}

impl TinyGraphNode for BufferConsumerNode {
    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        let buffer = rctx.rt_name_tag(self.name);
        rctx.use_begin(buffer, ResourceUsage::READ)?;
        if rctx.is_consume_phase() {
            assert!(rctx.get_buffer(buffer).is_ok());
            assert!(rctx.try_get_buffer(buffer)?.is_some());
            assert!(rctx.get_texture(buffer).is_err());
            if let Some(observed) = &self.observed {
                observed.borrow_mut().push(
                    rctx.resource_allocator()
                        .resolved_allocation_id(buffer)?
                        .unwrap(),
                );
            }
        }
        rctx.use_end(buffer)
    }
}

struct ImportTextureNode {
    name: &'static str,
    external_id: ExternalFrameResourceId,
    desc: FrameTextureDesc,
}

impl TinyGraphNode for ImportTextureNode {
    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        let tag = rctx.rt_name_tag(self.name);
        rctx.import_texture(tag, self.external_id, self.desc.clone())?;
        rctx.use_begin(tag, ResourceUsage::READ)?;
        if rctx.is_consume_phase() {
            assert!(rctx.get_texture(tag).is_ok());
        }
        rctx.use_end(tag)
    }
}

struct ImportBufferNode {
    name: &'static str,
    external_id: ExternalFrameResourceId,
    desc: FrameBufferDesc,
}

impl TinyGraphNode for ImportBufferNode {
    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        let tag = rctx.rt_name_tag(self.name);
        rctx.import_buffer(tag, self.external_id, self.desc.clone())?;
        rctx.use_begin(tag, ResourceUsage::READ)?;
        if rctx.is_consume_phase() {
            assert!(rctx.get_buffer(tag).is_ok());
        }
        rctx.use_end(tag)
    }
}

struct SwapNode {
    first_name: &'static str,
    second_name: &'static str,
    desc: FrameTextureDesc,
    before: Option<(FrameResourceAllocationId, FrameResourceAllocationId)>,
}

impl TinyGraphNode for SwapNode {
    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        let first = rctx.rt_name_tag(self.first_name);
        let second = rctx.rt_name_tag(self.second_name);
        rctx.declare_resource(first, FrameResourceDesc::Texture(self.desc.clone()))?;
        rctx.declare_resource(second, FrameResourceDesc::Texture(self.desc.clone()))?;
        rctx.use_begin(first, ResourceUsage::WRITE)?;
        rctx.use_end(first)?;
        rctx.use_begin(second, ResourceUsage::WRITE)?;
        rctx.use_end(second)?;

        if rctx.is_consume_phase() {
            let first_before = rctx
                .resource_allocator()
                .resolved_allocation_id(first)?
                .unwrap();
            let second_before = rctx
                .resource_allocator()
                .resolved_allocation_id(second)?
                .unwrap();
            assert_ne!(first_before, second_before);
            self.before = Some((first_before, second_before));
        }

        rctx.swap(first, second)?;

        if rctx.is_consume_phase() {
            let (first_before, second_before) = self.before.unwrap();
            assert_eq!(
                rctx.resource_allocator().resolved_allocation_id(first)?,
                Some(second_before)
            );
            assert_eq!(
                rctx.resource_allocator().resolved_allocation_id(second)?,
                Some(first_before)
            );
        }

        Ok(())
    }
}

struct DecisionBranchNode {
    ignore_stabilized_value: bool,
}

impl TinyGraphNode for DecisionBranchNode {
    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        let runtime_decision = !rctx.is_consume_phase();
        let stabilized_decision = rctx.decision(runtime_decision)?;
        let branch_decision = if self.ignore_stabilized_value {
            runtime_decision
        } else {
            stabilized_decision
        };

        let tag = if branch_decision {
            rctx.rt_name_tag("branch_a")
        } else {
            rctx.rt_name_tag("branch_b")
        };
        rctx.declare_resource(tag, FrameResourceDesc::Texture(texture_desc(32)))
    }
}

struct MissingConsumeReplayNode {
    consumed_once: bool,
}

impl TinyGraphNode for MissingConsumeReplayNode {
    fn run(&mut self, rctx: &mut RenderNodeImplContext<'_>) -> FrameResourceResult<()> {
        if rctx.is_consume_phase() && !self.consumed_once {
            self.consumed_once = true;
            return Ok(());
        }

        let tag = rctx.rt_name_tag("must_replay");
        rctx.declare_resource(tag, FrameResourceDesc::Texture(texture_desc(32)))
    }
}

#[derive(Clone)]
struct TinyGraphUseNode {
    name: &'static str,
    usage: ResourceUsage,
}

impl TinyGraphUseNode {
    fn new(name: &'static str, usage: ResourceUsage) -> Self {
        Self { name, usage }
    }
}

impl RenderNodeImpl for TinyGraphUseNode {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Require
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        let tag = rctx.rt_name_tag(self.name);
        rctx.use_begin(tag, self.usage)?;
        rctx.use_end(tag)?;
        Ok(())
    }
}

#[derive(Clone)]
struct TinyLogNode {
    name: &'static str,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl TinyLogNode {
    fn new(name: &'static str, log: Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self { name, log }
    }
}

impl RenderNodeImpl for TinyLogNode {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        if rctx.is_consume_phase() {
            self.log.lock().unwrap().push(self.name);
        }
        Ok(())
    }
}

struct RenderJobHarness {
    jobs: LeetJobSystem,
}

impl RenderJobHarness {
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

impl Drop for RenderJobHarness {
    fn drop(&mut self) {
        self.jobs.shutdown();
    }
}

fn build_command_group_node(
    factory: &mut RenderNodeGraphFactory,
    group: super::super::NodeGroupId,
    label: &'static str,
    resource_name: &'static str,
) -> RenderGraphResult<RenderNodeId> {
    let node = factory.begin_command_list_group(
        group,
        RenderNodeKind::Stage,
        RenderNodeSubtype::DEFAULT,
        label,
        RenderQueueKind::Graphics,
    )?;
    factory.create_subnode(RenderNodeDeclareResources::new(
        label,
        vec![RenderNodeResourceDeclaration::buffer(
            resource_name,
            buffer_desc(256),
        )],
    ))?;
    factory.create_subnode(TinyGraphUseNode::new(resource_name, ResourceUsage::WRITE))?;
    factory.end_command_list_group()?;
    Ok(node)
}

fn build_core_tiny_graph() -> RenderGraphResult<BuiltRenderNodeGraph> {
    let mut factory = RenderNodeGraphFactory::new();
    let group = factory.create_group()?;
    let start = factory.create_system_node(
        group,
        RenderNodeKind::Unique,
        RenderNodeSubtype::new(10),
        RenderNodeStartRender::new(),
    )?;
    let draw_a = build_command_group_node(&mut factory, group, "DrawA", "draw_a_buffer")?;
    let draw_b = build_command_group_node(&mut factory, group, "DrawB", "draw_b_buffer")?;
    let sync = factory.create_system_node(
        group,
        RenderNodeKind::Stage,
        RenderNodeSubtype::DEFAULT,
        RenderNodeSynchronize::new(QueueSyncKind::Barrier, "BarrierAfterDraws"),
    )?;
    let declare_target = factory.create_system_node(
        group,
        RenderNodeKind::Stage,
        RenderNodeSubtype::DEFAULT,
        RenderNodeDeclareResources::new(
            "DeclareMainTarget",
            vec![RenderNodeResourceDeclaration::buffer(
                "main_target",
                buffer_desc(512),
            )],
        ),
    )?;
    let begin_target = factory.create_system_node(
        group,
        RenderNodeKind::Stage,
        RenderNodeSubtype::DEFAULT,
        RenderNodeBeginRenderTargets::new("BeginMainTarget", "main_target", ResourceUsage::WRITE),
    )?;
    let end_target = factory.create_system_node(
        group,
        RenderNodeKind::Stage,
        RenderNodeSubtype::DEFAULT,
        RenderNodeEndRenderTargets::new("EndMainTarget", "main_target"),
    )?;

    factory.link_cpu(start, draw_a)?;
    factory.link_cpu(draw_a, draw_b)?;
    factory.link_cpu(draw_b, sync)?;
    factory.link_cpu(sync, declare_target)?;
    factory.link_cpu(declare_target, begin_target)?;
    factory.link_cpu(begin_target, end_target)?;

    factory.link_gpu(draw_b, draw_a)?;
    factory.link_gpu(draw_a, sync)?;
    factory.link_gpu(sync, declare_target)?;
    factory.link_gpu(declare_target, begin_target)?;
    factory.link_gpu(begin_target, end_target)?;

    factory.finish()
}

fn command_submission_labels(submissions: &[FrameCommandSubmission]) -> Vec<&str> {
    submissions
        .iter()
        .map(|submission| submission.label.as_str())
        .collect()
}

#[test]
fn tiny_graph_transient_texture_and_buffer_nodes_replay_and_cleanup() {
    let mut graph = TinyGraph::new();
    let texture_observed = Rc::new(RefCell::new(Vec::new()));
    let buffer_observed = Rc::new(RefCell::new(Vec::new()));
    graph.add_node(TextureProducerNode {
        name: "scene_color",
        desc: texture_desc(64),
        observed: Some(texture_observed.clone()),
    });
    graph.add_node(TextureConsumerNode {
        name: "scene_color",
        observed: Some(texture_observed.clone()),
    });
    graph.add_node(BufferProducerNode {
        name: "clustered_lights",
        desc: buffer_desc(4096),
        observed: Some(buffer_observed.clone()),
    });
    graph.add_node(BufferConsumerNode {
        name: "clustered_lights",
        observed: Some(buffer_observed.clone()),
    });

    graph.execute().unwrap();

    let texture_observed = texture_observed.borrow();
    assert_eq!(texture_observed.len(), 2);
    assert_eq!(texture_observed[0], texture_observed[1]);

    let buffer_observed = buffer_observed.borrow();
    assert_eq!(buffer_observed.len(), 2);
    assert_eq!(buffer_observed[0], buffer_observed[1]);

    assert_eq!(graph.allocator.phase(), ResourceAllocatorPhase::Cleanup);
    assert_eq!(graph.allocator.request_group_count(), 0);
}

#[test]
fn tiny_graph_imports_texture_and_buffer_before_materialized_resolve() {
    let texture_id = ExternalFrameResourceId::new(70);
    let buffer_id = ExternalFrameResourceId::new(71);
    let texture = texture_desc(64);
    let buffer = buffer_desc(2048);
    let mut graph = TinyGraph::new();
    graph.add_node(ImportTextureNode {
        name: "history_color",
        external_id: texture_id,
        desc: texture.clone(),
    });
    graph.add_node(ImportBufferNode {
        name: "readback",
        external_id: buffer_id,
        desc: buffer.clone(),
    });

    graph
        .execute_with_hooks(
            |allocator, render_device| {
                let (external_texture, external_view) =
                    create_external_texture(render_device, &texture);
                allocator.register_external_texture(
                    texture_id,
                    texture.clone(),
                    external_texture,
                    external_view,
                )?;

                let external_buffer = create_external_buffer(render_device, &buffer);
                allocator.register_external_buffer(buffer_id, buffer.clone(), external_buffer)
            },
            |allocator, _| {
                let imported_count = allocator
                    .resource_pool()
                    .allocations()
                    .iter()
                    .filter(|allocation| allocation.ownership() == FrameResourceOwnership::Imported)
                    .count();
                assert_eq!(imported_count, 2);
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn tiny_graph_swap_path_uses_current_consume_position() {
    let mut graph = TinyGraph::new();
    graph.add_node(SwapNode {
        first_name: "ping",
        second_name: "pong",
        desc: texture_desc(64),
        before: None,
    });

    graph.execute().unwrap();
}

#[test]
fn tiny_graph_decision_catches_divergent_branch() {
    let mut graph = TinyGraph::new();
    graph.add_node(DecisionBranchNode {
        ignore_stabilized_value: true,
    });

    assert!(graph.execute().is_err());
}

#[test]
fn tiny_graph_cleanup_requires_full_consume_replay() {
    let mut graph = TinyGraph::new();
    graph.add_node(MissingConsumeReplayNode {
        consumed_once: false,
    });

    assert!(graph.execute().is_err());
}

#[test]
fn tiny_graph_cleanup_leaves_allocator_ready_for_next_frame() {
    let mut graph = TinyGraph::new();
    graph.add_node(TextureProducerNode {
        name: "scene_color",
        desc: texture_desc(64),
        observed: None,
    });
    graph.add_node(TextureConsumerNode {
        name: "scene_color",
        observed: None,
    });

    graph.execute().unwrap();
    assert_eq!(graph.allocator.phase(), ResourceAllocatorPhase::Cleanup);

    graph.execute().unwrap();
    assert_eq!(graph.allocator.phase(), ResourceAllocatorPhase::Cleanup);
    assert_eq!(graph.allocator.request_group_count(), 0);
}

#[test]
fn tiny_graph_core_runner_executes_production_shaped_graph() {
    let built = build_core_tiny_graph().unwrap();
    let jobs = RenderJobHarness::new();
    let mut builder = jobs.builder();
    let mut runner = RenderGraphCoreRunner::new();

    let report = runner.execute_built_graph(&built, &mut builder).unwrap();

    assert_eq!(
        report.phase_order,
        vec![
            ResourceAllocatorPhase::Startup,
            ResourceAllocatorPhase::PreConsume,
            ResourceAllocatorPhase::Resolve,
            ResourceAllocatorPhase::Consume,
            ResourceAllocatorPhase::Cleanup,
        ]
    );
    assert_eq!(
        report
            .consume_report
            .ready_batches
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        vec![1, 1, 1, 1, 1, 1, 1]
    );
    assert_eq!(
        command_submission_labels(&report.command_submissions),
        vec!["DrawB", "DrawA"]
    );
    assert!(report.terminal_completed_before_cleanup);
    assert!(report.cleanup_ran);
    assert_eq!(runner.allocator().phase(), ResourceAllocatorPhase::Cleanup);
    assert_eq!(runner.allocator().request_group_count(), 0);
}

#[test]
fn tiny_graph_command_group_subnodes_record_requests_in_order() {
    let built = build_core_tiny_graph().unwrap();
    let jobs = RenderJobHarness::new();
    let mut builder = jobs.builder();
    let mut state = super::super::RenderNodeProcessState::new();
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();

    let reports = execute_graph_sequential_gpu_order(
        built.graph(),
        built.impl_store(),
        built.command_group_store(),
        &mut state,
        &mut allocator,
        &mut builder,
    )
    .unwrap();
    let draw_b_report = reports
        .iter()
        .find(|report| {
            built
                .graph()
                .node(report.node)
                .unwrap()
                .debug_name()
                .as_str()
                == "DrawB"
        })
        .unwrap();
    let requests = allocator
        .request_group(draw_b_report.flow_group)
        .unwrap()
        .requests();

    assert!(matches!(
        requests,
        [
            ResourceRequest::BeginQueue {
                queue: RenderQueueKind::Graphics
            },
            ResourceRequest::Declare { .. },
            ResourceRequest::UseBegin { .. },
            ResourceRequest::UseEnd { .. },
            ResourceRequest::EndQueue,
        ]
    ));
}

#[test]
fn tiny_graph_gpu_order_controls_command_submission() {
    let built = build_core_tiny_graph().unwrap();
    let jobs = RenderJobHarness::new();
    let mut builder = jobs.builder();
    let mut runner = RenderGraphCoreRunner::new();

    let report = runner.execute_built_graph(&built, &mut builder).unwrap();

    assert_eq!(
        command_submission_labels(&report.command_submissions),
        vec!["DrawB", "DrawA"]
    );
}

#[test]
fn tiny_graph_imported_camera_topology_builds_flow_groups_after_merge() {
    let mut final_graph = RenderNodeGraph::new();
    let mut camera_graph = RenderNodeGraph::new();

    let begin = camera_graph
        .add_node(
            RenderNodeParameters::new(
                RenderNodeKind::SequenceBegin,
                RenderNodeRole::LifecycleSystem,
                RenderNodeSubtype::new(80),
                None,
                RenderNodeDebugName::new("CameraBegin"),
            ),
            RenderNodeExecutionMetadata::new(Some(0), None),
        )
        .unwrap();
    let draw = camera_graph
        .add_node(
            RenderNodeParameters::new(
                RenderNodeKind::Stage,
                RenderNodeRole::LifecycleSystem,
                RenderNodeSubtype::new(81),
                None,
                RenderNodeDebugName::new("CameraDraw"),
            ),
            RenderNodeExecutionMetadata::new(Some(0), None),
        )
        .unwrap();
    let end = camera_graph
        .add_node(
            RenderNodeParameters::new(
                RenderNodeKind::SequenceEnd,
                RenderNodeRole::LifecycleSystem,
                RenderNodeSubtype::new(80),
                None,
                RenderNodeDebugName::new("CameraEnd"),
            ),
            RenderNodeExecutionMetadata::new(Some(0), None),
        )
        .unwrap();
    camera_graph
        .add_dependency(RenderNodeDependencyKind::Cpu, begin, draw)
        .unwrap();
    camera_graph
        .add_dependency(RenderNodeDependencyKind::Cpu, draw, end)
        .unwrap();
    camera_graph
        .add_dependency(RenderNodeDependencyKind::Gpu, begin, draw)
        .unwrap();
    camera_graph
        .add_dependency(RenderNodeDependencyKind::Gpu, draw, end)
        .unwrap();

    final_graph
        .add_graph(&camera_graph, AddGraphOptions::default())
        .unwrap();
    final_graph.build_flow_groups().unwrap();

    assert!(final_graph.is_built());
    assert_eq!(final_graph.node_count(), 1);
    assert_eq!(
        final_graph
            .flattened_nodes(RenderNodeDependencyKind::Cpu)
            .len(),
        1
    );
    assert_eq!(
        final_graph
            .flattened_nodes(RenderNodeDependencyKind::Gpu)
            .len(),
        1
    );
}

#[test]
fn tiny_graph_cache_hit_executes_retained_final_graph() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut cache = RenderGraphCache::with_capacity(1);
    let hash = graph_hash(18);

    {
        let lookup = cache.get_graph(hash, 1, 1, false).unwrap();
        assert!(lookup.needs_rebuild);

        let mut factory = RenderNodeGraphFactory::new();
        let group = factory.create_group().unwrap();
        factory
            .create_node(
                group,
                RenderNodeKind::Stage,
                RenderNodeSubtype::DEFAULT,
                TinyLogNode::new("cached", Arc::clone(&log)),
            )
            .unwrap();
        lookup.entry.set_final_graph(factory.finish().unwrap());
    }

    let lookup = cache.get_graph(hash, 1, 2, false).unwrap();
    assert!(!lookup.needs_rebuild);
    let jobs = RenderJobHarness::new();
    let mut builder = jobs.builder();
    let mut runner = RenderGraphCoreRunner::new();
    runner
        .execute_built_graph(lookup.entry.final_graph().unwrap(), &mut builder)
        .unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), &["cached"]);
}
