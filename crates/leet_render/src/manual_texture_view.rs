use crate::{Extract, ExtractSchedule, RenderApp};
use bevy_app::{App, Plugin};
use bevy_camera::ManualTextureViewHandle;
use bevy_ecs::prelude::{Res, ResMut, Resource};
use bevy_math::UVec2;
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};
use wgpu::TextureFormat;

/// A manually managed texture view for use as a camera render target.
#[derive(Clone)]
pub struct ManualTextureView {
    pub texture_view: wgpu::TextureView,
    pub size: UVec2,
    pub view_format: TextureFormat,
}

impl ManualTextureView {
    pub fn with_default_format(texture_view: wgpu::TextureView, size: UVec2) -> Self {
        Self {
            texture_view,
            size,
            view_format: TextureFormat::Rgba8UnormSrgb,
        }
    }
}

/// Render-target texture views that are created and owned outside LEET's renderer bootstrap.
#[derive(Default, Clone, Resource)]
pub struct ManualTextureViews(HashMap<ManualTextureViewHandle, ManualTextureView>);

impl Deref for ManualTextureViews {
    type Target = HashMap<ManualTextureViewHandle, ManualTextureView>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ManualTextureViews {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub struct ManualTextureViewPlugin;

impl Plugin for ManualTextureViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ManualTextureViews>();

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<ManualTextureViews>()
                .add_systems(ExtractSchedule, extract_manual_texture_views);
        }
    }
}

fn extract_manual_texture_views(
    mut extracted_manual_texture_views: ResMut<ManualTextureViews>,
    manual_texture_views: Extract<Option<Res<ManualTextureViews>>>,
) {
    if let Some(manual_texture_views) = manual_texture_views.as_ref() {
        *extracted_manual_texture_views = manual_texture_views.deref().clone();
    } else {
        extracted_manual_texture_views.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::ManualTextureViews;
    use crate::{RenderApp, RenderDevice, RenderPlugin};
    use bevy_app::App;
    use bevy_camera::ManualTextureViewHandle;
    use bevy_math::UVec2;
    use wgpu::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

    #[test]
    fn manual_texture_view_extraction_clears_removed_entries() {
        let mut app = App::new();
        app.add_plugins(RenderPlugin);

        let handle = ManualTextureViewHandle(99);
        let texture_view = {
            let device = app.world().resource::<RenderDevice>();
            let texture = device.0.create_texture(&wgpu::TextureDescriptor {
                label: Some("leet_manual_texture_view_cleanup_test"),
                size: Extent3d {
                    width: 32,
                    height: 16,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8UnormSrgb,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            texture.create_view(&wgpu::TextureViewDescriptor::default())
        };

        app.world_mut().resource_mut::<ManualTextureViews>().insert(
            handle,
            super::ManualTextureView {
                texture_view,
                size: UVec2::new(32, 16),
                view_format: TextureFormat::Rgba8UnormSrgb,
            },
        );

        app.update();

        {
            let render_app = app
                .get_sub_app(RenderApp)
                .expect("LEET render sub-app missing");
            let extracted_manual_texture_views =
                render_app.world().resource::<ManualTextureViews>();
            assert!(extracted_manual_texture_views.contains_key(&handle));
        }

        app.world_mut()
            .resource_mut::<ManualTextureViews>()
            .remove(&handle);

        app.update();

        let render_app = app
            .get_sub_app(RenderApp)
            .expect("LEET render sub-app missing");
        let extracted_manual_texture_views = render_app.world().resource::<ManualTextureViews>();
        assert!(
            !extracted_manual_texture_views.contains_key(&handle),
            "removed main-world manual texture views should be cleared from the render world"
        );
    }
}
