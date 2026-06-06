use bevy_camera::{NormalizedRenderTarget, RenderTargetInfo};
use bevy_ecs::{
    entity::{ContainsEntity, Entity},
    prelude::Query,
};
use bevy_math::UVec2;

use crate::ManualTextureViews;

pub(crate) fn get_render_target_info(
    target: &NormalizedRenderTarget,
    windows: &Query<(Entity, &bevy_window::Window)>,
    images: Option<&bevy_asset::Assets<bevy_image::Image>>,
    manual_texture_views: &ManualTextureViews,
) -> Option<RenderTargetInfo> {
    match target {
        NormalizedRenderTarget::Window(window_ref) => windows
            .iter()
            .find(|(entity, _)| *entity == window_ref.entity())
            .map(|(_, window)| RenderTargetInfo {
                physical_size: window.physical_size(),
                scale_factor: window.resolution.scale_factor(),
            }),
        NormalizedRenderTarget::Image(image_target) => images
            .and_then(|images| images.get(&image_target.handle))
            .map(|image| RenderTargetInfo {
                physical_size: image.size(),
                scale_factor: image_target.scale_factor,
            }),
        NormalizedRenderTarget::TextureView(id) => {
            manual_texture_views.get(id).map(|view| RenderTargetInfo {
                physical_size: view.size,
                scale_factor: 1.0,
            })
        }
        NormalizedRenderTarget::None { width, height } => Some(RenderTargetInfo {
            physical_size: UVec2::new(*width, *height),
            scale_factor: 1.0,
        }),
    }
}
