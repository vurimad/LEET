/// Scheduling lane used by ready jobs and counters.
///
/// Larger values are more urgent. The ready queue scans priorities in descending
/// order, then preserves FIFO order inside the selected priority lane.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Priority {
    /// Background work that should not block visible/render-critical progress.
    Latent = 0,
    /// Render-path work that should be helped by render-frame flushes.
    RenderPath = 1,
    /// Default priority for ordinary jobs and dependency chains.
    CriticalPath = 2,
    /// Highest-priority lane for work that should be popped first.
    Immediate = 3,
}

/// Scheduling properties attached to counters and continuation work.
///
/// v1 keeps the contract intentionally narrow: priority is the only scheduling
/// dimension represented in the core crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduleParam {
    /// Priority used when work associated with this parameter becomes ready.
    pub priority: Priority,
}

impl Default for ScheduleParam {
    /// Uses the normal critical-path lane for jobs without an explicit priority.
    fn default() -> Self {
        Self {
            priority: Priority::CriticalPath,
        }
    }
}

// Test bodies live in `src/tests`; the declaration stays here to keep the unit
// tests attached to the module whose ordering contract they validate.
#[cfg(test)]
#[path = "tests/priority.rs"]
mod tests;
