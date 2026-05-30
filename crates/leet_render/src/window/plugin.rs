use super::{
    cleanup_stale_surfaces, create_surfaces, prepare_windows, smoke_test_render_windows,
    window_surface_needs_configuration, RenderWindowRegistry, WindowSurfaces,
};
use crate::{Render, RenderApp, RenderSystems};
use bevy_app::{App, Plugin};
use bevy_ecs::schedule::IntoScheduleConfigs;

/// Registers render-world window runtime state.
///
/// Extraction stays centralized elsewhere; this plugin owns the destination
/// render-world window table and any future window-target runtime systems.
#[derive(Default)]
pub struct RenderWindowPlugin;

impl Plugin for RenderWindowPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_resource::<RenderWindowRegistry>()
            .init_resource::<WindowSurfaces>()
            .add_systems(
                Render,
                cleanup_stale_surfaces
                    .in_set(RenderSystems::Prepare)
                    .before(create_surfaces),
            )
            .add_systems(
                Render,
                create_surfaces
                    .run_if(window_surface_needs_configuration)
                    .in_set(RenderSystems::Prepare)
                    .before(prepare_windows),
            )
            .add_systems(Render, prepare_windows.in_set(RenderSystems::Prepare))
            .add_systems(
                Render,
                smoke_test_render_windows.in_set(RenderSystems::Render),
            );
    }
}
