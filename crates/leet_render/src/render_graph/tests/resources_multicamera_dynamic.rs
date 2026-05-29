use super::super::{
    ExternalFrameResourceId, FrameResourceAllocator, FrameResourceDesc, FrameResourceOwnership,
    FrameResourceResult, FrameTextureDesc, ImportedFrameResource, RenderFlowGroup, RenderFlowName,
    RenderFlowNameTag, RenderFlowSpace, RenderNodeImplContext, ResourceAllocatorPhase,
    ResourceRequest, ResourceUsage,
};
use crate::{RenderDevice, RenderPlugin};
use bevy_app::App;
use wgpu::{Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages};

fn flow_group(index: u16) -> RenderFlowGroup {
    RenderFlowGroup::new(index)
}

fn camera_space(index: u8) -> RenderFlowSpace {
    RenderFlowSpace::new(index)
}

fn camera_tag(name: &'static str, camera_index: u8) -> RenderFlowNameTag {
    RenderFlowNameTag::new(
        RenderFlowName::from_static(name),
        camera_space(camera_index),
    )
}

fn texture_size(width: u32) -> Extent3d {
    Extent3d {
        width,
        height: 32,
        depth_or_array_layers: 1,
    }
}

fn texture_desc(width: u32) -> FrameTextureDesc {
    texture_desc_with_capacity(width, width)
}

