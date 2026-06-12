use super::super::resources::{
    FrameResourceDesc, FrameTextureDesc, RenderFlowGroup, RenderFlowName, RenderFlowNameTag,
    RenderFlowSpace, RenderResourceAllocator, ResourceAllocatorPhase, ResourceRequest,
    ResourceUsage, MAX_RENDER_FLOW_GROUPS,
};
use wgpu::{Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages};

fn tag(name: &'static str) -> RenderFlowNameTag {
    RenderFlowNameTag::new(RenderFlowName::from_static(name), RenderFlowSpace::SHARED)
}

fn flow_group(index: u16) -> RenderFlowGroup {
    RenderFlowGroup::new(index)
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

fn declare_color() -> ResourceRequest {
    ResourceRequest::Declare {
        tag: tag("color"),
        desc: FrameResourceDesc::Texture(texture_desc(64, 32)),
    }
}

fn use_color() -> ResourceRequest {
    ResourceRequest::UseBegin {
        tag: tag("color"),
        usage: ResourceUsage::READ,
    }
}

#[test]
fn allocator_valid_phase_sequence_records_replays_and_cleans_up() {
    let mut allocator = RenderResourceAllocator::new();

    assert_eq!(allocator.phase(), ResourceAllocatorPhase::Startup);
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    allocator
        .record_request(flow_group(0), declare_color())
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    allocator
        .record_request(flow_group(0), declare_color())
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Cleanup)
        .unwrap();

    assert_eq!(allocator.phase(), ResourceAllocatorPhase::Cleanup);
    assert_eq!(allocator.request_group_count(), 0);

    allocator
        .set_phase(ResourceAllocatorPhase::Startup)
        .unwrap();
    assert_eq!(allocator.phase(), ResourceAllocatorPhase::Startup);
}

#[test]
fn allocator_invalid_phase_transitions_fail_loudly() {
    let mut allocator = RenderResourceAllocator::new();

    assert!(allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .is_err());
    assert!(allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .is_err());

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    assert!(allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .is_err());
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    assert!(allocator
        .set_phase(ResourceAllocatorPhase::Cleanup)
        .is_err());
}

#[test]
fn allocator_rejects_requests_outside_preconsume_and_consume() {
    let mut allocator = RenderResourceAllocator::new();

    assert!(allocator
        .record_request(flow_group(0), declare_color())
        .is_err());

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    assert!(allocator
        .record_request(flow_group(0), declare_color())
        .is_err());

    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Cleanup)
        .unwrap();
    assert!(allocator
        .record_request(flow_group(0), declare_color())
        .is_err());
}

#[test]
fn allocator_cleanup_requires_full_consume_replay() {
    let mut allocator = RenderResourceAllocator::new();

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    allocator
        .record_request(flow_group(0), declare_color())
        .unwrap();
    allocator
        .record_request(flow_group(0), use_color())
        .unwrap();
    allocator
        .record_request(flow_group(0), ResourceRequest::UseEnd { tag: tag("color") })
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    allocator
        .record_request(flow_group(0), declare_color())
        .unwrap();

    assert!(allocator
        .set_phase(ResourceAllocatorPhase::Cleanup)
        .is_err());
    assert_eq!(allocator.phase(), ResourceAllocatorPhase::Consume);
    assert_eq!(
        allocator
            .request_group(flow_group(0))
            .unwrap()
            .consume_cursor(),
        1
    );
}

#[test]
fn allocator_retrieval_phase_gate_allows_only_consume() {
    let mut allocator = RenderResourceAllocator::new();

    assert!(allocator.validate_resource_retrieval_phase().is_err());
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    assert!(allocator.validate_resource_retrieval_phase().is_err());
    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    assert!(allocator.validate_resource_retrieval_phase().is_err());
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    assert!(allocator.validate_resource_retrieval_phase().is_ok());
    allocator
        .set_phase(ResourceAllocatorPhase::Cleanup)
        .unwrap();
    assert!(allocator.validate_resource_retrieval_phase().is_err());
}

#[test]
fn allocator_clear_all_caches_is_cleanup_only() {
    let mut allocator = RenderResourceAllocator::new();

    assert!(allocator.clear_all_caches().is_err());
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

    allocator.clear_all_caches().unwrap();
    assert_eq!(allocator.caches_cleared_count(), 1);

    allocator
        .set_phase(ResourceAllocatorPhase::Startup)
        .unwrap();
    assert!(allocator.clear_all_caches().is_err());
}

#[test]
fn allocator_resolve_validates_recorded_declarations() {
    let mut allocator = RenderResourceAllocator::new();

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    allocator
        .record_request(
            flow_group(0),
            ResourceRequest::Declare {
                tag: tag("invalid"),
                desc: FrameResourceDesc::Texture(texture_desc(0, 32)),
            },
        )
        .unwrap();

    assert!(allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .is_err());
    assert_eq!(allocator.phase(), ResourceAllocatorPhase::PreConsume);
}

#[test]
fn allocator_next_preconsume_starts_without_previous_frame_requests() {
    let mut allocator = RenderResourceAllocator::new();

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    allocator
        .record_request(flow_group(3), declare_color())
        .unwrap();
    assert_eq!(allocator.request_group_count(), 4);

    allocator
        .set_phase(ResourceAllocatorPhase::Resolve)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Consume)
        .unwrap();
    allocator
        .record_request(flow_group(3), declare_color())
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Cleanup)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::Startup)
        .unwrap();
    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();

    assert_eq!(allocator.request_group_count(), 0);
}

#[test]
fn allocator_caps_render_flow_groups() {
    let mut allocator = RenderResourceAllocator::new();

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();
    allocator
        .prepare_preconsume_groups(MAX_RENDER_FLOW_GROUPS)
        .unwrap();

    let last_valid = flow_group((MAX_RENDER_FLOW_GROUPS - 1) as u16);
    allocator
        .record_request(last_valid, declare_color())
        .unwrap();
    assert!(allocator.request_group(last_valid).is_some());

    let invalid = flow_group(MAX_RENDER_FLOW_GROUPS as u16);
    assert!(allocator.record_request(invalid, declare_color()).is_err());
    assert!(allocator.request_group(invalid).is_none());
}

#[test]
fn allocator_rejects_oversized_preconsume_group_prepare() {
    let mut allocator = RenderResourceAllocator::new();

    allocator
        .set_phase(ResourceAllocatorPhase::PreConsume)
        .unwrap();

    assert!(allocator
        .prepare_preconsume_groups(MAX_RENDER_FLOW_GROUPS + 1)
        .is_err());
}
