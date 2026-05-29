use std::sync::Arc;

use super::*;

pub(crate) fn counter_entry() -> Arc<CounterEntry> {
    CounterEntry::new(Priority::CriticalPath, "test counter")
}

pub(crate) fn waiting_len(counter: &CounterEntry) -> usize {
    counter
        .waiting
        .lock()
        .expect("counter waiting-list lock poisoned")
        .len()
}
