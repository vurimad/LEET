//! Example game using MANAGED mode
//!
//! In managed mode, the #[leet_main] macro generates the main() function for you.

use leet::prelude::*;

#[leet_main]
fn game_setup(app: &mut App) {
    info!("[Game] game_setup() called - game will be set up here.");
    let _ = app;
}
