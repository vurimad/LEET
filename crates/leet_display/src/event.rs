use crate::window::LeetWindow;
use leet_core::{Leeror, LeetResult};
use leet_log::{info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent as WinitWindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window as WinitWindow, WindowAttributes, WindowId};

// =============================================================================
// Identifiers
// =============================================================================

/// Stable, opaque identifier for a LEET window.
/// Assigned at `create_window` time, before the OS window actually exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LeetWindowId(u64);

// =============================================================================
// Platform-agnostic event types
// =============================================================================

#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// The OS window is alive and ready to use. A [`LeetWindow`] handle is provided.
    Created {
        id: LeetWindowId,
    },
    /// User (or OS) asked the window to close.
    CloseRequested {
        id: LeetWindowId,
    },
    Resized {
        id: LeetWindowId,
        width: u32,
        height: u32,
    },
    Focused {
        id: LeetWindowId,
    },
    Unfocused {
        id: LeetWindowId,
    },
    Moved {
        id: LeetWindowId,
        x: i32,
        y: i32,
    },
}

#[derive(Debug, Clone)]
pub enum InputEvent {
    KeyPressed { key: String, scancode: u32 },
    KeyReleased { key: String, scancode: u32 },
    MouseButtonPressed { button: u32 },
    MouseButtonReleased { button: u32 },
    MouseMoved { x: f64, y: f64 },
    MouseWheel { delta_x: f32, delta_y: f32 },
}

#[derive(Debug, Clone)]
pub enum LeetEvent {
    Window(WindowEvent),
    Input(InputEvent),
    Exit,
}

// =============================================================================
// Window configuration
// =============================================================================

#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "LEET Application".to_string(),
            width: 1280,
            height: 720,
            resizable: true,
        }
    }
}

// =============================================================================
// LeetRunLoop — the trait that drives the engine from inside winit
// =============================================================================

/// Implement this trait to receive engine callbacks driven by the winit event loop.
///
/// Per-frame call order (every iteration of `about_to_wait`):
/// ```text
/// on_window_ready  — once, when the OS window is created
/// on_event         — once per queued platform event (before the frame tick)
/// ┌─ on_begin_frame
/// │  on_update
/// └─ on_end_frame
/// should_exit      — sampled after on_end_frame; true → loop exits
/// ```
pub trait LeetRunLoop {
    /// The OS window is ready. Store the [`LeetWindow`] handle here
    /// (e.g. pass it to the renderer for surface creation).
    fn on_window_ready(&mut self, _id: LeetWindowId, _window: LeetWindow) {}

    /// Receive a translated [`LeetEvent`].
    /// Called once per event, *before* the frame tick begins.
    fn on_event(&mut self, event: LeetEvent);

    /// Called at the very start of each frame tick, before `on_update`.
    /// Use this to reset per-frame state (e.g. clear input deltas, start timers).
    fn on_begin_frame(&mut self) {}

    /// Per-frame update tick.
    fn on_update(&mut self);

    /// Called at the very end of each frame tick, after `on_update`.
    /// Use this to flush queued commands, submit render work, end profiler scopes, etc.
    fn on_end_frame(&mut self) {}

    /// Return `true` to stop the loop cleanly.
    fn should_exit(&self) -> bool;
}

// =============================================================================
// LeetEventLoop — public API
// =============================================================================

/// Owns the winit [`EventLoop`] and the list of windows to be created.
///
/// Usage:
/// ```no_run
/// let mut event_loop = LeetEventLoop::new()?;
/// let win_id = event_loop.create_window(WindowConfig::default());
/// event_loop.run_blocking(my_runner); // blocks until exit
/// ```
pub struct LeetEventLoop {
    event_loop: EventLoop<()>,
    /// Windows queued before `run_blocking` is called.
    pending_windows: Vec<(LeetWindowId, WindowConfig)>,
    next_id: u64,
}

impl LeetEventLoop {
    /// Create a new event loop. Configures winit for `Poll` mode so our
    /// `about_to_wait` callback fires every frame.
    pub fn new() -> LeetResult<Self> {
        let event_loop = EventLoop::new()
            .map_err(|e| Leeror::Init(format!("Failed to create event loop: {}", e)))?;

        event_loop.set_control_flow(ControlFlow::Poll);

        info!("[LEET Display] Event loop created");

        Ok(Self {
            event_loop,
            pending_windows: Vec::new(),
            next_id: 0,
        })
    }

    /// Queue a window to be created. Returns its stable [`LeetWindowId`] immediately,
    /// before the OS window exists. The window will be materialised inside
    /// `run_blocking` on the first `resumed` callback.
    pub fn create_window(&mut self, config: WindowConfig) -> LeetWindowId {
        let id = LeetWindowId(self.next_id);
        self.next_id += 1;
        info!("[LEET Display] Window creation queued: {:?}", id);
        self.pending_windows.push((id, config));
        id
    }

