use super::RenderingPreprocessingPlugin;
use crate::{GpuScene, RenderApp, RenderAppPlugin, RenderProxyDescriptor};
use bevy_app::App;

#[test]
fn prepare_system_uploads_scene_input_buffers() {
    let mut app = App::new();
    app.add_plugins(RenderAppPlugin);

    {
        let render_app = app
            .get_sub_app_mut(RenderApp)
            .expect("LEET render sub-app missing");
        render_app
            .world_mut()
            .resource_mut::<GpuScene>()
            .allocate_proxy(RenderProxyDescriptor::default());
    }

    app.update();

    let render_app = app
        .get_sub_app(RenderApp)
        .expect("LEET render sub-app missing");
    let scene = render_app.world().resource::<GpuScene>();
    assert!(scene.current_inputs().buffer().is_some());
    assert!(scene.previous_inputs().buffer().is_some());
    assert!(app.is_plugin_added::<RenderingPreprocessingPlugin>());
}
