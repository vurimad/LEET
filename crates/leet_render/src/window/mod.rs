mod plugin;
mod render_windows;
mod surface;

pub use plugin::RenderWindowPlugin;
pub use render_windows::{RenderWindow, RenderWindowRegistry};
pub use surface::{
    cleanup_stale_surfaces, create_surfaces, prepare_windows, smoke_test_render_windows,
    window_surface_needs_configuration, WindowSurfaces,
};
