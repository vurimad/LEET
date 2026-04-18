//! Minimal renderer viewport state.
//!
//! For now LEET only needs one robust presentation viewport. This type keeps
//! the viewport-specific metadata separate from the lower-level wgpu surface so
//! we can grow toward a clean `BeginFrame/SubmitFrame` entry without
//! jumping straight to a full viewport manager.

use crate::frame_submission::{
    RenderCameraFrameInfo, RenderFramePurpose, RenderFrameSubmission, ViewportFrameInfo,
};
use crate::surface::RenderSurface;
use leet_core::{Leeror, LeetResult};

/// Whether a viewport is attached to a real OS window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderViewportOwner {
    WindowBacked,
    Detached,
}

/// Renderer-facing viewport metadata for one presentation target.
#[derive(Debug)]
pub struct RenderViewport {
    name: String,
    size: (u32, u32),
    owner: RenderViewportOwner,
    surface: Option<RenderSurface>,
}

impl RenderViewport {
    pub fn main_window(size: (u32, u32)) -> Self {
        Self {
            name: "MainViewport".to_string(),
            size,
            owner: RenderViewportOwner::WindowBacked,
            surface: None,
        }
    }

    pub fn main_window_with_surface(size: (u32, u32), surface: RenderSurface) -> Self {
        Self {
            name: "MainViewport".to_string(),
            size,
            owner: RenderViewportOwner::WindowBacked,
            surface: Some(surface),
        }
    }

    pub fn detached(name: impl Into<String>, size: (u32, u32)) -> Self {
        Self {
            name: name.into(),
            size,
            owner: RenderViewportOwner::Detached,
            surface: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn owner(&self) -> RenderViewportOwner {
        self.owner
    }

    pub fn is_window_owned(&self) -> bool {
        matches!(self.owner, RenderViewportOwner::WindowBacked)
    }

    pub fn surface(&self) -> Option<&RenderSurface> {
        self.surface.as_ref()
    }

    pub fn resize(&mut self, device: &wgpu::Device, new_width: u32, new_height: u32) {
        if new_width == 0 || new_height == 0 {
            return;
        }

        self.size = (new_width, new_height);
        if let Some(surface) = self.surface.as_mut() {
            surface.resize(device, new_width, new_height);
        }
    }

    pub fn begin_frame(&self) -> ViewportFrameInfo {
        self.begin_frame_for_purpose(RenderFramePurpose::Normal, true)
    }

    pub fn begin_frame_for_purpose(
        &self,
        purpose: RenderFramePurpose,
        present: bool,
    ) -> ViewportFrameInfo {
        ViewportFrameInfo::new(self.name.clone(), self.size, present, purpose)
    }

    pub fn submit_frame(
        &self,
        frame_info: ViewportFrameInfo,
        camera_info: RenderCameraFrameInfo,
    ) -> LeetResult<RenderFrameSubmission> {
        self.validate_frame_info(&frame_info)?;
        Ok(RenderFrameSubmission::new(frame_info, camera_info))
    }

    fn validate_frame_info(&self, frame_info: &ViewportFrameInfo) -> LeetResult<()> {
        if frame_info.viewport_name() != self.name() {
            return Err(Leeror::Runtime(format!(
                "frame viewport '{}' does not match viewport '{}'",
                frame_info.viewport_name(),
                self.name(),
            )));
        }

        if frame_info.viewport_size() != self.size() {
            return Err(Leeror::Runtime(format!(
                "frame viewport size {:?} does not match viewport {:?}",
                frame_info.viewport_size(),
                self.size(),
            )));
        }

        if frame_info.present() && !self.is_window_owned() {
            return Err(Leeror::Runtime(
                "cannot submit a presenting frame on a detached viewport".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_builds_matching_frame_info() {
        let viewport = RenderViewport::main_window((1600, 900));
        let frame_info = viewport.begin_frame();

        assert_eq!(frame_info.viewport_name(), "MainViewport");
        assert_eq!(frame_info.viewport_size(), (1600, 900));
        assert!(frame_info.present());
        assert_eq!(frame_info.purpose(), RenderFramePurpose::Normal);
        assert!(viewport.is_window_owned());
    }

    #[test]
    fn detached_viewport_rejects_presenting_frame_submission() {
        let viewport = RenderViewport::detached("PreviewViewport", (512, 512));
        let frame_info = viewport.begin_frame_for_purpose(RenderFramePurpose::Blank, true);
        let camera_info = RenderCameraFrameInfo::default();

        let result = viewport.submit_frame(frame_info, camera_info);

        assert!(result.is_err());
    }

    #[test]
    fn viewport_submits_matching_frame() {
        let viewport = RenderViewport::main_window((1280, 720));
        let frame_info = viewport.begin_frame();
        let camera_info = RenderCameraFrameInfo::default();

        let submission = viewport.submit_frame(frame_info, camera_info).unwrap();

        assert_eq!(submission.frame_info().viewport_name(), "MainViewport");
        assert_eq!(submission.frame_info().viewport_size(), (1280, 720));
    }
}
