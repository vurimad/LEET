use bevy_camera::{
    CameraOutputMode, ClearColorConfig, CompositingSpace, MsaaWriteback, NormalizedRenderTarget,
};
use bevy_math::{Mat4, URect, UVec2};
use bevy_transform::components::GlobalTransform;
use wgpu::TextureFormat;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RenderCameraFeatures {
    bits: u64,
}

impl RenderCameraFeatures {
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self { bits }
    }

    pub const fn bits(self) -> u64 {
        self.bits
    }

    pub const fn contains(self, feature: Self) -> bool {
        self.bits & feature.bits == feature.bits
    }

    pub fn insert(&mut self, feature: Self) {
        self.bits |= feature.bits;
    }

    pub fn remove(&mut self, feature: Self) {
        self.bits &= !feature.bits;
    }
}

#[derive(Debug, Clone)]
pub struct RenderCamera {
    pub target: Option<NormalizedRenderTarget>,
    pub physical_target_size: Option<UVec2>,
    pub clip_from_view: Mat4,
    pub world_from_view: GlobalTransform,
    pub viewport: URect,
    pub invert_culling: bool,
    pub main_pass_texture_format: TextureFormat,
    pub order: isize,
    pub output_mode: CameraOutputMode,
    pub msaa_writeback: MsaaWriteback,
    pub clear_color: ClearColorConfig,
    pub exposure: f32,
    pub hdr: bool,
    pub features: RenderCameraFeatures,
    pub compositing_space: Option<CompositingSpace>,
}
