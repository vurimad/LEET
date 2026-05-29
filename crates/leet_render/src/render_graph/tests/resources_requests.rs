use super::super::resources::{
    ExternalFrameResourceId, FrameBufferDesc, FrameResourceDesc, FrameTextureDesc,
    ImportedFrameResource, QueueSyncKind, RenderFlowName, RenderFlowNameTag, RenderFlowSpace,
    RenderQueueKind, RequestGroup, RequestGroupAction, ResourceAllocatorPhase, ResourceRequest,
    ResourceUsage,
};
use wgpu::{
    BufferDescriptor, BufferUsages, Extent3d, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages,
};

fn tag(name: &'static str) -> RenderFlowNameTag {
    RenderFlowNameTag::new(RenderFlowName::from_static(name), RenderFlowSpace::SHARED)
}

fn texture_desc(label: Option<&'static str>) -> FrameTextureDesc {
    FrameTextureDesc::new(TextureDescriptor {
        label,
        size: Extent3d {
            width: 64,
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

fn buffer_desc(label: Option<&'static str>) -> FrameBufferDesc {
    FrameBufferDesc::new(BufferDescriptor {
        label,
        size: 256,
        usage: BufferUsages::COPY_DST | BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

fn declare_color() -> ResourceRequest {
    ResourceRequest::Declare {
        tag: tag("color"),
        desc: FrameResourceDesc::Texture(texture_desc(None)),
    }
}

fn use_color() -> ResourceRequest {
    ResourceRequest::UseBegin {
        tag: tag("color"),
        usage: ResourceUsage::READ,
    }
}

#[test]
fn request_group_records_during_preconsume() {
    let mut group = RequestGroup::new();
    let action = group
        .apply(ResourceAllocatorPhase::PreConsume, declare_color())
        .unwrap();

    assert_eq!(action.id().get(), 0);
    assert_eq!(group.requests().len(), 1);
    assert!(group.touched());
}

#[test]
fn request_group_rejects_invalid_phase_operations() {
    let mut group = RequestGroup::new();

    assert!(group
        .apply(ResourceAllocatorPhase::Resolve, declare_color())
        .is_err());
    assert!(group
        .apply(ResourceAllocatorPhase::Cleanup, declare_color())
        .is_err());
}

#[test]
fn consume_replay_accepts_matching_requests_in_order() {
    let mut group = RequestGroup::new();
    group
        .apply(ResourceAllocatorPhase::PreConsume, declare_color())
        .unwrap();
    group
        .apply(ResourceAllocatorPhase::PreConsume, use_color())
        .unwrap();

    group.reset_consume_cursor();
    let first = group
        .apply(ResourceAllocatorPhase::Consume, declare_color())
        .unwrap();
    let second = group
        .apply(ResourceAllocatorPhase::Consume, use_color())
        .unwrap();

    assert_eq!(first.id().get(), 0);
    assert_eq!(second.id().get(), 1);
    assert!(group.is_consume_finished());
    assert!(group.validate_consume_finished().is_ok());
}

#[test]
fn consume_replay_rejects_missing_extra_reordered_and_mismatched_requests() {
    let mut empty = RequestGroup::new();
    assert!(empty
        .apply(ResourceAllocatorPhase::Consume, declare_color())
        .is_err());

    let mut reordered = RequestGroup::new();
    reordered
        .apply(ResourceAllocatorPhase::PreConsume, declare_color())
        .unwrap();
    reordered
        .apply(ResourceAllocatorPhase::PreConsume, use_color())
        .unwrap();
    reordered.reset_consume_cursor();
    assert!(reordered
        .apply(ResourceAllocatorPhase::Consume, use_color())
        .is_err());

    let mut mismatched = RequestGroup::new();
    mismatched
        .apply(ResourceAllocatorPhase::PreConsume, declare_color())
        .unwrap();
    mismatched.reset_consume_cursor();
    assert!(mismatched
        .apply(
            ResourceAllocatorPhase::Consume,
            ResourceRequest::Declare {
                tag: tag("color"),
                desc: FrameResourceDesc::Texture(texture_desc(None).with_current_size(Extent3d {
                    width: 32,
                    height: 32,
                    depth_or_array_layers: 1,
                })),
            },
        )
        .is_err());

    let mut missing = RequestGroup::new();
    missing
        .apply(ResourceAllocatorPhase::PreConsume, declare_color())
        .unwrap();
    missing.reset_consume_cursor();
    assert!(missing.validate_consume_finished().is_err());
}

#[test]
fn decision_value_is_stabilized_but_branch_divergence_is_rejected() {
    let mut group = RequestGroup::new();
    group
        .apply(
            ResourceAllocatorPhase::PreConsume,
            ResourceRequest::Decision { value: true },
        )
        .unwrap();
    group
        .apply(ResourceAllocatorPhase::PreConsume, declare_color())
        .unwrap();

    group.reset_consume_cursor();
    let action = group
        .apply(
            ResourceAllocatorPhase::Consume,
            ResourceRequest::Decision { value: false },
        )
        .unwrap();
    let Some(ResourceRequest::Decision { value }) = action.recorded_request() else {
        panic!("decision replay should return recorded decision request");
    };
    assert!(*value);

    assert!(group
        .apply(ResourceAllocatorPhase::Consume, use_color())
        .is_err());
}

#[test]
fn is_declared_replay_is_exact_and_deterministic() {
    let mut group = RequestGroup::new();
    group
        .apply(
            ResourceAllocatorPhase::PreConsume,
            ResourceRequest::IsDeclared {
                tag: tag("depth"),
                declared: true,
            },
        )
        .unwrap();

    group.reset_consume_cursor();
    assert!(group
        .apply(
            ResourceAllocatorPhase::Consume,
            ResourceRequest::IsDeclared {
                tag: tag("depth"),
                declared: false,
            },
        )
        .is_err());
}

#[test]
fn import_replay_uses_logical_identity_not_debug_label() {
    let imported_pre = ImportedFrameResource::texture(
        ExternalFrameResourceId::new(7),
        texture_desc(Some("preconsume import")),
    );
    let imported_consume = ImportedFrameResource::texture(
        ExternalFrameResourceId::new(7),
        texture_desc(Some("consume import")),
    );

    let mut group = RequestGroup::new();
    group
        .apply(
            ResourceAllocatorPhase::PreConsume,
            ResourceRequest::Import {
                tag: tag("history"),
                resource: imported_pre,
            },
        )
        .unwrap();

    group.reset_consume_cursor();
    assert!(group
        .apply(
            ResourceAllocatorPhase::Consume,
            ResourceRequest::Import {
                tag: tag("history"),
                resource: imported_consume,
            },
        )
        .is_ok());
}

#[test]
fn swap_with_external_replay_validates_logical_identity() {
    let imported_pre =
        ImportedFrameResource::buffer(ExternalFrameResourceId::new(9), buffer_desc(Some("pre")));
    let imported_consume =
        ImportedFrameResource::buffer(ExternalFrameResourceId::new(10), buffer_desc(Some("pre")));

    let mut group = RequestGroup::new();
    group
        .apply(
            ResourceAllocatorPhase::PreConsume,
            ResourceRequest::SwapWithExternal {
                tag: tag("readback"),
                resource: imported_pre,
            },
        )
        .unwrap();

    group.reset_consume_cursor();
    assert!(group
        .apply(
            ResourceAllocatorPhase::Consume,
            ResourceRequest::SwapWithExternal {
                tag: tag("readback"),
                resource: imported_consume,
            },
        )
        .is_err());
}

#[test]
fn queue_requests_validate_kind_and_sync_type() {
    assert!(ResourceRequest::BeginQueue {
        queue: RenderQueueKind::Graphics
    }
    .matches_replay(&ResourceRequest::BeginQueue {
        queue: RenderQueueKind::Graphics
    }));
    assert!(!ResourceRequest::BeginQueue {
        queue: RenderQueueKind::Compute
    }
    .matches_replay(&ResourceRequest::BeginQueue {
        queue: RenderQueueKind::Graphics
    }));
    assert!(ResourceRequest::QueueSync {
        sync: QueueSyncKind::Fork
    }
    .matches_replay(&ResourceRequest::QueueSync {
        sync: QueueSyncKind::Fork
    }));
    assert!(!ResourceRequest::QueueSync {
        sync: QueueSyncKind::Join
    }
    .matches_replay(&ResourceRequest::QueueSync {
        sync: QueueSyncKind::Fork
    }));
}

#[test]
fn request_group_action_reports_recorded_request_on_replay() {
    let mut group = RequestGroup::new();
    group
        .apply(ResourceAllocatorPhase::PreConsume, declare_color())
        .unwrap();

    group.reset_consume_cursor();
    let action = group
        .apply(ResourceAllocatorPhase::Consume, declare_color())
        .unwrap();

    assert!(matches!(action, RequestGroupAction::Replayed { .. }));
    assert!(matches!(
        action.recorded_request(),
        Some(ResourceRequest::Declare { .. })
    ));
}
