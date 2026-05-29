use super::super::resources::{
    ExternalFrameResourceId, FrameBufferDesc, FrameResourceAllocator, FrameResourceDesc,
    FrameResourceOwnership, FrameTextureDesc, ImportedFrameResource, RenderFlowGroup,
    RenderFlowName, RenderFlowNameTag, RenderFlowSpace, ResourceAllocatorPhase, ResourceRequest,
    ResourceUsage,
};
use crate::{RenderDevice, RenderPlugin};
use bevy_app::App;
use wgpu::{
    BufferDescriptor, BufferUsages, Extent3d, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages,
};

fn tag(name: &'static str) -> RenderFlowNameTag {
    RenderFlowNameTag::new(RenderFlowName::from_static(name), RenderFlowSpace::SHARED)
}

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

fn declare_texture(name: &'static str, width: u32) -> ResourceRequest {
    ResourceRequest::Declare {
        tag: tag(name),
        desc: FrameResourceDesc::Texture(texture_desc(width)),
    }
}

fn declare_buffer(name: &'static str, size: u64) -> ResourceRequest {
    ResourceRequest::Declare {
        tag: tag(name),
        desc: FrameResourceDesc::Buffer(buffer_desc(size)),
    }
}

fn use_begin(name: &'static str) -> ResourceRequest {
    ResourceRequest::UseBegin {
        tag: tag(name),
        usage: ResourceUsage::WRITE,
    }
}

fn use_end(name: &'static str) -> ResourceRequest {
    ResourceRequest::UseEnd { tag: tag(name) }
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

fn preconsume_requests(allocator: &mut FrameResourceAllocator, requests: &[ResourceRequest]) {
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    for request in requests {
        allocator
            .record_request(flow_group(0), request.clone())
            .unwrap();
    }
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
}

fn replay_requests(allocator: &mut FrameResourceAllocator, requests: &[ResourceRequest]) {
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    for request in requests {
        allocator
            .record_request(flow_group(0), request.clone())
            .unwrap();
    }
}

#[test]
fn getters_fail_before_materialized_resolve() {
    let mut allocator = FrameResourceAllocator::new();
    let requests = vec![
        declare_texture("color", 64),
        use_begin("color"),
        use_end("color"),
    ];

    preconsume_requests(&mut allocator, &requests);
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    allocator
        .record_request(flow_group(0), requests[0].clone())
        .unwrap();

    assert!(allocator.get_texture(tag("color")).is_err());
}

#[test]
fn resolve_materializes_owned_texture_and_getter_returns_it() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let requests = vec![
        declare_texture("color", 64),
        use_begin("color"),
        use_end("color"),
    ];

    preconsume_requests(&mut allocator, &requests);
    allocator.resolve_frame_resources(&render_device).unwrap();

    assert!(allocator.resources_resolved());
    assert_eq!(allocator.resource_pool().allocations().len(), 1);

    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    allocator
        .record_request(flow_group(0), requests[0].clone())
        .unwrap();

    assert!(allocator.get_texture(tag("color")).is_ok());
    assert!(allocator.try_get_texture(tag("color")).unwrap().is_some());
    assert!(allocator.get_buffer(tag("color")).is_err());
}

#[test]
fn resolve_materializes_owned_buffer_and_getter_returns_it() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let requests = vec![
        declare_buffer("lights", 4096),
        use_begin("lights"),
        use_end("lights"),
    ];

    preconsume_requests(&mut allocator, &requests);
    allocator.resolve_frame_resources(&render_device).unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    allocator
        .record_request(flow_group(0), requests[0].clone())
        .unwrap();

    assert!(allocator.get_buffer(tag("lights")).is_ok());
    assert!(allocator.try_get_buffer(tag("lights")).unwrap().is_some());
    assert!(allocator.get_texture(tag("lights")).is_err());
}

