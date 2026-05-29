//! Frame texture and buffer descriptors.

use super::{FrameResourceError, FrameResourceResult};
use wgpu::{BufferDescriptor, Extent3d, TextureDescriptor};

#[derive(Clone, Debug)]
pub enum FrameResourceDesc {
    Texture(FrameTextureDesc),
    Buffer(FrameBufferDesc),
}

impl FrameResourceDesc {
    pub fn validate(&self) -> FrameResourceResult<()> {
        match self {
            Self::Texture(desc) => desc.validate(),
            Self::Buffer(desc) => desc.validate(),
        }
    }

    pub fn is_exact_match(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Texture(a), Self::Texture(b)) => a.is_exact_match(b),
            (Self::Buffer(a), Self::Buffer(b)) => a.is_exact_match(b),
            _ => false,
        }
    }

    pub fn is_equal_ignoring_max_size(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Texture(a), Self::Texture(b)) => a.is_equal_ignoring_max_size(b),
            (Self::Buffer(a), Self::Buffer(b)) => a.is_equal_ignoring_max_size(b),
            _ => false,
        }
    }

    pub fn is_compatible_for_swap(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Texture(a), Self::Texture(b)) => a.is_compatible_for_swap(b),
            (Self::Buffer(a), Self::Buffer(b)) => a.is_compatible_for_swap(b),
            _ => false,
        }
    }

    pub fn can_reuse_for(&self, request: &Self) -> bool {
        match (self, request) {
            (Self::Texture(existing), Self::Texture(request)) => existing.can_reuse_for(request),
            (Self::Buffer(existing), Self::Buffer(request)) => existing.can_reuse_for(request),
            _ => false,
        }
    }

    pub fn current_allocation_shape(&self) -> FrameResourceShape {
        match self {
            Self::Texture(desc) => desc.current_allocation_shape(),
            Self::Buffer(desc) => desc.current_allocation_shape(),
        }
    }

    pub fn max_capacity_shape(&self) -> FrameResourceShape {
        match self {
            Self::Texture(desc) => desc.max_capacity_shape(),
            Self::Buffer(desc) => desc.max_capacity_shape(),
        }
    }

    pub fn concrete_capacity_desc_for_shape(
        &self,
        shape: FrameResourceShape,
    ) -> FrameResourceResult<Self> {
        match self {
            Self::Texture(desc) => Ok(Self::Texture(desc.concrete_capacity_desc_for_shape(shape)?)),
            Self::Buffer(desc) => Ok(Self::Buffer(desc.concrete_capacity_desc_for_shape(shape)?)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrameTextureDesc {
    descriptor: TextureDescriptor<'static>,
    current_size: Extent3d,
    max_size: Option<Extent3d>,
    current_mip_level_count: u32,
    max_mip_level_count: Option<u32>,
    clear_to_zero: bool,
    optimal_clear_value: Option<[f32; 4]>,
}

impl FrameTextureDesc {
    pub fn new(descriptor: TextureDescriptor<'static>) -> Self {
        Self {
            current_size: descriptor.size,
            max_size: None,
            current_mip_level_count: descriptor.mip_level_count,
            max_mip_level_count: None,
            descriptor,
            clear_to_zero: false,
            optimal_clear_value: None,
        }
    }

    pub const fn descriptor(&self) -> &TextureDescriptor<'static> {
        &self.descriptor
    }

    pub const fn current_size(&self) -> Extent3d {
        self.current_size
    }

    pub const fn max_size(&self) -> Option<Extent3d> {
        self.max_size
    }

    pub const fn current_mip_level_count(&self) -> u32 {
        self.current_mip_level_count
    }

    pub const fn max_mip_level_count(&self) -> Option<u32> {
        self.max_mip_level_count
    }

    pub const fn clear_to_zero(&self) -> bool {
        self.clear_to_zero
    }

    pub const fn optimal_clear_value(&self) -> Option<[f32; 4]> {
        self.optimal_clear_value
    }

    pub fn with_current_size(mut self, current_size: Extent3d) -> Self {
        self.current_size = current_size;
        self.descriptor.size = current_size;
        self
    }

    pub fn with_max_size(mut self, max_size: Option<Extent3d>) -> Self {
        self.max_size = max_size;
        self
    }

    pub fn with_current_mip_level_count(mut self, current_mip_level_count: u32) -> Self {
        self.current_mip_level_count = current_mip_level_count;
        self.descriptor.mip_level_count = current_mip_level_count;
        self
    }

    pub fn with_max_mip_level_count(mut self, max_mip_level_count: Option<u32>) -> Self {
        self.max_mip_level_count = max_mip_level_count;
        self
    }

    pub fn with_clear_to_zero(mut self, clear_to_zero: bool) -> Self {
        self.clear_to_zero = clear_to_zero;
        self
    }

    pub fn with_optimal_clear_value(mut self, optimal_clear_value: Option<[f32; 4]>) -> Self {
        self.optimal_clear_value = optimal_clear_value;
        self
    }

    pub fn validate(&self) -> FrameResourceResult<()> {
        validate_texture_size("current texture size", self.current_size)?;

        if let Some(max_size) = self.max_size {
            validate_texture_size("maximum texture size", max_size)?;
            if !texture_size_contains(max_size, self.current_size) {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "FrameTextureDesc::validate",
                    reason: "maximum texture size is smaller than current texture size",
                });
            }
        }

        if self.current_mip_level_count == 0 {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameTextureDesc::validate",
                reason: "current mip level count must be greater than zero",
            });
        }

        if self.current_mip_level_count > max_mip_levels_for_size(self.current_size) {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameTextureDesc::validate",
                reason: "current mip level count exceeds current texture size",
            });
        }

        if let Some(max_mip_level_count) = self.max_mip_level_count {
            if max_mip_level_count < self.current_mip_level_count {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "FrameTextureDesc::validate",
                    reason: "maximum mip level count is smaller than current mip level count",
                });
            }

            if max_mip_level_count > max_mip_levels_for_size(self.capacity_size()) {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "FrameTextureDesc::validate",
                    reason: "maximum mip level count exceeds maximum texture size",
                });
            }
        }

        Ok(())
    }

    pub fn is_exact_match(&self, other: &Self) -> bool {
        self.current_size == other.current_size
            && self.max_size == other.max_size
            && self.current_mip_level_count == other.current_mip_level_count
            && self.max_mip_level_count == other.max_mip_level_count
            && self.clear_to_zero == other.clear_to_zero
            && self.optimal_clear_value == other.optimal_clear_value
            && self.creation_fields_equal(other)
    }

    pub fn is_equal_ignoring_max_size(&self, other: &Self) -> bool {
        self.current_size == other.current_size
            && self.current_mip_level_count == other.current_mip_level_count
            && self.clear_to_zero == other.clear_to_zero
            && self.optimal_clear_value == other.optimal_clear_value
            && self.creation_fields_equal(other)
    }

    pub fn is_compatible_for_swap(&self, other: &Self) -> bool {
        self.current_size == other.current_size
            && self.current_mip_level_count == other.current_mip_level_count
            && self.creation_fields_equal(other)
    }

    pub fn can_reuse_for(&self, request: &Self) -> bool {
        texture_size_contains(self.capacity_size(), request.current_size)
            && self.capacity_mips() >= request.current_mip_level_count
            && self.creation_fields_allow_reuse_for(request)
    }

    pub fn current_allocation_shape(&self) -> FrameResourceShape {
        FrameResourceShape::Texture {
            size: self.current_size,
            mip_level_count: self.current_mip_level_count,
        }
    }

    pub fn max_capacity_shape(&self) -> FrameResourceShape {
        FrameResourceShape::Texture {
            size: self.capacity_size(),
            mip_level_count: self.capacity_mips(),
        }
    }

    pub fn concrete_descriptor_for_shape(
        &self,
        shape: FrameResourceShape,
    ) -> FrameResourceResult<TextureDescriptor<'static>> {
        let FrameResourceShape::Texture {
            size,
            mip_level_count,
        } = shape
        else {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameTextureDesc::concrete_descriptor_for_shape",
                reason: "shape is not a texture shape",
            });
        };

        validate_texture_size("concrete texture size", size)?;
        if !texture_size_contains(self.capacity_size(), size) {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameTextureDesc::concrete_descriptor_for_shape",
                reason: "concrete texture size exceeds descriptor capacity",
            });
        }
        if mip_level_count == 0 || mip_level_count > self.capacity_mips() {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameTextureDesc::concrete_descriptor_for_shape",
                reason: "concrete mip level count exceeds descriptor capacity",
            });
        }
        if mip_level_count > max_mip_levels_for_size(size) {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameTextureDesc::concrete_descriptor_for_shape",
                reason: "concrete mip level count exceeds concrete texture size",
            });
        }

        let mut descriptor = self.descriptor.clone();
        descriptor.size = size;
        descriptor.mip_level_count = mip_level_count;
        Ok(descriptor)
    }

    pub fn concrete_capacity_desc_for_shape(
        &self,
        shape: FrameResourceShape,
    ) -> FrameResourceResult<Self> {
        let descriptor = self.concrete_descriptor_for_shape(shape)?;
        Ok(Self::new(descriptor)
            .with_clear_to_zero(self.clear_to_zero)
            .with_optimal_clear_value(self.optimal_clear_value))
    }

    fn capacity_size(&self) -> Extent3d {
        self.max_size.unwrap_or(self.current_size)
    }

    fn capacity_mips(&self) -> u32 {
        self.max_mip_level_count
            .unwrap_or(self.current_mip_level_count)
    }

    fn creation_fields_equal(&self, other: &Self) -> bool {
        self.descriptor.sample_count == other.descriptor.sample_count
            && self.descriptor.dimension == other.descriptor.dimension
            && self.descriptor.format == other.descriptor.format
            && self.descriptor.usage == other.descriptor.usage
            && self.descriptor.view_formats == other.descriptor.view_formats
    }

    fn creation_fields_allow_reuse_for(&self, request: &Self) -> bool {
        self.descriptor.sample_count == request.descriptor.sample_count
            && self.descriptor.dimension == request.descriptor.dimension
            && self.descriptor.format == request.descriptor.format
            && self.descriptor.usage.contains(request.descriptor.usage)
            && self.descriptor.view_formats == request.descriptor.view_formats
    }
}

