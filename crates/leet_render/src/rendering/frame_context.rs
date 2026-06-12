//! Render-frame job context.

use leet_jobs2::{Builder, RunContext};

use super::FrameInput;

pub type RenderJobBuilder = Builder;

pub struct RenderFrameContext {
    pub builder: RenderJobBuilder,
    pub frame_input: FrameInput,
    pub dispatcher_thread_index: u32,
}

impl RenderFrameContext {
    pub fn construct(run_context: &RunContext, frame_input: FrameInput) -> Self {
        Self {
            builder: run_context.create_builder(),
            frame_input,
            dispatcher_thread_index: run_context.thread_index,
        }
    }
}
