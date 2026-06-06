use super::super::{
    ExternalFrameResourceId, FrameBufferDesc, FrameResourceAllocator, FrameResourceDesc,
    FrameResourceError, FrameResourceOwnership, FrameTextureDesc, RenderCameraAccess,
    RenderFlowGroup, RenderFlowSpace, RenderGraphError, RenderGraphResult, RenderNodeFrameRuntime,
    RenderNodeImplContext, RenderNodeImplContextInit, ResourceAllocatorPhase, ResourceRequest,
    ResourceUsage,
};
use crate::{RenderAppPlugin, RenderDevice};
use bevy_app::App;
use bevy_math::URect;
use wgpu::{
    BufferDescriptor, BufferUsages, Extent3d, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages,
};

fn flow_group(index: u16) -> super::super::FrameResourceFlowGroup {
    super::super::FrameResourceFlowGroup::new(index)
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

fn render_device() -> RenderDevice {
    let mut app = App::new();
    app.add_plugins(RenderAppPlugin);
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

#[test]
fn camera_and_unique_node_flow_spaces_match_context_rules() {
    let mut allocator = FrameResourceAllocator::new();
    let color = "scene_color";
    let shared = RenderNodeImplContext::rt_shared_name_tag(color);

    let camera_a =
        RenderNodeImplContext::camera_node(&mut allocator, flow_group(0), RenderFlowSpace::new(1));
    let tag_a = camera_a.rt_name_tag(color);
    drop(camera_a);

    let camera_b =
        RenderNodeImplContext::camera_node(&mut allocator, flow_group(0), RenderFlowSpace::new(2));
    let tag_b = camera_b.rt_name_tag(color);
    drop(camera_b);

    let unique = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
    let unique_tag = unique.rt_name_tag(color);

    assert_ne!(tag_a, tag_b);
    assert_ne!(tag_a, shared);
    assert_eq!(unique_tag, shared);
}

#[test]
fn wrappers_record_the_same_allocator_request_stream() {
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    let color_tag;
    let temp_tag;
    {
        let mut rctx = RenderNodeImplContext::camera_node(
            &mut allocator,
            flow_group(0),
            RenderFlowSpace::new(4),
        );
        color_tag = rctx.rt_name_tag("color");
        temp_tag = rctx.temp_resource_tag("scratch").unwrap();
        rctx.declare_resource(color_tag, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();
        rctx.declare_resource_like(temp_tag, color_tag).unwrap();
        rctx.use_begin(color_tag, ResourceUsage::WRITE).unwrap();
        rctx.use_end(color_tag).unwrap();
    }

    let requests = allocator.request_group(flow_group(0)).unwrap().requests();
    assert!(matches!(
        requests[0],
        ResourceRequest::Declare { tag, .. } if tag == color_tag
    ));
    assert!(matches!(
        requests[1],
        ResourceRequest::DeclareLike { dst, src } if dst == temp_tag && src == color_tag
    ));
    assert!(matches!(
        requests[2],
        ResourceRequest::UseBegin { tag, .. } if tag == color_tag
    ));
    assert!(matches!(
        requests[3],
        ResourceRequest::UseEnd { tag } if tag == color_tag
    ));
    assert_eq!(requests.len(), 4);
}

#[test]
fn temp_tags_reproduce_from_the_same_request_position_during_consume() {
    let mut allocator = FrameResourceAllocator::new();
    let preconsume_temp;
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::camera_node(
            &mut allocator,
            flow_group(0),
            RenderFlowSpace::new(1),
        );
        preconsume_temp = rctx.temp_resource_tag("scratch").unwrap();
        rctx.declare_resource(
            preconsume_temp,
            FrameResourceDesc::Texture(texture_desc(64)),
        )
        .unwrap();
    }

    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    let consume_temp;
    {
        let mut rctx = RenderNodeImplContext::camera_node(
            &mut allocator,
            flow_group(0),
            RenderFlowSpace::new(1),
        );
        consume_temp = rctx.temp_resource_tag("scratch").unwrap();
        rctx.declare_resource(consume_temp, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();
    }

    assert_eq!(preconsume_temp, consume_temp);
}

#[test]
fn temp_tags_are_unique_when_request_position_advances() {
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();

    let first;
    let second;
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        first = rctx.temp_resource_tag("scratch").unwrap();
        rctx.declare_resource(first, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();

        second = rctx.temp_resource_tag("scratch").unwrap();
        rctx.declare_resource(second, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();
    }

    assert_ne!(first, second);
}

#[test]
fn explicit_use_begin_and_end_record_use_range() {
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    let color;
    {
        let mut rctx = RenderNodeImplContext::camera_node(
            &mut allocator,
            flow_group(0),
            RenderFlowSpace::new(1),
        );
        color = rctx.rt_name_tag("color");
        rctx.declare_resource(color, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();
        rctx.use_begin(color, ResourceUsage::WRITE).unwrap();
        rctx.use_end(color).unwrap();
    }

    let requests = allocator.request_group(flow_group(0)).unwrap().requests();
    assert!(matches!(
        requests[1],
        ResourceRequest::UseBegin { tag, .. } if tag == color
    ));
    assert!(matches!(
        requests[2],
        ResourceRequest::UseEnd { tag } if tag == color
    ));
}

#[test]
fn decision_returns_preconsume_value_during_consume_replay() {
    let mut allocator = FrameResourceAllocator::new();

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        assert!(rctx.decision(true).unwrap());
    }

    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        assert!(rctx.decision(false).unwrap());
    }
}

#[test]
fn is_declared_records_deterministic_request_result() {
    let mut allocator = FrameResourceAllocator::new();

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    let color;
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        color = rctx.rt_name_tag("color");
        assert!(!rctx.is_declared(color).unwrap());
        rctx.declare_resource(color, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();
        assert!(rctx.is_declared(color).unwrap());
        rctx.free(color).unwrap();
        assert!(!rctx.is_declared(color).unwrap());
    }

    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        assert!(!rctx.is_declared(color).unwrap());
        rctx.declare_resource(color, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();
        assert!(rctx.is_declared(color).unwrap());
        rctx.free(color).unwrap();
        assert!(!rctx.is_declared(color).unwrap());
    }
}

#[test]
fn node_getters_forward_to_current_allocator_timeline() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let color;

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::camera_node(
            &mut allocator,
            flow_group(0),
            RenderFlowSpace::new(1),
        );
        color = rctx.rt_name_tag("color");
        rctx.declare_resource(color, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();
        rctx.use_begin(color, ResourceUsage::WRITE).unwrap();
        rctx.use_end(color).unwrap();
    }

    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator.resolve_frame_resources(&render_device).unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::camera_node(
            &mut allocator,
            flow_group(0),
            RenderFlowSpace::new(1),
        );
        rctx.declare_resource(color, FrameResourceDesc::Texture(texture_desc(64)))
            .unwrap();
        rctx.use_begin(color, ResourceUsage::WRITE).unwrap();
        assert!(rctx.get_texture(color).is_ok());
        rctx.use_end(color).unwrap();
    }
}

#[test]
fn node_getter_is_unavailable_during_preconsume() {
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();

    let err = {
        let rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        match rctx.get_texture(rctx.rt_name_tag("color")) {
            Ok(_) => panic!("resource getter unexpectedly succeeded during pre-consume"),
            Err(err) => err,
        }
    };

    assert_eq!(
        err,
        FrameResourceError::InvalidOperation {
            operation: "FrameResourceAllocator::validate_resource_retrieval_phase",
            reason: "resource retrieval is only valid during consume",
        }
    );
}

#[test]
fn node_buffer_getter_forwards_to_current_allocator_timeline() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let lights;

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::camera_node(
            &mut allocator,
            flow_group(0),
            RenderFlowSpace::new(1),
        );
        lights = rctx.rt_name_tag("clustered_lights");
        rctx.declare_resource(lights, FrameResourceDesc::Buffer(buffer_desc(4096)))
            .unwrap();
        rctx.use_begin(lights, ResourceUsage::WRITE).unwrap();
        rctx.use_end(lights).unwrap();
    }

    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator.resolve_frame_resources(&render_device).unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::camera_node(
            &mut allocator,
            flow_group(0),
            RenderFlowSpace::new(1),
        );
        rctx.declare_resource(lights, FrameResourceDesc::Buffer(buffer_desc(4096)))
            .unwrap();
        rctx.use_begin(lights, ResourceUsage::WRITE).unwrap();
        assert!(rctx.get_buffer(lights).is_ok());
        assert!(rctx.try_get_buffer(lights).unwrap().is_some());
        rctx.use_end(lights).unwrap();
    }
}

