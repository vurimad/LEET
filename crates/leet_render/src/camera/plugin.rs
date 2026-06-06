use crate::{
    camera::{sync_render_camera_storage, RenderCameraStorage},
    render_target::get_render_target_info,
    ManualTextureViews, Render, RenderApp, RenderSystems, TexturePlugin,
};
use bevy_app::{App, Plugin, PostUpdate};
use bevy_camera::{Camera, CameraUpdateSystems, RenderTarget};
use bevy_ecs::{
    entity::Entity,
    prelude::{Query, Res},
    schedule::IntoScheduleConfigs,
};
use bevy_math::UVec2;
use bevy_transform::{TransformPlugin, TransformSystems};

#[derive(Default)]
pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TransformPlugin>() {
            app.add_plugins(TransformPlugin);
        }

        app.add_systems(
            PostUpdate,
            camera_system
                .in_set(CameraUpdateSystems)
                .after(TransformSystems::Propagate),
        );

        if !app.is_plugin_added::<TexturePlugin>() {
            app.add_plugins(TexturePlugin);
        }

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<RenderCameraStorage>()
                .add_systems(
                    Render,
                    sync_render_camera_storage.in_set(RenderSystems::Prepare),
                );
        }
    }
}

pub fn camera_system(
    mut cameras: Query<(Entity, &mut Camera, &RenderTarget)>,
    windows: Query<(Entity, &bevy_window::Window)>,
    images: Option<Res<bevy_asset::Assets<bevy_image::Image>>>,
    manual_texture_views: Res<ManualTextureViews>,
) {
    for (_entity, mut camera, render_target) in &mut cameras {
        let (viewport_size, _target_size) = if let Some(normalized_target) =
            render_target.normalize(windows.single().ok().map(|(e, _)| e))
        {
            let info = get_render_target_info(
                &normalized_target,
                &windows,
                images.as_deref(),
                &manual_texture_views,
            );

            (
                camera
                    .logical_viewport_size()
                    .unwrap_or(bevy_math::Vec2::ZERO),
                info.map(|i| i.physical_size).unwrap_or(UVec2::ZERO),
            )
        } else {
            (bevy_math::Vec2::ZERO, UVec2::ZERO)
        };

        if camera.computed.old_viewport_size != Some(viewport_size.as_uvec2()) {
            camera.computed.old_viewport_size = Some(viewport_size.as_uvec2());
            // camera.computed.old_sub_camera_view = camera.sub_camera_view;
        }
    }
}
