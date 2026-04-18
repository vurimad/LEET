//! LEET App - Application framework and runtime
//!
//! Provides [`App`], the top-level entry point for an LEET application.
//!
//! # Lifecycle
//!
//! ```text
//! App::run()
//!   ├─ LeetEventLoop::new()          — create the winit event loop
//!   ├─ event_loop.create_window()    — queue the main window
//!   ├─ self.acquire_main_loop()      — wire app state into the runner
//!   └─ runner.run_main_loop()        — hand control to winit (blocking)
//!          ├─ resumed()       → on_window_ready fires
//!          ├─ window_event()  → on_event fires per platform event
//!          └─ about_to_wait() → on_begin_frame → on_update → on_end_frame
//! ```

pub use leet_core::*;
pub use leet_macros::leet_main;

pub mod app;
pub mod core_systems;

pub use app::{App, AppConfig, AppRunner};
pub use core_systems::CoreSystems;

// =============================================================================
// Prelude
// =============================================================================

pub mod prelude {
    pub use crate::{leet_main, App, AppConfig, CoreSystems};
    pub use leet_core::*;
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_creation() {
        let app = App::new();
        assert_eq!(app.config.title, "LEET Game");
    }

    #[test]
    fn test_app_with_config() {
        let config = AppConfig {
            title: "My Game".to_string(),
            width: 800,
            height: 600,
        };
        let app = App::with_config(config);
        assert_eq!(app.config.title, "My Game");
        assert_eq!(app.config.width, 800);
    }
}
