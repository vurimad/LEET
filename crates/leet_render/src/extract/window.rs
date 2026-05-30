use crate::{Extract, RenderWindow, RenderWindowRegistry};
use bevy_ecs::{
    entity::{Entity, EntityHashSet},
    prelude::{Query, ResMut},
};
use bevy_window::{PrimaryWindow, RawHandleWrapper, Window};

pub(crate) fn extract_windows(
    render_windows: Option<ResMut<RenderWindowRegistry>>,
    windows: Extract<Query<(Entity, &Window, &RawHandleWrapper, Option<&PrimaryWindow>)>>,
) {
    let Some(mut render_windows) = render_windows else {
        return;
    };

    render_windows.primary = None;
    let mut live_windows = EntityHashSet::default();

    for (entity, window, handle, primary) in windows.iter() {
        live_windows.insert(entity);

        if primary.is_some() {
            render_windows.primary = Some(entity);
        }

        let new_width = window.resolution.physical_width().max(1);
        let new_height = window.resolution.physical_height().max(1);

        let render_window = render_windows
            .entry(entity)
            .or_insert_with(|| RenderWindow {
                entity,
                handle: handle.clone(),
                physical_width: new_width,
                physical_height: new_height,
                present_mode: window.present_mode,
                desired_maximum_frame_latency: window.desired_maximum_frame_latency,
                alpha_mode: window.composite_alpha_mode,
                swap_chain_texture_view: None,
                swap_chain_texture: None,
                swap_chain_texture_format: None,
                swap_chain_texture_view_format: None,
                handle_changed: false,
                size_changed: false,
                present_mode_changed: false,
                needs_surface_reconfigure: false,
                needs_surface_rebuild: false,
                needs_initial_present: true,
            });

        if render_window.swap_chain_texture.is_none() {
            render_window.swap_chain_texture_view = None;
        }

        let previous_window_handle = render_window.handle.get_window_handle();
        let previous_display_handle = render_window.handle.get_display_handle();
        let new_window_handle = handle.get_window_handle();
        let new_display_handle = handle.get_display_handle();
        render_window.handle_changed = previous_window_handle != new_window_handle
            || previous_display_handle != new_display_handle;

        render_window.size_changed = new_width != render_window.physical_width
            || new_height != render_window.physical_height;
        render_window.present_mode_changed = window.present_mode != render_window.present_mode;

        if render_window.handle_changed {
            render_window.handle = handle.clone();
            render_window.needs_surface_rebuild = true;
        }

        if render_window.size_changed {
            render_window.physical_width = new_width;
            render_window.physical_height = new_height;
        }

        if render_window.present_mode_changed {
            render_window.present_mode = window.present_mode;
        }

        render_window.desired_maximum_frame_latency = window.desired_maximum_frame_latency;
        render_window.alpha_mode = window.composite_alpha_mode;
        render_window.needs_surface_reconfigure = render_window.size_changed
            || render_window.present_mode_changed
            || render_window.needs_surface_reconfigure;
    }

    let stale_windows: Vec<_> = render_windows
        .keys()
        .copied()
        .filter(|entity| !live_windows.contains(entity))
        .collect();
    for stale_window in stale_windows {
        render_windows.remove(&stale_window);
        if render_windows.primary == Some(stale_window) {
            render_windows.primary = None;
        }
    }
}

#[cfg(test)]
#[path = "../tests/extract/window.rs"]
mod tests;
