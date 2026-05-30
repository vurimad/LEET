//! Mirror of RED `jobDispatcherEntries.h`.
//!
//! These are the small queue/wait-list records used by the dispatcher.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::counter::CounterInner;
use crate::job_decl::{EpilogueFunc, OwnedJobDecl, ParallelForJobFunc};
use crate::priority::Priority;

/// Mirror of RED `prv::WaitingListEntry`.
///
/// RED stores an intrusive `next` pointer and optional stack-trace pointer.
/// LEET currently stores waiting entries inside `CounterInner::waiting`, so the
/// list link is represented by the `Vec<WaitingListEntry>` container instead.
pub(crate) struct WaitingListEntry {
    pub(crate) job_decl: OwnedJobDecl,
    /// Counter to decrement when this job finishes.
    pub(crate) accumulate_counter_entry: Arc<CounterInner>,
    /// The priority lane this job should be queued into when released.
    pub(crate) priority: Priority,
}

/// Mirror of RED `prv::JobQueueEntry`.
pub(crate) struct JobQueueEntry {
    pub(crate) job_decl: OwnedJobDecl,
    pub(crate) accumulate_counter_entry: Arc<CounterInner>,
}

/// Mirror of RED `c_localQueueDefaultCapacity`.
pub(crate) const C_LOCAL_QUEUE_DEFAULT_CAPACITY: usize = 256;

/// Mirror of RED `TLocalQueue`.
pub(crate) type TLocalQueue = VecDeque<(JobQueueEntry, Priority)>;

/// Mirror of RED `prv::ParallelForSharedCounterEntry`.
pub(crate) struct ParallelForSharedCounterEntry {
    pub(crate) counter: AtomicU32,
}

impl ParallelForSharedCounterEntry {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            counter: AtomicU32::new(0),
        })
    }

    pub(crate) fn next_index(&self) -> u32 {
        self.counter.fetch_add(1, Ordering::AcqRel)
    }
}

/// Mirror of RED `prv::ParallelForJobEntry`.
///
/// This is kept as the structural mirror.  The current Rust parallel-for path
/// captures these fields directly into closures instead of allocating a pooled
/// `ParallelForJobEntry` object per team member.
#[allow(dead_code)]
pub(crate) struct ParallelForJobEntry {
    pub(crate) job_func: Option<ParallelForJobFunc>,
    pub(crate) epilogue_func: Option<EpilogueFunc>,
    pub(crate) shared_data: *mut c_void,
    pub(crate) elements: *mut c_void,
    pub(crate) shared_counter_entry: Option<Arc<ParallelForSharedCounterEntry>>,
    pub(crate) num_elements: u32,
    pub(crate) team_size: u32,
    pub(crate) team_index: u32,
    pub(crate) max_batch_size: u32,
}
