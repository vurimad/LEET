use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::stack_trace::{StackTrace, StackTraceCacheEntry, StackTraceCacheEntryPath};

/// Cache for deduplicating stack traces and stack-trace paths.
#[allow(dead_code)]
pub struct StackTraceCache {
    stack_traces: RwLock<HashMap<StackTrace, Arc<StackTraceCacheEntry>>>,
    stack_trace_paths: RwLock<HashMap<Vec<usize>, Arc<StackTraceCacheEntryPath>>>,
}

#[allow(dead_code)]
impl StackTraceCache {
    pub fn new() -> Self {
        Self {
            stack_traces: RwLock::new(HashMap::new()),
            stack_trace_paths: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_stack_cached_trace(&self, stack_trace: &StackTrace) -> Arc<StackTraceCacheEntry> {
        if let Some(entry) = self.stack_traces.read().unwrap().get(stack_trace) {
            return Arc::clone(entry);
        }

        let mut write = self.stack_traces.write().unwrap();
        if let Some(entry) = write.get(stack_trace) {
            return Arc::clone(entry);
        }

        let entry = Arc::new(StackTraceCacheEntry {
            stack_trace: stack_trace.clone(),
            debug_string: stack_trace.debug_string(),
        });
        write.insert(stack_trace.clone(), Arc::clone(&entry));
        entry
    }

    pub fn get_stack_path_cached_trace(
        &self,
        stack_trace: &StackTrace,
        parent_path: Option<&StackTraceCacheEntryPath>,
    ) -> Arc<StackTraceCacheEntryPath> {
        let entry = self.get_stack_cached_trace(stack_trace);

        let mut path_key: Vec<usize> =
            Vec::with_capacity(StackTraceCacheEntryPath::MAX_CHAIN_TRACE_LEVELS);
        path_key.push(Arc::as_ptr(&entry) as usize);

        let mut path_entries: Vec<Arc<StackTraceCacheEntry>> =
            Vec::with_capacity(StackTraceCacheEntryPath::MAX_CHAIN_TRACE_LEVELS);
        path_entries.push(entry);

        if let Some(parent_path) = parent_path {
            for parent_entry in parent_path.iter_stack_traces() {
                if path_entries.len() >= StackTraceCacheEntryPath::MAX_CHAIN_TRACE_LEVELS {
                    break;
                }
                path_key.push(Arc::as_ptr(parent_entry) as usize);
                path_entries.push(Arc::clone(parent_entry));
            }
        }

        if let Some(path) = self.stack_trace_paths.read().unwrap().get(&path_key) {
            return Arc::clone(path);
        }

        let mut write = self.stack_trace_paths.write().unwrap();
        if let Some(path) = write.get(&path_key) {
            return Arc::clone(path);
        }

        let path = Arc::new(StackTraceCacheEntryPath::new(path_entries));
        write.insert(path_key, Arc::clone(&path));
        path
    }
}
