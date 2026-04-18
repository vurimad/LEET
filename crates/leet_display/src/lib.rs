//! LEET Display - Windowing and display management
//!
//! This crate handles window creation and management using winit.
//!
//! # Architecture
//!
//! The display system keeps window creation and event handling fully decoupled:
//!
//! - **Event System** ([`event`]): [`LeetEventLoop`] is the single point of control.
//!   - Creates and manages OS windows (returns [`LeetWindowId`])
//!   - Converts raw platform events into [`LeetEvent`] values
//!   - Call `pump_events()` each frame to collect events; no blocking
//!
//! - **Window Handle** ([`window`]): [`LeetWindow`] is a thin, clonable wrapper.
//!   - Holds only an `Arc<WinitWindow>` — no event loop, no state
//!   - Exposes: `request_redraw`, `inner_size`, `set_title`, `raw`, etc.
//!   - Obtained from [`LeetEventLoop::get_window`]
//!
//! - **Event Handler Trait** ([`event::LeetRunLoop`]): implement this to drive your
//!   app from inside the winit loop. Callbacks: `on_window_ready`, `on_event`,
//!   `on_update`, `should_exit`.
//!
//! # Quick Start
//!
//! ```no_run
//! use leet_display::{LeetEventLoop, LeetEvent, LeetRunLoop, LeetWindow, LeetWindowId, WindowConfig, WindowEvent};
//!
//! struct MyRunner { running: bool, window: Option<LeetWindow> }
//!
//! impl LeetRunLoop for MyRunner {
//!     fn on_window_ready(&mut self, _id: LeetWindowId, window: LeetWindow) {
//!         self.window = Some(window);
//!     }
//!     fn on_event(&mut self, event: LeetEvent) {
//!         if let LeetEvent::Window(WindowEvent::CloseRequested { .. }) = event {
//!             self.running = false;
//!         }
//!     }
//!     fn on_update(&mut self) { /* tick */ }
//!     fn should_exit(&self) -> bool { !self.running }
//! }
//!
//! fn main() -> leet_core::LeetResult<()> {
//!     let mut event_loop = LeetEventLoop::new()?;
//!     let _win = event_loop.create_window(WindowConfig::default());
//!     event_loop.run_blocking(MyRunner { running: true, window: None })
//! }
//! ```

pub mod event;
pub mod window;

pub use event::{
    InputEvent, LeetEvent, LeetEventLoop, LeetRunLoop, LeetWindowId, WindowConfig, WindowEvent,
};
pub use window::LeetWindow;
