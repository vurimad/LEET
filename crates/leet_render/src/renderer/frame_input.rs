//! Submitted frame packet assembled before entering the render command path.

use bevy_ecs::entity::Entity;
use bevy_math::UVec2;

use crate::{ExtractedCamera, ExtractedView, RenderViewportRect};

use super::{RenderFrameError, RenderFrameResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderSceneId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderCameraId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CameraRenderSetupKey(pub u64);

#[derive(Clone, Debug)]
pub struct FrameInput {
    pub target: FrameTarget,
    pub camera_views: Vec<FrameCameraView>,
    pub scene: RenderSceneId,
    pub timing: FrameTiming,
    pub mode: FrameRenderingMode,
    pub purpose: FramePurpose,
    pub presentation: PresentationIntent,
    pub capture: FrameCaptureIntent,
    pub debug: FrameDebugIntent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameTarget {
    pub key: FrameTargetKey,
    pub extent: UVec2,
    pub format: Option<wgpu::TextureFormat>,
}

impl FrameTarget {
    pub fn full_rect(&self) -> RenderViewportRect {
        RenderViewportRect::new(0, 0, self.extent.x, self.extent.y)
    }

    pub fn validate(&self) -> RenderFrameResult<()> {
        if self.extent.x == 0 || self.extent.y == 0 {
            return Err(RenderFrameError::MissingFrameTarget {
                reason: "frame target extent is zero",
            });
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameTargetKey {
    Window(Entity),
    Image(Entity),
    ManualTextureView(Entity),
    External(u64),
}

#[derive(Clone, Debug)]
pub struct FrameCameraView {
    pub camera_entity: Entity,
    pub camera_id: RenderCameraId,
    pub camera_order: isize,
    pub target_view_index: u32,
    pub viewport: RenderViewportRect,
    pub clear: ViewClearState,
    pub camera: ExtractedCamera,
    pub view: ExtractedView,
    pub render_setup: CameraRenderSetupKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameTiming {
    pub frame_index: u64,
    pub delta_seconds: f32,
    pub elapsed_seconds: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameRenderingMode {
    #[default]
    Shaded,
    OverlayOnly,
    Blank,
}

impl FrameRenderingMode {
    pub const fn allows_camera_jitter(self) -> bool {
        matches!(self, Self::Shaded)
    }

    pub const fn flushes_temporary_cameras(self) -> bool {
        matches!(self, Self::Shaded)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FramePurpose {
    #[default]
    Normal,
    Blank,
    Screenshot,
    DeferredScreenshot,
    EnvProbe,
    GlobalIllumination,
    GlobalIlluminationBackfaces,
}

impl FramePurpose {
    pub const fn requires_stable_dissolves(self) -> bool {
        matches!(
            self,
            Self::Blank
                | Self::Screenshot
                | Self::DeferredScreenshot
                | Self::EnvProbe
                | Self::GlobalIllumination
                | Self::GlobalIlluminationBackfaces
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentationIntent {
    #[default]
    Present,
    NoPresent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameCaptureIntent {
    #[default]
    None,
    Color,
    Layered,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameDebugIntent {
    pub stable_dissolves: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewClearState {
    pub color: Option<wgpu::Color>,
    pub depth: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct FrameInputBuilder {
    target: FrameTarget,
    camera_views: Vec<FrameCameraView>,
    scene: RenderSceneId,
    timing: FrameTiming,
    mode: FrameRenderingMode,
    purpose: FramePurpose,
    presentation: PresentationIntent,
    capture: FrameCaptureIntent,
    debug: FrameDebugIntent,
}

impl FrameInputBuilder {
    pub fn new(target: FrameTarget, scene: RenderSceneId) -> Self {
        Self {
            target,
            camera_views: Vec::new(),
            scene,
            timing: FrameTiming::default(),
            mode: FrameRenderingMode::default(),
            purpose: FramePurpose::default(),
            presentation: PresentationIntent::default(),
            capture: FrameCaptureIntent::default(),
            debug: FrameDebugIntent::default(),
        }
    }

    pub fn target_key(&self) -> FrameTargetKey {
        self.target.key
    }

    pub fn push_camera_view(&mut self, view: FrameCameraView) {
        self.camera_views.push(view);
    }

    pub fn finish(self) -> RenderFrameResult<FrameInput> {
        self.target.validate()?;

        if self.camera_views.is_empty() && self.purpose != FramePurpose::Blank {
            return Err(RenderFrameError::InvalidFrameInput {
                reason: "frame has no camera views",
            });
        }

        Ok(FrameInput {
            target: self.target,
            camera_views: self.camera_views,
            scene: self.scene,
            timing: self.timing,
            mode: self.mode,
            purpose: self.purpose,
            presentation: self.presentation,
            capture: self.capture,
            debug: self.debug,
        })
    }
}
