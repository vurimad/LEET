//! Submitted frame packet assembled before entering the render command path.

use bevy_camera::NormalizedRenderTarget;
use bevy_ecs::entity::ContainsEntity;
use bevy_math::UVec2;

use crate::{
    CameraPrepareContext, FrameOutput, FrameTargetResolver, GpuScene, GpuScenePhase,
    PreparedFrameCamera, RenderCameraRegistrationRef, RenderCameraStorage, RenderViewport,
    RenderWindowRegistry,
};

use super::{RenderFrameError, RenderFrameResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderCameraId(pub u64);

pub struct FrameInput {
    pub viewport: RenderViewport,
    pub output: FrameOutput,
    pub cameras: Vec<PreparedFrameCamera>,
    pub scene: FrameGpuScene,
    pub timing: FrameTiming,
    pub mode: FrameRenderingMode,
    pub purpose: FramePurpose,
    pub presentation: PresentationIntent,
    pub capture: FrameCaptureIntent,
    pub debug: FrameDebugIntent,
}

#[derive(Clone, Debug)]
pub struct FrameGpuScene {
    pub live_proxy_count: u32,
    pub slot_capacity: u32,
    pub current_input_count: u32,
    pub previous_input_count: u32,
    pub current_input_buffer: Option<wgpu::Buffer>,
    pub previous_input_buffer: Option<wgpu::Buffer>,
    pub computed_instance_buffer: Option<wgpu::Buffer>,
    pub phase_index_buffers: Vec<FrameGpuScenePhaseIndexBuffer>,
}

impl FrameGpuScene {
    pub fn empty() -> Self {
        Self {
            live_proxy_count: 0,
            slot_capacity: 0,
            current_input_count: 0,
            previous_input_count: 0,
            current_input_buffer: None,
            previous_input_buffer: None,
            computed_instance_buffer: None,
            phase_index_buffers: Vec::new(),
        }
    }

    fn from_gpu_scene(scene: &GpuScene) -> Self {
        const PHASES: [GpuScenePhase; 6] = [
            GpuScenePhase::Opaque,
            GpuScenePhase::AlphaMask,
            GpuScenePhase::Transparent,
            GpuScenePhase::Shadow,
            GpuScenePhase::Deferred,
            GpuScenePhase::Prepass,
        ];

        let phase_index_buffers = PHASES
            .into_iter()
            .filter_map(|phase| {
                scene.phase_instance_index_buffer(phase).map(|buffer| {
                    FrameGpuScenePhaseIndexBuffer {
                        phase,
                        buffer: buffer.clone(),
                    }
                })
            })
            .collect();

        Self {
            live_proxy_count: scene.live_proxy_count() as u32,
            slot_capacity: scene.slot_capacity() as u32,
            current_input_count: scene.current_inputs().len(),
            previous_input_count: scene.previous_inputs().len(),
            current_input_buffer: scene
                .current_inputs()
                .buffer()
                .map(|buffer| (**buffer).clone()),
            previous_input_buffer: scene
                .previous_inputs()
                .buffer()
                .map(|buffer| (**buffer).clone()),
            computed_instance_buffer: scene.computed_instance_buffer().cloned(),
            phase_index_buffers,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrameGpuScenePhaseIndexBuffer {
    pub phase: crate::GpuScenePhase,
    pub buffer: wgpu::Buffer,
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
    SafeMode,
    GBufferOnly,
    Blank,
}

impl FrameRenderingMode {
    pub const fn allows_camera_jitter(self) -> bool {
        matches!(self, Self::Shaded | Self::GBufferOnly)
    }

    pub const fn flushes_temporary_cameras(self) -> bool {
        matches!(self, Self::Shaded | Self::GBufferOnly | Self::SafeMode)
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
    pub graph_view: FrameDebugGraphView,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameDebugGraphView {
    #[default]
    None,
    Visualization,
}

#[derive(Clone, Debug)]
pub struct FrameInputBuilder {
    target_groups: Vec<FrameInputTargetGroup>,
    scene: FrameGpuScene,
    timing: FrameTiming,
    mode: FrameRenderingMode,
    purpose: FramePurpose,
    presentation: PresentationIntent,
    capture: FrameCaptureIntent,
    debug: FrameDebugIntent,
}

impl FrameInputBuilder {
    pub fn construct() -> Self {
        Self {
            target_groups: Vec::new(),
            scene: FrameGpuScene::empty(),
            timing: FrameTiming::default(),
            mode: FrameRenderingMode::default(),
            purpose: FramePurpose::default(),
            presentation: PresentationIntent::default(),
            capture: FrameCaptureIntent::default(),
            debug: FrameDebugIntent::default(),
        }
    }

    fn add_submitted_camera(
        &mut self,
        camera: RenderCameraRegistrationRef<'_>,
        windows: &RenderWindowRegistry,
    ) -> RenderFrameResult<()> {
        let Some(target_group) = FrameInputTargetGroup::from_camera(camera, windows)? else {
            return Ok(());
        };

        if let Some(existing) = self
            .target_groups
            .iter_mut()
            .find(|existing| existing.declared == target_group.declared)
        {
            existing.requested_cameras.push(camera.camera_id);
        } else {
            self.target_groups.push(target_group);
        }

        Ok(())
    }

    pub fn build(
        mut self,
        camera_storage: &mut RenderCameraStorage,
        windows: &mut RenderWindowRegistry,
        gpu_scene: &GpuScene,
    ) -> RenderFrameResult<Vec<FrameInput>> {
        self.scene = FrameGpuScene::from_gpu_scene(gpu_scene);

        for camera_id in camera_storage.submitted_camera_ids().iter().copied() {
            let Some(camera) = camera_storage.registered_camera(camera_id) else {
                continue;
            };
            self.add_submitted_camera(camera, windows)?;
        }

        let mut target_resolver = FrameTargetResolver::new(windows);
        self.build_targets(camera_storage, &mut target_resolver)
    }

    fn build_targets(
        self,
        camera_storage: &mut RenderCameraStorage,
        target_resolver: &mut FrameTargetResolver<'_>,
    ) -> RenderFrameResult<Vec<FrameInput>> {
        let mut frame_inputs = Vec::with_capacity(self.target_groups.len());

        for target_group in self.target_groups {
            let (viewport, output) = target_resolver.resolve(
                target_group.declared,
                target_group.extent,
                target_group.format,
            )?;
            let camera_context = CameraPrepareContext::new(
                self.mode.allows_camera_jitter(),
                self.mode.flushes_temporary_cameras(),
                self.purpose.requires_stable_dissolves() || self.debug.stable_dissolves,
                self.timing.frame_index,
                viewport.full_rect(),
            );
            let cameras = camera_storage
                .prepare_frame_cameras(camera_context, &target_group.requested_cameras)?;
            frame_inputs.push(build_frame_input(
                viewport,
                output,
                cameras,
                self.scene.clone(),
                self.timing,
                self.mode,
                self.purpose,
                self.presentation,
                self.capture,
                self.debug,
            )?);
        }

        Ok(frame_inputs)
    }
}

#[derive(Clone, Debug)]
struct FrameInputTargetGroup {
    declared: NormalizedRenderTarget,
    extent: UVec2,
    format: Option<wgpu::TextureFormat>,
    requested_cameras: Vec<RenderCameraId>,
}

impl FrameInputTargetGroup {
    fn from_camera(
        camera: RenderCameraRegistrationRef<'_>,
        windows: &RenderWindowRegistry,
    ) -> RenderFrameResult<Option<Self>> {
        let Some(target) = &camera.camera.target else {
            return Ok(None);
        };
        let Some(extent) = camera.camera.physical_target_size else {
            return Ok(None);
        };

        match target {
            NormalizedRenderTarget::Window(window_ref) => {
                let window_entity = window_ref.entity();
                let format = windows
                    .get(&window_entity)
                    .and_then(|window| {
                        window
                            .swap_chain_texture_view_format
                            .or(window.swap_chain_texture_format)
                    })
                    .unwrap_or(camera.camera.main_pass_texture_format);

                Ok(Some(Self {
                    declared: target.clone(),
                    extent,
                    format: Some(format),
                    requested_cameras: vec![camera.camera_id],
                }))
            }
            NormalizedRenderTarget::Image(_)
            | NormalizedRenderTarget::TextureView(_)
            | NormalizedRenderTarget::None { .. } => Ok(None),
        }
    }
}

fn build_frame_input(
    viewport: RenderViewport,
    output: FrameOutput,
    cameras: Vec<PreparedFrameCamera>,
    scene: FrameGpuScene,
    timing: FrameTiming,
    mode: FrameRenderingMode,
    purpose: FramePurpose,
    presentation: PresentationIntent,
    capture: FrameCaptureIntent,
    debug: FrameDebugIntent,
) -> RenderFrameResult<FrameInput> {
    if cameras.is_empty() && purpose != FramePurpose::Blank {
        return Err(RenderFrameError::InvalidFrameInput {
            reason: "frame has no camera views",
        });
    }

    Ok(FrameInput {
        viewport,
        output,
        cameras,
        scene,
        timing,
        mode,
        purpose,
        presentation,
        capture,
        debug,
    })
}
