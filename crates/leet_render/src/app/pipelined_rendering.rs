use crate::RenderApp;
use async_channel::{Receiver, Sender};
use bevy_app::{App, AppExit, AppLabel, Plugin, SubApp};
use bevy_ecs::{
    prelude::Resource,
    schedule::MainThreadExecutor,
    world::{Mut, World},
};
use bevy_tasks::ComputeTaskPool;
use bevy_tasks::TaskPool;

/// Label for the sub-app that performs render extraction on the main thread while the render
/// world itself lives on a dedicated render thread.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, AppLabel)]
pub struct RenderExtractApp;

/// Channels used to shuttle LEET's render sub-app to and from the dedicated render thread.
#[derive(Resource)]
pub struct RenderAppChannels {
    app_to_render_sender: Sender<SubApp>,
    render_to_app_receiver: Receiver<SubApp>,
    render_app_in_render_thread: bool,
}

impl RenderAppChannels {
    pub fn new(
        app_to_render_sender: Sender<SubApp>,
        render_to_app_receiver: Receiver<SubApp>,
    ) -> Self {
        Self {
            app_to_render_sender,
            render_to_app_receiver,
            render_app_in_render_thread: false,
        }
    }

    pub fn send_blocking(&mut self, render_app: SubApp) {
        self.app_to_render_sender.send_blocking(render_app).unwrap();
        self.render_app_in_render_thread = true;
    }

    pub async fn recv(&mut self) -> Option<SubApp> {
        let render_app = self.render_to_app_receiver.recv().await.ok()?;
        self.render_app_in_render_thread = false;
        Some(render_app)
    }
}

impl Drop for RenderAppChannels {
    fn drop(&mut self) {
        if self.render_app_in_render_thread {
            self.render_to_app_receiver.recv_blocking().ok();
        }
    }
}

/// Runs LEET's render sub-app on a dedicated thread using the same extract boundary as immediate mode.
#[derive(Default)]
pub struct PipelinedRenderingPlugin;

impl Plugin for PipelinedRenderingPlugin {
    fn build(&self, app: &mut App) {
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }

        ComputeTaskPool::get_or_init(TaskPool::default);
        app.insert_resource(MainThreadExecutor::new());

        let mut extract_app = SubApp::new();
        extract_app.set_extract(renderer_extract);
        app.insert_sub_app(RenderExtractApp, extract_app);
    }

    fn cleanup(&self, app: &mut App) {
        if app.get_sub_app(RenderExtractApp).is_none() {
            return;
        }

        let (app_to_render_sender, app_to_render_receiver) = async_channel::bounded::<SubApp>(1);
        let (render_to_app_sender, render_to_app_receiver) = async_channel::bounded::<SubApp>(1);

        let mut render_app = app
            .remove_sub_app(RenderApp)
            .expect("LEET pipelined rendering could not detach RenderApp during plugin cleanup");

        let executor = app
            .world()
            .get_resource::<MainThreadExecutor>()
            .expect("LEET pipelined rendering requires MainThreadExecutor during cleanup");
        render_app.world_mut().insert_resource(executor.clone());

        render_to_app_sender.send_blocking(render_app).unwrap();

        app.insert_resource(RenderAppChannels::new(
            app_to_render_sender,
            render_to_app_receiver,
        ));

        std::thread::spawn(move || {
            let compute_task_pool = ComputeTaskPool::get();
            loop {
                let sent_app = compute_task_pool
                    .scope(|scope| {
                        scope.spawn(async { app_to_render_receiver.recv().await });
                    })
                    .pop();
                let Some(Ok(mut render_app)) = sent_app else {
                    break;
                };

                render_app.update();

                if render_to_app_sender.send_blocking(render_app).is_err() {
                    break;
                }
            }
        });
    }
}

fn renderer_extract(app_world: &mut World, _world: &mut World) {
    app_world.resource_scope(|world, main_thread_executor: Mut<MainThreadExecutor>| {
        world.resource_scope(|world, mut render_channels: Mut<RenderAppChannels>| {
            if let Some(mut render_app) = ComputeTaskPool::get()
                .scope_with_executor(true, Some(&*main_thread_executor.0), |scope| {
                    scope.spawn(async { render_channels.recv().await });
                })
                .pop()
                .unwrap()
            {
                render_app.extract(world);
                render_channels.send_blocking(render_app);
            } else {
                world.write_message(AppExit::error());
            }
        });
    });
}

#[cfg(test)]
#[path = "../tests/app/pipelined_rendering.rs"]
mod tests;