#[test]
fn import_and_swap_external_wrappers_record_resource_identity() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let history_id = ExternalFrameResourceId::new(33);
    let desc = texture_desc(64);
    let history;

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        history = rctx.rt_name_tag("history");
        rctx.import_texture(history, history_id, desc.clone())
            .unwrap();
    }
    let (texture, view) = create_external_texture(&render_device, &desc);
    allocator
        .register_external_texture(history_id, desc.clone(), texture, view)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator.resolve_frame_resources(&render_device).unwrap();
    assert_eq!(
        allocator.resource_pool().allocations()[0].ownership(),
        FrameResourceOwnership::Imported
    );

    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        rctx.import_texture(history, history_id, desc).unwrap();
        assert!(rctx.get_texture(history).is_ok());
    }
}

#[test]
fn swap_external_wrappers_record_texture_and_buffer_requests() {
    let mut allocator = FrameResourceAllocator::new();
    let texture_id = ExternalFrameResourceId::new(51);
    let buffer_id = ExternalFrameResourceId::new(52);
    let texture = texture_desc(64);
    let buffer = buffer_desc(4096);

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    let color;
    let lights;
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        color = rctx.rt_name_tag("color");
        lights = rctx.rt_name_tag("clustered_lights");
        rctx.declare_resource(color, FrameResourceDesc::Texture(texture.clone()))
            .unwrap();
        rctx.swap_external_texture(color, texture_id, texture.clone())
            .unwrap();
        rctx.declare_resource(lights, FrameResourceDesc::Buffer(buffer.clone()))
            .unwrap();
        rctx.swap_external_buffer(lights, buffer_id, buffer.clone())
            .unwrap();
    }

    let requests = allocator.request_group(flow_group(0)).unwrap().requests();
    assert!(matches!(
        &requests[1],
        ResourceRequest::SwapWithExternal {
            tag,
            resource,
        } if *tag == color
            && resource.external_id() == texture_id
            && resource
                .desc()
                .is_exact_match(&FrameResourceDesc::Texture(texture.clone()))
    ));
    assert!(matches!(
        &requests[3],
        ResourceRequest::SwapWithExternal {
            tag,
            resource,
        } if *tag == lights
            && resource.external_id() == buffer_id
            && resource
                .desc()
                .is_exact_match(&FrameResourceDesc::Buffer(buffer.clone()))
    ));
}

