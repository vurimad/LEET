use super::super::resources::{
    AllocationRequestSource, ExternalFrameResourceId, FrameLifetimeSolution,
    FrameResourceAllocator, FrameResourceDesc, FrameTextureDesc, ImportedFrameResource,
    QueueSyncKind, RenderFlowGroup, RenderFlowName, RenderFlowNameTag, RenderFlowSpace,
    RequestGroup, RequestTime, ResourceAllocatorPhase, ResourceRequest, ResourceUsage,
    TagLifetimeEventKind,
};
use wgpu::{Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages};

fn tag(name: &'static str) -> RenderFlowNameTag {
    RenderFlowNameTag::new(RenderFlowName::from_static(name), RenderFlowSpace::SHARED)
}

fn flow_group(index: u16) -> RenderFlowGroup {
    RenderFlowGroup::new(index)
}

fn time(index: u32) -> RequestTime {
    RequestTime::new(flow_group(0), index)
}

fn texture_desc(width: u32, height: u32) -> FrameTextureDesc {
    FrameTextureDesc::new(TextureDescriptor {
        label: None,
        size: Extent3d {
            width,
            height,
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

fn declare(name: &'static str, width: u32) -> ResourceRequest {
    ResourceRequest::Declare {
        tag: tag(name),
        desc: FrameResourceDesc::Texture(texture_desc(width, 32)),
    }
}

fn use_begin(name: &'static str) -> ResourceRequest {
    ResourceRequest::UseBegin {
        tag: tag(name),
        usage: ResourceUsage::READ,
    }
}

fn use_end(name: &'static str) -> ResourceRequest {
    ResourceRequest::UseEnd { tag: tag(name) }
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

#[test]
fn simple_declare_use_free_produces_one_owned_lifetime() {
    let solution = solve(vec![
        declare("color", 64),
        use_begin("color"),
        use_end("color"),
        ResourceRequest::Free { tag: tag("color") },
    ]);

    assert_eq!(solution.allocation_requests().len(), 1);
    let allocation = &solution.allocation_requests()[0];
    assert_eq!(allocation.tag(), tag("color"));
    assert_eq!(allocation.source(), AllocationRequestSource::Owned);
    assert_eq!(allocation.lifetime().start(), time(0));
    assert_eq!(allocation.lifetime().end(), time(3));
    assert!(allocation.can_reuse_same_frame());
    assert!(allocation.can_cache_across_frames());
    assert_eq!(
        solution
            .lookup_allocation_for_tag(tag("color"), time(2))
            .unwrap(),
        Some(allocation.id())
    );
    assert_eq!(
        solution
            .lookup_allocation_for_tag(tag("color"), time(3))
            .unwrap(),
        None
    );
}

#[test]
fn declared_but_unused_resource_produces_no_allocation_request() {
    let solution = solve(vec![declare("unused", 64)]);

    assert!(solution.allocation_requests().is_empty());
    assert_eq!(
        solution
            .lookup_allocation_for_tag(tag("unused"), time(0))
            .unwrap(),
        None
    );
}

#[test]
fn declare_like_copies_source_descriptor_at_request_time() {
    let solution = solve(vec![
        declare("source", 128),
        ResourceRequest::DeclareLike {
            dst: tag("copy"),
            src: tag("source"),
        },
        use_begin("copy"),
        use_end("copy"),
    ]);

    assert_eq!(solution.allocation_requests().len(), 1);
    let FrameResourceDesc::Texture(copy_desc) = solution.allocation_requests()[0].desc() else {
        panic!("declare-like copy should preserve texture descriptor kind");
    };
    assert_eq!(copy_desc.current_size().width, 128);
}

#[test]
fn import_creates_tracked_non_owned_lifetime() {
    let imported =
        ImportedFrameResource::texture(ExternalFrameResourceId::new(11), texture_desc(64, 32));
    let solution = solve(vec![ResourceRequest::Import {
        tag: tag("history"),
        resource: imported,
    }]);

    assert_eq!(solution.allocation_requests().len(), 1);
    let allocation = &solution.allocation_requests()[0];
    assert_eq!(
        allocation.source(),
        AllocationRequestSource::Imported(ExternalFrameResourceId::new(11))
    );
    assert!(!allocation.can_reuse_same_frame());
    assert!(!allocation.can_cache_across_frames());
    assert_eq!(
        solution
            .lookup_allocation_for_tag(tag("history"), time(0))
            .unwrap(),
        Some(allocation.id())
    );
}

#[test]
fn swap_timeline_lookup_returns_different_allocations_after_swap() {
    let solution = solve(vec![
        declare("a", 64),
        declare("b", 64),
        use_begin("a"),
        use_end("a"),
        use_begin("b"),
        use_end("b"),
        ResourceRequest::Swap {
            a: tag("a"),
            b: tag("b"),
        },
    ]);

    let a_before = solution
        .lookup_allocation_for_tag(tag("a"), time(5))
        .unwrap()
        .unwrap();
    let b_before = solution
        .lookup_allocation_for_tag(tag("b"), time(5))
        .unwrap()
        .unwrap();
    assert_ne!(a_before, b_before);
    assert_eq!(
        solution
            .lookup_allocation_for_tag(tag("a"), time(6))
            .unwrap(),
        Some(b_before)
    );
    assert_eq!(
        solution
            .lookup_allocation_for_tag(tag("b"), time(6))
            .unwrap(),
        Some(a_before)
    );

    let a_events = solution.tag_lifetime(tag("a")).unwrap().events();
    assert!(a_events
        .iter()
        .any(|event| event.kind() == TagLifetimeEventKind::Swap));
}

#[test]
fn swap_with_external_restricts_old_and_external_allocation_reuse() {
    let external =
        ImportedFrameResource::texture(ExternalFrameResourceId::new(17), texture_desc(64, 32));
    let solution = solve(vec![
        declare("color", 64),
        use_begin("color"),
        use_end("color"),
        ResourceRequest::SwapWithExternal {
            tag: tag("color"),
            resource: external,
        },
    ]);

    assert_eq!(solution.allocation_requests().len(), 2);
    let old = &solution.allocation_requests()[0];
    let external = &solution.allocation_requests()[1];
    assert_eq!(old.source(), AllocationRequestSource::Owned);
    assert!(!old.can_reuse_same_frame());
    assert!(!old.can_cache_across_frames());
    assert_eq!(
        external.source(),
        AllocationRequestSource::ExternalSwap(ExternalFrameResourceId::new(17))
    );
    assert!(!external.can_reuse_same_frame());
    assert!(!external.can_cache_across_frames());
    assert_eq!(
        solution
            .lookup_allocation_for_tag(tag("color"), time(3))
            .unwrap(),
        Some(external.id())
    );
}

#[test]
fn queue_sync_is_accepted_inside_balanced_use_range() {
    let solution = solve(vec![
        declare("color", 64),
        use_begin("color"),
        ResourceRequest::QueueSync {
            sync: QueueSyncKind::Fork,
        },
        use_end("color"),
    ]);

    assert_eq!(solution.allocation_requests().len(), 1);
    assert!(solution.allocation_requests()[0]
        .lifetime()
        .touches(time(2)));
}

#[test]
fn missing_tag_lookup_fails_loudly() {
    let solution = solve(vec![declare("unused", 64)]);

    assert!(solution
        .lookup_allocation_for_tag(tag("missing"), time(0))
        .is_err());
}

#[test]
fn unbalanced_or_invalid_use_requests_fail_during_solve() {
    let mut group = RequestGroup::new();
    group
        .apply(ResourceAllocatorPhase::PreConsume, declare("color", 64))
        .unwrap();
    group
        .apply(ResourceAllocatorPhase::PreConsume, use_end("color"))
        .unwrap();
    assert!(FrameLifetimeSolution::solve_request_groups(&[group]).is_err());

    let mut group = RequestGroup::new();
    group
        .apply(ResourceAllocatorPhase::PreConsume, declare("color", 64))
        .unwrap();
    group
        .apply(ResourceAllocatorPhase::PreConsume, use_begin("color"))
        .unwrap();
    assert!(FrameLifetimeSolution::solve_request_groups(&[group]).is_err());
}

#[test]
fn use_begin_requires_read_or_write_not_only_no_discard() {
    let mut group = RequestGroup::new();
    group
        .apply(ResourceAllocatorPhase::PreConsume, declare("color", 64))
        .unwrap();
    group
        .apply(
            ResourceAllocatorPhase::PreConsume,
            ResourceRequest::UseBegin {
                tag: tag("color"),
                usage: ResourceUsage::NO_DISCARD,
            },
        )
        .unwrap();

    assert!(FrameLifetimeSolution::solve_request_groups(&[group]).is_err());
}

#[test]
fn swap_requires_descriptor_compatibility() {
    let mut group = RequestGroup::new();
    group
        .apply(ResourceAllocatorPhase::PreConsume, declare("a", 64))
        .unwrap();
    group
        .apply(ResourceAllocatorPhase::PreConsume, declare("b", 128))
        .unwrap();
    group
        .apply(
            ResourceAllocatorPhase::PreConsume,
            ResourceRequest::Swap {
                a: tag("a"),
                b: tag("b"),
            },
        )
        .unwrap();

    assert!(FrameLifetimeSolution::solve_request_groups(&[group]).is_err());
}

#[test]
fn allocator_resolve_stores_lifetime_solution() {
    let mut allocator = FrameResourceAllocator::new();

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

    assert_eq!(
        allocator
            .lifetime_solution()
            .unwrap()
            .allocation_requests()
            .len(),
        1
    );
}
