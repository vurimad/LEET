use bevy_ecs::{
    entity::{Entity, EntityHashMap},
    prelude::Resource,
};
use bevy_window::{CompositeAlphaMode, PresentMode, RawHandleWrapper};
use std::{
    num::NonZero,
    ops::{Deref, DerefMut},
};
use wgpu::{TextureFormat, TextureView, TextureViewDescriptor};

/// Render-world window state for a window entity.
pub struct RenderWindow {
    pub entity: Entity,
    pub handle: RawHandleWrapper,
    pub physical_width: u32,
    pub physical_height: u32,
    pub present_mode: PresentMode,
    pub desired_maximum_frame_latency: Option<NonZero<u32>>,
    pub alpha_mode: CompositeAlphaMode,
    pub swap_chain_texture_view: Option<TextureView>,
    pub swap_chain_texture: Option<wgpu::SurfaceTexture>,
    pub swap_chain_texture_format: Option<TextureFormat>,
    pub swap_chain_texture_view_format: Option<TextureFormat>,
    pub handle_changed: bool,
    pub size_changed: bool,
    pub present_mode_changed: bool,
    pub needs_surface_reconfigure: bool,
    pub needs_surface_rebuild: bool,
    pub needs_initial_present: bool,
}

impl RenderWindow {
    pub fn set_swapchain_texture(&mut self, frame: wgpu::SurfaceTexture) {
        self.swap_chain_texture_view_format = Some(frame.texture.format().add_srgb_suffix());
        let texture_view_descriptor = TextureViewDescriptor {
            format: self.swap_chain_texture_view_format,
            ..Default::default()
        };
        self.swap_chain_texture_view = Some(frame.texture.create_view(&texture_view_descriptor));
        self.swap_chain_texture = Some(frame);
    }

    pub fn has_swapchain_texture(&self) -> bool {
        self.swap_chain_texture_view.is_some() && self.swap_chain_texture.is_some()
    }

    pub fn clear_swapchain_texture(&mut self) {
        self.swap_chain_texture_view = None;
        self.swap_chain_texture = None;
    }

    pub fn present(&mut self) {
        self.swap_chain_texture_view = None;
        if let Some(surface_texture) = self.swap_chain_texture.take() {
            surface_texture.present();
        }
    }
}

/// Render-world table of the windows available to rendering.
#[derive(Default, Resource)]
pub struct RenderWindowRegistry {
    pub primary: Option<Entity>,
    pub windows: EntityHashMap<RenderWindow>,
}

impl Deref for RenderWindowRegistry {
    type Target = EntityHashMap<RenderWindow>;

    fn deref(&self) -> &Self::Target {
        &self.windows
    }
}

impl DerefMut for RenderWindowRegistry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.windows
    }
}
