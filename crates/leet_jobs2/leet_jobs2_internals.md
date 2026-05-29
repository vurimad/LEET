# leet_jobs2 — Internal Design Notes

This file covers ONLY what lives inside the job system crate itself.
Nothing in here about render graphs, command handlers, or engine code.
Those belong in the engine that uses this crate.

---

## What leet_jobs2 owns

```
leet_jobs2
├── Counter          — public handle to a CounterEntry
├── CounterEntry     — internal shared state (atomic value, waiting list)
├── JobDecl          — a boxed closure + name + hint
├── ParallelForJob   — parallel-for closure + optional epilogue
├── RunContext        — passed to a job when it runs
├── Builder          — scoped helper for dispatching ordered/parallel jobs
├── CompletionDeferral — RAII guard that keeps a counter alive
├── LeetJobSystem    — public handle to worker threads and queues
└── worker threads   — pop jobs and execute them
```

leet_jobs2 does NOT know about:
- RenderCommandHandler
- RenderFrame
- Any engine-specific types
- Any GPU or graphics concepts

---

## Decisions Made (v1 scope)

| Question | Decision |
|---|---|
| API shape | Rust-native, not a copy of C++ API |
| Job ownership | `Send + 'static` only — jobs own their data via `Arc` |
| Builder misuse | Runtime panic (not compile-time, not debug-only) |
| FlushCounter callers | Render thread only — one designated flush thread, debug assert if called from any other thread |
| Flush behavior | Run any available eligible job while waiting |
| Priority levels | Keep all four: `Latent`, `RenderPath`, `CriticalPath`, `Immediate` |
| Job hints | Keep `Trivial` and `Large` only — drop `PhysX` and `AudioEvent` |
| Config | Use `JobSystemConfig`, not global `InitParam` |
| Queue capacity | Bounded, fixed size at init — same as C++ |
| Shutdown behavior | Just stop — no drain in v1 |
| Profiling hooks | Stubbed behind a feature flag in v1 |
| Parallel-for | Implement manually using atomic-index pattern, same as C++ |
| `dispatch_job` API | Generic `<F>` on public API for clean call site, `Box<dyn FnOnce>` for internal storage |

---

## Out of Scope for v1

Do not implement these. Do not let Codex implement them either.

- PhysX hint and special-case priority behavior
- Console core 7 affinity queue
- Resource throttler threads
- Local queue optimization (disabled in C++ with `if (false)` anyway)
- Frame allocator debug bans
- IO priority thread-locals
- Scoped (non-`'static`) jobs

---

## Implementation Order (do not skip ahead)

The canonical execution checklist lives in `leet_jobs2_execution_passes.md`.
That file owns pass boundaries, per-pass tests, and handoff rules.

Summary:

0. Crate skeleton and static types — exact module map, config, priorities,
   job declarations, run-context shape. No runtime behavior.
1. `CounterEntry` — atomic value, waiting list, `Arc` ownership.
   Single-threaded tests only.
2. Bounded ready queues — one FIFO lane per priority, strict priority pop order.
3. Worker thread pool — pop and execute, single priority, no dependencies yet.
4. Wire counters into workers — dependency gates, decrement on completion,
   flush on zero.
5. `Builder` + `CompletionDeferral`
5.5. Bevy render-world hookup — optional resource support in `leet_jobs2`,
     registered in the render sub-app, with the render schedule claiming the
     flush thread once.
6. Parallel-for last.

Each layer must have passing tests before moving to the next.

---

## Public Types

### JobDecl

Internal storage for a single job. Not exposed directly — created inside `dispatch_job`.

```rust
pub(crate) struct JobDecl {
    func: Box<dyn FnOnce(&RunContext) + Send + 'static>,
    name: &'static str,
    hint: JobHint,
}

impl JobDecl {
    pub(crate) fn new<F>(name: &'static str, hint: JobHint, f: F) -> Self
    where
        F: FnOnce(&RunContext) + Send + 'static,
    {
        Self {
            func: Box::new(f),
            name,
            hint,
        }
    }
}
```

- `func` — the closure, heap allocated, type erased. Called once then dropped.
- `name` — string literal baked into binary. Replaces C++ `static InstrumentationObject`.
- `hint` — `None`, `Trivial`, or `Large` only.

### ParallelForJob

Internal storage for a parallel-for job. Not exposed directly — created inside
`dispatch_parallel_for` and related builder methods.

```rust
type ParallelForEpilogue = Box<dyn FnOnce(&RunContext) + Send + 'static>;

pub(crate) struct TakeOnceEpilogue {
    inner: Mutex<Option<ParallelForEpilogue>>,
}

pub(crate) struct ParallelForJob {
    func: Box<dyn Fn(u32, u32, &RunContext) + Send + Sync + 'static>,
    epilogue: Option<Arc<TakeOnceEpilogue>>,
    num_elements: u32,
    max_batch_size: u32,
    name: &'static str,
    hint: JobHint,
}
```

- `func` is `Fn`, not `FnOnce`, because it may be called by multiple chunks.
- `func` is `Sync` because chunks may run on multiple worker threads at once.
- `func` is `Box` because it is owned by `ParallelForJob`. Multiple team jobs
  access it through the shared `Arc<ParallelForJob>`, so `func` does not need
  its own `Arc` in this struct.
- `epilogue` wraps a `FnOnce` in a take-once holder because it may be observed
  by multiple team jobs, but must run exactly once after all chunks finish.
- If no team job claims the epilogue, such as because work exits early or a job
  panics, the epilogue closure is dropped when the last `Arc<ParallelForJob>` is
  dropped. Do not add a separate epilogue-ran flag outside `TakeOnceEpilogue`.
- C++ `initSharedDataCallback` is not supported in v1. Add an explicit setup API
  later only if a real caller needs team-size-dependent shared state.
- C++ `elements` is not stored. Rust callers capture owned/shared data directly
  in the closure, such as `Arc<[T]>`, engine handles, atomics, or buffers.
- C++ `JobDebugFlags` and `debugFlags` are dropped in v1 with the frame allocator
  debug-ban machinery.
- C++ `JobHint::PhysX` and `JobHint::AudioEvent` are dropped in v1.

### RunContext

Passed to a job when it runs. Read-only from the job's perspective.

```rust
pub struct RunContext {
    pub name: &'static str,
    pub thread_index: u32,          // 0 = render/flush thread, 1..N = workers
    pub parallel_for_index: i32,    // -1 if not a parallel-for job
    pub(crate) dispatcher: DispatcherHandle,
    pub(crate) continuation: ContinuationContext,
}

pub(crate) struct ContinuationContext {
    pub counter: Arc<CounterEntry>,
    pub param: ScheduleParam,
}
```

### Counter

Public handle. Move-only (no Clone). Carries a private dispatcher handle and an `Arc<CounterEntry>`.

```rust
pub struct Counter {
    pub(crate) dispatcher: DispatcherHandle,
    entry: Arc<CounterEntry>,
}
```

C++ `Counter` is also move-only. Keep it that way.

### CounterEntry

Internal shared state. Never exposed publicly.

```rust
pub(crate) type WaitingList = Vec<WaitingJob>;

pub(crate) struct CounterEntry {
    value: AtomicU32,
    waiting: Mutex<WaitingList>,
    priority: Priority,
    name: &'static str,
}
```

The `Arc<CounterEntry>` replaces C++'s manual `DispatcherRefCountMask`.
When all holders (Counter handle, queued jobs, deferrals) drop their Arc, the entry frees itself.

### JobHint

```rust
pub enum JobHint {
    None,
    Trivial,  // smaller than scheduling cost — may run inline
    Large,    // long running — avoid blocking high priority flush
}
```

### Priority

```rust
pub enum Priority {
    Latent = 0,
    RenderPath = 1,
    CriticalPath = 2,
    Immediate = 3,
}
```

### ScheduleParam

C++ `ScheduleParam` carries:

- `priority`
- `affinity`
- `ioPriority`

Rust v1 keeps only the core scheduling priority:

```rust
pub struct ScheduleParam {
    pub priority: Priority,
}
```

`ScheduleParam::default()` should use `Priority::CriticalPath`, matching the C++
default. Affinity and IO priority are intentionally dropped for v1; do not keep
placeholder fields that have no behavior.

---

## dispatch_job API Pattern

Generic on the outside for a clean call site.
`Box` on the inside because the queue must store mixed closure types.

```rust
// Public API on Builder
pub fn dispatch_job<F>(&mut self, name: &'static str, f: F)
where
    F: FnOnce(&RunContext) + Send + 'static,
{
    let job = JobDecl::new(name, JobHint::None, f);
    // ... queue or wait
}

// How callers write it — clean, no Box::new visible
builder.dispatch_job("MyJob", move |ctx| {
    do_something(ctx);
});
```

The generic public method keeps the call site clean while hiding the internal
type erasure. The queue stores mixed job types, so the closure is boxed before it
enters the dispatcher.

---

## Function Pointer Translation (C++ JobShim → Rust Box)

C++ stores a job as two raw pointers:
```
jobDecl.jobFunc  →  pointer to RunJob() function
jobDecl.jobData  →  pointer to heap-allocated lambda data (void*)
```

Rust `Box<dyn FnOnce>` is also two pointers:
```
data ptr    →  heap-allocated captured data
vtable ptr  →  compiler-generated table with call_fn and drop_fn
```

| C++ | Rust |
|---|---|
| `jobDecl.jobFunc` | vtable `call_fn` |
| `jobDecl.jobData` | data ptr |
| `static_cast<JobShim*>(jobData)` | handled by compiler, never visible |
| `RED_DELETE(shim)` after run | `drop_fn` in vtable, automatic |
| `RED_NEW(JobShim)(lambda)` | `Box::new(closure)` |

Both designs have a similar cost model: captured job data is heap allocated and
called through an indirect function/vtable path. Do not claim exact identical
performance; measure later if this becomes hot.

---

## jobRunner.h / jobRunner.inl / jobShim.h

These C++ files are mostly template glue:

- `jobRunner.h` exposes `BuildJob`, `DispatchJob`, `DispatchParallelForJob`,
  and related helper templates.
- `jobRunner.inl` turns a C++ lambda into a `JobDecl` or `JobDeclParallelFor`.
- `jobShim.h` stores the lambda object behind `void*` and deletes it after run.

Rust does not need a direct copy of this file split.

| C++ file | Rust home | Notes |
|---|---|---|
| `jobDecl.h` | `job_decl.rs` | data structs: `JobDecl`, `ParallelForJob`, `RunContext` |
| `jobRunner.h/.inl` | mostly `builder.rs` + small constructors in `job_decl.rs` | public dispatch methods create internal job declarations |
| `jobShim.h` | no direct file | replaced by boxed Rust closures; parallel-for shares the whole `ParallelForJob` through `Arc<ParallelForJob>` |

