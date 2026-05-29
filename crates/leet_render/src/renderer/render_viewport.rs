//! Concrete frame output viewport resolved from a declared [`FrameTarget`].

use bevy_math::UVec2;

use crate::RenderViewportRect;

use super::{FrameTarget, FrameTargetKey, PresentationIntent, RenderFrameError, RenderFrameResult};

/// Renderer-facing viewport for one concrete frame output.
///
/// This is the LEET equivalent of the RED render-frame `RenderViewport` lookup:
/// it describes the resolved output target, exposes the texture view that the
/// graph will render into, and owns presentation for swapchain-backed targets.
pub struct RenderViewport {
    key: FrameTargetKey,
    extent: UVec2,
    format: wgpu::TextureFormat,
    full_rect: RenderViewportRect,
    output: RenderViewportOutput,
    presentation: PresentationIntent,
}

enum RenderViewportOutput {
    WindowSurface {
        surface_texture: wgpu::SurfaceTexture,
        view: wgpu::TextureView,
    },
    TextureView {
        view: wgpu::TextureView,
    },
}

impl RenderViewport {
    pub fn window(
        target: FrameTarget,
        format: wgpu::TextureFormat,
        surface_texture: wgpu::SurfaceTexture,
        view: wgpu::TextureView,
        presentation: PresentationIntent,
    ) -> RenderFrameResult<Self> {
        if !matches!(target.key, FrameTargetKey::Window(_)) {
            return Err(RenderFrameError::InvalidFrameInput {
                reason: "window viewport was resolved for a non-window frame target",
            });
        }

        Ok(Self::new(
            target,
            format,
            RenderViewportOutput::WindowSurface {
                surface_texture,
                view,
            },
            presentation,
        ))
    }

    pub fn texture_view(
        target: FrameTarget,
        format: wgpu::TextureFormat,
        view: wgpu::TextureView,
        presentation: PresentationIntent,
    ) -> RenderFrameResult<Self> {
        if matches!(target.key, FrameTargetKey::Window(_)) {
            return Err(RenderFrameError::InvalidFrameInput {
                reason: "texture viewport was resolved for a window frame target",
            });
        }

        Ok(Self::new(
            target,
            format,
            RenderViewportOutput::TextureView { view },
            presentation,
        ))
    }

    fn new(
        target: FrameTarget,
        format: wgpu::TextureFormat,
        output: RenderViewportOutput,
        presentation: PresentationIntent,
    ) -> Self {
        Self {
            key: target.key,
            extent: target.extent,
            format,
            full_rect: target.full_rect(),
            output,
            presentation,
        }
    }

    pub const fn key(&self) -> FrameTargetKey {
        self.key
    }

    pub const fn extent(&self) -> UVec2 {
        self.extent
    }

    pub const fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub const fn full_rect(&self) -> RenderViewportRect {
        self.full_rect
    }

    pub const fn view(&self) -> &wgpu::TextureView {
        match &self.output {
            RenderViewportOutput::WindowSurface { view, .. } => view,
            RenderViewportOutput::TextureView { view } => view,
        }
    }

    pub const fn presentation(&self) -> PresentationIntent {
        self.presentation
    }

    pub const fn is_window_surface(&self) -> bool {
        matches!(self.output, RenderViewportOutput::WindowSurface { .. })
    }

    pub fn finish(self) {
        if let RenderViewportOutput::WindowSurface {
            surface_texture, ..
        } = self.output
        {
            if matches!(self.presentation, PresentationIntent::Present) {
                surface_texture.present();
            }
        }
    }
}
