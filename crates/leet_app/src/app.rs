//! [`App`], [`AppConfig`], [`AppRunner`], and the internal [`AppLoopRunner`].

use crate::core_systems::CoreSystems;
use leet_core::EngineClock;
use leet_display::{
    LeetEvent, LeetEventLoop, LeetRunLoop, LeetWindow, LeetWindowId, WindowConfig, WindowEvent,
};
use leet_log::info;

// =============================================================================
// AppConfig
// =============================================================================

/// High-level application configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "LEET Game".to_string(),
            width: 1280,
            height: 720,
        }
    }
}

// =============================================================================
// App
// =============================================================================

/// The main application structure. Build it, optionally configure it, then
/// call [`App::run`].
pub struct App {
    pub(crate) config: AppConfig,
    core_systems: Option<CoreSystems>,
}

impl App {
    /// Preform the system-level initialization that must happen before the event loop starts.
    fn inner_initialization(&mut self) {
        EngineClock::reset();
        self.core_systems =
            Some(CoreSystems::init().expect("[LEET] Failed to initialize core systems"));
    }

    /// Create with default [`AppConfig`].
    pub fn new() -> Self {
        Self::with_config(AppConfig::default())
    }

    /// Create with a custom [`AppConfig`].
    pub fn with_config(config: AppConfig) -> Self {
        let mut app = Self {
            config,
            core_systems: None,
        };
        leet_log::init();
        info!("[LEET] Initializing: \"{}\"", app.config.title);

        app.inner_initialization();
        app
    }

    /// Start the application.
    ///
    /// 1. Creates the [`LeetEventLoop`].
    /// 2. Queues the main window.
    /// 3. Calls [`Self::acquire_main_loop`] to produce an [`AppRunner`].
    /// 4. Calls [`AppRunner::run_main_loop`] — this is **blocking**.
    pub fn run(self) {
        let mut event_loop = LeetEventLoop::new().expect("[LEET] Failed to create event loop");

        let main_window_id = event_loop.create_window(WindowConfig {
            title: self.config.title.clone(),
            width: self.config.width,
            height: self.config.height,
            resizable: true,
        });

        let runner = self.acquire_main_loop(event_loop, main_window_id);
        runner.run_main_loop();
    }

    /// Wire `self` into the event loop and return a ready-to-run [`AppRunner`].
    ///
    /// This is explicit so that future code can configure the runner before
    /// `run_main_loop` hands control to winit.
    fn acquire_main_loop(
        self,
        event_loop: LeetEventLoop,
        main_window_id: LeetWindowId,
    ) -> AppRunner {
        info!("[LEET] Acquiring main event loop");
        AppRunner {
            event_loop,
            inner: AppLoopRunner {
                _config: self.config,
                main_window_id,
                main_window: None,
                core_systems: self.core_systems.expect(
                    "[LEET] Core systems should be initialized before acquiring the main loop",
                ),
                running: true,
            },
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// AppRunner
// =============================================================================

/// Holds the event loop and app state together, ready to run.
///
/// Returned by [`App::acquire_main_loop`]. Call [`AppRunner::run_main_loop`] to start.
pub struct AppRunner {
    event_loop: LeetEventLoop,
    inner: AppLoopRunner,
}

impl AppRunner {
    /// Consume the runner and start the blocking winit event loop.
    ///
    /// Returns only after the application exits (e.g. the main window is closed).
    pub fn run_main_loop(self) {
        info!("[LEET] Starting main loop");
        let AppRunner { event_loop, inner } = self;
        event_loop
            .run_blocking(inner)
            .expect("[LEET] Event loop exited with an error");
    }
}

// =============================================================================
// AppLoopRunner — implements LeetRunLoop
// =============================================================================

/// The internal runner that lives inside the winit event loop and implements
/// the per-frame engine callbacks.
struct AppLoopRunner {
    _config: AppConfig,
    main_window_id: LeetWindowId,
    /// Set as soon as the OS window is ready.
    main_window: Option<LeetWindow>,
    core_systems: CoreSystems,
    running: bool,
}

impl LeetRunLoop for AppLoopRunner {
    fn on_window_ready(&mut self, id: LeetWindowId, window: LeetWindow) {
        if id == self.main_window_id {
            info!("[LEET] Main window ready: {:?}", id);
            self.core_systems
                .renderer
                .create_viewport(window.raw_arc(), window.inner_size())
                .expect("[LEET] Failed to create main viewport");
            self.main_window = Some(window);
        }
    }

    fn on_event(&mut self, event: LeetEvent) {
        match event {
            LeetEvent::Window(WindowEvent::CloseRequested { .. }) | LeetEvent::Exit => {
                info!("[LEET] Shutdown requested — stopping main loop.");
                self.running = false;
            }

            LeetEvent::Window(WindowEvent::Resized { width, height, .. }) => {
                info!("[LEET] Window resized to {}x{}", width, height);
                self.core_systems.renderer.resize(width, height);
            }

            _ => {}
        }
    }

    fn on_begin_frame(&mut self) {
        EngineClock::advance();
        // TODO: reset per-frame state (input deltas, profiler scope start, etc.)
    }

    fn on_update(&mut self) {
        // TODO: tick ECS systems, process input, step physics, etc.
    }

    fn on_end_frame(&mut self) {
        self.core_systems
            .sync_worlds_to_renderer()
            .expect("[LEET] Failed to sync ECS worlds to renderer");
        self.core_systems
            .renderer
            .dispatch_general_rendering()
            .expect("[LEET] Failed to dispatch general rendering");
    }

    fn should_exit(&self) -> bool {
        !self.running
    }
}
