//! Frame submission packet dispatcher.

use bevy_camera::NormalizedRenderTarget;
use bevy_ecs::entity::ContainsEntity;
use bevy_ecs::prelude::{Res, ResMut};

use crate::ExtractedWindows;

use super::{
    FrameCameraView, FrameInput, FrameInputBuilder, FrameTarget, FrameTargetKey,
    RenderCameraStorage, RenderCommandHandler, RenderFrameResult, RenderSceneId,
};

#[derive(Default)]
pub struct FrameDispatcher {
    frame_inputs: Vec<FrameInput>,
}

impl FrameDispatcher {
    pub fn construct(frame_inputs: Vec<FrameInput>) -> Self {
        Self { frame_inputs }
    }

    pub fn construct_general_rendering(
        camera_storage: &RenderCameraStorage,
        windows: &ExtractedWindows,
    ) -> RenderFrameResult<Self> {
        let mut builders = Vec::<FrameInputBuildGroup>::new();

        for camera_id in camera_storage.submitted_camera_ids().iter().copied() {
            let Some(camera_view) = camera_storage.camera_view(camera_id) else {
                continue;
            };
            let Some(target) = build_frame_target(camera_view, windows)? else {
                continue;
            };

            let group_index = match builders
                .iter()
                .position(|group| group.target_key == target.key)
            {
                Some(index) => index,
                None => {
                    let group_index = builders.len();
                    builders.push(FrameInputBuildGroup {
                        target_key: target.key,
                        builder: FrameInputBuilder::new(target, RenderSceneId(0)),
                    });
                    group_index
                }
            };

            builders[group_index]
                .builder
                .push_camera_view(camera_view.clone());
        }

        let mut frame_inputs = Vec::with_capacity(builders.len());
        for group in builders {
            frame_inputs.push(group.builder.finish()?);
        }

        Ok(Self { frame_inputs })
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
    camera_storage: Res<RenderCameraStorage>,
    windows: Res<ExtractedWindows>,
) {
    let result = FrameDispatcher::construct_general_rendering(&camera_storage, &windows)
        .and_then(|dispatcher| dispatcher.resolve_frames(&mut render_commands));

    if let Err(error) = result {
        tracing::error!(%error, "general frame dispatch failed");
    }
}

struct FrameInputBuildGroup {
    target_key: FrameTargetKey,
    builder: FrameInputBuilder,
}

fn build_frame_target(
    camera_view: &FrameCameraView,
    windows: &ExtractedWindows,
) -> RenderFrameResult<Option<FrameTarget>> {
    let Some(target) = &camera_view.camera.target else {
        return Ok(None);
    };
    let Some(extent) = camera_view.camera.physical_target_size else {
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
                .unwrap_or(camera_view.view.target_format);

            Ok(Some(FrameTarget {
                key: FrameTargetKey::Window(window_entity),
                extent,
                format: Some(format),
            }))
        }
        NormalizedRenderTarget::Image(_)
        | NormalizedRenderTarget::TextureView(_)
        | NormalizedRenderTarget::None { .. } => Ok(None),
    }
}
