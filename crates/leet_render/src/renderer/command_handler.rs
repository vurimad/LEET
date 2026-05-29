//! RED-style render command handler boundary.

use std::{collections::VecDeque, sync::Mutex};

use bevy_ecs::prelude::Resource;
use leet_jobs2::{Counter, LeetJobSystem, Priority};

use super::{
    FrameInput, FrameRenderer, FrameRendererHandle, RenderFrameContext, RenderFrameError,
    RenderFrameResult, RenderJobBuilder,
};

#[derive(Resource)]
pub struct RenderCommandHandler {
    jobs: LeetJobSystem,
    renderer: FrameRendererHandle,
    frame_flush_counter: Counter,
    command_queues: Mutex<RenderCommandQueues>,
}

impl RenderCommandHandler {
    pub fn new(jobs: LeetJobSystem, renderer: FrameRenderer) -> Self {
        let frame_flush_counter = jobs.create_counter(Priority::RenderPath);
        Self {
            jobs,
            renderer: FrameRendererHandle::new(renderer),
            frame_flush_counter,
            command_queues: Mutex::new(RenderCommandQueues::default()),
        }
    }

    pub fn renderer(&self) -> &FrameRendererHandle {
        &self.renderer
    }

    pub fn render_scene(&mut self, frame_input: FrameInput) -> RenderFrameResult<()> {
        let mut builder = self.begin_render_scene_builder()?;
        self.dispatch_render_frame_job(&mut builder, frame_input);

        self.frame_flush_counter = builder.extract_wait_counter();
        Ok(())
    }

    pub fn flush_previous_frame_commands_processing(&self) {
        self.jobs
            .flush_counter_render_frame(&self.frame_flush_counter);
    }

    pub fn sync_with_render_commands(&mut self) -> RenderFrameResult<()> {
        let mut builder = self.jobs.create_builder(Priority::RenderPath);
        builder.dispatch_wait(&self.frame_flush_counter);
        self.dispatch_queued_render_commands(&mut builder)?;
        self.frame_flush_counter = builder.extract_wait_counter();
        self.jobs
            .flush_counter_render_frame(&self.frame_flush_counter);
        Ok(())
    }

    pub fn commit_command(&self, command: RenderCommand) -> RenderFrameResult<()> {
        let mut queues =
            self.command_queues
                .lock()
                .map_err(|_| RenderFrameError::InvalidFrameInput {
                    reason: "render command queue lock was poisoned",
                })?;
        queues.in_order.push_back(command);
        Ok(())
    }

    fn dispatch_queued_render_commands(
        &self,
        builder: &mut RenderJobBuilder,
    ) -> RenderFrameResult<()> {
        let commands = {
            let mut queues =
                self.command_queues
                    .lock()
                    .map_err(|_| RenderFrameError::InvalidFrameInput {
                        reason: "render command queue lock was poisoned",
                    })?;
            queues.take_in_order()
        };

        if !commands.is_empty() {
            builder.dispatch_job("RenderScene/FlushInOrderCommands", move |run_context| {
                for command in commands {
                    command.execute(run_context);
                }
            });
        }

        Ok(())
    }

    fn begin_render_scene_builder(&self) -> RenderFrameResult<RenderJobBuilder> {
        let mut builder = self.jobs.create_builder(Priority::RenderPath);
        builder.dispatch_wait(&self.frame_flush_counter);
        self.dispatch_queued_render_commands(&mut builder)?;
        Ok(builder)
    }

    fn dispatch_render_frame_job(&self, builder: &mut RenderJobBuilder, frame_input: FrameInput) {
        let renderer = self.renderer.handle_for_job();

        builder.dispatch_job("RenderScene/RenderFrame", move |run_context| {
            let ctx = RenderFrameContext::construct(run_context, frame_input);
            renderer.render_frame(ctx);
        });
    }
}

#[derive(Default)]
struct RenderCommandQueues {
    in_order: VecDeque<RenderCommand>,
}

impl RenderCommandQueues {
    fn take_in_order(&mut self) -> Vec<RenderCommand> {
        self.in_order.drain(..).collect()
    }
}

pub struct RenderCommand {
    pub debug_name: &'static str,
    pub queue_kind: RenderCommandQueueKind,
    execute: Box<dyn FnOnce(&leet_jobs2::RunContext) + Send + 'static>,
}

impl RenderCommand {
    pub fn new(
        debug_name: &'static str,
        queue_kind: RenderCommandQueueKind,
        execute: impl FnOnce(&leet_jobs2::RunContext) + Send + 'static,
    ) -> Self {
        Self {
            debug_name,
            queue_kind,
            execute: Box::new(execute),
        }
    }

    fn execute(self, run_context: &leet_jobs2::RunContext) {
        (self.execute)(run_context);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderCommandQueueKind {
    InOrder,
    ProxyParallel {
        proxy_id: u64,
        safety: RenderCommandSafety,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderCommandSafety {
    NotSafeWithProxyAddRemove,
    SafeWithProxyAddRemove,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use bevy_math::UVec2;
    use leet_jobs2::JobSystemConfig;

    use crate::{
        FrameCaptureIntent, FrameDebugIntent, FramePurpose, FrameRenderingMode, FrameTarget,
        FrameTargetKey, FrameTiming, PresentationIntent, RenderSceneId,
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
        FrameInput {
            target: FrameTarget {
                key: FrameTargetKey::External(1),
                extent: UVec2::new(width, height),
                format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            },
            camera_views: Vec::new(),
            scene: RenderSceneId(1),
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
}
