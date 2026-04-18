/// Job priority lanes — same four levels as the C++ original.
///
/// Higher numeric value = higher urgency.
/// The dispatcher always drains `Immediate` before `CriticalPath`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(usize)]
pub enum Priority {
    /// Background / async streaming work. Runs when nothing else is queued.
    Latent = 0,
    /// Render-thread dependency chain.
    RenderPath = 1,
    /// Default: main game-logic work.
    CriticalPath = 2,
    /// Urgent one-shot tasks that must not wait behind anything else.
    Immediate = 3,
}

/// Total number of priority lanes — used to size fixed arrays.
pub const PRIORITY_COUNT: usize = 4;

impl Priority {
    /// All variants in ascending order, useful for iteration.
    pub const ALL: [Priority; PRIORITY_COUNT] = [
        Priority::Latent,
        Priority::RenderPath,
        Priority::CriticalPath,
        Priority::Immediate,
    ];
}

/// Parameters attached to a job or counter when it is created.
#[derive(Debug, Clone, Copy)]
pub struct ScheduleParam {
    pub priority: Priority,
}

impl Default for ScheduleParam {
    fn default() -> Self {
        Self {
            priority: Priority::CriticalPath,
        }
    }
}
