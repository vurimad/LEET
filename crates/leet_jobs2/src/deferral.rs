//! Completion deferrals attach externally finished work to counters.
//!
//! A deferral is a counted unit of work whose actual completion is controlled
//! by the caller. This is useful when a dependency is represented by an outside
//! lifecycle event rather than by a job closure running on the worker pool.

use std::sync::Arc;

use crate::{counter::CounterEntry, dispatcher::DispatcherHandle};

/// Move-only guard for one outstanding unit on a counter.
///
/// Creating a deferral increments the target counter immediately. Finishing or
/// dropping the deferral decrements it exactly once through the dispatcher, so a
/// deferral behaves like a job whose completion is controlled by external code.
///
/// ```compile_fail
/// fn assert_clone<T: Clone>() {}
/// assert_clone::<leet_jobs2::CompletionDeferral>();
/// ```
pub struct CompletionDeferral {
    dispatcher: DispatcherHandle,
    counter: Option<Arc<CounterEntry>>,
    name: &'static str,
    is_finished: bool,
}

impl CompletionDeferral {
    /// Creates a deferral and immediately counts it as outstanding work.
    pub(crate) fn new(
        dispatcher: DispatcherHandle,
        counter: Arc<CounterEntry>,
        name: &'static str,
    ) -> Self {
        counter.increment();
        Self {
            dispatcher,
            counter: Some(counter),
            name,
            is_finished: false,
        }
    }

    /// Completes this deferral and decrements its counter exactly once.
    ///
    /// Calling `finish()` more than once is a logic error. Dropping an
    /// unfinished deferral performs the same decrement without panicking.
    pub fn finish(&mut self) {
        assert!(
            !self.is_finished,
            "completion deferral was already finished"
        );

        self.is_finished = true;
        if let Some(counter) = self.counter.take() {
            self.dispatcher.decrement_counter_entry(counter);
        }
    }

    /// Static label associated with the external work represented by this guard.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl Drop for CompletionDeferral {
    fn drop(&mut self) {
        if self.is_finished {
            return;
        }

        // Drop mirrors `finish()` but must stay non-panicking for normal RAII
        // cleanup paths. The `Option` take keeps the decrement single-shot.
        self.is_finished = true;
        if let Some(counter) = self.counter.take() {
            self.dispatcher.decrement_counter_entry(counter);
        }
    }
}

// Test bodies live in `src/tests`; the declaration stays here so the unit tests
// remain child modules with access to private deferral state.
#[cfg(test)]
#[path = "tests/deferral.rs"]
mod tests;
