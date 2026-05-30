use crate::{PipelinedRenderingPlugin, RenderAppPlugin};
use bevy_a11y::AccessibilityPlugin;
use bevy_app::{PluginGroup, PluginGroupBuilder, TaskPoolPlugin};
use bevy_input::InputPlugin;
use bevy_input_focus::InputFocusPlugin;
use bevy_window::WindowPlugin;
use bevy_winit::WinitPlugin;

/// A ready-to-use Bevy shell for LEET's custom renderer.
///
/// This keeps the required Bevy support plugins explicit in one place and avoids the
/// "works only if you remembered plugin X" startup path for the common windowed case.
/// It also enables LEET's pipelined render-thread mode by default so apps exercise the
/// real extract/render thread boundary early.
///
/// Apps can still override any plugin in the group using Bevy's normal `.set(...)` flow.
#[derive(Default)]
pub struct RenderShellPlugins;

impl PluginGroup for RenderShellPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(TaskPoolPlugin::default())
            .add(AccessibilityPlugin)
            .add(InputPlugin)
            .add(InputFocusPlugin)
            .add(WindowPlugin::default())
            .add(WinitPlugin::default())
            .add(RenderAppPlugin)
            .add(PipelinedRenderingPlugin)
    }
}
