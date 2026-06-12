use super::super::resources::{
    ExternalFrameResourceId, FrameBufferDesc, FrameLifetimeSolution, FrameResourceAllocationClass,
    FrameResourceAllocationId, FrameResourceDesc, FrameResourceOwnership, FrameResourcePool,
    FrameResourcePoolCandidate, FrameResourcePoolPlan, FrameResourceReuseRejectionReason,
    FrameResourceShape, FrameTextureDesc, ImportedFrameResource, RenderFlowGroup, RenderFlowName,
    RenderFlowNameTag, RenderFlowSpace, RenderResourceAllocator, RequestGroup, RequestRange,
    RequestTime, ResourceAllocatorPhase, ResourceRequest, ResourceUsage,
};
use crate::{RenderAppPlugin, RenderDevice};
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

fn time(index: u32) -> RequestTime {
    RequestTime::new(flow_group(0), index)
}

fn texture_desc(width: u32) -> FrameTextureDesc {
    texture_desc_with_capacity(width, width)
}

fn texture_desc_with_capacity(current_width: u32, max_width: u32) -> FrameTextureDesc {
    FrameTextureDesc::new(TextureDescriptor {
        label: None,
        size: Extent3d {
            width: current_width,
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
    .with_max_size(Some(Extent3d {
        width: max_width,
        height: 32,
        depth_or_array_layers: 1,
    }))
}

fn buffer_desc(size: u64) -> FrameBufferDesc {
    FrameBufferDesc::new(BufferDescriptor {
        label: None,
        size,
        usage: BufferUsages::COPY_DST | BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

fn declare(name: &'static str, width: u32) -> ResourceRequest {
    ResourceRequest::Declare {
        tag: tag(name),
        desc: FrameResourceDesc::Texture(texture_desc(width)),
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

fn free(name: &'static str) -> ResourceRequest {
    ResourceRequest::Free { tag: tag(name) }
}

fn solve(requests: Vec<ResourceRequest>) -> FrameLifetimeSolution {
    let mut group = RequestGroup::new();
    for request in requests {
        group
            .apply(ResourceAllocatorPhase::PreConsume, request)
            .unwrap();
    }

    FrameLifetimeSolution::solve_request_groups(&[group]).unwrap()
}

fn plan(requests: Vec<ResourceRequest>) -> FrameResourcePoolPlan {
    FrameResourcePoolPlan::plan(&solve(requests)).unwrap()
}

fn render_device() -> RenderDevice {
    let mut app = App::new();
    app.add_plugins(RenderAppPlugin);
    app.world().resource::<RenderDevice>().clone()
}

#[test]
fn overlapping_lifetimes_never_share_owned_allocation() {
    let plan = plan(vec![
        declare("a", 64),
        declare("b", 64),
        use_begin("a"),
        use_begin("b"),
        use_end("a"),
        use_end("b"),
    ]);

    assert_eq!(plan.assignments().len(), 2);
    assert_ne!(
        plan.assignments()[0].allocation_id(),
        plan.assignments()[1].allocation_id()
    );
    assert!(
        plan.rejections()
            .iter()
            .any(|rejection| rejection.reason()
                == FrameResourceReuseRejectionReason::LifetimeOverlaps)
    );
}

#[test]
fn touching_lifetime_endpoints_count_as_overlap() {
    let a = RequestRange::new(time(0), time(3)).unwrap();
    let b = RequestRange::new(time(3), time(5)).unwrap();

    assert!(a.overlaps(b));
}

#[test]
fn non_overlapping_owned_lifetimes_reuse_same_allocation() {
    let plan = plan(vec![
        declare("a", 64),
        use_begin("a"),
        use_end("a"),
        free("a"),
        declare("b", 64),
        use_begin("b"),
        use_end("b"),
        free("b"),
    ]);

    assert_eq!(plan.assignments().len(), 2);
    assert_eq!(
        plan.assignments()[0].allocation_id(),
        plan.assignments()[1].allocation_id()
    );
    assert!(plan.assignments()[1].reused_existing());
}

#[test]
fn incompatible_cached_descriptor_rejects_reuse_and_explains_why() {
    let solution = solve(vec![
        declare("color", 256),
        use_begin("color"),
        use_end("color"),
    ]);
    let cached = FrameResourcePoolCandidate::owned_reusable(
        FrameResourceAllocationId::new(50),
        FrameResourceDesc::Texture(texture_desc(128)),
    );
    let plan = FrameResourcePoolPlan::plan_with_cached_allocations(&solution, &[cached]).unwrap();

    assert_ne!(
        plan.assignments()[0].allocation_id(),
        FrameResourceAllocationId::new(50)
    );
    assert!(plan
        .rejections()
        .iter()
        .any(|rejection| rejection.reason()
            == FrameResourceReuseRejectionReason::DescriptorIncompatible));
}

#[test]
fn larger_cached_capacity_satisfies_smaller_request() {
    let solution = solve(vec![
        declare("color", 64),
        use_begin("color"),
        use_end("color"),
    ]);
    let cached = FrameResourcePoolCandidate::owned_reusable(
        FrameResourceAllocationId::new(12),
        FrameResourceDesc::Texture(texture_desc(128)),
    );
    let plan = FrameResourcePoolPlan::plan_with_cached_allocations(&solution, &[cached]).unwrap();

    assert_eq!(
        plan.assignments()[0].allocation_id(),
        FrameResourceAllocationId::new(12)
    );
    assert!(plan.assignments()[0].reused_existing());
}

#[test]
fn request_max_size_is_not_treated_as_allocated_capacity() {
    let solution = solve(vec![
        ResourceRequest::Declare {
            tag: tag("larger"),
            desc: FrameResourceDesc::Texture(texture_desc_with_capacity(96, 128)),
        },
        use_begin("larger"),
        use_end("larger"),
        free("larger"),
    ]);
    let cached = FrameResourcePoolCandidate::owned_reusable(
        FrameResourceAllocationId::new(64),
        FrameResourceDesc::Texture(texture_desc(64)),
    );
    let plan = FrameResourcePoolPlan::plan_with_cached_allocations(&solution, &[cached]).unwrap();

    assert_ne!(
        plan.assignments()[0].allocation_id(),
        FrameResourceAllocationId::new(64)
    );
    assert!(plan
        .rejections()
        .iter()
        .any(|rejection| rejection.reason()
            == FrameResourceReuseRejectionReason::DescriptorIncompatible));
}

#[test]
fn different_resource_kinds_never_reuse_the_same_allocation() {
    let solution = solve(vec![
        declare("color", 64),
        use_begin("color"),
        use_end("color"),
    ]);
    let cached = FrameResourcePoolCandidate::owned_reusable(
        FrameResourceAllocationId::new(6),
        FrameResourceDesc::Buffer(buffer_desc(4096)),
    );
    let plan = FrameResourcePoolPlan::plan_with_cached_allocations(&solution, &[cached]).unwrap();

    assert_ne!(
        plan.assignments()[0].allocation_id(),
        FrameResourceAllocationId::new(6)
    );
}

#[test]
fn imported_allocations_are_unique_and_not_recycled() {
    let imported_a =
        ImportedFrameResource::texture(ExternalFrameResourceId::new(1), texture_desc(64));
    let imported_b =
        ImportedFrameResource::texture(ExternalFrameResourceId::new(2), texture_desc(64));
    let plan = plan(vec![
        ResourceRequest::Import {
            tag: tag("history_a"),
            resource: imported_a,
        },
        ResourceRequest::Import {
            tag: tag("history_b"),
            resource: imported_b,
        },
    ]);

    assert_eq!(plan.assignments().len(), 2);
    assert_eq!(
        plan.assignments()[0].class(),
        FrameResourceAllocationClass::Imported
    );
    assert_eq!(
        plan.assignments()[1].class(),
        FrameResourceAllocationClass::Imported
    );
    assert_ne!(
        plan.assignments()[0].allocation_id(),
        plan.assignments()[1].allocation_id()
    );
}

#[test]
fn external_swap_allocations_are_restricted() {
    let external =
        ImportedFrameResource::texture(ExternalFrameResourceId::new(4), texture_desc(64));
    let plan = plan(vec![
        declare("color", 64),
        use_begin("color"),
        use_end("color"),
        ResourceRequest::SwapWithExternal {
            tag: tag("color"),
            resource: external,
        },
    ]);

    assert_eq!(plan.assignments().len(), 2);
    assert!(plan
        .assignments()
        .iter()
        .any(|assignment| assignment.class() == FrameResourceAllocationClass::OwnedRestricted));
    assert!(plan
        .assignments()
        .iter()
        .any(|assignment| assignment.class() == FrameResourceAllocationClass::ExternalSwap));
}

#[test]
fn restricted_cached_candidate_is_rejected_for_owned_reusable_request() {
    let solution = solve(vec![
        declare("color", 64),
        use_begin("color"),
        use_end("color"),
    ]);
    let cached = FrameResourcePoolCandidate::restricted(
        FrameResourceAllocationId::new(8),
        FrameResourceDesc::Texture(texture_desc(64)),
    );
    let plan = FrameResourcePoolPlan::plan_with_cached_allocations(&solution, &[cached]).unwrap();

    assert!(plan
        .rejections()
        .iter()
        .any(|rejection| rejection.reason()
            == FrameResourceReuseRejectionReason::CandidateNotReusable));
}

#[test]
fn cached_allocation_ids_must_be_unique() {
    let solution = solve(vec![
        declare("color", 64),
        use_begin("color"),
        use_end("color"),
    ]);
    let first = FrameResourcePoolCandidate::owned_reusable(
        FrameResourceAllocationId::new(8),
        FrameResourceDesc::Texture(texture_desc(64)),
    );
    let second = FrameResourcePoolCandidate::owned_reusable(
        FrameResourceAllocationId::new(8),
        FrameResourceDesc::Texture(texture_desc(128)),
    );

    assert!(
        FrameResourcePoolPlan::plan_with_cached_allocations(&solution, &[first, second]).is_err()
    );
}

#[test]
fn largest_first_assignment_order_is_deterministic() {
    let solution = solve(vec![
        declare("small", 64),
        use_begin("small"),
        use_end("small"),
        free("small"),
        declare("large", 256),
        use_begin("large"),
        use_end("large"),
        free("large"),
    ]);
    let small_id = solution
        .allocation_requests()
        .iter()
        .find(|request| request.tag() == tag("small"))
        .unwrap()
        .id();
    let large_id = solution
        .allocation_requests()
        .iter()
        .find(|request| request.tag() == tag("large"))
        .unwrap()
        .id();
    let plan = FrameResourcePoolPlan::plan(&solution).unwrap();

    assert_eq!(plan.assignment_order(), &[large_id, small_id]);
}

#[test]
fn allocator_resolve_stores_pool_plan() {
    let mut allocator = RenderResourceAllocator::new();

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    allocator
        .record_request(flow_group(0), declare("color", 64))
        .unwrap();
    allocator
        .record_request(flow_group(0), use_begin("color"))
        .unwrap();
    allocator
        .record_request(flow_group(0), use_end("color"))
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();

    assert_eq!(allocator.pool_plan().unwrap().assignments().len(), 1);
}

#[test]
fn pool_stores_owned_texture_and_buffer_resources() {
    let render_device = render_device();
    let mut pool = FrameResourcePool::new();

    pool.create_owned_texture(
        FrameResourceAllocationId::new(1),
        texture_desc(64),
        &render_device,
    )
    .unwrap();
    pool.create_owned_buffer(
        FrameResourceAllocationId::new(2),
        buffer_desc(1024),
        &render_device,
    )
    .unwrap();

    let texture = pool.allocation(FrameResourceAllocationId::new(1)).unwrap();
    assert_eq!(texture.ownership(), FrameResourceOwnership::Owned);
    assert!(texture.cacheable());
    assert!(texture.resource().as_texture().is_some());

    let buffer = pool.allocation(FrameResourceAllocationId::new(2)).unwrap();
    assert_eq!(buffer.ownership(), FrameResourceOwnership::Owned);
    assert!(buffer.resource().as_buffer().is_some());
}

#[test]
fn pool_owned_texture_uses_current_shape_as_concrete_capacity() {
    let render_device = render_device();
    let mut pool = FrameResourcePool::new();

    pool.create_owned_texture(
        FrameResourceAllocationId::new(7),
        texture_desc_with_capacity(64, 128),
        &render_device,
    )
    .unwrap();

    let allocation = pool.allocation(FrameResourceAllocationId::new(7)).unwrap();
    assert_eq!(
        allocation.desc().current_allocation_shape(),
        FrameResourceShape::Texture {
            size: Extent3d {
                width: 64,
                height: 32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
        }
    );
    assert_eq!(
        allocation.desc().max_capacity_shape(),
        allocation.desc().current_allocation_shape()
    );
}

#[test]
fn pool_imported_texture_is_tracked_but_not_recycled() {
    let render_device = render_device();
    let texture_descriptor = texture_desc(64)
        .concrete_descriptor_for_shape(texture_desc(64).max_capacity_shape())
        .unwrap();
    let texture = render_device.0.create_texture(&texture_descriptor);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut pool = FrameResourcePool::new();

    pool.import_texture(
        FrameResourceAllocationId::new(3),
        texture_desc(64),
        texture,
        view,
    )
    .unwrap();

    let imported = pool.allocation(FrameResourceAllocationId::new(3)).unwrap();
    assert_eq!(imported.ownership(), FrameResourceOwnership::Imported);
    assert!(!imported.cacheable());
    assert!(pool.planner_candidates().is_empty());

    pool.cleanup_after_frame();
    assert!(pool.allocation(FrameResourceAllocationId::new(3)).is_none());
}

#[test]
fn pool_cleanup_ages_and_evicts_owned_allocations() {
    let render_device = render_device();
    let mut pool = FrameResourcePool::with_max_unused_age(1);

    pool.create_owned_buffer(
        FrameResourceAllocationId::new(4),
        buffer_desc(1024),
        &render_device,
    )
    .unwrap();

    pool.cleanup_after_frame();
    assert_eq!(
        pool.allocation(FrameResourceAllocationId::new(4))
            .unwrap()
            .age(),
        0
    );
    pool.cleanup_after_frame();
    assert_eq!(
        pool.allocation(FrameResourceAllocationId::new(4))
            .unwrap()
            .age(),
        1
    );
    pool.cleanup_after_frame();
    assert!(pool.allocation(FrameResourceAllocationId::new(4)).is_none());
}

#[test]
fn pool_cleanup_can_skip_owned_cache_eviction() {
    let render_device = render_device();
    let mut pool = FrameResourcePool::with_max_unused_age(1);

    pool.create_owned_buffer(
        FrameResourceAllocationId::new(4),
        buffer_desc(1024),
        &render_device,
    )
    .unwrap();

    pool.cleanup_after_frame();
    pool.cleanup_after_frame_with_eviction(false);
    pool.cleanup_after_frame_with_eviction(false);

    let allocation = pool.allocation(FrameResourceAllocationId::new(4)).unwrap();
    assert_eq!(allocation.age(), 0);
    assert!(allocation.cacheable());
}

#[test]
fn pool_non_cacheable_owned_allocation_is_released_on_cleanup() {
    let render_device = render_device();
    let mut pool = FrameResourcePool::new();

    pool.create_owned_buffer(
        FrameResourceAllocationId::new(5),
        buffer_desc(1024),
        &render_device,
    )
    .unwrap();
    pool.mark_non_cacheable(FrameResourceAllocationId::new(5))
        .unwrap();
    pool.cleanup_after_frame();

    assert!(pool.allocation(FrameResourceAllocationId::new(5)).is_none());
}

#[test]
fn pool_reports_planner_candidates_and_oversized_cached_allocations() {
    let render_device = render_device();
    let mut pool = FrameResourcePool::new();

    pool.create_owned_texture(
        FrameResourceAllocationId::new(6),
        texture_desc(128),
        &render_device,
    )
    .unwrap();

    let candidates = pool.planner_candidates();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].allocation_id(),
        FrameResourceAllocationId::new(6)
    );

    let oversized =
        pool.oversized_cached_allocations_for(&FrameResourceDesc::Texture(texture_desc(64)));
    assert_eq!(oversized, vec![FrameResourceAllocationId::new(6)]);
}