Do not create a large `job_runner.rs` just to mirror C++. If we later need a
central place for profiling wrappers, panic policy, or thread-local setup around
actual job execution, a small `job_runner.rs` can own that execution hook only.

---

## Critical Invariants

These are the behaviors that must be correct.
Codex tends to get these wrong — do not trust generated code on these points.

**1. Increment before queue.**
`accumulateCounter` value is incremented BEFORE the job is queued or added to a
waiting list. Never after. If you get this wrong, the counter can hit zero too early.

**2. Recheck under lock.**
When inserting a job onto a counter's waiting list, recheck the counter value under
the same lock. Without this there is a race:
- Thread A decrements counter to zero, does not flush yet
- Thread B increments counter back to 1, adds a new job to waiting list
- Thread A flushes — incorrectly runs the new job before the counter is zero again

See the long comment in C++ `jobDispatcher.cpp` `FlushWaitingList()`.

**3. Counter stays alive.**
`CounterEntry` must stay alive while any job or deferral may still touch it.
`Arc<CounterEntry>` handles this — every job that accumulates into a counter
holds an `Arc` clone. When the last holder drops, the entry frees itself.
Do not drop the Arc early.

**4. Flush is not reentrant.**
`flush_counter` must panic if called while already flushing.
Use a bool flag, same as C++ `m_isFlushingCounter`.

**5. Empty parallel-for still increments.**
If `num_elements == 0`:
- If there is an epilogue function, queue one job that only runs the epilogue.
- If there is no epilogue, queue one empty job.
Either way the counter must be incremented and decremented to preserve dependency chains.

**6. Builder fence ordering.**
After dispatching with `Fence::None` (parallel dispatch), `dispatch_fence_explicitly()`
must be called before any ordered dispatch or before the builder is destroyed.
In v1: runtime panic if violated. Use a `debug_needs_fence: bool` flag same as C++.

**7. Continuation extends parent.**
A `Builder` created from a `RunContext` must link its final counter into the parent
job's continuation counter on drop. The parent job is not finished until all child
builders resolve.

If `extract_wait_counter()` is called on a continuation builder, it must still run
the final-sync path and link into the parent continuation counter before returning.
C++ then rewires the returned counter to wait on the parent continuation counter.
Do not treat extraction as "skip continuation linking".

---

## Counter Lifetime

C++ uses `DispatcherRefCountMask` to manually track when it is safe to free a `CounterEntry`.

Rust uses `Arc<CounterEntry>` — the same logic, automatic:

| Who holds a ref | When it drops |
|---|---|
| Public `Counter` handle | When the Counter is dropped or moved |
| Each queued `JobDecl` | When the job finishes running |
| Each `CompletionDeferral` | When `finish_deferral()` is called or deferral is dropped |

When the last Arc drops, `CounterEntry` is freed. No manual ref counting needed.

---

## Bevy Integration

### The global dispatcher becomes a Bevy resource

C++ uses a global raw pointer `prv::gDispatcher`.
In Bevy the equivalent is a resource in the render world.

```rust
// Systems access it like any other resource
fn render_system(job_system: Res<LeetJobSystem>, ...) {
    let mut builder = job_system.create_builder(Priority::RenderPath);
    builder.dispatch_job("MyPass", move |ctx| { ... });
}
```

The job system lives in the render sub-app world, not the main app world.
It is inserted during plugin setup.

`leet_jobs2` keeps this dependency optional: enabling the crate's `bevy` feature
implements Bevy's `Resource` marker for `LeetJobSystem`, but all scheduling and
render-app registration lives at the engine boundary. The render crate inserts a
single `LeetJobSystem` into the render sub-app so systems can request
`Res<LeetJobSystem>` directly.

The render-side registration lives in `LeetJobPlugin`. That plugin owns the
resource insertion, the flush-thread claim system, and the shutdown guard. The
full render plugin installs `LeetJobPlugin` into the render sub-app before other
render plugins add systems that may use the job system.

### The flush thread is the render thread, not the OS main thread

C++ says "only the main thread can call FlushCounter."
The real rule is: only one designated thread can call `flush_counter`, and
that thread executes jobs while it waits.

In this engine, that thread is the **render thread** spawned by
`LeetPipelinedRenderingPlugin`. The OS main thread never calls `flush_counter`.

The job system stores which thread is allowed to flush:

```rust
struct Dispatcher {
    flush_thread_id: OnceLock<ThreadId>,  // set once, by the render thread
    // ... queues, workers, etc
}

impl LeetJobSystem {
    /// Called once at the start of the render thread's first frame.
    /// Panics if called more than once.
    pub fn claim_flush_thread(&self) {
        self.flush_thread_id
            .set(std::thread::current().id())
            .expect("flush thread already claimed");
    }

    pub fn flush_counter(&self, counter: &Counter) {
        debug_assert!(
            self.flush_thread_id.get() == Some(&std::thread::current().id()),
            "flush_counter called from wrong thread — must be called from the render thread"
        );
        // ... flush logic
    }
}
```

`claim_flush_thread` is called once by the render schedule before extraction,
prepare, render, or cleanup systems can flush counters. In pipelined rendering
that schedule runs on the dedicated render thread; in immediate rendering it
runs on the same thread that owns the render sub-app update.

### Plugin structure

```
LeetRenderPlugin
└── installs LeetJobPlugin into the render sub-app

LeetJobPlugin
└── inserts LeetJobSystem into render sub-app world
└── adds the first render-system set that claims the flush thread once
└── stores a private shutdown guard next to the public LeetJobSystem resource

LeetPipelinedRenderingPlugin
└── spawns render thread
└── render thread runs the same render schedule, so the claim happens there
```

The job system itself does not know it is inside Bevy.
It is a plain Rust struct that happens to be stored as a Bevy resource.

---

## jobSystem.h / jobSystem.cpp

C++ exposes a global job-system lifecycle:

```cpp
job::Initialize(setup);
job::Shutdown();
```

Rust should not mirror the global singleton. The equivalent is an owned
`LeetJobSystem` handle created by the plugin or by tests:

```rust
impl LeetJobSystem {
    pub fn new(config: JobSystemConfig) -> Self;
    pub fn shutdown(&self);
    pub fn claim_flush_thread(&self);
    pub fn num_worker_threads(&self) -> usize;
    pub fn current_thread_index() -> Option<u32>;
    pub fn dump_to_log(&self);
}
```

### Lifecycle Mapping

| C++ | Rust |
|---|---|
| `Initialize(const InitParam&)` | `LeetJobSystem::new(JobSystemConfig)` |
| `Shutdown()` | explicit `LeetJobSystem::shutdown()`; no joining `Drop` in v1 |
| global `prv::gDispatcher` | `Arc<Dispatcher>` behind `LeetJobSystem` |
| `InitializeJobMemoryPools()` | no v1 equivalent |

`shutdown()` should be idempotent if practical. C++ asserts that the global
dispatcher exists, then deletes it. Rust should be friendlier for tests and
plugin teardown: repeated shutdown should not double-join or double-drop worker
threads.

Plugin teardown should call `shutdown()` explicitly. Do not rely on
`Dispatcher::drop` as the primary shutdown path while worker threads hold
`Arc<Dispatcher>`.

### Thread Index Mapping

C++ `GetDispatcherThreadIndex()` returns:

- `0` for the main thread
- `UINT32_MAX` for threads outside the job system
- `1..N` for dispatcher workers

Rust should expose this as:

```rust
pub fn current_thread_index() -> Option<u32>;
```

Where:

- `Some(0)` means the claimed flush/render thread
- `Some(1..N)` means a worker thread
- `None` means a thread outside `leet_jobs2`

Worker threads set this in thread-local storage when they start. The render
thread gets index `0` when it calls `claim_flush_thread()`.

### Debug Helpers

| C++ | Rust v1 |
|---|---|
| `DumpToLog()` | `dump_to_log()`, allowed to be a stub first |
| `DebugTraceCall()` | omit or feature-gate with profiling/debugger work |
| `FatalAssertIfDispatcherThread()` | internal `panic_if_worker_thread()` only if needed |

The IO-priority thread-local code in `jobSystem.cpp` is out of scope for v1.

---

## Debugging / Profiling Hooks

Rust v1 keeps job names and hook points, but does not implement the C++ job
debugger.

### What Rust Keeps

- `JobDecl.name`
- `ParallelForJob.name`
- `RunContext.name`
- `CompletionDeferral.name`
- `JobSystemConfig::use_debugger` as a stub/config flag
- `LeetJobSystem::dump_to_log()` as a stub first

C++ `InstrumentationObject` maps to `&'static str` job names in Rust v1.

### Hook Points

All job execution should pass through one dispatcher/job-runner function:

```rust
inner.run_job_queue_entry(entry, thread_index, priority);
```

That function is the only required v1 hook point for profiling:

1. before the job closure runs
2. after the job closure returns
3. before decrementing the accumulate counter

Flush-executed jobs also call `run_job_queue_entry`, so they automatically use
the same hooks as worker-executed jobs.

### Suggested Internal Shape

Keep this small and no-op by default:

```rust
fn on_job_start(name: &'static str, thread_index: u32, priority: Priority) {}
fn on_job_finish(name: &'static str, thread_index: u32, priority: Priority) {}
```

These can later become `tracing` spans, profiler markers, or engine-specific
callbacks behind a feature flag.

### Out Of Scope

Rust v1 should not implement:

- C++ `Debugger`
- registered counter/deferral graph analysis
- `StackTraceCache`
- `StackTraceHandle`
- `DebugTraceCall`
- timeout `AnalyzeCounter`
- debugger stack-trace pointers on waiting-list entries
- C++ `JobDebugFlags`
- frame allocator debug bans
- `InstrumentationObject` identity/debug-name validation

The important v1 contract is that the execution path has stable hook points and
every job has a name.

---

## jobDispatcherInitParam.h / jobDispatcherInitParam.cpp

C++ `InitParam` becomes Rust `JobSystemConfig`.

```rust
pub struct JobSystemConfig {
    pub max_latent_jobs: usize,
    pub max_critical_path_jobs: usize,
    pub max_immediate_jobs: usize,
    pub worker_thread_stack_size: Option<usize>,
    pub max_threads: usize,
    pub all_jobs_critical_path: bool,
    pub use_debugger: bool,
}
```

Use bytes for `worker_thread_stack_size`; C++ stores this as KB. `None` means use
the Rust standard library default stack size.

### Field Mapping

