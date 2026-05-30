use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};

use leet_log::{error, warn};

use crate::counter::{Counter, CounterInner};
use crate::debugger_stack_trace_cache::StackTraceCache;
use crate::deferral::CompletionDeferral;
use crate::priority::Priority;
use crate::stack_trace::StackTraceCacheEntryPath;

struct DebuggerState {
    registered_counters: HashMap<usize, Arc<CounterInner>>,
    registered_deferrals: HashSet<usize>,
}

impl DebuggerState {
    fn new() -> Self {
        Self {
            registered_counters: HashMap::new(),
            registered_deferrals: HashSet::new(),
        }
    }
}

/// RED-style job debugger mirror.
pub struct Debugger {
    register_lock: RwLock<DebuggerState>,
    #[allow(dead_code)]
    stack_trace_cache: StackTraceCache,
}

impl Debugger {
    pub fn new() -> Self {
        Self {
            register_lock: RwLock::new(DebuggerState::new()),
            stack_trace_cache: StackTraceCache::new(),
        }
    }

    pub fn analyze_counter(&self, counter: &CounterInner) {
        let state = self.register_lock.read().unwrap();

        let is_zero = counter.is_zero_snapshot();
        error!(
            "[leet_jobs] analyzing counter {:p}: is_zero={}",
            counter, is_zero
        );
        if is_zero {
            return;
        }

        error!(
            "[leet_jobs] registered counters: {}, registered deferrals: {}",
            state.registered_counters.len(),
            state.registered_deferrals.len()
        );

        let mut seen_counters: HashSet<usize> = HashSet::new();
        let mut counters_to_check: VecDeque<(usize, usize)> = VecDeque::new();
        let mut blockers: Vec<BlockerEntry> = Vec::new();

        let root_addr = counter as *const CounterInner as usize;
        seen_counters.insert(root_addr);
        counters_to_check.push_back((root_addr, 0));

        while let Some((current_addr, depth)) = counters_to_check.pop_front() {
            let mut job_priorities: Vec<Priority> = Vec::new();
            let mut deferrals: Vec<usize> = Vec::new();

            for deferral_addr in &state.registered_deferrals {
                if *deferral_addr == current_addr {
                    deferrals.push(*deferral_addr);
                }
            }

            for registered_counter in state.registered_counters.values() {
                let waiting = registered_counter.waiting.lock().unwrap();
                for job in waiting.iter() {
                    let accumulate_addr = Arc::as_ptr(&job.accumulate_counter_entry) as usize;
                    if accumulate_addr == current_addr {
                        job_priorities.push(job.priority);
                    }
                }
            }

            blockers.push(BlockerEntry {
                counter_addr: current_addr,
                depth,
                deferrals,
                job_priorities,
            });

            for registered_counter in state.registered_counters.values() {
                let registered_addr = Arc::as_ptr(registered_counter) as usize;
                if registered_addr == current_addr || seen_counters.contains(&registered_addr) {
                    continue;
                }

                let waiting = registered_counter.waiting.lock().unwrap();
                let depends_on_current = waiting
                    .iter()
                    .any(|job| Arc::as_ptr(&job.accumulate_counter_entry) as usize == current_addr);

                if depends_on_current {
                    seen_counters.insert(registered_addr);
                    counters_to_check.push_back((registered_addr, depth + 1));
                }
            }
        }

        for entry in blockers {
            let is_runnable = entry.deferrals.is_empty() && entry.job_priorities.is_empty();
            error!(
                "[{}] Counter depth={}: {:p}",
                if is_runnable { "RUNNABLE" } else { "WAITING" },
                entry.depth,
                entry.counter_addr as *const CounterInner
            );

            if !entry.deferrals.is_empty() {
                error!("\tBlocked by {} deferrals", entry.deferrals.len());
                for deferral_addr in entry.deferrals {
                    error!("\t\tDeferral addr: 0x{:x}", deferral_addr);
                }
            }

            if !entry.job_priorities.is_empty() {
                error!(
                    "\tBlocked by {} jobs on counter {:p}",
                    entry.job_priorities.len(),
                    entry.counter_addr as *const CounterInner
                );
                for priority in entry.job_priorities {
                    error!("\t\tJob priority: {:?}", priority);
                }
            }
        }

        error!("[leet_jobs] debugger dump end");
    }

    pub fn register_counter(&self, counter: &Counter) {
        let mut state = self.register_lock.write().unwrap();
        let addr = Arc::as_ptr(counter.inner()) as usize;
        state
            .registered_counters
            .insert(addr, Arc::clone(counter.inner()));
    }

    pub fn unregister_counter(&self, counter: &Counter) {
        let mut state = self.register_lock.write().unwrap();
        let addr = Arc::as_ptr(counter.inner()) as usize;
        state.registered_counters.remove(&addr);
    }

    pub fn register_deferral(&self, deferral: &CompletionDeferral) {
        let mut state = self.register_lock.write().unwrap();
        if let Some(addr) = deferral.debug_counter_addr() {
            state.registered_deferrals.insert(addr);
        }
    }

    pub fn unregister_deferral(&self, deferral: &CompletionDeferral) {
        let mut state = self.register_lock.write().unwrap();
        if let Some(addr) = deferral.debug_counter_addr() {
            state.registered_deferrals.remove(&addr);
        }
    }

    pub fn trace_call(&self) -> Option<Arc<StackTraceCacheEntryPath>> {
        let _ = &self.stack_trace_cache;
        warn!("[leet_jobs] trace_call requested, returning no stack trace yet");
        None
    }
}

struct BlockerEntry {
    counter_addr: usize,
    depth: usize,
    deferrals: Vec<usize>,
    job_priorities: Vec<Priority>,
}
