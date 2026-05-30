//! Frame renderer boundary behind the render command handler.

use std::sync::{Arc, Mutex};

use super::{RenderFrameContext, RenderFrameError, RenderFrameResult};

pub struct RenderProfilerScope {
    _entered: tracing::span::EnteredSpan,
}

impl RenderProfilerScope {
    pub fn render_frame() -> Self {
        Self {
            _entered: tracing::trace_span!("FrameRenderer::render_frame").entered(),
        }
    }
}

#[derive(Default)]
pub struct FrameRenderer;

#[derive(Clone)]
pub struct FrameRendererHandle {
    inner: Arc<Mutex<FrameRenderer>>,
}

impl FrameRendererHandle {
    pub fn new(renderer: FrameRenderer) -> Self {
        Self {
            inner: Arc::new(Mutex::new(renderer)),
        }
    }

    pub fn handle_for_job(&self) -> Self {
        self.clone()
    }

    pub fn with<R>(&self, f: impl FnOnce(&FrameRenderer) -> R) -> RenderFrameResult<R> {
        let renderer = self
            .inner
            .lock()
            .map_err(|_| RenderFrameError::LockPoisoned {
                resource: "FrameRenderer",
            })?;
        Ok(f(&renderer))
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut FrameRenderer) -> R) -> RenderFrameResult<R> {
        let mut renderer = self
            .inner
            .lock()
            .map_err(|_| RenderFrameError::LockPoisoned {
                resource: "FrameRenderer",
            })?;
        Ok(f(&mut renderer))
    }

    pub fn render_frame(&self, ctx: RenderFrameContext) {
        let Ok(mut renderer) = self.inner.lock() else {
            return;
        };

        renderer.render_frame(ctx);
    }
}

impl From<FrameRenderer> for FrameRendererHandle {
    fn from(renderer: FrameRenderer) -> Self {
        Self::new(renderer)
    }
}

impl FrameRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render_frame(&mut self, ctx: RenderFrameContext) {
        let _scope = RenderProfilerScope::render_frame();
        let _frame = &ctx.frame_input;

        // RED equivalent:
        // const RenderViewport* viewport = info.GetViewport();
        //
        // LEET will resolve this through:
        // frame_target_resolver.resolve(frame.target)?.viewport()
    }
}
