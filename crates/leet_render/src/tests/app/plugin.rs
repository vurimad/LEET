use super::{ExtractSchedule, JobPlugin, MainWorld, Render, RenderApp, RenderAppPlugin};
use bevy_app::App;
use bevy_ecs::prelude::{Res, ResMut, Resource};
use bevy_transform::TransformPlugin as BevyTransformPlugin;
use leet_jobs2::LeetJobSystem;

#[derive(Resource)]
struct SourceValue(u32);

#[derive(Resource, Default)]
struct ExtractedValue(u32);

#[test]
fn job_plugin_installs_job_system_resource() {
    let mut app = App::new();
    app.add_schedule(Render::base_schedule());
    app.add_plugins(JobPlugin);

    assert!(app.world().contains_resource::<LeetJobSystem>());
}

#[test]
fn extract_schedule_can_read_main_world_resources() {
    let mut app = App::new();
    app.insert_resource(SourceValue(7));
    app.add_plugins(RenderAppPlugin);

    {
        let render_app = app
            .get_sub_app_mut(RenderApp)
            .expect("LEET render sub-app missing");
        render_app.init_resource::<ExtractedValue>();
        render_app.add_systems(
            ExtractSchedule,
            |main_world: Res<MainWorld>, mut extracted: ResMut<ExtractedValue>| {
                extracted.0 = main_world.resource::<SourceValue>().0;
            },
        );
    }

    app.update();

    let render_app = app
        .get_sub_app(RenderApp)
        .expect("LEET render sub-app missing");
    assert_eq!(render_app.world().resource::<ExtractedValue>().0, 7);
}

#[test]
fn plugin_installs_transform_foundation() {
    let mut app = App::new();
    app.add_plugins(RenderAppPlugin);

    assert!(app.is_plugin_added::<BevyTransformPlugin>());
}

#[test]
fn plugin_installs_job_system_in_render_world() {
    let mut app = App::new();
    app.add_plugins(RenderAppPlugin);

    let render_app = app
        .get_sub_app(RenderApp)
        .expect("LEET render sub-app missing");

    assert!(render_app.world().contains_resource::<LeetJobSystem>());
}

#[test]
fn render_schedule_claims_job_system_flush_thread_once() {
    let mut app = App::new();
    app.add_plugins(RenderAppPlugin);

    app.update();
    app.update();

    assert_eq!(LeetJobSystem::current_thread_index(), Some(0));
    drop(app);
    assert_eq!(LeetJobSystem::current_thread_index(), None);
}

#[test]
fn dropping_render_app_shuts_down_job_system() {
    let jobs = {
        let mut app = App::new();
        app.add_plugins(RenderAppPlugin);
        app.get_sub_app(RenderApp)
            .expect("LEET render sub-app missing")
            .world()
            .resource::<LeetJobSystem>()
            .clone()
    };

    assert_eq!(jobs.num_worker_threads(), 0);
}
