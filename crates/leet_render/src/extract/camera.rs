use crate::{
    Extract, ManualTextureViews, RenderCamera, RenderCameraFeatures, RenderCameraStorage,
    RenderWindowRegistry,
};
use bevy_asset::Assets;
use bevy_camera::{Camera, CompositingSpace, Exposure, Hdr, NormalizedRenderTarget, RenderTarget};
use bevy_ecs::{
    entity::{ContainsEntity, Entity},
    prelude::{Query, Res, ResMut, With},
    query::Has,
};
use bevy_image::Image;
use bevy_math::URect;
use bevy_transform::components::GlobalTransform;
use bevy_window::PrimaryWindow;
use wgpu::TextureFormat;

pub(crate) fn extract_cameras(
    mut camera_storage: ResMut<RenderCameraStorage>,
    query: Extract<
        Query<(
            Entity,
            &Camera,
            &RenderTarget,
            &GlobalTransform,
            (Has<Hdr>, Option<&CompositingSpace>, Option<&Exposure>),
        )>,
    >,
    primary_window: Extract<Query<Entity, With<PrimaryWindow>>>,
    render_windows: Res<RenderWindowRegistry>,
    images: Extract<Option<Res<Assets<Image>>>>,
    manual_texture_views: Res<ManualTextureViews>,
) {
    let primary_window = primary_window.iter().next();
    camera_storage.clear_extracted_cameras();

    for (main_entity, camera, render_target, transform, (hdr, compositing_space, exposure)) in
        query.iter()
    {
        if !camera.is_active {
            continue;
        }

        let (
            Some(URect {
                min: viewport_origin,
                ..
            }),
            Some(viewport_size),
            Some(target_size),
        ) = (
            camera.physical_viewport_rect(),
            camera.physical_viewport_size(),
            camera.physical_target_size(),
        )
        else {
            continue;
        };

        if target_size.x == 0 || target_size.y == 0 {
            continue;
        }

        let target = render_target.normalize(primary_window);
        let output_texture_format = target
            .as_ref()
            .and_then(|target| {
                get_target_texture_view_format(
                    target,
                    &render_windows,
                    images.as_deref(),
                    &manual_texture_views,
                )
            })
            .map(|format| normalize_bgra8(format))
            .unwrap_or(TextureFormat::Rgba8UnormSrgb);

        let target_format = if hdr {
            TextureFormat::Rgba16Float
        } else if compositing_space.is_some_and(|space| *space == CompositingSpace::Srgb) {
            TextureFormat::Rgba8Unorm
        } else {
            output_texture_format
        };

        camera_storage.insert_extracted_camera(
            main_entity,
            RenderCamera {
                target: target.clone(),
                physical_target_size: Some(target_size),
                clip_from_view: camera.clip_from_view(),
                world_from_view: *transform,
                viewport: URect::new(
                    viewport_origin.x,
                    viewport_origin.y,
                    viewport_origin.x + viewport_size.x,
                    viewport_origin.y + viewport_size.y,
                ),
                invert_culling: camera.invert_culling,
                main_pass_texture_format: target_format,
                order: camera.order,
                output_mode: camera.output_mode,
                msaa_writeback: camera.msaa_writeback,
                clear_color: camera.clear_color.clone(),
                exposure: exposure
                    .map(Exposure::exposure)
                    .unwrap_or_else(|| Exposure::default().exposure()),
                hdr,
                features: RenderCameraFeatures::empty(),
                compositing_space: compositing_space.copied(),
            },
        );
    }
}

fn get_target_texture_view_format(
    target: &NormalizedRenderTarget,
    windows: &RenderWindowRegistry,
    images: Option<&Assets<Image>>,
    manual_texture_views: &ManualTextureViews,
) -> Option<TextureFormat> {
    match target {
        NormalizedRenderTarget::Window(window_ref) => windows
            .get(&window_ref.entity())
            .and_then(|window| window.swap_chain_texture_view_format),
        NormalizedRenderTarget::Image(image_target) => images
            .and_then(|images| images.get(&image_target.handle))
            .map(image_texture_view_format),
        NormalizedRenderTarget::TextureView(id) => {
            manual_texture_views.get(id).map(|view| view.view_format)
        }
        NormalizedRenderTarget::None { .. } => None,
    }
}

fn normalize_bgra8(format: TextureFormat) -> TextureFormat {
    if format == TextureFormat::Bgra8UnormSrgb {
        return TextureFormat::Rgba8UnormSrgb;
    }

    format
}

fn image_texture_view_format(image: &Image) -> TextureFormat {
    image.texture_descriptor.format
}