| C++ field | Rust field | Keep? |
|---|---|---|
| `maxLatentJobs` | `max_latent_jobs` | yes |
| `maxCriticalPathJobs` | `max_critical_path_jobs` | yes |
| `maxImmediateJobs` | `max_immediate_jobs` | yes |
| `maxCore7Jobs` | none | no, console-specific |
| `workerThreadStackSizeKB` | `worker_thread_stack_size` | yes, optional |
| `maxThreads` | `max_threads` | yes |
| `allJobsCriticalPath` | `all_jobs_critical_path` | yes |
| `useJobDebugger` | `use_debugger` | keep as stub/feature flag |

### Defaults

C++ defaults are platform-tuned:

- latent queue: `32 * 1024`, multiplied by `4` on WinPC debug/editor builds
- critical-path queue: `16 * 1024`, multiplied by `4` on WinPC debug/editor builds
- immediate queue: `2 * 1024`
- worker stack: `1024 KB`
- max threads: `RED_MAX_JOB_THREADS`
- `allJobsCriticalPath = false`
- `useJobDebugger = false`

Rust v1 should not copy every platform macro. Use explicit defaults and make
them configurable. Suggested starting point:

```rust
impl Default for JobSystemConfig {
    fn default() -> Self {
        Self {
            max_latent_jobs: 32 * 1024,
            max_critical_path_jobs: 16 * 1024,
            max_immediate_jobs: 2 * 1024,
            worker_thread_stack_size: Some(1024 * 1024),
            max_threads: default_max_worker_threads(),
            all_jobs_critical_path: false,
            use_debugger: false,
        }
    }
}
```

`default_max_worker_threads()` should use available parallelism minus one,
clamped to at least one worker. Keep a configurable cap for tests and tools.

### Named Presets

C++ has `DefaultEditorInitParam()` and `DefaultToolInitParam()`.

Rust equivalents:

```rust
impl JobSystemConfig {
    pub fn editor() -> Self;
    pub fn tool() -> Self;
}
```

`editor()` can multiply queue capacities for headroom. `tool()` should mirror
the C++ intent: huge critical-path capacity, tiny latent/immediate queues, and
`all_jobs_critical_path = true`.

### C++-Specific Pieces To Drop

- crash-data registration from `InitParam::SetCrashData`
- console-specific `maxCore7Jobs`
- Xbox/PlayStation queue multipliers
- compile-time `RED_MAX_JOB_THREADS` platform matrix

---

## JobSystem as the Entry Point

In C++, `Counter` and `Builder` construct themselves by reaching out to the global
`prv::gDispatcher`.

In Rust there is no global dispatcher. Public construction goes through
`LeetJobSystem`, which is a cheap cloneable handle around shared job-system state.

```rust
#[derive(Clone)]
pub struct LeetJobSystem {
    inner: Arc<Dispatcher>,
}

#[derive(Clone)]
pub(crate) struct DispatcherHandle {
    inner: Arc<Dispatcher>,
}
```

Engine code normally accesses the job system from Bevy systems:

```rust
fn render_system(jobs: Res<LeetJobSystem>) {
    let mut builder = jobs.create_builder(Priority::RenderPath);

    builder.dispatch_job("MyPass", move |ctx| { ... });

    let counter = builder.extract_wait_counter();
    jobs.flush_counter(&counter);
}
```

Callers do not construct `Counter`, `Builder`, or `CompletionDeferral` directly.
They come from `LeetJobSystem` or from an existing `Counter`.

```rust
impl LeetJobSystem {
    pub fn create_counter(&self, priority: Priority) -> Counter;
    pub fn create_builder(&self, priority: Priority) -> Builder;
    pub fn create_builder_from_context(&self, ctx: &RunContext) -> Builder;
}
```

### Why public types store an internal handle

Some important job-system events happen after the Bevy system has returned, and
often on worker threads:

- a job finishes and decrements its accumulate counter
- a counter reaches zero and releases waiting jobs
- a `CompletionDeferral` is finished or dropped
- a `Builder` is dropped and finalizes its dependency chain
- a continuation builder links child work back into the parent job

These events cannot wait for a Bevy system. They must run immediately where the
lifecycle event happens.

So `Counter`, `Builder`, and `CompletionDeferral` store a private dispatcher
handle:

```rust
pub struct Counter {
    pub(crate) dispatcher: DispatcherHandle,
    pub(crate) entry: Arc<CounterEntry>,
}

pub struct Builder {
    dispatcher: DispatcherHandle,
    wait_counter: Counter,
    accum_counter: Counter,
    continuation_counter: Option<Arc<CounterEntry>>,
    priority: Priority,
    is_extracted: bool,
    debug_needs_fence: bool,
}

pub struct CompletionDeferral {
    dispatcher: DispatcherHandle,
    counter: Option<Arc<CounterEntry>>,
    is_finished: bool,
}
```

`CompletionDeferral::finish(&mut self)` requires unique mutable access in Rust v1.
Together with move-only/no-`Clone` ownership, safe Rust cannot race `finish()`
against `Drop`. C++ uses an atomic finished flag because its move/destructor model
allows `TryFinishDeferral()` to be called from multiple paths; Rust should not
copy that mechanically. If v2 ever makes deferrals shareable or changes finish to
`finish(&self)`, redesign the state around a lock/atomic-owned slot together.
Changing only `is_finished` to `AtomicBool` would not make
`counter: Option<Arc<CounterEntry>>` race-safe.

This handle is cheap to clone. It is just an `Arc` to shared job-system internals.
It does not clone queues, workers, or counters.

`Arc` is not used because the runtime needs to clone the entire job system. It is
used because queued jobs and RAII cleanup can outlive the Bevy system call that
created them, and Rust needs that lifetime to be represented safely. Alternatives
like raw pointers or leaked singletons would recreate the C++ global lifetime
assumption and make shutdown, tests, and restart much harder.

### Continuation builders

For continuation jobs, create the child builder through the job-system handle,
using the run context:

```rust
let job_system_handle = jobs.clone();

builder.dispatch_job("Parent", move |ctx| {
    let mut child = job_system_handle.create_builder_from_context(ctx);

    child.dispatch_job("Child", move |_ctx| {
        // child work
    });
});
```

Internally, `RunContext` carries the private dispatcher handle plus the parent
continuation counter:

```rust
pub struct RunContext {
    pub name: &'static str,
    pub thread_index: u32,
    pub parallel_for_index: i32,
    pub(crate) dispatcher: DispatcherHandle,
    pub(crate) continuation: ContinuationContext,
}

pub(crate) struct ContinuationContext {
    pub counter: Arc<CounterEntry>,
    pub param: ScheduleParam,
}
```

`RunContext.continuation.counter` is constructed from the same `Arc<CounterEntry>`
as the running job's own accumulate counter. The dispatcher decrements that
accumulate counter only after the job closure returns. Therefore, any continuation
builder created during the job can add child work to the same counter before the
parent job's own decrement happens, naturally extending the parent job lifetime.

This keeps normal engine code Bevy-centered while still allowing jobs to create
continuation work from worker threads.

### Bevy boundary

Bevy ECS is the entry point for frame/render systems, but job closures run outside
Bevy's schedule. Job closures must not capture `Res<T>`, `Commands`, `World`, or
borrowed ECS data.

If a job needs data, move owned data into the closure, usually through `Arc`,
engine handles, atomics, or other thread-safe containers.

---

## jobDeferral.h / jobDeferral.cpp

`CompletionDeferral` is one externally-controlled outstanding unit of work on a
counter. Conceptually it is a fake job.

Creation must increment the target counter before returning the deferral.

Finishing or dropping the deferral must decrement the target counter. If that
decrement reaches zero, waiting jobs are released immediately through the
dispatcher.

```rust
pub struct CompletionDeferral {
    dispatcher: DispatcherHandle,
    counter: Option<Arc<CounterEntry>>,
    name: &'static str,
    is_finished: bool,
}
```

`is_finished` is a plain bool in Rust v1 because `finish(&mut self)` requires
unique access and `CompletionDeferral` is move-only. Do not change this to only an
`AtomicBool` unless the rest of the state is redesigned too; the `counter:
Option<Arc<CounterEntry>>` slot would also need thread-safe ownership if deferrals
ever became shareable.

### Creation

Created through `Counter`, not directly:

```rust
impl Counter {
    pub fn create_deferral(&self, name: &'static str) -> CompletionDeferral;
}
```

Creation does:

1. increment counter value by one
2. clone/keep the counter entry alive
3. return `CompletionDeferral`

### Finish

```rust
impl CompletionDeferral {
    pub fn finish(&mut self);
}
```

`finish()` does:

1. panic if already finished
2. mark finished
3. take the counter entry
4. decrement counter through the dispatcher
5. flush waiting jobs if the counter reaches zero

### Drop

`Drop` auto-finishes unfinished deferrals. Drop must not panic for an already
finished deferral.

### Move / Clone

`CompletionDeferral` is move-only and must not implement `Clone`.

C++ supports a default inert deferral plus move assignment. Rust should prefer
`Option<CompletionDeferral>` for optional/inert storage.

### Debug Fields

C++ stores debug name, debug user data, and debugger registration state. Rust v1
keeps only `name: &'static str`. Debugger registration is out of scope.

---

## counter.rs file map (jobCounter.h + jobCounterOwner.h + jobCounterOwner.cpp)

All counter-related C++ files collapse into one Rust file.

```
counter.rs contains:

// Internal — pub(crate) only
pub(crate) struct WaitingJob {
    job: JobDecl,
    accum_counter: Arc<CounterEntry>,
}

pub(crate) type WaitingList = Vec<WaitingJob>;

pub(crate) struct CounterEntry {
    value: AtomicU32,                 // replaces DispatcherCounterValue
    waiting: Mutex<WaitingList>,      // replaces waitingJobsHead + waitingListLock
    priority: Priority,
    name: &'static str,
}
// Note: Arc<CounterEntry> replaces DispatcherRefCountMask entirely.
// Ref counting is handled by Arc. No manual ref count needed.

impl CounterEntry {
    pub(crate) fn new(priority: Priority, name: &'static str) -> Arc<Self>
    pub(crate) fn increment(&self)
    pub(crate) fn decrement(&self) -> bool        // true if hit zero
    pub(crate) fn flush_waiting(&self) -> WaitingList        // recheck under lock
    pub(crate) fn try_add_to_waiting(&self, job: WaitingJob) -> Result<(), WaitingJob>  // recheck under lock
    pub(crate) fn is_zero(&self) -> bool          // snapshot only, not authoritative
}

// Public handle — move only, no Clone
pub struct Counter {
    pub(crate) dispatcher: DispatcherHandle,
    pub(crate) entry: Arc<CounterEntry>,
}

impl Counter {
    // Internal constructor used by LeetJobSystem and builder/dispatcher internals.
    // Public callers never construct Counter directly.
    pub(crate) fn from_entry(dispatcher: DispatcherHandle, entry: Arc<CounterEntry>) -> Self
    pub fn create_deferral(&self, name: &'static str) -> CompletionDeferral
    pub fn reset(&mut self, other: Counter)   // replaces C++ Reset(Counter&&)
    pub fn is_zero(&self) -> bool             // snapshot
}

// Dependency composition — A += &B means A won't reach zero before B
// Internally queues an invisible empty job: waits on B, accumulates into A
// Skips if B is already zero (optimization from C++)
impl AddAssign<&Counter> for Counter { ... }

impl Drop for Counter { ... }  // Arc handles CounterEntry lifetime automatically
```

