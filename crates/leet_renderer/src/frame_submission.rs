//! Frame submission metadata for the renderer.
//!
//! These types are the first step toward an explicit
//! `BeginFrame/SubmitFrame(frame_info, camera_info)` boundary without
//! committing to the full engine-side frame object yet.

use leet_math::{Mat4, Vec2, Vec3};

/// High-level reason for a submitted frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderFramePurpose {
    Normal,
    Blank,
}

impl Default for RenderFramePurpose {
    fn default() -> Self {
        Self::Normal
    }
}

/// Per-frame submission metadata derived from the active viewport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewportFrameInfo {
    viewport_name: String,
    viewport_size: (u32, u32),
    present: bool,
    purpose: RenderFramePurpose,
}

impl ViewportFrameInfo {
    pub fn new(
        viewport_name: impl Into<String>,
        viewport_size: (u32, u32),
        present: bool,
        purpose: RenderFramePurpose,
    ) -> Self {
        Self {
            viewport_name: viewport_name.into(),
            viewport_size,
            present,
            purpose,
        }
    }

    pub fn viewport_name(&self) -> &str {
        &self.viewport_name
    }

    pub fn viewport_size(&self) -> (u32, u32) {
        self.viewport_size
    }

    pub fn present(&self) -> bool {
        self.present
    }

    pub fn purpose(&self) -> RenderFramePurpose {
        self.purpose
    }
}

/// Per-frame camera data passed alongside the frame submission.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderCameraFrameInfo {
    world_position: Vec3,
    view: Mat4,
    projection: Mat4,
    sub_pixel_offset: Vec2,
    sub_pixel_index: u32,
    fov_multiplier: f32,
    rendering_mask: u64,
}

impl RenderCameraFrameInfo {
    pub fn new(world_position: Vec3, view: Mat4, projection: Mat4) -> Self {
        Self {
            world_position,
            view,
            projection,
            sub_pixel_offset: Vec2::ZERO,
            sub_pixel_index: 0,
            fov_multiplier: 1.0,
            rendering_mask: u64::MAX,
        }
    }

    pub fn world_position(&self) -> Vec3 {
        self.world_position
    }

    pub fn view(&self) -> Mat4 {
        self.view
    }

    pub fn projection(&self) -> Mat4 {
        self.projection
    }

    pub fn sub_pixel_offset(&self) -> Vec2 {
        self.sub_pixel_offset
    }

    pub fn sub_pixel_index(&self) -> u32 {
        self.sub_pixel_index
    }

    pub fn fov_multiplier(&self) -> f32 {
        self.fov_multiplier
    }

    pub fn rendering_mask(&self) -> u64 {
        self.rendering_mask
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection * self.view
    }

    pub fn with_sub_pixel_offset(mut self, sub_pixel_offset: Vec2) -> Self {
        self.sub_pixel_offset = sub_pixel_offset;
        self
    }

    pub fn with_sub_pixel_index(mut self, sub_pixel_index: u32) -> Self {
        self.sub_pixel_index = sub_pixel_index;
        self
    }

    pub fn with_fov_multiplier(mut self, fov_multiplier: f32) -> Self {
        self.fov_multiplier = fov_multiplier;
        self
    }

    pub fn with_rendering_mask(mut self, rendering_mask: u64) -> Self {
        self.rendering_mask = rendering_mask;
        self
    }
}

impl Default for RenderCameraFrameInfo {
    fn default() -> Self {
        Self {
            world_position: Vec3::ZERO,
            view: Mat4::IDENTITY,
            projection: Mat4::IDENTITY,
            sub_pixel_offset: Vec2::ZERO,
            sub_pixel_index: 0,
            fov_multiplier: 1.0,
            rendering_mask: u64::MAX,
        }
    }
}

/// Frame submission package returned by [`crate::render_viewport::RenderViewport`].
#[derive(Clone, Debug, PartialEq)]
pub struct RenderFrameSubmission {
    frame_info: ViewportFrameInfo,
    camera_info: RenderCameraFrameInfo,
}

impl RenderFrameSubmission {
    pub fn new(frame_info: ViewportFrameInfo, camera_info: RenderCameraFrameInfo) -> Self {
        Self {
            frame_info,
            camera_info,
        }
    }

    pub fn frame_info(&self) -> &ViewportFrameInfo {
        &self.frame_info
    }

    pub fn camera_info(&self) -> &RenderCameraFrameInfo {
        &self.camera_info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_info_stores_viewport_metadata() {
        let frame_info = ViewportFrameInfo::new(
            "MainViewport",
            (1280, 720),
            true,
            RenderFramePurpose::Normal,
        );

        assert_eq!(frame_info.viewport_name(), "MainViewport");
        assert_eq!(frame_info.viewport_size(), (1280, 720));
        assert!(frame_info.present());
        assert_eq!(frame_info.purpose(), RenderFramePurpose::Normal);
    }

    #[test]
    fn camera_info_builds_view_projection() {
        let camera = RenderCameraFrameInfo::new(
            Vec3::new(1.0, 2.0, 3.0),
            Mat4::from_translation(Vec3::new(0.0, 0.0, -5.0)),
            Mat4::IDENTITY,
        )
        .with_sub_pixel_offset(Vec2::new(0.25, -0.25))
        .with_sub_pixel_index(2)
        .with_fov_multiplier(1.1)
        .with_rendering_mask(7);

        assert_eq!(camera.world_position(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(camera.sub_pixel_offset(), Vec2::new(0.25, -0.25));
        assert_eq!(camera.sub_pixel_index(), 2);
        assert_eq!(camera.fov_multiplier(), 1.1);
        assert_eq!(camera.rendering_mask(), 7);
        assert_eq!(
            camera.view_projection(),
            camera.projection() * camera.view()
        );
    }
}
