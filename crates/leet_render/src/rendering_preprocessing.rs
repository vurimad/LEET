use crate::{
    buffer_uploaders::{
        run_sparse_buffer_update_jobs, SparseBufferUpdateJobs, SparseBufferUpdatePipeline,
    },
    render_resources_from_wgpu, GpuScene, Render, RenderApp, RenderDevice, RenderQueue,
    RenderSystems,
};
use bevy_app::{App, Plugin};
use bevy_ecs::{
    prelude::{Res, ResMut},
    schedule::IntoScheduleConfigs,
};

/// Scene-side host preprocessing that prepares sparse GPU uploads before the
/// shared sparse-upload dispatcher runs.
#[derive(Default)]
pub struct RenderingPreprocessingPlugin;

impl Plugin for RenderingPreprocessingPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app.add_systems(
            Render,
            prepare_gpu_scene_input_uploads
                .before(run_sparse_buffer_update_jobs)
                .in_set(RenderSystems::Prepare),
        );
    }
}

fn prepare_gpu_scene_input_uploads(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    sparse_buffer_update_pipeline: Res<SparseBufferUpdatePipeline>,
    mut sparse_buffer_update_jobs: ResMut<SparseBufferUpdateJobs>,
    mut gpu_scene: ResMut<GpuScene>,
) {
    let (render_device, render_queue) =
        render_resources_from_wgpu(&render_device.0, &render_queue.0);
    gpu_scene.write_and_prepare_input_buffers(
        &render_device,
        &render_queue,
        &mut sparse_buffer_update_jobs,
        &sparse_buffer_update_pipeline,
    );
}

#[cfg(test)]
mod tests {
    use super::RenderingPreprocessingPlugin;
    use crate::{GpuScene, RenderApp, RenderPlugin, RenderProxyDescriptor};
    use bevy_app::App;

    #[test]
    fn prepare_system_uploads_scene_input_buffers() {
        let mut app = App::new();
        app.add_plugins(RenderPlugin);

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
}
