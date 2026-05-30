use crate::{
    BufferUploadPlugin, CameraPlugin, ExtractionPlugin, GpuScenePlugin, RHIPlugin,
    RenderWindowPlugin, RenderingPreprocessingPlugin,
};
use bevy_app::{App, AppLabel, Plugin, SubApp};
use bevy_ecs::{
    prelude::{Mut, Res, ResMut, Resource},
    schedule::{
        IntoScheduleConfigs, Schedule, ScheduleBuildSettings, ScheduleLabel, Schedules, SystemSet,
    },
    world::World,
};
use leet_jobs2::{JobSystemConfig, LeetJobSystem};
use std::ops::{Deref, DerefMut};

/// Application label for LEET's render sub-app.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, AppLabel)]
pub struct RenderApp;

/// Schedule that extracts render-relevant data from the main world.
#[derive(ScheduleLabel, PartialEq, Eq, Debug, Clone, Hash, Default)]
pub struct ExtractSchedule;

/// Main render schedule for the LEET render sub-app.
#[derive(ScheduleLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct Render;

impl Render {
    pub fn base_schedule() -> Schedule {
        let mut schedule = Schedule::new(Self);
        schedule.configure_sets(
            (
                RenderSystems::ClaimFlushThread,
                RenderSystems::ExtractCommands,
                RenderSystems::Prepare,
                RenderSystems::Render,
                RenderSystems::Cleanup,
            )
                .chain(),
        );
        schedule
    }
}

/// Ordered system sets for the minimal LEET render schedule.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum RenderSystems {
    /// Claims job-system flush ownership before any render work can flush counters.
    ClaimFlushThread,
    /// Applies deferred commands recorded during extraction.
    ExtractCommands,
    /// Prepares renderer-owned frame state such as window surfaces.
    Prepare,
    /// Runs the actual render work.
    Render,
    /// Runs after render work for end-of-frame cleanup.
    Cleanup,
}

/// Main-world access resource made available to the extract schedule.
#[derive(Resource, Default)]
pub struct MainWorld(World);

impl Deref for MainWorld {
    type Target = World;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MainWorld {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl MainWorld {
    #[cfg(test)]
    pub(crate) fn from_world(world: World) -> Self {
        Self(world)
    }
}

/// Scratch world used to swap the main world into the render app during extraction.
#[derive(Resource, Default)]
struct ScratchMainWorld(World);

/// Tracks whether this render world has claimed its job-system flush thread.
#[derive(Resource, Default)]
struct JobSystemFlushThreadClaim {
    claimed: bool,
}

/// Shuts down the render-world job system when the render world is dropped.
///
/// `LeetJobSystem` handles are intentionally cheap clones and do not stop worker
/// threads on drop. This private guard keeps teardown attached to the render
/// world's lifetime while still letting systems use `Res<LeetJobSystem>`
/// directly.
#[derive(Resource)]
struct JobSystemShutdownGuard(LeetJobSystem);

impl Drop for JobSystemShutdownGuard {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// Installs the LEET job system into the app that runs the render schedule.
///
/// `RenderAppPlugin` uses this for the render sub-app. The plugin is public so
/// tests and tools can build the same resource layout without also installing
/// the full renderer stack.
#[derive(Clone, Copy, Default)]
pub struct JobPlugin;

impl Plugin for JobPlugin {
    fn build(&self, app: &mut App) {
        let job_system = LeetJobSystem::new(JobSystemConfig::default());

        app.insert_resource(job_system.clone())
            .insert_resource(JobSystemShutdownGuard(job_system))
            .init_resource::<JobSystemFlushThreadClaim>()
            .add_systems(
                Render,
                claim_job_system_flush_thread_once.in_set(RenderSystems::ClaimFlushThread),
            );
    }
}

/// Minimal render plugin that creates a render sub-app plus extract/render schedules.
#[derive(Default)]
pub struct RenderAppPlugin;

impl Plugin for RenderAppPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScratchMainWorld>();

        let mut render_app = SubApp::new();

        let mut extract_schedule = Schedule::new(ExtractSchedule);
        extract_schedule.set_build_settings(ScheduleBuildSettings {
            // Extraction should only record commands; they are applied later in render order.
            auto_insert_apply_deferred: false,
            ..Default::default()
        });
        extract_schedule.set_apply_final_deferred(false);

        render_app
            .add_schedule(Render::base_schedule())
            .add_schedule(extract_schedule)
            .allow_ambiguous_resource::<MainWorld>();
        render_app.add_plugins(JobPlugin);
        render_app.add_systems(
            Render,
            apply_extract_commands.in_set(RenderSystems::ExtractCommands),
        );
        render_app.update_schedule = Some(Render.intern());
        render_app.set_extract(extract);

        app.insert_sub_app(RenderApp, render_app);
        app.add_plugins(GpuScenePlugin);
        app.add_plugins(RHIPlugin);
        app.add_plugins(RenderWindowPlugin);
        app.add_plugins(ExtractionPlugin);
        app.add_plugins(BufferUploadPlugin);
        app.add_plugins(RenderingPreprocessingPlugin);
        app.add_plugins(CameraPlugin);
    }
}

fn claim_job_system_flush_thread_once(
    job_system: Res<LeetJobSystem>,
    mut claim: ResMut<JobSystemFlushThreadClaim>,
) {
    if claim.claimed {
        return;
    }

    job_system.claim_flush_thread();
    claim.claimed = true;
}

fn apply_extract_commands(render_world: &mut World) {
    render_world.resource_scope(|render_world, mut schedules: Mut<Schedules>| {
        schedules
            .get_mut(ExtractSchedule)
            .expect("LEET extract schedule missing")
            .apply_deferred(render_world);
    });
}

pub fn extract(main_world: &mut World, render_world: &mut World) {
    let scratch_world = main_world
        .remove_resource::<ScratchMainWorld>()
        .expect("LEET scratch main world missing");
    let inserted_world = std::mem::replace(main_world, scratch_world.0);

    render_world.insert_resource(MainWorld(inserted_world));
    render_world.run_schedule(ExtractSchedule);

    let inserted_world = render_world
        .remove_resource::<MainWorld>()
        .expect("LEET main world resource missing");
    let scratch_world = std::mem::replace(main_world, inserted_world.0);
    main_world.insert_resource(ScratchMainWorld(scratch_world));
}

#[cfg(test)]
#[path = "../tests/app/plugin.rs"]
mod tests;
