use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use thread_priority::{set_current_thread_priority, ThreadPriority};

use leet_log::warn;

use crate::dispatcher::{DispatcherInner, WorkerPriority};
use crate::dispatcher_entries::{JobQueueEntry, TLocalQueue, C_LOCAL_QUEUE_DEFAULT_CAPACITY};
use crate::priority::Priority;

thread_local! {
    static LOCAL_QUEUE: RefCell<TLocalQueue> =
        RefCell::new(TLocalQueue::with_capacity(C_LOCAL_QUEUE_DEFAULT_CAPACITY));
    static CURRENT_DISPATCHER_THREAD_INDEX: Cell<u32> = const { Cell::new(0) };
    static IS_DISPATCHER_THREAD: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn current_dispatcher_thread_index() -> u32 {
    CURRENT_DISPATCHER_THREAD_INDEX.with(Cell::get)
}

#[allow(dead_code)]
pub(crate) fn is_dispatcher_thread() -> bool {
    IS_DISPATCHER_THREAD.with(Cell::get)
}

pub(crate) fn pop_local_job() -> Option<(JobQueueEntry, Priority)> {
    LOCAL_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let popped = q.pop_front();
        if q.capacity() > C_LOCAL_QUEUE_DEFAULT_CAPACITY
            && q.len() <= C_LOCAL_QUEUE_DEFAULT_CAPACITY
        {
            q.shrink_to_fit();
        }
        popped
    })
}

pub(crate) fn push_local_job(job: JobQueueEntry, priority: Priority) {
    LOCAL_QUEUE.with(|q| {
        q.borrow_mut().push_back((job, priority));
    });
}

/// RED-style worker setup payload.
///
/// LEET keeps the structural mirror, but the actual stack/affinity handling is
/// adapted to the Rust thread model and current WorkerConfig.
#[derive(Debug, Clone, Default)]
pub struct DispatcherThreadSetup {
    pub stack_size_kb: usize,
    pub core_affinity: Option<Vec<usize>>,
    pub dispatcher_thread_index: u32,
}

/// Mirror of RED's `job::prv::DispatcherThread`.
///
/// In LEET this wraps the spawned OS thread handle and readiness flag.
pub struct DispatcherThread {
    handle: JoinHandle<()>,
    is_ready: Arc<AtomicBool>,
    setup: DispatcherThreadSetup,
}

impl DispatcherThread {
    pub(crate) fn spawn(
        thread_name: String,
        dispatcher: Arc<DispatcherInner>,
        setup: DispatcherThreadSetup,
        worker_priority: WorkerPriority,
    ) -> Self {
        let is_ready = Arc::new(AtomicBool::new(false));
        let is_ready_thread = Arc::clone(&is_ready);
        let setup_for_thread = setup.clone();
        let builder = if setup.stack_size_kb > 0 {
            thread::Builder::new()
                .name(thread_name.clone())
                .stack_size(setup.stack_size_kb.saturating_mul(1024))
        } else {
            thread::Builder::new().name(thread_name.clone())
        };

        let handle = builder
            .spawn(move || {
                setup_dispatcher(&setup_for_thread, worker_priority, &is_ready_thread);
                do_work_loop(dispatcher, setup_for_thread.dispatcher_thread_index);
            })
            .expect("[leet_jobs] failed to spawn dispatcher thread");

        Self {
            handle,
            is_ready,
            setup,
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::Acquire)
    }

    pub(crate) fn join(self) -> thread::Result<()> {
        self.handle.join()
    }

    #[allow(dead_code)]
    pub(crate) fn setup(&self) -> &DispatcherThreadSetup {
        &self.setup
    }
}

fn setup_dispatcher(
    setup: &DispatcherThreadSetup,
    worker_priority: WorkerPriority,
    is_ready: &AtomicBool,
) {
    CURRENT_DISPATCHER_THREAD_INDEX.with(|index| index.set(setup.dispatcher_thread_index));
    IS_DISPATCHER_THREAD.with(|flag| flag.set(true));

    if let Some(ref cores) = setup.core_affinity {
        let core_ids: Vec<core_affinity::CoreId> = cores
            .iter()
            .map(|&c| core_affinity::CoreId { id: c })
            .collect();

        if core_ids.is_empty() {
            warn!(
                "[leet_jobs] worker-{}: core_affinity list is empty, skipping affinity",
                setup.dispatcher_thread_index,
            );
        } else {
            let target = core_ids[0];
            if !core_affinity::set_for_current(target) {
                warn!(
                    "[leet_jobs] worker-{}: failed to set core affinity to core {}",
                    setup.dispatcher_thread_index, target.id,
                );
            }
        }
    }

    let tp = match worker_priority {
        WorkerPriority::Lowest => ThreadPriority::Min,
        WorkerPriority::Low => ThreadPriority::Crossplatform(
            thread_priority::ThreadPriorityValue::try_from(20u8)
                .unwrap_or(thread_priority::ThreadPriorityValue::MIN),
        ),
        WorkerPriority::Normal => ThreadPriority::Crossplatform(
            thread_priority::ThreadPriorityValue::try_from(50u8)
                .unwrap_or(thread_priority::ThreadPriorityValue::MIN),
        ),
        WorkerPriority::High => ThreadPriority::Crossplatform(
            thread_priority::ThreadPriorityValue::try_from(80u8)
                .unwrap_or(thread_priority::ThreadPriorityValue::MIN),
        ),
        WorkerPriority::Highest => ThreadPriority::Max,
    };
    if let Err(e) = set_current_thread_priority(tp) {
        warn!(
            "[leet_jobs] worker-{}: failed to set thread priority {:?}: {:?}",
            setup.dispatcher_thread_index, worker_priority, e,
        );
    }

    is_ready.store(true, Ordering::Release);
}

fn do_work_loop(dispatcher: Arc<DispatcherInner>, _thread_index: u32) {
    loop {
        if let Some((queued, _priority)) = pop_local_job() {
            let JobQueueEntry {
                job_decl,
                accumulate_counter_entry,
                ..
            } = queued;
            dispatcher.execute_job(job_decl, accumulate_counter_entry);
            continue;
        }

        dispatcher.acquire_work_signal();
        if dispatcher.is_exiting() {
            break;
        }

        if let Some((queued, _priority)) = dispatcher.try_pop() {
            let JobQueueEntry {
                job_decl,
                accumulate_counter_entry,
                ..
            } = queued;
            dispatcher.execute_job(job_decl, accumulate_counter_entry);
        }
    }
}
