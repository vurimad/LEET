use super::{extract_cameras, extract_manual_texture_views, extract_windows};
use crate::{ExtractSchedule, RenderApp};
use bevy_app::{App, Plugin};
use bevy_ecs::schedule::IntoScheduleConfigs;

/// Registers LEET extraction systems.
///
/// This plugin owns extraction scheduling only. Destination render-world
/// resources are owned by their domain plugins.
#[derive(Default)]
pub struct ExtractionPlugin;

impl Plugin for ExtractionPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app.add_systems(
            ExtractSchedule,
            (
                extract_windows,
                extract_manual_texture_views,
                extract_cameras,
            )
                .chain(),
        );
    }
}