fn texture_desc_with_capacity(current_width: u32, max_width: u32) -> FrameTextureDesc {
    FrameTextureDesc::new(TextureDescriptor {
        label: None,
        size: texture_size(current_width),
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
    .with_max_size(Some(texture_size(max_width)))
}

fn declare(tag: RenderFlowNameTag, desc: FrameTextureDesc) -> ResourceRequest {
    ResourceRequest::Declare {
        tag,
        desc: FrameResourceDesc::Texture(desc),
    }
}

fn use_begin(tag: RenderFlowNameTag) -> ResourceRequest {
    ResourceRequest::UseBegin {
        tag,
        usage: ResourceUsage::WRITE,
    }
}

fn use_end(tag: RenderFlowNameTag) -> ResourceRequest {
    ResourceRequest::UseEnd { tag }
}

fn free(tag: RenderFlowNameTag) -> ResourceRequest {
    ResourceRequest::Free { tag }
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

fn preconsume_and_resolve(
    allocator: &mut FrameResourceAllocator,
    requests: &[ResourceRequest],
) -> FrameResourceResult<()> {
    allocator.set_phase(ResourceAllocatorPhase::PreConsume)?;
    for request in requests {
        allocator.record_request(flow_group(0), request.clone())?;
    }
    allocator.set_phase(ResourceAllocatorPhase::Resolve)
}

fn materialize_and_cleanup_frame(
    allocator: &mut FrameResourceAllocator,
    render_device: &RenderDevice,
    requests: &[ResourceRequest],
) -> FrameResourceResult<()> {
    preconsume_and_resolve(allocator, requests)?;
    allocator.resolve_frame_resources(render_device)?;
    allocator.set_phase(ResourceAllocatorPhase::Consume)?;
    for request in requests {
        allocator.record_request(flow_group(0), request.clone())?;
    }
    allocator.set_phase(ResourceAllocatorPhase::Cleanup)
}

fn empty_frame(
    allocator: &mut FrameResourceAllocator,
    render_device: &RenderDevice,
) -> FrameResourceResult<()> {
    if allocator.phase() == ResourceAllocatorPhase::Cleanup {
        allocator.set_phase(ResourceAllocatorPhase::Startup)?;
    }
    materialize_and_cleanup_frame(allocator, render_device, &[])
}

#[test]
fn camera_contexts_with_same_resource_name_create_distinct_logical_tags() {
    let mut allocator = FrameResourceAllocator::new();
    let main = RenderNodeImplContext::camera_node(&mut allocator, flow_group(0), camera_space(1))
        .rt_name_tag("scene_color");
    let mirror = RenderNodeImplContext::camera_node(&mut allocator, flow_group(0), camera_space(2))
        .rt_name_tag("scene_color");

    assert_ne!(main, mirror);
    assert_eq!(main.flow_space(), camera_space(1));
    assert_eq!(mirror.flow_space(), camera_space(2));
}

#[test]
fn simultaneous_main_and_mirror_targets_do_not_share_allocations() {
    let main = camera_tag("scene_color", 1);
    let mirror = camera_tag("scene_color", 2);
    let requests = vec![
        declare(main, texture_desc(128)),
        declare(mirror, texture_desc(64)),
        use_begin(main),
        use_begin(mirror),
        use_end(main),
        use_end(mirror),
    ];
    let mut allocator = FrameResourceAllocator::new();

    preconsume_and_resolve(&mut allocator, &requests).unwrap();

    let solution = allocator.lifetime_solution().unwrap();
    let main_request = solution
        .allocation_requests()
        .iter()
        .find(|request| request.tag() == main)
        .unwrap();
    let mirror_request = solution
        .allocation_requests()
        .iter()
        .find(|request| request.tag() == mirror)
        .unwrap();
    let plan = allocator.pool_plan().unwrap();
    let main_assignment = plan.assignment_for_request(main_request.id()).unwrap();
    let mirror_assignment = plan.assignment_for_request(mirror_request.id()).unwrap();

    assert_ne!(main, mirror);
    assert_ne!(
        main_assignment.allocation_id(),
        mirror_assignment.allocation_id()
    );
}

#[test]
fn compatible_non_overlapping_camera_targets_reuse_same_frame_allocation() {
    let main = camera_tag("scene_color", 1);
    let mirror = camera_tag("scene_color", 2);
    let requests = vec![
        declare(main, texture_desc(96)),
        use_begin(main),
        use_end(main),
        free(main),
        declare(mirror, texture_desc(96)),
        use_begin(mirror),
        use_end(mirror),
        free(mirror),
    ];
    let mut allocator = FrameResourceAllocator::new();

    preconsume_and_resolve(&mut allocator, &requests).unwrap();

    let solution = allocator.lifetime_solution().unwrap();
    let main_request = solution
        .allocation_requests()
        .iter()
        .find(|request| request.tag() == main)
        .unwrap();
    let mirror_request = solution
        .allocation_requests()
        .iter()
        .find(|request| request.tag() == mirror)
        .unwrap();
    let plan = allocator.pool_plan().unwrap();
    let main_assignment = plan.assignment_for_request(main_request.id()).unwrap();
    let mirror_assignment = plan.assignment_for_request(mirror_request.id()).unwrap();

    assert_eq!(
        main_assignment.allocation_id(),
        mirror_assignment.allocation_id()
    );
    assert!(mirror_assignment.reused_existing());
}

#[test]
fn dynamic_resolution_growth_beyond_cached_capacity_allocates_larger_texture() {
    let render_device = render_device();
    let tag = camera_tag("scene_color", 1);
    let small_frame = vec![
        declare(tag, texture_desc_with_capacity(64, 128)),
        use_begin(tag),
        use_end(tag),
    ];
    let large_frame = vec![
        declare(tag, texture_desc_with_capacity(256, 256)),
        use_begin(tag),
        use_end(tag),
    ];
    let mut allocator = FrameResourceAllocator::new();

    materialize_and_cleanup_frame(&mut allocator, &render_device, &small_frame).unwrap();
    let cached_small = allocator.resource_pool().allocations()[0].id();
    allocator
        .set_phase(ResourceAllocatorPhase::Startup)
        .unwrap();
    preconsume_and_resolve(&mut allocator, &large_frame).unwrap();
    allocator.resolve_frame_resources(&render_device).unwrap();

    let request_id = allocator
        .lifetime_solution()
        .unwrap()
        .allocation_requests()
        .iter()
        .find(|request| request.tag() == tag)
        .unwrap()
        .id();
    let large_assignment = allocator
        .pool_plan()
        .unwrap()
        .assignment_for_request(request_id)
        .unwrap();

    assert_ne!(large_assignment.allocation_id(), cached_small);
    assert!(allocator
        .pool_plan()
        .unwrap()
        .rejections()
        .iter()
        .any(|rejection| rejection.candidate_allocation_id() == cached_small));
}

#[test]
fn dynamic_resolution_growth_within_logical_max_still_respects_concrete_cache_size() {
    let render_device = render_device();
    let tag = camera_tag("scene_color", 1);
    let small_frame = vec![
        declare(tag, texture_desc_with_capacity(64, 128)),
        use_begin(tag),
        use_end(tag),
    ];
    let larger_frame = vec![
        declare(tag, texture_desc_with_capacity(96, 128)),
        use_begin(tag),
        use_end(tag),
    ];
    let mut allocator = FrameResourceAllocator::new();

    materialize_and_cleanup_frame(&mut allocator, &render_device, &small_frame).unwrap();
    let cached_small = allocator.resource_pool().allocations()[0].id();
    allocator
        .set_phase(ResourceAllocatorPhase::Startup)
        .unwrap();
    preconsume_and_resolve(&mut allocator, &larger_frame).unwrap();
    allocator.resolve_frame_resources(&render_device).unwrap();

    let request_id = allocator
        .lifetime_solution()
        .unwrap()
        .allocation_requests()
        .iter()
        .find(|request| request.tag() == tag)
        .unwrap()
        .id();
    let larger_assignment = allocator
        .pool_plan()
        .unwrap()
        .assignment_for_request(request_id)
        .unwrap();

    assert_ne!(larger_assignment.allocation_id(), cached_small);
}

#[test]
fn dynamic_resolution_shrink_reuses_larger_cached_capacity_until_eviction() {
    let render_device = render_device();
    let tag = camera_tag("scene_color", 1);
    let large_frame = vec![
        declare(tag, texture_desc_with_capacity(128, 128)),
        use_begin(tag),
        use_end(tag),
    ];
    let small_frame = vec![
        declare(tag, texture_desc_with_capacity(64, 128)),
        use_begin(tag),
        use_end(tag),
    ];
    let mut allocator = FrameResourceAllocator::new();

    materialize_and_cleanup_frame(&mut allocator, &render_device, &large_frame).unwrap();
    let cached_large = allocator.resource_pool().allocations()[0].id();
    allocator
        .set_phase(ResourceAllocatorPhase::Startup)
        .unwrap();
    materialize_and_cleanup_frame(&mut allocator, &render_device, &small_frame).unwrap();

    assert_eq!(allocator.resource_pool().allocations().len(), 1);
    assert_eq!(
        allocator.resource_pool().allocations()[0].id(),
        cached_large
    );
    assert_eq!(
        allocator
            .resource_pool()
            .oversized_cached_allocations_for(&FrameResourceDesc::Texture(texture_desc(64))),
        vec![cached_large]
    );

    for _ in 0..=super::super::resources::FrameResourcePool::DEFAULT_MAX_UNUSED_AGE {
        empty_frame(&mut allocator, &render_device).unwrap();
    }

    assert!(allocator.resource_pool().allocations().is_empty());
}

#[test]
fn repeated_dynamic_resolution_shrink_evicts_overcapacity_cache_even_when_used() {
    let render_device = render_device();
    let tag = camera_tag("scene_color", 1);
    let large_frame = vec![
        declare(tag, texture_desc(128)),
        use_begin(tag),
        use_end(tag),
    ];
    let small_frame = vec![declare(tag, texture_desc(64)), use_begin(tag), use_end(tag)];
    let mut allocator = FrameResourceAllocator::new();

    materialize_and_cleanup_frame(&mut allocator, &render_device, &large_frame).unwrap();
    assert_eq!(allocator.resource_pool().allocations().len(), 1);

    for _ in 0..=super::super::resources::FrameResourcePool::DEFAULT_MAX_UNUSED_AGE {
        allocator
            .set_phase(ResourceAllocatorPhase::Startup)
            .unwrap();
        materialize_and_cleanup_frame(&mut allocator, &render_device, &small_frame).unwrap();
    }

    assert!(allocator.resource_pool().allocations().is_empty());
}

#[test]
fn imported_camera_target_is_tracked_but_not_recycled() {
    let render_device = render_device();
    let tag = camera_tag("camera_target", 1);
    let external_id = ExternalFrameResourceId::new(500);
    let desc = texture_desc(128);
    let requests = vec![
        ResourceRequest::Import {
            tag,
            resource: ImportedFrameResource::texture(external_id, desc.clone()),
        },
        use_begin(tag),
        use_end(tag),
    ];
    let mut allocator = FrameResourceAllocator::new();

    preconsume_and_resolve(&mut allocator, &requests).unwrap();
    let (texture, view) = create_external_texture(&render_device, &desc);
    allocator
        .register_external_texture(external_id, desc, texture, view)
        .unwrap();
    allocator.resolve_frame_resources(&render_device).unwrap();

    assert_eq!(allocator.resource_pool().allocations().len(), 1);
    assert_eq!(
        allocator.resource_pool().allocations()[0].ownership(),
        FrameResourceOwnership::Imported
    );

    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    for request in &requests {
        allocator
            .record_request(flow_group(0), request.clone())
            .unwrap();
    }
    allocator
        .set_phase(ResourceAllocatorPhase::Cleanup)
        .unwrap();

    assert!(allocator.resource_pool().allocations().is_empty());
}