`AddAssign` uses `self.dispatcher` to queue the invisible empty job. This is why
`Counter` stores a `DispatcherHandle`; dependency composition must still work
after the counter has left the `LeetJobSystem` call that created it.

When internal code wraps an existing `Arc<CounterEntry>` into a `Counter`, it
must clone the `Arc`. This maps to the C++ private `Counter(CounterEntry&)`
constructor adding a dispatcher refcount. Do not move the only `Arc` out of the
owner that still needs it.

### counterValue vs refCountMask — what each was in C++ and what replaces it

| C++ field | Purpose | Rust replacement |
|---|---|---|
| `counterValue` | counts outstanding jobs | `AtomicU32 value` in CounterEntry |
| `refCountMask` | keeps CounterEntry alive | `Arc<CounterEntry>` ref count |

These are completely separate concerns. Do not conflate them.

Additional behavior from `DispatcherCounterValue`:

- increment/add returns whether the old value was zero
- decrement returns whether the new value is zero
- decrement must panic on underflow
- `is_zero_snapshot` is only a snapshot, not a synchronization guarantee by itself

`DispatcherRefCountMask` has no Rust equivalent beyond `Arc<CounterEntry>`.
Do not create a separate refcount type.

---

## jobCounterFunctions.h — No Rust Equivalent

This file is six free functions that are all one-line wrappers forwarding to `gDispatcher`.
It exists in C++ purely to hide the global dispatcher header from callers.

In Rust there is no global dispatcher. This file has no equivalent.
Its contents collapse into existing files:

| C++ free function | Rust equivalent | Lives in |
|---|---|---|
| `RunJob` | internal method, callers use `Builder` | `dispatcher.rs` |
| `RunParallelForJob` | internal method, callers use `Builder` | `dispatcher.rs` |
| `CreateDeferral` | `counter.create_deferral(name)` | `counter.rs` — method on Counter, returns CompletionDeferral from deferral.rs |
| `FlushCounter(const&, ...)` | `job_system.flush_counter(&counter)` | `dispatcher.rs` |
| `FlushCounter(&&, ...)` | same — Rust does not need two overloads | `dispatcher.rs` |
| `FlushCounterOnProcessFrame` | `job_system.flush_counter_render_frame(&counter)` | `dispatcher.rs` |

### The two FlushCounter overloads

C++ needs two overloads — one takes `const Counter&`, one takes `Counter&&` (consuming).
Rust does not need this. Take a reference and let the caller decide whether to drop after:

```rust
pub fn flush_counter(&self, counter: &Counter) -> bool
```

### FlushCounterOnProcessFrame

This is not a general flush. It has specific policy:
- Flushes at `RenderPath` priority
- Only processes large jobs if worker thread count is below 3

Keep it as a separate named method so the policy is explicit:

```rust
pub fn flush_counter_render_frame(&self, counter: &Counter) -> bool {
    let process_large = self.num_worker_threads() < 3;
    self.flush_counter_with_priority(counter, Priority::RenderPath, process_large)
}
```

The threshold of `3` comes from C++. Intent: if there are very few workers, the
flush thread also helps with large jobs to avoid starving the system. With enough
workers, large jobs can wait. Treat this as a starting point, not Rust tuning.

---

## jobDispatcherEntries.h

This file defines internal dispatcher payloads. Rust should preserve the roles,
not the C++ memory layout.

### WaitingListEntry

Already represented in `counter.rs` as:

```rust
pub(crate) struct WaitingJob {
    job: JobDecl,
    accum_counter: Arc<CounterEntry>,
}
```

C++ stores waiting jobs as a manually allocated singly linked list:

```cpp
WaitingListEntry* next;
```

Rust v1 stores them in `CounterEntry.waiting: Mutex<WaitingList>`, where
`WaitingList = Vec<WaitingJob>`.

Dropped from v1:

- `StackTraceCacheEntryPath* debugTrace`
- manual linked-list nodes
- memory pool allocation
- static size asserts

### JobQueueEntry

C++:

```cpp
struct JobQueueEntry {
    JobDecl jobDecl;
    CounterEntry* accumulateCounterEntry;
};
```

Rust:

```rust
pub(crate) struct JobQueueEntry {
    job: JobDecl,
    accum_counter: Arc<CounterEntry>,
}
```

A `JobQueueEntry` is a ready-to-run job. It must hold the accumulate counter alive
until the job finishes. After the job function returns, the dispatcher decrements
`accum_counter`.

In Rust v1, queued jobs should always have an accumulate counter. C++ has some
comments around nullable accumulate counters, but actual execution asserts one.

### Local Queue

C++ defines:

```cpp
const Uint32 c_localQueueDefaultCapacity = 256;
using TLocalQueue = red::CircularBuffer<std::pair<JobQueueEntry, Priority>>;
```

The normal local-queue optimization is disabled in C++ with:

```cpp
if (false)//useLocalQueue)
```

Rust v1 should not implement local worker queues. Use the global bounded priority
queues first. If bounded queue pressure becomes a real issue, define an explicit
backpressure policy instead of copying the hidden C++ emergency local fallback.

### ParallelForSharedCounterEntry

C++ uses this as a shared atomic work index for parallel-for team jobs:

```cpp
struct ParallelForSharedCounterEntry {
    Atomic<Uint32> counter;
};
```

Rust equivalent:

```rust
pub(crate) struct ParallelForSharedState {
    next_batch: AtomicU32,
    finished_teams: AtomicU32,
}
```

This is shared by team jobs with `Arc<ParallelForSharedState>`.
`next_batch` claims chunks. `finished_teams` lets exactly one team job observe
that all teams have exited the chunk loop and run the epilogue if present.

### ParallelForJobEntry

C++ stores raw callbacks, raw shared data, raw elements, shared counter, team
size, team index, and max batch size.

Rust should represent this through safe owned/shared state:

```rust
pub(crate) struct ParallelForTeamJob {
    job: Arc<ParallelForJob>,
    shared: Option<Arc<ParallelForSharedState>>,
    team_size: u32,
    team_index: u32,
}
```

Or these fields can be captured directly into the queued closure. The important
contract is behavioral, not the exact struct shape.

Each team job holds an `Arc<ParallelForJob>`. That outer Arc keeps the job
description, function, hint, name, and epilogue holder alive until all team jobs
finish. Do not move or clone the function separately per team job.

Must preserve:

- each team job knows its `team_index`
- multi-team jobs share one atomic batch counter
- multi-team jobs coordinate completion so the epilogue runs once
- `max_batch_size == 0` means automatic batching
- `team_size == 1` does not need a shared atomic
- epilogue runs exactly once after all chunks finish
- zero-element parallel-for still queues an empty/epilogue job to preserve chains

### C++-Specific Pieces To Drop

- memory pool macros
- static 64-byte size assertions
- linked-list `next`
- debugger stack-trace pointer
- `red::CircularBuffer` local queue in v1
- raw `void* sharedData`
- raw `void* elements`

---

## jobDispatcherQueue.h / jobDispatcherQueue.hpp

`DispatcherQueue` owns the ready-to-run job queues.

C++ uses one bounded MPMC queue per priority plus a semaphore used to wake
workers. Rust should preserve the behavior, not the exact lock-free queue
implementation.

MPMC means multi-producer, multi-consumer: many threads can push jobs and many
threads can pop jobs.

### Queue Shape

C++ priorities have separate queues:

```cpp
m_jobQueue[Priority::Latent]
m_jobQueue[Priority::RenderPath]
m_jobQueue[Priority::CriticalPath]
m_jobQueue[Priority::Immediate]
```

Rust equivalent:

```rust
pub(crate) struct ReadyQueues {
    latent: BoundedQueue<JobQueueEntry>,
    render_path: BoundedQueue<JobQueueEntry>,
    critical_path: BoundedQueue<JobQueueEntry>,
    immediate: BoundedQueue<JobQueueEntry>,
}
```

### Ordering

Each priority lane is FIFO for pop order.

The dispatcher chooses priority first, then FIFO order inside that priority:

1. `Immediate`
2. `CriticalPath`
3. `RenderPath`
4. `Latent`

Execution and completion order are not guaranteed, because multiple workers run
jobs concurrently after popping them.

A later high-priority job can be popped before an earlier low-priority job.
Flush can also disturb FIFO order when it pops a job, decides not to run it, and
requeues it behind other jobs in that priority lane.

### Capacity Mapping

C++ queue setup:

```cpp
numLowPriorityJobs    -> Latent queue
numNormalPriorityJobs -> RenderPath queue
numNormalPriorityJobs -> CriticalPath queue
numHighPriorityJobs   -> Immediate queue
```

Rust mapping:

- `max_latent_jobs` for `Latent`
- `max_critical_path_jobs` for both `RenderPath` and `CriticalPath`
- `max_immediate_jobs` for `Immediate`

If `all_jobs_critical_path` is true, C++ maps priorities to `CriticalPath` and
also sizes the latent queue from critical-path capacity. Rust should preserve
the priority-mapping behavior, with the `JobHint::Large` exception documented in
the dispatcher section. Exact queue capacity adjustment can stay an implementation
detail as long as no mapped jobs can overflow unexpectedly.

### Push

C++:

```cpp
Bool TryPush(entry, priority, affinity)
```

Rust:

```rust
pub(crate) fn try_push(
    &self,
    entry: JobQueueEntry,
    priority: Priority,
) -> Result<(), QueueFull>;
```

C++ `TryPush` is named like a fallible call, but the current underlying
`LockFreeQueueMPMCExternalBuffer::Push()` busy-waits while full and then returns
true. Rust v1 should not copy hidden spin-until-space behavior blindly.

Queue-full behavior must be explicit. Prefer a clear panic or error on capacity
exhaustion over a hidden worker deadlock.

Pushing a job must wake one blocked worker.

### Pop

Worker pop blocks until a job is available or shutdown begins:

```rust
pub(crate) fn pop_blocking(&self) -> Option<(JobQueueEntry, Priority)>;
```

