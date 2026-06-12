//! Concrete render viewport resolved for a submitted frame.

use bevy_math::{URect, UVec2};

/// Renderer-facing viewport for one concrete frame output.
///
/// Describes the renderable target area and exposes the texture view that the
/// graph will render into. Output lifetime and presentation live on
/// `FrameInput`, not here.
pub struct RenderViewport {
    extent: UVec2,
    format: wgpu::TextureFormat,
    full_rect: URect,
    view: Option<wgpu::TextureView>,
}

impl RenderViewport {
    pub fn window(extent: UVec2, format: wgpu::TextureFormat, view: wgpu::TextureView) -> Self {
        Self::new(extent, format, Some(view))
    }

    pub fn texture_view(
        extent: UVec2,
        format: wgpu::TextureFormat,
        view: wgpu::TextureView,
    ) -> Self {
        Self::new(extent, format, Some(view))
    }

    pub fn targetless(extent: UVec2, format: wgpu::TextureFormat) -> Self {
        Self::new(extent, format, None)
    }

    fn new(extent: UVec2, format: wgpu::TextureFormat, view: Option<wgpu::TextureView>) -> Self {
        let full_rect = URect::from_corners(UVec2::ZERO, extent);
        Self {
            extent,
            format,
            full_rect,
            view,
        }
    }

    pub fn extent(&self) -> UVec2 {
        self.extent
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub fn full_rect(&self) -> URect {
        self.full_rect
    }

    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.view.as_ref()
    }

    pub fn is_targetless(&self) -> bool {
        self.view.is_none()
    }
}
