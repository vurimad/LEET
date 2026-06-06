//! Frame submission packet dispatcher.

use bevy_ecs::prelude::{Res, ResMut};

use crate::{GpuScene, RenderCameraStorage, RenderWindowRegistry};

use super::{FrameInput, FrameInputBuilder, RenderCommandHandler, RenderFrameResult};

#[derive(Default)]
pub struct FrameDispatcher {
    frame_inputs: Vec<FrameInput>,
}

impl FrameDispatcher {
    fn from_frame_inputs(frame_inputs: Vec<FrameInput>) -> Self {
        Self { frame_inputs }
    }

    pub fn construct(
        camera_storage: &mut RenderCameraStorage,
        windows: &mut RenderWindowRegistry,
        gpu_scene: &GpuScene,
    ) -> RenderFrameResult<Self> {
        let frame_inputs =
            FrameInputBuilder::construct().build(camera_storage, windows, gpu_scene)?;
        Ok(Self::from_frame_inputs(frame_inputs))
    }

    pub fn resolve_frames(
        self,
        render_commands: &mut RenderCommandHandler,
    ) -> RenderFrameResult<()> {
        for frame_input in self.frame_inputs {
            render_commands.render_scene(frame_input)?;
        }

        Ok(())
    }
}

pub fn dispatch_general_rendering(
    mut render_commands: ResMut<RenderCommandHandler>,
    mut camera_storage: ResMut<RenderCameraStorage>,
    mut windows: ResMut<RenderWindowRegistry>,
    gpu_scene: Res<GpuScene>,
) {
    let result = FrameDispatcher::construct(&mut camera_storage, &mut windows, &gpu_scene)
        .and_then(|dispatcher| dispatcher.resolve_frames(&mut render_commands));

    if let Err(error) = result {
        tracing::error!(%error, "general frame dispatch failed");
    }
}