It must scan queues in strict priority order.

### Try Pop

Flush uses nonblocking pop:

```rust
pub(crate) fn try_pop(&self) -> Option<(JobQueueEntry, Priority)>;
```

It uses the same strict priority order and returns `None` if no job is available.

### Wakeup Model

C++ uses `DispatcherSemaphore`:

- push releases the semaphore
- worker pop acquires it
- flush try-pop uses try-acquire
- shutdown wakes blocked workers indirectly through shutdown signaling/null jobs

Rust can use channels, `Mutex + Condvar`, or another primitive, but must preserve:

- workers block when no jobs are available
- pushing a job wakes a worker
- flush can attempt nonblocking pop
- shutdown wakes blocked workers

### Affinity Queues

C++ supports special queues for `ConsoleCore7` and `ResourceThrottler`.

Rust v1 drops these. All jobs use the normal priority queues.

### C++-Specific Pieces To Drop

- `LockFreeQueueMPMCExternalBuffer`
- custom spin counts
- external queue storage arrays
- semaphore implementation details
- `ConsoleCore7` queue
- `ResourceThrottler` queue
- queue memory pools

---

## jobDispatcherThread.h / jobDispatcherThread.cpp

`DispatcherThread` owns one worker thread.

C++ worker responsibilities:

1. initialize thread-local job-system state
2. block/pop ready jobs from dispatcher queues
3. run jobs through `Dispatcher::DoRunJobQueueEntry`
4. exit when dispatcher shutdown is requested

Rust equivalent lives in `worker.rs`.

### Worker Setup

C++ sets:

```cpp
g_tls_IsDispatcherThread = true;
g_tls_DispatcherThreadIndex = dispatcherThreadIndex;
```

Rust should do the same with thread-local state:

```rust
thread_local! {
    static THREAD_INDEX: Cell<Option<u32>> = const { Cell::new(None) };
}
```

Worker threads get indices `1..N`.

The claimed flush/render thread uses index `0`.

Threads outside the job system return `None` from `current_thread_index()`.

### Worker Loop

C++ normal loop:

```cpp
loop {
    entry, priority = pop job
    if exit requested { break }
    run job entry
}
```

Rust equivalent:

```rust
fn worker_loop(index: u32, inner: Arc<Dispatcher>) {
    set_current_thread_index(Some(index));

    while let Some((entry, priority)) = inner.ready_queues.pop_blocking() {
        if inner.is_shutdown() {
            break;
        }

        inner.run_job_queue_entry(entry, index, priority);
    }

    set_current_thread_index(None);
}
```

The exact shutdown check can live inside `pop_blocking()` or outside it. The
required behavior is that blocked workers wake and exit during shutdown.

### Local Queue

C++ workers have a local queue:

```cpp
TLocalQueue m_localQueue;
```

The worker drains it before popping globally.

However, the normal local-queue optimization is disabled in dispatcher code. Rust
v1 should not implement local worker queues.

### Readiness

C++ has `IsReady()` so initialization can wait for worker startup.

Rust can track this if needed for tests/debugging, but it is not core behavior.
Starting workers in `LeetJobSystem::new()` and keeping their join handles is
enough for v1 unless tests need a readiness barrier.

### Thread Names and Stack Size

C++ names workers `redDispatcher1`, `redDispatcher2`, etc. and uses configured
stack size.

Rust should preserve the useful parts:

```rust
std::thread::Builder::new()
    .name(format!("leet_dispatcher_{index}"))
```

Use `JobSystemConfig::worker_thread_stack_size` when present.

### Job Execution Hook

Workers should not call job closures directly from `worker.rs`.

They should call a dispatcher/job-runner function:

```rust
inner.run_job_queue_entry(entry, thread_index, priority);
```

That function is responsible for:

- building `RunContext`
- running instrumentation/profiling hooks
- invoking the job closure
- decrementing the accumulate counter after the job returns
- flushing waiting jobs if the counter reaches zero

This keeps the worker loop simple.

### Console / Resource Threads

C++ has special loops for:

- `DoConsoleCore7WorkLoop`
- `DoResourceThrottlerWorkLoop`

Rust v1 drops these. All workers use the normal ready queues.

### C++-Specific Pieces To Drop

- platform affinity masks
- `ConsoleCore7` worker loop
- `ResourceThrottler` worker loop
- memory registration
- job-scope memory allocator
- profiler thread initialization in v1
- local queue in v1

---

## jobDispatcher.h / jobDispatcher.cpp — Surface Map

C++ `prv::Dispatcher` is the real job-system owner behind global
`prv::gDispatcher`.

Rust splits that role into a public handle and private shared state:

```rust
#[derive(Clone)]
pub struct LeetJobSystem {
    inner: Arc<Dispatcher>,
}

pub(crate) struct Dispatcher {
    config: JobSystemConfig,
    ready_queues: ReadyQueues,
    worker_handles: Mutex<Vec<JoinHandle<()>>>,
    shutdown: AtomicBool,
    flush_thread_id: OnceLock<ThreadId>,
    is_flushing: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct DispatcherHandle {
    inner: Arc<Dispatcher>,
}
```

`LeetJobSystem` is the public handle. `DispatcherHandle` is the private handle
stored by `Counter`, `Builder`, `CompletionDeferral`, and `RunContext`.

Important ownership note:

Worker threads may hold `Arc<Dispatcher>` while running. Rust v1 should use
explicit, idempotent `shutdown()` to signal workers and join handles. Do not
rely on `Dispatcher::drop` as the primary shutdown path unless the final
ownership design proves workers cannot keep inner state alive forever.

### C++ Fields Mapping

| C++ `Dispatcher` field | Rust equivalent |
|---|---|
| `InitParam m_setup` | `JobSystemConfig config` |
| `Priority m_priorityMap[]` | `map_priority(...)`, derived from config |
| `DispatcherQueue<JobQueueEntry> m_dispatcherQueue` | `ReadyQueues ready_queues` |
| `DynArray<DispatcherThread> m_dispatcherThreads` | worker join handles |
| `Debugger* m_debugger` | feature-gated/debug stub later |
| `JobScopeMemoryAllocator* m_jobScopeAllocator` | dropped in v1 |
| `Atomic<Bool> m_isExitRequested` | `shutdown: AtomicBool` |
| `Atomic<Bool> m_shouldPumpMessagesForDXGI` | dropped in v1 |
| `Bool m_isFlushingCounter` | `is_flushing: AtomicBool` |

`is_flushing` is an `AtomicBool` for `Dispatcher: Sync` compatibility. It is
only accessed from the claimed render/flush thread; no cross-thread synchronization
policy is intended beyond the reentrancy guard.

### Method Groups

#### Lifecycle

C++:

```cpp
Dispatcher(const InitParam&);
~Dispatcher();
Init();
InitJobQueue(setup);
Shutdown();
GetNumDispatcherThreads(includeCore7);
```

Rust:

```rust
impl LeetJobSystem {
    pub fn new(config: JobSystemConfig) -> Self;
    pub fn shutdown(&self);
    pub fn num_worker_threads(&self) -> usize;
}
```

Detailed shutdown behavior is covered in `jobDispatcher.cpp — Shutdown` below.

#### Counter Creation / Deferral

C++:

```cpp
InitJobCounter(...);
CreateDeferral(...);
IsZero_Snapshot(...);
AddRefJobCounterInternal(...);
ReleaseJobCounterInternal(...);
```

Rust:

```rust
impl LeetJobSystem {
    pub fn create_counter(&self, priority: Priority) -> Counter;
}

impl Counter {
    pub fn create_deferral(&self, name: &'static str) -> CompletionDeferral;
    pub fn is_zero(&self) -> bool;
}
```

`AddRef` / `Release` have no direct Rust method. `Arc<CounterEntry>` owns the
counter-entry lifetime.

#### Job Dispatch

C++:

```cpp
RunJob(...);
RunParallelForJob(...);
QueueJobOrWait(...);
TryPutOnWaitingList(...);
QueueJobAndSignal(...);
InitWithEmptyJob(...);
```

Rust internal:

```rust
impl Dispatcher {
    pub(crate) fn run_job(...);
    pub(crate) fn run_parallel_for(...);
    pub(crate) fn queue_job_or_wait(...);
}
```

Detailed dispatch behavior comes later.

#### Worker Queue API

C++:

```cpp
PopJobQueueEntry(...);
TryPopJobQueueEntry(...);
DoRunJobQueueEntry(...);
IsExitRequested();
```

Rust:

```rust
impl Dispatcher {
    pub(crate) fn pop_ready_job_blocking(&self) -> Option<(JobQueueEntry, Priority)>;
    pub(crate) fn try_pop_ready_job(&self) -> Option<(JobQueueEntry, Priority)>;
    pub(crate) fn run_job_queue_entry(
        &self,
        entry: JobQueueEntry,
        thread_index: u32,
        priority: Priority,
    );
    pub(crate) fn is_shutdown(&self) -> bool;
}
```

#### Flush

C++:

```cpp
FlushCounter(...);
```

Rust:

```rust
impl LeetJobSystem {
    pub fn flush_counter(&self, counter: &Counter) -> bool;
    pub fn flush_counter_with_timeout(&self, counter: &Counter, timeout: Duration) -> bool;
    pub fn flush_counter_render_frame(&self, counter: &Counter) -> bool;
}
```

Detailed flush behavior comes later.

#### Priority Mapping

C++:

```cpp
MapPriority(priority);
```

Rust:

```rust
fn map_priority(priority: Priority, config: &JobSystemConfig) -> Priority;
```

If `all_jobs_critical_path` is true, normal priorities map to `CriticalPath`.
`JobHint::Large` is an explicit exception at queueing time: large jobs stay
`Latent` so urgent flush paths do not accidentally pick up long-running work.

### C++-Specific Pieces To Drop

- global `prv::gDispatcher`
- manual counter refcount methods
- `JobScopeMemoryAllocator`
- Win32/DXGI message pump reminder logic in v1
- debugger object in v1
- platform affinity/resource-throttler/core-7 details

---

## jobDispatcher.cpp — Dispatch Path

This pass covers single-job dispatch: `RunJob`, `QueueJobOrWait`,
`TryPutOnWaitingList`, `QueueJobAndSignal`, and `InitWithEmptyJob`.

### C++ Flow

```cpp
RunJob(job, wait_counter, accum_counter)
    increment accum_counter
    QueueJobOrWait(job, wait_counter, accum_counter)

QueueJobOrWait(...)
    if wait_counter exists and job can be parked:
        put job on wait_counter waiting list
    else:
        QueueJobAndSignal(job, accum_counter)

DecrementCounterEntryInternal(wait_counter)
    if wait_counter reaches zero:
        flush waiting list
        QueueJobAndSignal(each waiting job, its accum_counter)
```

