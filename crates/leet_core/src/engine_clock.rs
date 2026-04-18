//! Global frame clock shared across engine systems.

use std::sync::atomic::{AtomicU64, Ordering};

static CURRENT_FRAME: AtomicU64 = AtomicU64::new(0);

/// Global engine frame clock.
pub struct EngineClock;

impl EngineClock {
    /// Reset the global frame counter back to zero.
    pub fn reset() {
        CURRENT_FRAME.store(0, Ordering::Relaxed);
    }

    /// Advance the clock by one frame and return the new frame index.
    pub fn advance() -> u64 {
        CURRENT_FRAME.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Return the current global frame index.
    pub fn current_frame() -> u64 {
        CURRENT_FRAME.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::EngineClock;

    #[test]
    fn engine_clock_resets_and_advances() {
        EngineClock::reset();
        assert_eq!(EngineClock::current_frame(), 0);
        assert_eq!(EngineClock::advance(), 1);
        assert_eq!(EngineClock::current_frame(), 1);
        assert_eq!(EngineClock::advance(), 2);
        assert_eq!(EngineClock::current_frame(), 2);
        EngineClock::reset();
        assert_eq!(EngineClock::current_frame(), 0);
    }
}