#[test]
fn buffer_import_wrapper_records_buffer_kind() {
    let mut allocator = FrameResourceAllocator::new();
    let buffer_id = ExternalFrameResourceId::new(40);

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        let tag = rctx.rt_name_tag("clustered_lights");
        rctx.import_buffer(tag, buffer_id, buffer_desc(4096))
            .unwrap();
    }

    let requests = allocator.request_group(flow_group(0)).unwrap().requests();
    assert!(matches!(
        &requests[0],
        ResourceRequest::Import {
            resource,
            ..
        } if resource
            .desc()
            .is_exact_match(&FrameResourceDesc::Buffer(buffer_desc(4096)))
    ));
}

#[test]
fn typed_declare_helpers_preserve_descriptor_capacity_fields() {
    let mut allocator = FrameResourceAllocator::new();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();

    let texture = texture_desc(64).with_max_size(Some(wgpu::Extent3d {
        width: 128,
        height: 128,
        depth_or_array_layers: 1,
    }));
    let buffer = buffer_desc(256).with_max_size_bytes(Some(1024));

    {
        let mut rctx = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));
        rctx.declare_texture(rctx.rt_name_tag("typed_texture"), texture.clone())
            .unwrap();
        rctx.declare_buffer(rctx.rt_name_tag("typed_buffer"), buffer.clone())
            .unwrap();
    }

    let requests = allocator.request_group(flow_group(0)).unwrap().requests();
    assert!(matches!(
        &requests[0],
        ResourceRequest::Declare {
            desc: FrameResourceDesc::Texture(recorded),
            ..
        } if recorded.current_size().width == 64
            && recorded.max_size().unwrap().width == 128
    ));
    assert!(matches!(
        &requests[1],
        ResourceRequest::Declare {
            desc: FrameResourceDesc::Buffer(recorded),
            ..
        } if recorded.current_size_bytes() == 256
            && recorded.max_size_bytes() == Some(1024)
    ));
}

#[test]
fn context_reports_uninitialized_state_loudly() {
    let mut allocator = FrameResourceAllocator::new();
    let rctx = RenderNodeImplContext::uninitialized(&mut allocator);

    assert!(matches!(
        rctx.ensure_setup(),
        Err(RenderGraphError::InvalidState { .. })
    ));
}

