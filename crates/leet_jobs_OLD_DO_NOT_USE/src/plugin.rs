use bevy_app::{App, Plugin};

/// Placeholder Bevy plugin for the LEET job dispatcher.
///
/// This is intentionally not wired yet. The intended future shape is:
/// - register the dispatcher as a Bevy resource,
/// - expose lookup helpers from the Bevy side,
/// - remove the remaining RED-style free-function TODOs.
pub struct LeetJobsPlugin;

impl Plugin for LeetJobsPlugin {
    fn build(&self, _app: &mut App) {
        unimplemented!("[leet_jobs] LeetJobsPlugin is a placeholder for the Bevy resource bridge");
    }
}
