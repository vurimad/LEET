//! Resolves declared frame targets into concrete renderable viewports.

use bevy_camera::NormalizedRenderTarget;
use bevy_ecs::entity::ContainsEntity;
use bevy_math::UVec2;

use crate::{RenderFrameError, RenderFrameResult, RenderWindowRegistry};

use super::{FrameOutput, RenderViewport};

/// Render-world resolver for frame output targets.
///
/// This resolver is the boundary that proves the requested output has a
/// concrete resource for this frame.
pub struct FrameTargetResolver<'w> {
    windows: &'w mut RenderWindowRegistry,
}

impl<'w> FrameTargetResolver<'w> {
    pub fn new(windows: &'w mut RenderWindowRegistry) -> Self {
        Self { windows }
    }

    pub fn resolve(
        &mut self,
        declared: NormalizedRenderTarget,
        extent: UVec2,
        format: Option<wgpu::TextureFormat>,
    ) -> RenderFrameResult<(RenderViewport, FrameOutput)> {
        validate_target_extent(extent)?;

        match declared {
            NormalizedRenderTarget::Window(window) => {
                self.resolve_window(window.entity(), extent, format)
            }
            NormalizedRenderTarget::Image(_) => Err(RenderFrameError::NotImplemented {
                operation: "resolve image frame target",
            }),
            NormalizedRenderTarget::TextureView(_) => Err(RenderFrameError::NotImplemented {
                operation: "resolve manual texture-view frame target",
            }),
            NormalizedRenderTarget::None { .. } => Err(RenderFrameError::NotImplemented {
                operation: "resolve targetless offscreen frame target",
            }),
        }
    }

    fn resolve_window(
        &mut self,
        window: bevy_ecs::entity::Entity,
        extent: UVec2,
        requested_format: Option<wgpu::TextureFormat>,
    ) -> RenderFrameResult<(RenderViewport, FrameOutput)> {
        let window = self
            .windows
            .get_mut(&window)
            .ok_or(RenderFrameError::MissingFrameTarget {
                reason: "frame target window was not extracted into the render world",
            })?;

        let format = window
            .swap_chain_texture_view_format
            .or(window.swap_chain_texture_format)
            .or(requested_format)
            .ok_or(RenderFrameError::MissingFrameTarget {
                reason: "frame target window has no swapchain texture format",
            })?;

        let surface_texture =
            window
                .swap_chain_texture
                .take()
                .ok_or(RenderFrameError::MissingFrameTarget {
                    reason: "frame target window has no acquired swapchain texture",
                })?;
        let view =
            window
                .swap_chain_texture_view
                .take()
                .ok_or(RenderFrameError::MissingFrameTarget {
                    reason: "frame target window has no acquired swapchain texture view",
                })?;

        Ok((
            RenderViewport::window(extent, format, view),
            FrameOutput::WindowSurface(surface_texture),
        ))
    }
}

fn validate_target_extent(extent: UVec2) -> RenderFrameResult<()> {
    if extent.x == 0 || extent.y == 0 {
        return Err(RenderFrameError::MissingFrameTarget {
            reason: "frame target extent is zero",
        });
    }

    Ok(())
}