#[test]
fn declared_but_unused_resource_resolves_without_allocation() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let requests = vec![declare_texture("optional", 64)];

    preconsume_requests(&mut allocator, &requests);
    allocator.resolve_frame_resources(&render_device).unwrap();
    assert!(allocator.resource_pool().allocations().is_empty());

    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    allocator
        .record_request(flow_group(0), requests[0].clone())
        .unwrap();

    assert!(allocator
        .try_get_texture(tag("optional"))
        .unwrap()
        .is_none());
    assert!(allocator.get_texture(tag("optional")).is_err());
}

#[test]
fn imported_texture_attaches_registered_external_resource() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let external_id = ExternalFrameResourceId::new(10);
    let desc = texture_desc(64);
    let requests = vec![ResourceRequest::Import {
        tag: tag("history"),
        resource: ImportedFrameResource::texture(external_id, desc.clone()),
    }];

    preconsume_requests(&mut allocator, &requests);
    let (texture, view) = create_external_texture(&render_device, &desc);
    allocator
        .register_external_texture(external_id, desc, texture, view)
        .unwrap();
    allocator.resolve_frame_resources(&render_device).unwrap();

    let allocation = allocator.resource_pool().allocations().first().unwrap();
    assert_eq!(allocation.ownership(), FrameResourceOwnership::Imported);

    replay_requests(&mut allocator, &requests);
    assert!(allocator.get_texture(tag("history")).is_ok());

    allocator
        .set_phase(ResourceAllocatorPhase::Cleanup)
        .unwrap();
    assert!(allocator.resource_pool().allocations().is_empty());
}

#[test]
fn imported_buffer_attaches_registered_external_resource() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let external_id = ExternalFrameResourceId::new(11);
    let desc = buffer_desc(2048);
    let requests = vec![ResourceRequest::Import {
        tag: tag("readback"),
        resource: ImportedFrameResource::buffer(external_id, desc.clone()),
    }];

    preconsume_requests(&mut allocator, &requests);
    let buffer = create_external_buffer(&render_device, &desc);
    allocator
        .register_external_buffer(external_id, desc, buffer)
        .unwrap();
    allocator.resolve_frame_resources(&render_device).unwrap();
    replay_requests(&mut allocator, &requests);

    assert!(allocator.get_buffer(tag("readback")).is_ok());
}

#[test]
fn resolve_fails_when_registered_external_descriptor_mismatches_request() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let external_id = ExternalFrameResourceId::new(12);
    let requested = texture_desc(64);
    let registered = texture_desc(128);
    let requests = vec![ResourceRequest::Import {
        tag: tag("history"),
        resource: ImportedFrameResource::texture(external_id, requested),
    }];

    preconsume_requests(&mut allocator, &requests);
    let (texture, view) = create_external_texture(&render_device, &registered);
    allocator
        .register_external_texture(external_id, registered, texture, view)
        .unwrap();

    assert!(allocator.resolve_frame_resources(&render_device).is_err());
}

#[test]
fn getter_after_swap_uses_current_consume_time() {
    let render_device = render_device();
    let mut allocator = FrameResourceAllocator::new();
    let requests = vec![
        declare_texture("a", 64),
        declare_texture("b", 64),
        use_begin("a"),
        use_end("a"),
        use_begin("b"),
        use_end("b"),
        ResourceRequest::Swap {
            a: tag("a"),
            b: tag("b"),
        },
    ];

    preconsume_requests(&mut allocator, &requests);
    allocator.resolve_frame_resources(&render_device).unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();

    for request in &requests[..6] {
        allocator
            .record_request(flow_group(0), request.clone())
            .unwrap();
    }
    let a_before = allocator.resolved_allocation_id(tag("a")).unwrap().unwrap();
    let b_before = allocator.resolved_allocation_id(tag("b")).unwrap().unwrap();
    assert_ne!(a_before, b_before);

    allocator
        .record_request(flow_group(0), requests[6].clone())
        .unwrap();

    assert_eq!(
        allocator.resolved_allocation_id(tag("a")).unwrap(),
        Some(b_before)
    );
    assert_eq!(
        allocator.resolved_allocation_id(tag("b")).unwrap(),
        Some(a_before)
    );
}
