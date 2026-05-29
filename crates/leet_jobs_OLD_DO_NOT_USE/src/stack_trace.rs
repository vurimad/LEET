use std::backtrace::Backtrace;
use std::sync::Arc;

/// Lightweight Rust mirror of RED's captured stack trace payload.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StackTrace {
    pub frames: Vec<String>,
}

impl StackTrace {
    pub fn capture() -> Self {
        let rendered = Backtrace::force_capture().to_string();
        Self {
            frames: rendered.lines().map(|line| line.to_owned()).collect(),
        }
    }

    pub fn debug_string(&self) -> String {
        self.frames.join("\n")
    }
}

#[derive(Clone, Debug)]
pub struct StackTraceCacheEntry {
    pub stack_trace: StackTrace,
    pub debug_string: String,
}

#[derive(Clone, Debug)]
pub struct StackTraceCacheEntryPath {
    pub stack_traces: [Option<Arc<StackTraceCacheEntry>>; Self::MAX_CHAIN_TRACE_LEVELS],
    pub debug_string_view: [Option<String>; Self::MAX_CHAIN_TRACE_LEVELS],
}

impl StackTraceCacheEntryPath {
    pub const MAX_CHAIN_TRACE_LEVELS: usize = 8;

    pub fn new(stack_traces: Vec<Arc<StackTraceCacheEntry>>) -> Self {
        let mut out = Self {
            stack_traces: std::array::from_fn(|_| None),
            debug_string_view: std::array::from_fn(|_| None),
        };

        for (index, entry) in stack_traces
            .into_iter()
            .take(Self::MAX_CHAIN_TRACE_LEVELS)
            .enumerate()
        {
            out.debug_string_view[index] = Some(entry.debug_string.clone());
            out.stack_traces[index] = Some(entry);
        }

        out
    }

    pub fn iter_stack_traces(&self) -> impl Iterator<Item = &Arc<StackTraceCacheEntry>> {
        self.stack_traces.iter().filter_map(Option::as_ref)
    }
}

#[derive(Clone)]
pub struct StackTraceHandle {
    path: Arc<StackTraceCacheEntryPath>,
}

impl StackTraceHandle {
    fn null_path() -> Arc<StackTraceCacheEntryPath> {
        let mut path = StackTraceCacheEntryPath {
            stack_traces: std::array::from_fn(|_| None),
            debug_string_view: std::array::from_fn(|_| None),
        };
        path.debug_string_view[0] =
            Some("Run with '-jobDebugger' on the commandline to get stacktraces".to_owned());
        Arc::new(path)
    }

    pub fn new(path: Option<Arc<StackTraceCacheEntryPath>>) -> Self {
        Self {
            path: path.unwrap_or_else(Self::null_path),
        }
    }

    pub fn get_path(&self) -> &StackTraceCacheEntryPath {
        &self.path
    }
}

impl Default for StackTraceHandle {
    fn default() -> Self {
        Self::new(None)
    }
}
