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
mod tests {
    use super::{PipelinedRenderingPlugin, RenderAppChannels, RenderExtractApp};
    use crate::{Render, RenderApp, RenderPlugin, RenderSystems};
    use bevy_app::App;
    use bevy_ecs::prelude::{ResMut, Resource};
    use bevy_ecs::schedule::IntoScheduleConfigs;
    use leet_jobs2::LeetJobSystem;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn pipelined_plugin_detaches_render_app_during_cleanup() {
        let mut app = App::new();
        app.add_plugins((RenderPlugin, PipelinedRenderingPlugin));

        app.finish();
        app.cleanup();

        assert!(
            app.get_sub_app(RenderExtractApp).is_some(),
            "threaded mode should install the render extract sub-app"
        );
        assert!(
            app.get_sub_app(RenderApp).is_none(),
            "threaded mode should detach the render app from the main app after cleanup"
        );
        assert!(
            app.world().contains_resource::<RenderAppChannels>(),
            "threaded mode should leave behind render app channels for extract handoff"
        );
    }

    #[derive(Resource)]
    struct FlushThreadProbe(Option<mpsc::Sender<Option<u32>>>);

    fn send_flush_thread_probe(mut probe: ResMut<FlushThreadProbe>) {
        if let Some(tx) = probe.0.take() {
            tx.send(LeetJobSystem::current_thread_index()).unwrap();
        }
    }

    #[test]
    fn pipelined_render_update_claims_flush_thread_on_render_thread() {
        let mut app = App::new();
        app.add_plugins((RenderPlugin, PipelinedRenderingPlugin));
        let (tx, rx) = mpsc::channel();

        {
            let render_app = app
                .get_sub_app_mut(RenderApp)
                .expect("LEET render sub-app missing");
            render_app
                .world_mut()
                .insert_resource(FlushThreadProbe(Some(tx)));
            render_app.add_systems(
                Render,
                send_flush_thread_probe.in_set(RenderSystems::Prepare),
            );
        }

        app.finish();
        app.cleanup();

        let deadline = Instant::now() + Duration::from_secs(2);
        let observed = loop {
            match rx.try_recv() {
                Ok(index) => break index,
                Err(mpsc::TryRecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for render-thread flush claim"
                    );
                    app.update();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("render-thread flush probe disconnected");
                }
            }
        };

        assert_eq!(observed, Some(0));
    }
}