#[test]
fn camera_access_split_rejects_wrong_node_kind() {
    let mut allocator = FrameResourceAllocator::new();
    let unique = RenderNodeImplContext::unique_node(&mut allocator, flow_group(0));

    assert!(matches!(
        unique.current_camera_access(),
        Err(RenderGraphError::InvalidState { .. })
    ));
    assert_eq!(
        unique.indexed_camera_access(7).unwrap(),
        RenderCameraAccess::Indexed { camera_index: 7 }
    );
    assert_eq!(unique.all_camera_access().unwrap(), RenderCameraAccess::All);

    let camera =
        RenderNodeImplContext::camera_node(&mut allocator, flow_group(1), RenderFlowSpace::new(4));

    assert_eq!(
        camera.current_camera_access().unwrap(),
        RenderCameraAccess::Current { camera_index: 4 }
    );
    assert!(matches!(
        camera.indexed_camera_access(2),
        Err(RenderGraphError::InvalidState { .. })
    ));
    assert!(matches!(
        camera.all_camera_access(),
        Err(RenderGraphError::InvalidState { .. })
    ));
}

#[test]
fn context_worker_init_copy_preserves_flow_identity_but_changes_worker() {
    let init = RenderNodeImplContextInit::camera_node_with_index(
        flow_group(5),
        RenderFlowSpace::new(9),
        123,
    )
    .with_dispatcher_thread_index(0);
    let mut allocator = FrameResourceAllocator::new();
    let rctx = RenderNodeImplContext::new(&mut allocator, init);

    let worker_init = rctx.init_for_worker(77).unwrap();

    assert_eq!(worker_init.flow_group(), flow_group(5));
    assert_eq!(worker_init.flow_space(), RenderFlowSpace::new(9));
    assert_eq!(worker_init.camera_index(), Some(123));
    assert_eq!(worker_init.dispatcher_thread_index(), 77);
}

#[derive(Default)]
struct TestFrameRuntime {
    command_recorder_active: bool,
    active_pass: bool,
    calls: Vec<(RenderFlowGroup, URect)>,
}

impl RenderNodeFrameRuntime for TestFrameRuntime {
    fn has_command_recorder(&self, _flow_group: RenderFlowGroup) -> RenderGraphResult<bool> {
        Ok(self.command_recorder_active)
    }

    fn set_command_recorder_active(
        &mut self,
        _flow_group: RenderFlowGroup,
        active: bool,
    ) -> RenderGraphResult<()> {
        self.command_recorder_active = active;
        Ok(())
    }

    fn set_viewport(
        &mut self,
        flow_group: RenderFlowGroup,
        viewport: URect,
    ) -> RenderGraphResult<()> {
        if !self.active_pass {
            return Err(RenderGraphError::InvalidState {
                reason: "viewport requires an active pass",
            });
        }

        self.calls.push((flow_group, viewport));
        Ok(())
    }
}

#[test]
fn command_recorder_access_routes_through_frame_runtime() {
    let mut allocator = FrameResourceAllocator::new();
    let mut runtime = TestFrameRuntime::default();
    let mut rctx = RenderNodeImplContext::new_with_runtime(
        &mut allocator,
        &mut runtime,
        RenderNodeImplContextInit::unique_node(flow_group(3)),
    );

    assert!(rctx.has_frame_runtime());
    assert!(!rctx.has_command_recorder().unwrap());
    rctx.set_command_recorder_active(true).unwrap();
    assert!(rctx.has_command_recorder().unwrap());
}

#[test]
fn viewport_outside_active_pass_fails_loudly() {
    let mut allocator = FrameResourceAllocator::new();
    let mut runtime = TestFrameRuntime::default();
    let viewport = URect::new(1, 2, 64, 32);
    let mut rctx = RenderNodeImplContext::new_with_runtime(
        &mut allocator,
        &mut runtime,
        RenderNodeImplContextInit::unique_node(flow_group(4)),
    );

    assert!(matches!(
        rctx.set_viewport(viewport),
        Err(RenderGraphError::InvalidState { .. })
    ));
}

#[test]
fn viewport_inside_active_pass_routes_through_frame_runtime() {
    let mut allocator = FrameResourceAllocator::new();
    let mut runtime = TestFrameRuntime {
        active_pass: true,
        ..Default::default()
    };
    let viewport = URect::new(0, 0, 128, 96);
    {
        let mut rctx = RenderNodeImplContext::new_with_runtime(
            &mut allocator,
            &mut runtime,
            RenderNodeImplContextInit::unique_node(flow_group(6)),
        );
        rctx.set_viewport(viewport).unwrap();
    }

    assert_eq!(runtime.calls, vec![(flow_group(6), viewport)]);
}
