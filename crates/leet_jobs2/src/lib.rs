//! `leet_jobs2` is a small owned job-system runtime for render-side work.
//!
//! The public API is intentionally centered on `LeetJobSystem`: callers create
//! counters and builders from a concrete runtime instead of reaching through a
//! global singleton. Jobs own their captured data, counters are move-only public
//! handles, and shutdown is explicit so tests and plugins can control worker
//! lifetime cleanly.
//!
//! The crate has no knowledge of render graphs, ECS worlds, GPU resources, or
//! engine-specific types. Those layers can store `LeetJobSystem` as a resource
//! and use `Builder` to express job dependencies, while the core crate only owns
//! scheduling, counters, worker threads, and flush behavior.

mod builder;
mod config;
mod counter;
mod deferral;
mod dispatcher;
mod job_decl;
mod priority;
mod queue;
mod worker;

pub use builder::{Builder, Fence};
pub use config::JobSystemConfig;
pub use counter::Counter;
pub use deferral::CompletionDeferral;
pub use dispatcher::{DispatcherHandle, LeetJobSystem};
pub use job_decl::{JobHint, RunContext};
pub use priority::{Priority, ScheduleParam};

// Test bodies live in `src/tests`; the declaration stays here because these
// tests validate the crate-root public surface.
#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