### RunJob Contract

`RunJob` owns the "job becomes outstanding" transition.

Rules to preserve:

- validate the job before dispatch
- reject direct self-dependency: `wait_counter` and `accum_counter` cannot be the same
- increment `accum_counter` before the job is queued or added to a waiting list
- do not increment again when a waiting job later moves to the ready queue

Rust internal shape:

```rust
pub(crate) fn run_job(
    &self,
    job: JobDecl,
    wait_counter: Option<Arc<CounterEntry>>,
    accum_counter: Arc<CounterEntry>,
)
```

Unlike C++, Rust should move the `JobDecl`. It cannot copy `FnOnce` closures.
The job moves into exactly one place: either the waiting list or the ready queue.

### QueueJobOrWait Contract

`QueueJobOrWait` decides whether the job is ready now or must wait for another
counter to reach zero.

```rust
pub(crate) fn queue_job_or_wait(
    &self,
    job: JobDecl,
    wait_counter: Option<Arc<CounterEntry>>,
    accum_counter: Arc<CounterEntry>,
)
```

Behavior:

- if there is no wait counter, queue immediately
- if the wait counter is already zero, queue immediately
- otherwise try to park the job on the wait counter waiting list
- if the counter becomes zero during parking, queue immediately instead

### TryPutOnWaitingList Contract

C++ does two checks:

1. fast zero snapshot before allocating/inserting
2. locked zero recheck before linking into the waiting list

Rust must preserve the locked recheck:

```rust
fn try_put_on_waiting_list(
    &self,
    job: JobDecl,
    wait_counter: &Arc<CounterEntry>,
    accum_counter: Arc<CounterEntry>,
) -> Result<(), WaitingJob>
```

The return type does not need to be exactly this. The important part is that if
the job cannot be parked, ownership of the job is returned so it can be queued.

Waiting-list ordering is not semantic for correctness. Jobs parked on the same
counter must be independent siblings; if an ordering matters, the caller must
express it with a builder fence or another dependency counter.

For legacy parity, preserve the current observable release order: C++ prepends
to a linked list and then walks from the head, so jobs parked on the same counter
release in LIFO order. Rust can keep `WaitingList = Vec<WaitingJob>` as long as
insertion uses `push()` and release queues jobs by `pop()` from the end. Do not
iterate the vector front-to-back during flush unless we intentionally choose and
test FIFO behavior.

### QueueJobAndSignal Contract

`QueueJobAndSignal` converts a job into a ready `JobQueueEntry`, chooses the
ready-queue priority, pushes it, and wakes one worker.

Rust internal shape:

```rust
pub(crate) fn queue_job_and_signal(
    &self,
    job: JobDecl,
    accum_counter: Arc<CounterEntry>,
)
```

Priority rules:

- normally use `accum_counter.priority`
- if `all_jobs_critical_path` is enabled, priorities are already mapped when the
  counter is created
- if `job.hint == JobHint::Large` and `all_jobs_critical_path` is enabled, queue
  as `Latent` instead of `CriticalPath`
- `JobHint::PhysX` is dropped in Rust v1, so do not preserve its special case

C++ applies the `Large`/`allJobsCriticalPath` exception behind an editor build
guard. Rust v1 should treat it as always active when `all_jobs_critical_path` is
true; there is no matching editor-only build mode in this crate.

Pushing must wake one blocked worker. Queue-full behavior remains the explicit
policy from the queue section.

### InitWithEmptyJob

C++ creates a trivial no-op job for cases that still need to preserve dependency
chains, especially empty parallel-for dispatch.

Rust equivalent can be a small constructor:

```rust
JobDecl::empty("EmptyJob")
```

It should create a `JobHint::Trivial` no-op job. It still increments and
decrements its accumulate counter like any other job.

### C++-Specific Pieces To Drop

- debugger stack trace pointer passed into waiting-list entries
- manual waiting-list entry allocation/free
- local queue fallback
- `JobHint::PhysX` priority special case
- nullable accumulate counters in normal execution

---

## jobDispatcher.cpp — Counter Decrement and Waiting-List Flush

This pass covers what happens after a job finishes running.

C++ path:

```cpp
DoRunJobQueueEntry(...)
    run job function
    DecrementCounterEntryInternal(accumulateCounterEntry)

DecrementCounterEntryInternal(counter)
    decrement counter value
    if value is not zero:
        return

    entries = FlushWaitingList(counter)

    release dispatcher refcount / maybe free counter memory

    for each waiting entry:
        QueueJobAndSignal(entry.job, entry.accumulateCounterEntry)
```

### Decrement Contract

`DecrementCounterEntryInternal` is called exactly once after a queued job finishes
or a completion deferral finishes.

Rules to preserve:

- decrement the outstanding job count
- panic on counter underflow
- if the new value is nonzero, do nothing else
- if the new value is zero, try to release jobs waiting on this counter
- waiting jobs are queued without incrementing their accumulate counter again

Rust internal shape:

```rust
pub(crate) fn decrement_counter_entry(&self, counter: Arc<CounterEntry>)
```

The function may take `Arc<CounterEntry>` or `&Arc<CounterEntry>` depending on
the final call sites. The important behavior is that the counter entry stays
alive while decrement/flush logic is running.

### FlushWaitingList Contract

C++ does not blindly flush the waiting list after decrement reaches zero.

It locks the waiting-list mutex and checks the counter value again while holding
the lock:

```cpp
lock waiting list
if counter value is not zero:
    return null
take waiting list
unlock
```

Rust must preserve this.

```rust
fn take_waiting_jobs_if_still_zero(counter: &CounterEntry) -> WaitingList
```

Behavior:

- lock `counter.waiting`
- recheck `counter.value == 0` while holding the lock
- if value is nonzero, return an empty list
- otherwise drain/take the waiting jobs
- release the lock before queueing those jobs

Do not queue jobs while holding the waiting-list lock.

### Why The Locked Recheck Matters

There is a race if the dispatcher drains the waiting list just because one
decrement observed zero.

A counter can hit zero, then be reused/incremented again before the old decrement
thread gets around to flushing the waiting list. If another job parks behind that
new nonzero counter, flushing without the locked recheck would release that job
too early.

So the rule is:

> A waiting list may only be drained while holding its lock and while the counter
> value is still zero under that same lock.

This is the matching half of the `TryPutOnWaitingList` locked recheck.

### Refcount / Lifetime Mapping

C++ releases `DispatcherRefCountMask` after the counter reaches zero and may free
the raw `CounterEntry` before queueing waiting jobs.

Rust does not do this manually.

Rust replacement:

- `JobQueueEntry` owns `Arc<CounterEntry>` for its accumulate counter
- `WaitingJob` owns `Arc<CounterEntry>` for its accumulate counter
- `Counter`, `CompletionDeferral`, and continuation state also own Arcs
- after waiting jobs are drained, the wait counter is freed automatically when
  the last `Arc<CounterEntry>` is dropped

Do not create a Rust `DispatcherRefCountMask`.

### Queueing Released Waiting Jobs

After the waiting list is drained, each waiting job is moved into the ready
queue. Release jobs in LIFO order to match the linked-list prepend behavior
while keeping the ordering non-semantic for callers:

```rust
while let Some(waiting_job) = waiting_jobs.pop() {
    self.queue_job_and_signal(waiting_job.job, waiting_job.accum_counter);
}
```

When a `WaitingJob` becomes a `JobQueueEntry`, its `Arc<CounterEntry>` for the
accumulate counter is moved into the queued entry. Do not clone it for semantics,
and do not increment the accumulate counter again.

Do not call `run_job` here. These jobs were already counted as outstanding when
they were first dispatched. Calling `run_job` would increment their accumulate
counter a second time.

### C++-Specific Pieces To Drop

- manual linked-list traversal/free
- `DispatcherRefCountMask::Release`
- freeing `CounterEntry` inside decrement logic
- local queue fallback
- debugger waiting-list trace pointer

---

## jobDispatcher.cpp — FlushCounter

`FlushCounter` waits until a counter reaches zero, but it does not simply block.
The flush thread helps execute eligible queued jobs while waiting.

In LEET, the flush thread is the claimed render thread.

### Public Rust API

```rust
impl LeetJobSystem {
    pub fn flush_counter(&self, counter: &Counter) -> bool;
    pub fn flush_counter_with_timeout(
        &self,
        counter: &Counter,
        timeout: Duration,
    ) -> bool;

    pub fn flush_counter_render_frame(&self, counter: &Counter) -> bool;
}
```

`flush_counter` is the normal wait-until-zero call.

`flush_counter_render_frame` preserves the C++ `FlushCounterOnProcessFrame`
policy:

- process priority threshold is `RenderPath`
- large jobs are only processed if worker count is below `3`

### Flush Thread Rules

C++ asserts:

- flush can only run on the main thread
- flush cannot run from inside a job
- flush cannot be reentrant

Rust rules:

- flush can only run on the claimed flush/render thread
- flush must panic if called while already flushing
- jobs executed by the flush loop use `thread_index = 0`

```rust
self.assert_flush_thread();

if self.is_flushing.swap(true, Ordering::AcqRel) {
    panic!("reentrantly flushing counter");
}
```

Use a guard to reset `is_flushing` when the function exits.

### Core Loop

C++ shape:

```cpp
while (!counter.is_zero()) {
    if timeout expired:
        return false

    if try_pop_ready_job(entry, priority) {
        if should_run_during_flush(entry, priority, target_counter, policy) {
            run_job_queue_entry(entry, thread_index = 0, priority)
        } else {
            queue_job_and_signal(entry.job, entry.accumulateCounterEntry)
        }
    } else {
        pump messages / idle hook
    }
}

return true
```

Rust shape:

```rust
while !counter.entry.is_zero_snapshot() {
    if timed_out {
        return false;
    }

    match self.try_pop_ready_job() {
        Some((entry, priority)) if self.should_run_during_flush(&entry, priority, &policy) => {
            self.run_job_queue_entry(entry, 0, priority);
        }
        Some((entry, _priority)) => {
            self.requeue_job(entry);
            self.wait_for_queue_wakeup_timeout(FLUSH_IDLE_WAIT, || !counter.entry.is_zero());
        }
        None => {
            self.wait_for_ready_job_timeout(FLUSH_IDLE_WAIT, || !counter.entry.is_zero());
        }
    }
}

true
```

### Which Jobs Flush May Run

C++ runs a popped job during flush if any of these is true:

1. the job accumulates into the counter being flushed
2. the job priority is at least the flush priority threshold and it is not blocked by `Large`
3. the job is `Trivial`

Rust should preserve that policy:

