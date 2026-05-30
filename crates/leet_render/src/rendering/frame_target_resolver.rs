//! Resolves declared frame targets into concrete renderable viewports.

use crate::RenderWindowRegistry;

use super::{
    FrameTarget, FrameTargetKey, PresentationIntent, RenderFrameError, RenderFrameResult,
    RenderViewport,
};

/// Render-world resolver for frame output targets.
///
/// `FrameTarget` is the frame-builder request. This resolver is the boundary
/// that proves the requested output has a concrete resource for this frame.
pub struct FrameTargetResolver<'w> {
    windows: &'w mut RenderWindowRegistry,
}

impl<'w> FrameTargetResolver<'w> {
    pub fn new(windows: &'w mut RenderWindowRegistry) -> Self {
        Self { windows }
    }

    pub fn resolve(
        &mut self,
        target: FrameTarget,
        presentation: PresentationIntent,
    ) -> RenderFrameResult<RenderViewport> {
        target.validate()?;

        match target.key {
            FrameTargetKey::Window(window) => self.resolve_window(window, target, presentation),
            FrameTargetKey::Image(_) => Err(RenderFrameError::NotImplemented {
                operation: "resolve image frame target",
            }),
            FrameTargetKey::ManualTextureView(_) => Err(RenderFrameError::NotImplemented {
                operation: "resolve manual texture-view frame target",
            }),
            FrameTargetKey::External(_) => Err(RenderFrameError::NotImplemented {
                operation: "resolve external frame target",
            }),
        }
    }

    fn resolve_window(
        &mut self,
        window: bevy_ecs::entity::Entity,
        target: FrameTarget,
        presentation: PresentationIntent,
    ) -> RenderFrameResult<RenderViewport> {
        let window = self
            .windows
            .get_mut(&window)
            .ok_or(RenderFrameError::MissingFrameTarget {
                reason: "frame target window was not extracted into the render world",
            })?;

        let format = window
            .swap_chain_texture_view_format
            .or(window.swap_chain_texture_format)
            .or(target.format)
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

        RenderViewport::window(target, format, surface_texture, view, presentation)
    }
}
