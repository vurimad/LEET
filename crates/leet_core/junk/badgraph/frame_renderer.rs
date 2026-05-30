//! Frame-level execution wrapper for compiled render graphs.

use crate::render_graph::{RenderContext, RenderExecutionPlan};
use leet_core::LeetResult;
use leet_jobs2::Builder;

/// LEET analogue of RED's `SRenderFrameContext`.
///
/// This currently wraps the prepared frame state plus a small amount of
/// execution bookkeeping.
pub struct RenderFrameContext<'device> {
    builder: Builder,
    frame: RenderContext<'device>,
    dispatcher_thread_index: u32,
}

impl<'device> RenderFrameContext<'device> {
    pub fn new(builder: Builder, frame: RenderContext<'device>) -> Self {
        Self {
            builder,
            frame,
            dispatcher_thread_index: 0,
        }
    }

    pub fn with_dispatcher_thread_index(mut self, dispatcher_thread_index: u32) -> Self {
        self.dispatcher_thread_index = dispatcher_thread_index;
        self
    }

    pub fn dispatcher_thread_index(&self) -> u32 {
        self.dispatcher_thread_index
    }

    pub fn builder(&self) -> &Builder {
        &self.builder
    }

    pub fn builder_mut(&mut self) -> &mut Builder {
        &mut self.builder
    }

    pub fn frame(&self) -> &RenderContext<'device> {
        &self.frame
    }
}

/// Executes one compiled render plan against one prepared frame.
pub struct FrameRenderer;

impl Default for FrameRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameRenderer {
    pub fn new() -> Self {
        Self
    }

    pub fn render_frame(
        &self,
        plan: &RenderExecutionPlan,
        frame: RenderFrameContext<'_>,
    ) -> LeetResult<()> {
        let RenderFrameContext { frame, .. } = frame;
        plan.execute(&frame)?;
        frame.present();
        Ok(())
    }
}