```rust
fn should_run_during_flush(
    entry: &JobQueueEntry,
    priority: Priority,
    target: &Arc<CounterEntry>,
    policy: FlushPolicy,
) -> bool {
    let same_counter = Arc::ptr_eq(&entry.accum_counter, target);
    let trivial = entry.job.hint == JobHint::Trivial;
    let large = entry.job.hint == JobHint::Large;
    let priority_allowed = priority >= policy.min_priority;
    let size_allowed = policy.process_large || !large;

    same_counter || trivial || (priority_allowed && size_allowed)
}
```

`JobHint::AudioEvent` is dropped in Rust v1, so `Large` is the only large-job
flush exclusion.

### Priority Threshold

C++ overload:

```cpp
FlushCounter(counter, processLatent, timeout)
```

maps to:

```cpp
processPriority = processLatent ? Latent : counter.priority
processLarge = true
```

Rust v1 should expose the simple API first:

```rust
pub fn flush_counter(&self, counter: &Counter) -> bool
```

Internal policy:

```rust
FlushPolicy {
    min_priority: counter.entry.priority,
    process_large: true,
}
```

If a real caller needs `processLatent`, add a clearly named method later:

```rust
pub fn flush_counter_including_latent(&self, counter: &Counter) -> bool
```

Do not expose a vague boolean parameter.

### Requeue Behavior

If the flush thread pops a job it should not run, C++ pushes it back onto the
ready queue.

Rust must do the same:

```rust
self.queue_job_and_signal(entry.job, entry.accum_counter);
```

Important: this must not call `run_job`, because the job was already counted as
outstanding.

Requeue uses the normal signal path and may wake a worker. That is intentional:
if the flush thread declines to run a job, workers are the right place for it to
make progress. Do not add a silent requeue path in v1 just to avoid wakeups.

After requeueing an ineligible job, the flush loop should briefly wait on the
queue wake path even though the queue is nonempty. This prevents the flush thread
from repeatedly popping the same ineligible job in a tight loop while still
capping the quiet-case latency through the same small timeout used for empty
queue idle waits.

This can disturb FIFO order for skipped jobs. That is acceptable because C++ also
requeues skipped jobs.

### Timeout Behavior

C++ timeout:

- `timeoutMilliseconds == -1` means wait forever
- if timeout expires, optionally analyzes the counter through the debugger
- returns `false`

Rust:

- `flush_counter` waits forever
- `flush_counter_with_timeout` returns `false` on timeout
- debugger analysis is out of scope for v1

### Idle Behavior

C++ has a Win32/DXGI message pumper when no job can be popped.

Rust v1 should not copy that.

When no job is available:

- wait briefly on the ready-queue condvar with a small timeout
- wake immediately if a ready job is pushed
- cap the quiet-case latency for counter-only completion checks

Use a named timeout constant, and treat it as a fallback only. Queue pushes must
wake the queue wait path, and a counter zero transition must also wake flush
waiters even if no ready job is queued. A good v1 value after those wake paths
are reliable is `5us`, with the wait-side counter predicate checked while
holding the queue wait mutex so the zero transition cannot be missed just before
sleeping.

### C++-Specific Pieces To Drop

- Win32/DXGI message pumper
- debugger `AnalyzeCounter` on timeout in v1
- `JobHint::AudioEvent`
- `processLatent` boolean public API
- setting C++ dispatcher-thread TLS during flush

---

## jobDispatcher.cpp — Parallel-For Dispatch

`RunParallelForJob` expands one logical parallel-for into one or more normal
queued jobs. Each queued team job still follows the normal `RunJob` path:
increment accumulate counter, wait if needed, queue when ready, decrement after
running.

### C++ Flow

```cpp
RunParallelForJob(job, wait_counter, accum_counter)
    max_team_size = worker_count + 1
    team_size = min(num_elements, max_team_size)

    if num_elements == 0:
        queue epilogue-only job if epilogue exists
        otherwise queue empty job
        return

    if team_size == 1:
        queue one team job, no shared atomic
    else:
        allocate shared atomic batch counter
        queue team_size team jobs
```

The `+1` is important: C++ includes the flush/main thread as a possible worker.

### Team Size

C++ computes initial team size as:

```cpp
num_batches = ceil(num_elements / 1)
team_size = min(num_batches, worker_count + 1)
```

So effectively:

```rust
team_size = min(num_elements, num_worker_threads + 1)
```

For `num_elements == 0`, team size is `0`.

Rust should preserve:

- never spawn more team jobs than elements
- include the flush thread in the max team size
- use one job for a one-element or single-team parallel-for
- use zero normal team jobs for zero elements

### Multi-Team Execution

For `team_size > 1`, C++ shares one atomic counter between all team jobs.

Each team job loops:

```cpp
index = shared_counter.increment() - 1

if index < batch_count:
    run chunk [start, end)
else:
    maybe run epilogue if this is the last team job to exit
    break
```

Rust equivalent:

```rust
struct ParallelForSharedState {
    next_batch: AtomicU32,
    finished_teams: AtomicU32,
}
```

Each team job repeatedly claims the next batch until no batches remain.

### Batch Size

C++ behavior:

- if `maxBatchSize == 0`, batch count is the team size
- otherwise batch count is calculated from `numElements / maxBatchSize`
- then actual batch size is `ceil(numElements / batch_count)`

Important: despite the name, C++ `maxBatchSize` is not a strict cap in all cases.
Some counts can produce a chunk larger than `maxBatchSize`.

Rust v1 should preserve the C++ formula and document that the name is misleading.
If Rust later enforces a true maximum, rename the internal meaning clearly and
test the new behavior.

### RunContext

For normal chunk execution:

```rust
ctx.parallel_for_index = team_index as i32;
```

This is the team-job index, not the claimed batch index.

For epilogue execution:

```rust
ctx.parallel_for_index = -1;
```

C++ intentionally runs epilogue with the original run context, not the copied
parallel-for chunk context.

### Epilogue

The epilogue runs exactly once after all chunks finish.

C++ runs it inside the last team job to observe completion. This matters because
the accumulate counter for that team job is not decremented until the epilogue
returns, so the outer counter cannot reach zero before epilogue completes.

Rust should preserve that ordering:

- do not queue the epilogue as unrelated fire-and-forget work
- the logical parallel-for counter must not reach zero before epilogue finishes
- use `TakeOnceEpilogue` so exactly one team job runs it

### Single-Team Execution

If `team_size == 1`:

- no shared atomic counter is needed
- one job processes `[0, num_elements)`
- epilogue, if present, runs immediately after the chunk in that same job
- the chunk context has `parallel_for_index = 0`
- the epilogue context has `parallel_for_index = -1`

### Zero Elements

C++ still queues work for dependency correctness.

If `num_elements == 0`:

- with epilogue: queue one epilogue-only job
- without epilogue: queue one empty trivial job

Either way the accumulate counter is incremented and decremented through the
normal job lifecycle.

### C++ Public Shim vs Rust API

C++ public `DispatchParallelForJob` accepts a per-index lambda:

```cpp
func(index, runContext)
```

Internally, `JobDeclParallelFor` receives ranges:

```cpp
jobFunc(sharedData, elements, start, end, runContext)
```

The C++ shim loops `start..end` and calls the user lambda once per index.

Rust internal storage is already range-based:

```rust
Fn(u32, u32, &RunContext)
```

That is fine for v1. A per-index convenience wrapper can be added later if engine
call sites want C++-style syntax.

### Dynamic Element Count Overloads

C++ also has overloads that take a `numElementsFunc` instead of an immediate
element count.

Those overloads do not compute the count on the caller thread. They first queue a
normal continuation job behind the requested wait counter. When that job runs, it
calls `numElementsFunc()`, builds the real parallel-for, and accumulates the team
jobs into the continuation counter for that running job.

Rust v1 should omit this overload unless a real caller needs it. Use an eager
`u32` count at the public API boundary.

If dynamic counts are added later, preserve the C++ behavior:

- the count closure runs as a scheduled job after dependencies are satisfied
- the spawned parallel-for is linked through the running job's continuation
  counter
- the original accumulate counter cannot reach zero before the generated
  parallel-for work finishes

### C++-Specific Pieces To Drop

- raw `sharedData`: Rust closures capture typed shared data directly
- raw `elements`: Rust closures capture typed slices, buffers, or handles directly
- C++-style `initSharedDataCallback` in v1; add a Rust-native setup callback later
  only if a real caller needs team-size-dependent shared state
- dynamic `numElementsFunc` parallel-for overloads in v1
- manual allocation/free of `ParallelForJobEntry`
- manual allocation/free of `ParallelForSharedCounterEntry`
- console core-7 single-worker special case
- IO-priority thread-local setup

---

## jobDispatcher.cpp — Shutdown

C++ shutdown is idempotent and does not drain all outstanding work. It requests
exit, wakes worker threads, joins them, then clears the worker list.

### C++ Flow

```cpp
Shutdown()
    m_isExitRequested = true
    dispatcherQueue.Shutdown(...)

    create one dummy counter per priority

    for each dispatcher thread:
        QueueJobAndSignal(nullJob, latentCounter)
        QueueJobAndSignal(nullJob, renderCounter)
        QueueJobAndSignal(nullJob, criticalCounter)
        QueueJobAndSignal(nullJob, immediateCounter)

    for each dispatcher thread:
        JoinThread()

    release dummy counters
    clear dispatcherThreads
```

Normal workers exit like this:

```cpp
loop:
    PopJobQueueEntry(entry, priority)
    if dispatcher.IsExitRequested():
        break
    DoRunJobQueueEntry(entry, ...)
```

The dummy jobs are wakeup tokens. Workers pop something, see exit requested, and
exit before running the dummy job.

### Rust Shutdown Contract

Rust v1 should preserve the important behavior:

- shutdown is explicit
- shutdown is idempotent
- shutdown wakes blocked workers
- shutdown joins worker threads
- shutdown does not drain arbitrary pending jobs
- shutdown should happen only at a stable teardown point

```rust
impl LeetJobSystem {
    pub fn shutdown(&self);
}
```

Internal shape:

```rust
pub(crate) struct Dispatcher {
    shutdown: AtomicBool,
    worker_handles: Mutex<Vec<JoinHandle<()>>>,
    ready_queues: ReadyQueues,
}
```

Suggested implementation shape:

```rust
pub fn shutdown(&self) {
    if self.inner.shutdown.swap(true, Ordering::AcqRel) {
        return;
    }

    self.inner.ready_queues.wake_all_workers();

    let handles = std::mem::take(&mut *self.inner.worker_handles.lock().unwrap());
    for handle in handles {
        handle.join().expect("job worker panicked during shutdown");
    }
}
```

