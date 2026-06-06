use crate::{ManualTextureViews, RenderApp};
use bevy_app::{App, Plugin};

/// Registers render-world texture tables and domain boundaries.
#[derive(Default)]
pub struct TexturePlugin;

impl Plugin for TexturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ManualTextureViews>();

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.init_resource::<ManualTextureViews>();
        }
    }
}
