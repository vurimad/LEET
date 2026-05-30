use crate::{
    render_resources_from_wgpu, rendering::run_sparse_buffer_update_jobs, GpuScene, Render,
    RenderApp, RenderDevice, RenderQueue, RenderSystems, SparseBufferUpdateJobs,
    SparseBufferUpdatePipeline,
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
#[path = "../tests/scene/rendering_preprocessing.rs"]
mod tests;