    /// Consume the event loop and run until the runner signals exit.
    ///
    /// This is **blocking** — winit owns the thread from this point on.
    /// All engine callbacks are driven from inside the loop.
    pub fn run_blocking(self, runner: impl LeetRunLoop) -> LeetResult<()> {
        let mut adapter = RunLoopAdapter {
            runner,
            windows: HashMap::new(),
            pending_windows: self.pending_windows,
        };

        self.event_loop
            .run_app(&mut adapter)
            .map_err(|e| Leeror::Runtime(format!("Event loop exited with error: {}", e)))
    }
}

// =============================================================================
// RunLoopAdapter — private winit ApplicationHandler
// =============================================================================

/// Bridges winit's `ApplicationHandler` callbacks to the `LeetRunLoop` trait.
/// Lives entirely inside `run_blocking`; never exposed publicly.
struct RunLoopAdapter<R: LeetRunLoop> {
    runner: R,
    /// Live winit windows, keyed by winit's own `WindowId`.
    windows: HashMap<WindowId, WindowEntry>,
    /// Windows waiting to be created on the first `resumed`.
    pending_windows: Vec<(LeetWindowId, WindowConfig)>,
}

struct WindowEntry {
    leet_id: LeetWindowId,
    handle: Arc<WinitWindow>,
}

impl<R: LeetRunLoop> RunLoopAdapter<R> {
    fn create_pending_windows(&mut self, event_loop: &ActiveEventLoop) {
        for (leet_id, config) in self.pending_windows.drain(..) {
            let attributes = WindowAttributes::default()
                .with_title(&config.title)
                .with_inner_size(winit::dpi::LogicalSize::new(config.width, config.height))
                .with_resizable(config.resizable);

            match event_loop.create_window(attributes) {
                Ok(window) => {
                    info!(
                        "[LEET Display] Window created: {}x{} \"{}\" ({:?})",
                        config.width, config.height, config.title, leet_id
                    );
                    let winit_id = window.id();
                    let arc = Arc::new(window);

                    self.windows.insert(
                        winit_id,
                        WindowEntry {
                            leet_id,
                            handle: arc.clone(),
                        },
                    );

                    // Notify the runner that the window handle is ready.
                    let leet_window = LeetWindow::new(leet_id, arc);
                    self.runner.on_window_ready(leet_id, leet_window.clone());
                    self.runner
                        .on_event(LeetEvent::Window(WindowEvent::Created { id: leet_id }));
                }
                Err(e) => {
                    warn!("[LEET Display] Failed to create window: {}", e);
                }
            }
        }
    }
}

impl<R: LeetRunLoop> ApplicationHandler for RunLoopAdapter<R> {
    /// winit fires `resumed` on startup (desktop) and on app-resume (mobile).
    /// This is where we materialise any queued windows.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.create_pending_windows(event_loop);
    }

    /// Called every frame when no OS events are pending — our main tick.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.runner.on_begin_frame();
        self.runner.on_update();
        self.runner.on_end_frame();

        // Check for clean exit.
        if self.runner.should_exit() {
            info!("[LEET Display] Exit requested, stopping event loop.");
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WinitWindowEvent,
    ) {
        let Some(entry) = self.windows.get(&window_id) else {
            return;
        };
        let leet_id = entry.leet_id;

        match event {
            WinitWindowEvent::CloseRequested => {
                info!("[LEET Display] Close requested: {:?}", leet_id);
                self.runner
                    .on_event(LeetEvent::Window(WindowEvent::CloseRequested {
                        id: leet_id,
                    }));
                self.windows.remove(&window_id);

                // All windows gone: exit the loop. Don't emit LeetEvent::Exit here —
                // the runner already received CloseRequested and can act on it.
                // LeetEvent::Exit is reserved for platform-forced quits (no prior CloseRequested).
                if self.windows.is_empty() {
                    event_loop.exit();
                }
            }

            WinitWindowEvent::Resized(size) => {
                self.runner
                    .on_event(LeetEvent::Window(WindowEvent::Resized {
                        id: leet_id,
                        width: size.width,
                        height: size.height,
                    }));
            }

            WinitWindowEvent::Focused(focused) => {
                let ev = if focused {
                    WindowEvent::Focused { id: leet_id }
                } else {
                    WindowEvent::Unfocused { id: leet_id }
                };
                self.runner.on_event(LeetEvent::Window(ev));
            }

            WinitWindowEvent::Moved(pos) => {
                self.runner.on_event(LeetEvent::Window(WindowEvent::Moved {
                    id: leet_id,
                    x: pos.x,
                    y: pos.y,
                }));
            }

            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        _event: DeviceEvent,
    ) {
        // Future: translate raw device events to InputEvent and call runner.on_event
    }
}