#[derive(Clone, Debug)]
pub struct FrameBufferDesc {
    descriptor: BufferDescriptor<'static>,
    current_size_bytes: u64,
    max_size_bytes: Option<u64>,
    element_stride: Option<u64>,
}

impl FrameBufferDesc {
    pub fn new(descriptor: BufferDescriptor<'static>) -> Self {
        Self {
            current_size_bytes: descriptor.size,
            max_size_bytes: None,
            element_stride: None,
            descriptor,
        }
    }

    pub const fn descriptor(&self) -> &BufferDescriptor<'static> {
        &self.descriptor
    }

    pub const fn current_size_bytes(&self) -> u64 {
        self.current_size_bytes
    }

    pub const fn max_size_bytes(&self) -> Option<u64> {
        self.max_size_bytes
    }

    pub const fn element_stride(&self) -> Option<u64> {
        self.element_stride
    }

    pub fn with_current_size_bytes(mut self, current_size_bytes: u64) -> Self {
        self.current_size_bytes = current_size_bytes;
        self.descriptor.size = current_size_bytes;
        self
    }

    pub fn with_max_size_bytes(mut self, max_size_bytes: Option<u64>) -> Self {
        self.max_size_bytes = max_size_bytes;
        self
    }

    pub fn with_element_stride(mut self, element_stride: Option<u64>) -> Self {
        self.element_stride = element_stride;
        self
    }

    pub fn validate(&self) -> FrameResourceResult<()> {
        if self.current_size_bytes == 0 {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameBufferDesc::validate",
                reason: "current buffer size must be greater than zero",
            });
        }

        if let Some(max_size_bytes) = self.max_size_bytes {
            if max_size_bytes < self.current_size_bytes {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "FrameBufferDesc::validate",
                    reason: "maximum buffer size is smaller than current buffer size",
                });
            }
        }

        if let Some(element_stride) = self.element_stride {
            if element_stride == 0 {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "FrameBufferDesc::validate",
                    reason: "buffer element stride must be greater than zero",
                });
            }

            if self.current_size_bytes % element_stride != 0 {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "FrameBufferDesc::validate",
                    reason: "current buffer size is not a multiple of element stride",
                });
            }

            if let Some(max_size_bytes) = self.max_size_bytes {
                if max_size_bytes % element_stride != 0 {
                    return Err(FrameResourceError::InvalidOperation {
                        operation: "FrameBufferDesc::validate",
                        reason: "maximum buffer size is not a multiple of element stride",
                    });
                }
            }
        }

        Ok(())
    }

    pub fn is_exact_match(&self, other: &Self) -> bool {
        self.current_size_bytes == other.current_size_bytes
            && self.max_size_bytes == other.max_size_bytes
            && self.element_stride == other.element_stride
            && self.creation_fields_equal(other)
    }

    pub fn is_equal_ignoring_max_size(&self, other: &Self) -> bool {
        self.current_size_bytes == other.current_size_bytes
            && self.element_stride == other.element_stride
            && self.creation_fields_equal(other)
    }

    pub fn is_compatible_for_swap(&self, other: &Self) -> bool {
        self.current_size_bytes == other.current_size_bytes
            && self.element_stride == other.element_stride
            && self.creation_fields_equal(other)
    }

    pub fn can_reuse_for(&self, request: &Self) -> bool {
        self.capacity_size_bytes() >= request.current_size_bytes
            && self.element_stride == request.element_stride
            && self.creation_fields_allow_reuse_for(request)
    }

    pub fn current_allocation_shape(&self) -> FrameResourceShape {
        FrameResourceShape::Buffer {
            size_bytes: self.current_size_bytes,
        }
    }

    pub fn max_capacity_shape(&self) -> FrameResourceShape {
        FrameResourceShape::Buffer {
            size_bytes: self.capacity_size_bytes(),
        }
    }

    pub fn concrete_descriptor_for_shape(
        &self,
        shape: FrameResourceShape,
    ) -> FrameResourceResult<BufferDescriptor<'static>> {
        let FrameResourceShape::Buffer { size_bytes } = shape else {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameBufferDesc::concrete_descriptor_for_shape",
                reason: "shape is not a buffer shape",
            });
        };

        if size_bytes == 0 {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameBufferDesc::concrete_descriptor_for_shape",
                reason: "concrete buffer size must be greater than zero",
            });
        }
        if size_bytes > self.capacity_size_bytes() {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameBufferDesc::concrete_descriptor_for_shape",
                reason: "concrete buffer size exceeds descriptor capacity",
            });
        }
        if let Some(element_stride) = self.element_stride {
            if size_bytes % element_stride != 0 {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "FrameBufferDesc::concrete_descriptor_for_shape",
                    reason: "concrete buffer size is not a multiple of element stride",
                });
            }
        }

        Ok(BufferDescriptor {
            label: self.descriptor.label,
            size: size_bytes,
            usage: self.descriptor.usage,
            mapped_at_creation: self.descriptor.mapped_at_creation,
        })
    }

    pub fn concrete_capacity_desc_for_shape(
        &self,
        shape: FrameResourceShape,
    ) -> FrameResourceResult<Self> {
        let descriptor = self.concrete_descriptor_for_shape(shape)?;
        Ok(Self::new(descriptor).with_element_stride(self.element_stride))
    }

    fn capacity_size_bytes(&self) -> u64 {
        self.max_size_bytes.unwrap_or(self.current_size_bytes)
    }

    fn creation_fields_equal(&self, other: &Self) -> bool {
        self.descriptor.usage == other.descriptor.usage
            && self.descriptor.mapped_at_creation == other.descriptor.mapped_at_creation
    }

    fn creation_fields_allow_reuse_for(&self, request: &Self) -> bool {
        self.descriptor.usage.contains(request.descriptor.usage)
            && self.descriptor.mapped_at_creation == request.descriptor.mapped_at_creation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameResourceShape {
    Texture {
        size: Extent3d,
        mip_level_count: u32,
    },
    Buffer {
        size_bytes: u64,
    },
}

const fn texture_size_contains(capacity: Extent3d, requested: Extent3d) -> bool {
    capacity.width >= requested.width
        && capacity.height >= requested.height
        && capacity.depth_or_array_layers >= requested.depth_or_array_layers
}

fn validate_texture_size(operation_name: &'static str, size: Extent3d) -> FrameResourceResult<()> {
    if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 {
        return Err(FrameResourceError::InvalidOperation {
            operation: "FrameTextureDesc::validate",
            reason: operation_name,
        });
    }

    Ok(())
}

const fn max_mip_levels_for_size(size: Extent3d) -> u32 {
    let max_dimension = max3(size.width, size.height, size.depth_or_array_layers);
    32 - max_dimension.leading_zeros()
}

const fn max3(a: u32, b: u32, c: u32) -> u32 {
    let ab = if a > b { a } else { b };
    if ab > c {
        ab
    } else {
        c
    }
}
