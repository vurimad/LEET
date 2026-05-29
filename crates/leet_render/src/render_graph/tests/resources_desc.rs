use super::super::resources::{
    FrameBufferDesc, FrameResourceDesc, FrameResourceShape, FrameTextureDesc,
};
use wgpu::{
    BufferDescriptor, BufferUsages, Extent3d, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages,
};

fn texture_descriptor(label: Option<&'static str>) -> TextureDescriptor<'static> {
    TextureDescriptor {
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
    }
}

fn buffer_descriptor(label: Option<&'static str>) -> BufferDescriptor<'static> {
    BufferDescriptor {
        label,
        size: 256,
        usage: BufferUsages::COPY_DST | BufferUsages::STORAGE,
        mapped_at_creation: false,
    }
}

#[test]
fn texture_desc_validation_rejects_max_smaller_than_current() {
    let desc = FrameTextureDesc::new(texture_descriptor(None)).with_max_size(Some(Extent3d {
        width: 32,
        height: 32,
        depth_or_array_layers: 1,
    }));

    assert!(desc.validate().is_err());
}

#[test]
fn texture_desc_validation_rejects_zero_current_size() {
    let desc = FrameTextureDesc::new(texture_descriptor(None)).with_current_size(Extent3d {
        width: 0,
        height: 32,
        depth_or_array_layers: 1,
    });

    assert!(desc.validate().is_err());
}

#[test]
fn texture_desc_validation_rejects_impossible_current_mips() {
    let desc = FrameTextureDesc::new(texture_descriptor(None))
        .with_current_size(Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        })
        .with_current_mip_level_count(2);

    assert!(desc.validate().is_err());
}

#[test]
fn texture_desc_validation_rejects_impossible_max_mips() {
    let desc = FrameTextureDesc::new(texture_descriptor(None))
        .with_max_size(Some(Extent3d {
            width: 64,
            height: 32,
            depth_or_array_layers: 1,
        }))
        .with_max_mip_level_count(Some(16));

    assert!(desc.validate().is_err());
}

#[test]
fn texture_desc_exact_match_ignores_debug_label() {
    let a = FrameResourceDesc::Texture(FrameTextureDesc::new(texture_descriptor(Some("a"))));
    let b = FrameResourceDesc::Texture(FrameTextureDesc::new(texture_descriptor(Some("b"))));

    assert!(a.is_exact_match(&b));
}

#[test]
fn texture_desc_ignoring_max_size_still_checks_current_size() {
    let a = FrameResourceDesc::Texture(
        FrameTextureDesc::new(texture_descriptor(None)).with_max_size(Some(Extent3d {
            width: 128,
            height: 64,
            depth_or_array_layers: 1,
        })),
    );
    let b = FrameResourceDesc::Texture(
        FrameTextureDesc::new(texture_descriptor(None)).with_max_size(Some(Extent3d {
            width: 256,
            height: 128,
            depth_or_array_layers: 1,
        })),
    );
    let c = FrameResourceDesc::Texture(
        FrameTextureDesc::new(texture_descriptor(None)).with_current_size(Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        }),
    );

    assert!(a.is_equal_ignoring_max_size(&b));
    assert!(!a.is_equal_ignoring_max_size(&c));
}

#[test]
fn texture_swap_compatibility_ignores_clear_policy_but_not_usage() {
    let a = FrameResourceDesc::Texture(
        FrameTextureDesc::new(texture_descriptor(None)).with_clear_to_zero(true),
    );
    let b = FrameResourceDesc::Texture(
        FrameTextureDesc::new(texture_descriptor(None))
            .with_optimal_clear_value(Some([1.0, 0.0, 0.0, 1.0])),
    );

    let mut different_usage_desc = texture_descriptor(None);
    different_usage_desc.usage = TextureUsages::TEXTURE_BINDING;
    let c = FrameResourceDesc::Texture(FrameTextureDesc::new(different_usage_desc));

    assert!(a.is_compatible_for_swap(&b));
    assert!(!a.is_compatible_for_swap(&c));
}

#[test]
fn texture_reuse_allows_larger_capacity_and_usage_superset() {
    let existing = FrameResourceDesc::Texture(
        FrameTextureDesc::new(texture_descriptor(None)).with_max_size(Some(Extent3d {
            width: 128,
            height: 64,
            depth_or_array_layers: 1,
        })),
    );

    let mut request_descriptor = texture_descriptor(None);
    request_descriptor.size = Extent3d {
        width: 96,
        height: 48,
        depth_or_array_layers: 1,
    };
    request_descriptor.usage = TextureUsages::TEXTURE_BINDING;
    let request = FrameResourceDesc::Texture(FrameTextureDesc::new(request_descriptor));

    assert!(existing.can_reuse_for(&request));
}

