//! # LEET Game Engine
//!
//! LEET is a modern, modular game engine written in Rust.
//!
//! ## Quick Start
//!
//! ### Managed Mode (Recommended)
//!
//! ```ignore
//! use leet::prelude::*;
//!
//! #[leet_main]
//! fn game(app: &mut App) {
//!     // Setup your game
//!     app.add_system(MySystem);
//! }
//! ```
//!
//! ### Manual Mode (Advanced)
//!
//! ```ignore
//! use leet::prelude::*;
//!
//! fn main() {
//!     let mut app = App::new();
//!     app.add_system(MySystem);
//!     app.run();
//! }
//! ```
//!
//! ## Modules
//!
//! - [`app`] - Application framework and lifecycle
//! - [`ecs`] - Entity Component System
//! - [`math`] - Math primitives and utilities
//! - [`core`] - Core types and error handling
//!

// Re-export all submodules
pub use leet_app as app;
pub use leet_bridge as bridge;
pub use leet_core as core;
pub use leet_display as display;
pub use leet_ecs as ecs;
pub use leet_math as math;

// Re-export commonly used types at the crate root
pub use leet_app::{App, AppConfig, CoreSystems};
pub use leet_core::{EngineClock, Leeror, LeetResult};

// Re-export the macro
pub use leet_macros::leet_main;

/// Prelude module - import everything you need with `use leet::prelude::*;`
pub mod prelude {
    pub use crate::leet_main;
    pub use leet_app::{App, AppConfig, CoreSystems};
    pub use leet_core::{EngineClock, Leeror, LeetResult};
    // Logging macros
    pub use leet_log::init as init_log;
    pub use leet_log::{debug, error, info, trace, warn};
    // anyhow for game developer code - easy error propagation
    pub use anyhow::{anyhow, Context, Result as GameResult};
}
