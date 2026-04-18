//! Window handle module.
//!
//! A [`LeetWindow`] is a thin, clonable handle to an OS window.
//! It holds no event loop and performs no event processing.
//!
//! Windows are created via [`crate::event::LeetEventLoop::create_window`] and
//! retrieved after creation with [`crate::event::LeetEventLoop::get_window`].

use crate::event::LeetWindowId;
use std::sync::Arc;
use winit::window::Window as WinitWindow;

/// A handle to an OS window.
///
/// Wraps an [`Arc`] so it can be cloned and shared freely (e.g. with the renderer).
/// All heavy lifting — event loop, creation, destruction — lives in [`crate::event::LeetEventLoop`].
#[derive(Clone)]
pub struct LeetWindow {
    id: LeetWindowId,
    inner: Arc<WinitWindow>,
}

impl LeetWindow {
    /// Construct a handle. Called internally by [`crate::event::LeetEventLoop`].
    pub(crate) fn new(id: LeetWindowId, inner: Arc<WinitWindow>) -> Self {
        Self { id, inner }
    }

    /// The LEET window identifier.
    pub fn id(&self) -> LeetWindowId {
        self.id
    }

    /// Request a redraw on the next frame.
    pub fn request_redraw(&self) {
        self.inner.request_redraw();
    }

    /// Current inner size in physical pixels.
    pub fn inner_size(&self) -> (u32, u32) {
        let s = self.inner.inner_size();
        (s.width, s.height)
    }

    /// Current inner size as logical pixels at the given scale factor.
    pub fn logical_size(&self) -> (f64, f64) {
        let scale = self.inner.scale_factor();
        let (w, h) = self.inner_size();
        (w as f64 / scale, h as f64 / scale)
    }

    /// Change the window title.
    pub fn set_title(&self, title: &str) {
        self.inner.set_title(title);
    }

    /// DPI scale factor of the monitor the window is on.
    pub fn scale_factor(&self) -> f64 {
        self.inner.scale_factor()
    }

    /// Raw winit window, needed for renderer surface creation.
    pub fn raw(&self) -> &WinitWindow {
        &self.inner
    }

    /// `Arc` clone of the raw winit window (if the renderer wants shared ownership).
    pub fn raw_arc(&self) -> Arc<WinitWindow> {
        self.inner.clone()
    }
}
