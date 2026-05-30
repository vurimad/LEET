use bevy_camera::{
    CameraOutputMode, ClearColorConfig, CompositingSpace, MsaaWriteback, NormalizedRenderTarget,
    Viewport,
};
use bevy_math::{Mat4, UVec2, UVec4};
use bevy_transform::components::GlobalTransform;
use wgpu::TextureFormat;

#[derive(Debug, Clone)]
pub struct RenderCamera {
    pub target: Option<NormalizedRenderTarget>,
    pub physical_viewport_size: Option<UVec2>,
    pub physical_target_size: Option<UVec2>,
    pub viewport: Option<Viewport>,
    pub clip_from_view: Mat4,
    pub world_from_view: GlobalTransform,
    pub viewport_rect: UVec4,
    pub invert_culling: bool,
    pub main_pass_texture_format: TextureFormat,
    pub order: isize,
    pub output_mode: CameraOutputMode,
    pub msaa_writeback: MsaaWriteback,
    pub clear_color: ClearColorConfig,
    pub exposure: f32,
    pub hdr: bool,
    pub compositing_space: Option<CompositingSpace>,
}
