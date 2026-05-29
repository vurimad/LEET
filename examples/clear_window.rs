use bevy::{
    app::{App, PluginGroup},
    window::{PresentMode, Window, WindowPlugin},
};
use leet::LeetRenderShellPlugins;

// Smoke test only:
// This example exists to validate that LEET's custom render sub-app can present to a
// Bevy-managed window. It is not the target renderer architecture.
fn main() {
    App::new()
        .add_plugins(LeetRenderShellPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "LEET Clear Window".into(),
                present_mode: PresentMode::AutoNoVsync,
                ..Default::default()
            }),
            ..Default::default()
        }))
        .run();
}