Exact locking/poisoning policy can be chosen during implementation.

### Wakeup Model

C++ wakes normal workers by queueing dummy jobs. The queue shutdown method mainly
exists for special console/resource queues; normal workers are woken by the dummy
ready entries.

Rust does not need dummy jobs if the queue primitive supports shutdown wakeup.

Preferred Rust behavior:

```rust
ready_queues.shutdown();
ready_queues.notify_all();
```

Then worker pop returns `None`:

```rust
while let Some((entry, priority)) = ready_queues.pop_blocking() {
    if inner.is_shutdown() {
        break;
    }

    inner.run_job_queue_entry(entry, index, priority);
}
```

If the chosen queue primitive cannot wake all blocked workers directly, Rust may
use one wake token per worker. Keep those wake tokens internal and do not expose
them as jobs.

### No Drain

C++ does not guarantee that every queued job runs during shutdown.

Once `m_isExitRequested` is set, workers stop after their next pop. A job popped
after the flag is set is not executed.

Rust v1 should keep this policy:

- shutdown means stop worker threads
- it is not a flush
- users must flush important counters before shutdown if they need completion
- pending queued jobs may be dropped

This matches the existing v1 decision: "Just stop — no drain in v1."

### Dispatch After Shutdown

Rust should reject new dispatch once shutdown starts.

Recommended v1 policy:

- panic in debug/tests if a caller dispatches after shutdown
- in release, either panic too or return a clear `JobSystemShutdown` error if the
  API has become fallible by then

Do not silently accept jobs after shutdown. They may never run.

### Explicit Shutdown vs Drop

C++ calls `Shutdown()` from the dispatcher destructor.

Rust should not rely on `Dispatcher::drop` as the primary mechanism because
worker threads may hold `Arc<Dispatcher>`. If workers keep the last strong
references alive, `Drop` cannot be the thing that tells them to exit.

Rust rule:

- Bevy/plugin teardown must call `LeetJobSystem::shutdown()`
- tests should call `shutdown()` explicitly
- `Drop` on `LeetJobSystem` should not attempt to join worker threads in v1
- future versions may add a best-effort backup only if the final ownership design
  proves it cannot deadlock

A `Drop` implementation that joins worker threads can deadlock if workers are
blocked waiting for jobs that will never be queued. Keep shutdown explicit.

### Counter / Job Lifetime During Shutdown

Since v1 does not drain, queued jobs may be dropped without running.

Rust consequence:

- dropping a `JobQueueEntry` drops its `JobDecl`
- captured data in the closure is dropped normally
- `Arc<CounterEntry>` references held by queued jobs are released
- counters waiting for abandoned jobs should not be flushed after shutdown begins

Do not try to decrement abandoned job counters during shutdown unless the design
explicitly supports cancellation semantics. C++ does not model graceful job
cancellation here.

### C++-Specific Pieces To Drop

- null `JobDecl` shutdown jobs as a public concept
- dummy priority counters
- manual `ReleaseJobCounterInternal`
- console/resource queue shutdown details
- dispatcher destructor as primary shutdown path

---

## Crate File Map

One file per responsibility. Codex must not merge these or split them differently.

```
leet_jobs2/
├── lib.rs          — pub use of public types only, nothing else
├── config.rs       — JobSystemConfig, Default impl, editor() and tool() presets
│                     See jobDispatcherInitParam.h section for field mapping
├── priority.rs     — Priority enum, ScheduleParam
├── job_decl.rs     — JobDecl, ParallelForJob, JobHint, RunContext, ContinuationContext
├── counter.rs      — Counter (public handle), CounterEntry (internal, pub(crate))
├── deferral.rs     — CompletionDeferral
├── builder.rs      — Builder, Fence enum
├── queue.rs        — MPMC queue, one per priority level
├── worker.rs       — worker thread loop
└── dispatcher.rs   — LeetJobSystem handle, Dispatcher, dispatch/flush logic
```

Rules:
- `lib.rs` only re-exports. No logic in lib.rs.
- Internal types (`CounterEntry`, `JobDecl`, worker internals) are `pub(crate)`, never `pub`.
- No file reaches into another file's private fields. Only through methods.

---

## Builder File Map (jobBuilder.h → builder.rs)

### Why C++ has so many Dispatch functions

All C++ Builder dispatch functions are combinations of two axes:

- **Job kind**: single job / parallel-for / parallel-for with epilogue
- **Variant**: default (full fence) / no fence / after external wait + no fence

That is 3 × 3 = 9 combinations, doubled for the InstrumentationObject overloads = 18 functions.
They are all the same thing. C++ templates force writing each combination out explicitly.

### What Rust actually needs

The `AfterWait_NoFence` variants exist in C++ because combining two calls would
insert an unwanted fence between them. In Rust, `dispatch_wait` does not insert a
fence — it just adds the dependency. So two separate calls work fine and those
variants are not needed.

```
builder.rs contains:

pub enum Fence { Full, None }

pub struct Builder {
    dispatcher: DispatcherHandle,
    wait_counter: Option<Counter>,   // present until final-sync/extraction
    accum_counter: Option<Counter>,  // present until final-sync/extraction
    continuation_counter: Option<Arc<CounterEntry>>,  // parent continuation counter from RunContext
    priority: Priority,
    is_extracted: bool,
    debug_needs_fence: bool,  // runtime guard for Fence::None misuse
    not_send: PhantomData<Rc<()>>,
}

impl Builder {
    // Construction is routed through LeetJobSystem.
    pub(crate) fn new(dispatcher: DispatcherHandle, priority: Priority) -> Self
    pub(crate) fn from_context(dispatcher: DispatcherHandle, ctx: &RunContext) -> Self

    // Single job dispatch
    pub fn dispatch_job<F>(&mut self, name: &'static str, f: F)            // Fence::Full
    pub fn dispatch_job_no_fence<F>(&mut self, name: &'static str, f: F)   // Fence::None

    // Parallel-for dispatch
    pub fn dispatch_parallel_for<F>(&mut self, name, count, f)
    pub fn dispatch_parallel_for_no_fence<F>(&mut self, name, count, f)

    // Parallel-for with epilogue
    pub fn dispatch_parallel_for_with_epilogue<F, E>(&mut self, name, count, f, epilogue)
    pub fn dispatch_parallel_for_with_epilogue_no_fence<F, E>(&mut self, name, count, f, epilogue)

    // Dependency and ordering
    pub fn dispatch_wait(&mut self, counter: &Counter)       // add external dependency, no fence
    pub fn dispatch_fence_explicitly(&mut self)              // manual fence after Fence::None
    pub fn extract_wait_counter(&mut self) -> Counter        // take final counter, invalidates builder
}

impl Drop for Builder {
    // calls final_sync if not already extracted;
    // extract_wait_counter also calls final_sync before returning
}
```

`Builder::from_context` must inherit the priority from
`ctx.continuation.param`, not use `ScheduleParam::default()`. A continuation
builder dispatches work on behalf of its parent job and must stay on the same
priority level.

`dispatch_fence_explicitly()` and final-sync must check whether
`accum_counter` is already zero before rotating or consuming it.

For a normal fence:

- if `accum_counter` is zero, skip rotation and keep using the same accumulator
- if `accum_counter` is nonzero, move it into `wait_counter` and create a fresh
  accumulator

Rotating an empty accumulator breaks the dependency chain by inserting a counter
that no dispatched work actually uses.

For final-sync:

- if `accum_counter` is zero, discard it and keep the existing `wait_counter`
- if `accum_counter` is nonzero, move it into `wait_counter`
- after this point the builder has no valid accumulator left
- Rust implementation may store `wait_counter` and `accum_counter` as `Option`
  so extraction/final-sync can move those handles out without creating inert
  placeholder counters

If the builder was created from a `RunContext`, final-sync also links the
builder's retained `wait_counter` into the parent continuation counter.

`dispatch_wait()` directly adds the external dependency to `wait_counter`. It
does not rotate counters and is not exempt from the fence ordering rule. If it is
called after any `*_no_fence` dispatch while `debug_needs_fence` is still set, it
must panic until `dispatch_fence_explicitly()` has been called.

`extract_wait_counter()` runs final-sync, moves the retained `wait_counter` out,
and invalidates the builder. If this is a continuation builder, the returned
counter is then wrapped in a fresh counter that waits on the parent continuation
counter. The returned counter already incorporates the continuation dependency;
the caller does not need to do anything extra.

### What got cut from C++ and why

| C++ function | Rust | Reason |
|---|---|---|
| `DispatchJobAfterWait_NoFence` | removed | `dispatch_wait` + `dispatch_job_no_fence` achieves the same with no fence inserted between them |
| `DispatchParallelForJobAfterWait_NoFence` | removed | same reason |
| All `InstrumentationObject` overloads | removed | `&'static str` name replaces it |
| `DispatchJobWithHint` | `dispatch_job_with_hint` | kept, but only `Trivial` and `Large` hints |
| `DispatchParallelForJobWithEpilogueWithBatchSize` | fold into parallel_for as optional param | no need for a separate function |

---

## jobUtils.h

`jobUtils.h` currently provides `ParallelSort`, a helper built on top of
`job::Builder`. It is not core dispatcher behavior.

Rust v1 should not put this inside the core job-system implementation. If needed
later, it can become an optional utility module or an engine-side helper using
the public `Builder` API.

Important behavior if ported later:

- split only above a threshold
- dispatch left and right halves with `Fence::None`
- call `dispatch_fence_explicitly()`
- dispatch the merge step after the fence

This is a good example of how the builder API is meant to be used, but not a
required part of `leet_jobs2` v1.

---

## Pending Questions (answer before implementing that layer)

- What concrete queue primitive? `crossbeam-channel` bounded queues are acceptable,
  but `Mutex + Condvar` may be clearer if strict priority/wakeup behavior gets awkward.
  C++ spins briefly before falling back to a kernel wait; a plain
  `Mutex + Condvar` design is correct but may have higher wakeup latency.
- Should `Counter` implement `Clone`? No — C++ is move-only, keep it move-only.
  Sharing across threads is done by sharing the `Arc<CounterEntry>` internally.
- Should `Builder` be `!Send`? Yes — C++ asserts single-thread use. Mark it `!Send`.
- Where exactly does `claim_flush_thread()` get called? Answered in Pass 5.5:
  the render sub-app owns a first render-system set that claims it once before
  extraction, prepare, render, or cleanup work.
- Does `CounterEntry.waiting: Mutex<WaitingList>` need pre-reserved capacity
  or a different container? C++ allocates waiting-list nodes before taking the
  lock to keep the critical section short. Rust v1 can start with `Vec`, but the
  lock may be held across allocation during `push`.
