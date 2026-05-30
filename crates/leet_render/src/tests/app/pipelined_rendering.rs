use super::{PipelinedRenderingPlugin, RenderAppChannels, RenderExtractApp};
use crate::{Render, RenderApp, RenderAppPlugin, RenderSystems};
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
    app.add_plugins((RenderAppPlugin, PipelinedRenderingPlugin));

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
    app.add_plugins((RenderAppPlugin, PipelinedRenderingPlugin));
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
