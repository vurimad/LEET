//! Allocator phase types.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceAllocatorPhase {
    Startup,
    PreConsume,
    Resolve,
    Consume,
    Cleanup,
}

impl ResourceAllocatorPhase {
    pub const fn is_consume(self) -> bool {
        matches!(self, Self::Consume)
    }
}
