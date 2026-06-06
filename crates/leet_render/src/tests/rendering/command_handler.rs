use std::sync::{Arc, Mutex};

use bevy_math::UVec2;
use leet_jobs2::JobSystemConfig;

use crate::{
    FrameCaptureIntent, FrameDebugIntent, FrameGpuScene, FrameOutput, FramePurpose,
    FrameRenderingMode, FrameTiming, PresentationIntent, RenderViewport,
};

use super::*;

fn job_system() -> LeetJobSystem {
    let jobs = LeetJobSystem::new(JobSystemConfig {
        max_threads: 2,
        ..JobSystemConfig::default()
    });
    jobs.claim_flush_thread();
    jobs
}

fn blank_frame(width: u32, height: u32) -> FrameInput {
    let viewport = RenderViewport::targetless(
        UVec2::new(width, height),
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );

    FrameInput {
        viewport,
        output: FrameOutput::Targetless,
        cameras: Vec::new(),
        scene: FrameGpuScene::empty(),
        timing: FrameTiming {
            frame_index: 1,
            ..FrameTiming::default()
        },
        mode: FrameRenderingMode::Blank,
        purpose: FramePurpose::Blank,
        presentation: PresentationIntent::NoPresent,
        capture: FrameCaptureIntent::None,
        debug: FrameDebugIntent::default(),
    }
}

#[test]
fn render_scene_flushes_queued_commands_before_render_frame_job() {
    let jobs = job_system();
    let mut handler = RenderCommandHandler::new(jobs.clone(), FrameRenderer::new());
    let calls = Arc::new(Mutex::new(Vec::new()));

    let command_calls = Arc::clone(&calls);
    handler
        .commit_command(RenderCommand::new(
            "test command",
            RenderCommandQueueKind::InOrder,
            move |_| {
                command_calls.lock().unwrap().push("command");
            },
        ))
        .unwrap();

    handler.render_scene(blank_frame(64, 32)).unwrap();
    handler.sync_with_render_commands().unwrap();
    calls.lock().unwrap().push("after sync");

    assert_eq!(calls.lock().unwrap().as_slice(), ["command", "after sync"]);

    jobs.shutdown();
}

#[test]
fn render_scene_syncs_frame_job_without_error_bridge() {
    let jobs = job_system();
    let mut handler = RenderCommandHandler::new(jobs.clone(), FrameRenderer::new());

    handler.render_scene(blank_frame(0, 32)).unwrap();
    handler.sync_with_render_commands().unwrap();

    jobs.shutdown();
}
