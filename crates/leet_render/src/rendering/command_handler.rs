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
#[path = "../tests/rendering/command_handler.rs"]
mod tests;
