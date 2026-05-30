use bevy_camera::ManualTextureViewHandle;
use bevy_ecs::prelude::Resource;
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
pub struct ManualTextureViews(pub HashMap<ManualTextureViewHandle, ManualTextureView>);

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