#[test]
fn texture_concrete_descriptor_uses_selected_shape() {
    let desc =
        FrameTextureDesc::new(texture_descriptor(Some("texture"))).with_max_size(Some(Extent3d {
            width: 256,
            height: 128,
            depth_or_array_layers: 1,
        }));

    let concrete = desc
        .concrete_descriptor_for_shape(desc.max_capacity_shape())
        .unwrap();

    assert_eq!(
        concrete.size,
        Extent3d {
            width: 256,
            height: 128,
            depth_or_array_layers: 1
        }
    );
    assert_eq!(concrete.label, Some("texture"));
}

#[test]
fn texture_concrete_descriptor_rejects_invalid_shape() {
    let desc = FrameTextureDesc::new(texture_descriptor(None)).with_max_size(Some(Extent3d {
        width: 128,
        height: 64,
        depth_or_array_layers: 1,
    }));

    assert!(desc
        .concrete_descriptor_for_shape(FrameResourceShape::Texture {
            size: Extent3d {
                width: 0,
                height: 64,
                depth_or_array_layers: 1
            },
            mip_level_count: 1,
        })
        .is_err());
    assert!(desc
        .concrete_descriptor_for_shape(FrameResourceShape::Texture {
            size: Extent3d {
                width: 256,
                height: 64,
                depth_or_array_layers: 1
            },
            mip_level_count: 1,
        })
        .is_err());
    assert!(desc
        .concrete_descriptor_for_shape(FrameResourceShape::Texture {
            size: Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1
            },
            mip_level_count: 2,
        })
        .is_err());
}

#[test]
fn buffer_desc_validation_checks_size_and_stride() {
    let zero = FrameBufferDesc::new(buffer_descriptor(None)).with_current_size_bytes(0);
    let bad_stride = FrameBufferDesc::new(buffer_descriptor(None)).with_element_stride(Some(24));
    let good = FrameBufferDesc::new(buffer_descriptor(None)).with_element_stride(Some(16));

    assert!(zero.validate().is_err());
    assert!(bad_stride.validate().is_err());
    assert!(good.validate().is_ok());
}

#[test]
fn buffer_desc_validation_rejects_max_size_not_aligned_to_stride() {
    let desc = FrameBufferDesc::new(buffer_descriptor(None))
        .with_element_stride(Some(16))
        .with_max_size_bytes(Some(260));

    assert!(desc.validate().is_err());
}

#[test]
fn buffer_desc_exact_match_ignores_debug_label() {
    let a = FrameResourceDesc::Buffer(FrameBufferDesc::new(buffer_descriptor(Some("a"))));
    let b = FrameResourceDesc::Buffer(FrameBufferDesc::new(buffer_descriptor(Some("b"))));

    assert!(a.is_exact_match(&b));
}

#[test]
fn buffer_reuse_allows_larger_capacity_and_usage_superset() {
    let existing = FrameResourceDesc::Buffer(
        FrameBufferDesc::new(buffer_descriptor(None)).with_max_size_bytes(Some(1024)),
    );

    let request = FrameResourceDesc::Buffer(FrameBufferDesc::new(BufferDescriptor {
        label: None,
        size: 512,
        usage: BufferUsages::STORAGE,
        mapped_at_creation: false,
    }));

    assert!(existing.can_reuse_for(&request));
}

#[test]
fn buffer_concrete_descriptor_rejects_invalid_shape() {
    let desc = FrameBufferDesc::new(buffer_descriptor(None))
        .with_element_stride(Some(16))
        .with_max_size_bytes(Some(512));

    assert!(desc
        .concrete_descriptor_for_shape(FrameResourceShape::Buffer { size_bytes: 0 })
        .is_err());
    assert!(desc
        .concrete_descriptor_for_shape(FrameResourceShape::Buffer { size_bytes: 1024 })
        .is_err());
    assert!(desc
        .concrete_descriptor_for_shape(FrameResourceShape::Buffer { size_bytes: 260 })
        .is_err());
}

#[test]
fn resource_kinds_never_compare_compatible() {
    let texture = FrameResourceDesc::Texture(FrameTextureDesc::new(texture_descriptor(None)));
    let buffer = FrameResourceDesc::Buffer(FrameBufferDesc::new(buffer_descriptor(None)));

    assert!(!texture.is_exact_match(&buffer));
    assert!(!texture.is_equal_ignoring_max_size(&buffer));
    assert!(!texture.is_compatible_for_swap(&buffer));
    assert!(!texture.can_reuse_for(&buffer));
}

#[test]
fn resource_shapes_are_kind_specific() {
    let texture = FrameResourceDesc::Texture(FrameTextureDesc::new(texture_descriptor(None)));
    let buffer = FrameResourceDesc::Buffer(FrameBufferDesc::new(buffer_descriptor(None)));

    assert_eq!(
        texture.current_allocation_shape(),
        FrameResourceShape::Texture {
            size: Extent3d {
                width: 64,
                height: 32,
                depth_or_array_layers: 1
            },
            mip_level_count: 1,
        }
    );
    assert_eq!(
        buffer.current_allocation_shape(),
        FrameResourceShape::Buffer { size_bytes: 256 }
    );
}
