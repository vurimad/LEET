# leet_jobs2 — Execution Passes

This file is the implementation plan for `leet_jobs2`.
It exists to prevent "simple for now" code from becoming the foundation of a
production job system.

The passes are not placeholders. Each pass must leave its layer complete,
tested, and shaped for the later layers that will sit on top of it.

---

## Ground Rules

### No Half-Baked Behavior

If a behavior is part of the current pass, implement it correctly for the final
v1 design.

If a behavior belongs to a later pass, do one of these:

- leave the public API absent
- keep the function private and unused
- make the unsupported path fail loudly with a clear panic

Do not add a fake implementation that silently violates the design notes.
Stubs are allowed only for behavior explicitly marked as a stub in
`leet_jobs2_internals.md`, such as profiling hooks or `dump_to_log()`.

### Pass Boundaries Are Contracts

Each pass has:

- an owned scope
- explicit non-goals
- tests that must pass before moving on
- a clean handoff to the next pass

Later passes may extend earlier code, but they should not need to tear it apart.
If a pass discovers that an earlier shape cannot support the final design, stop
and fix that shape before layering more code on top.

### No Out-Of-Scope Drift

Do not implement these during any pass:

- PhysX or AudioEvent job hints
- console core 7 affinity queue
- resource throttler threads
- local worker queues
- frame allocator debug bans
- IO priority thread-locals
- scoped non-`'static` jobs
- engine, Bevy, render, GPU, or command-handler integration unless a pass
  explicitly names that integration boundary

### Comment Quality

The production code must be documented with high-quality comments where the
behavior is subtle, especially around:

- counter lifetime and `Arc` ownership
- increment-before-queue ordering
- locked waiting-list rechecks
- flush eligibility and requeue behavior
- shutdown no-drain semantics
- builder fence and continuation rules
- parallel-for batching and epilogue ownership

Comments in Rust source should explain the Rust invariant, the concurrency
reasoning, or the API contract. Do not make source comments depend on C++ file
names, C++ call paths, or "ported from C++" explanations.

If implementation needs to preserve or audit the relationship to the legacy C++
job system, track that in `leet_jobs2_execution_notes.md` or
`leet_jobs2_internals.md`, not in code comments.

---

## Common Definition Of Done

Every pass is done only when:

- `cargo fmt` has been run
- `cargo test -p leet_jobs2` passes
- tests cover the pass's invariants, not just happy paths
- panic paths that protect invariants are tested where practical
- public API added in the pass has final v1 semantics
- later-pass behavior is absent or loudly unavailable, not faked
- subtle production behavior is documented with comments in the Rust code
- source comments explain Rust invariants without referencing C++ files
- the crate still follows the file map from `leet_jobs2_internals.md`
- no unrelated engine or Bevy concepts entered the crate

---

## Pass 0 — Crate Skeleton And Static Types

### Owns

- exact crate file layout:
  - `lib.rs`
  - `config.rs`
  - `priority.rs`
  - `job_decl.rs`
  - `counter.rs`
  - `deferral.rs`
  - `builder.rs`
  - `queue.rs`
  - `worker.rs`
  - `dispatcher.rs`
- `lib.rs` re-exports only
- `JobSystemConfig`, defaults, `editor()`, and `tool()`
- `Priority` and `ScheduleParam`
- `JobHint`
- `JobDecl` boxed closure storage
- `ParallelForJob`, `TakeOnceEpilogue`, and related type storage
- `RunContext` and `ContinuationContext` type shape

### Does Not Own

- live counters
- queues
- workers
- dispatch
- flush
- builder behavior
- deferral behavior
- parallel-for execution

### Tests

- `ScheduleParam::default()` uses `Priority::CriticalPath`
- priority ordering matches strict pop order expectations
- config defaults and presets match documented capacities and flags
- `JobDecl` can store mixed closure types behind the internal box
- `TakeOnceEpilogue` runs at most once
- public re-exports compile from the crate root

---

## Pass 1 — CounterEntry Core

### Owns

- `WaitingJob`
- `WaitingList = Vec<WaitingJob>`
- `CounterEntry`
- atomic counter value operations
- underflow panic on decrement
- waiting-list insertion with locked zero recheck
- waiting-list flush with locked zero recheck
- `is_zero()` as a snapshot only
- `Arc<CounterEntry>` lifetime shape
- public `Counter` handle shape, without cloning

### Does Not Own

- ready queues
- dispatcher scheduling
- workers
- `Counter += &Counter`
- `CompletionDeferral`
- `Builder`
- flush loop

### Tests

- new counter starts at zero
- increment reports or preserves old-zero behavior as designed
- decrement returns true only when the new value is zero
- decrement from zero panics
- waiting job is not inserted when the wait counter is zero
- waiting job is inserted when the wait counter is nonzero
- `flush_waiting()` drains only if value is still zero under the lock
- `flush_waiting()` leaves jobs parked if value was incremented again
- `Counter` is move-only and does not implement `Clone`

Tests for this pass should stay single-threaded. The point is to lock down the
state machine before adding scheduling.

---

## Pass 2 — Bounded Ready Queues

### Owns

- one bounded FIFO lane per priority
- strict priority pop order:
  - `Immediate`
  - `CriticalPath`
  - `RenderPath`
  - `Latent`
- `try_push`
- blocking `pop_blocking`
- nonblocking `try_pop`
- queue-full policy
- shutdown wakeup behavior

### Does Not Own

- worker thread spawning
- job execution
- counters
- dependencies
- flush policy
- priority mapping from config

### Tests

- FIFO order inside each lane
- strict priority order across lanes
- `try_pop()` returns immediately with `None` when empty
- `pop_blocking()` wakes when a job is pushed
- queue capacity exhaustion is explicit
- shutdown wakes blocked poppers
- pop returns `None` after shutdown

The queue primitive can be `Mutex + Condvar` for v1. If another primitive is
chosen, it must still satisfy the same tests.

---

## Pass 3 — Worker Pool And Runtime Shell

### Owns

- `LeetJobSystem::new`
- `LeetJobSystem::shutdown`
- `LeetJobSystem::num_worker_threads`
- `LeetJobSystem::current_thread_index`
- `Dispatcher` shell
- `DispatcherHandle`
- worker thread spawning and joining
- worker thread names and optional stack size
- worker TLS indices:
  - `Some(1..N)` for workers
  - `None` outside the job system
- idempotent shutdown
- no-drain shutdown policy
- a minimal ready-job execution path with no dependencies

### Does Not Own

- wait counters
- dependency gates
- waiting-list flush
- `flush_counter`
- builder
- deferrals
- parallel-for

### Tests

- configured worker count is respected
- worker TLS index is visible inside a worker-run job
- outside threads report `None`
- queued no-dependency jobs run exactly once
- shutdown is idempotent
- blocked workers wake and join on shutdown
- pending jobs may be dropped after shutdown begins
- dispatch after shutdown panics

The worker must call the dispatcher execution hook, even in this early pass.
Do not let `worker.rs` call job closures directly.

---

## Pass 4 — Dispatcher And Counter Integration

### Owns

- `LeetJobSystem::create_counter`
- `claim_flush_thread`
- `flush_counter`
- `flush_counter_with_timeout`
- `flush_counter_render_frame`
- `Dispatcher::run_job`
- `queue_job_or_wait`
- `queue_job_and_signal`
- direct self-dependency rejection
- increment-before-queue
- dependency parking on wait counters
- decrement-after-run
- waiting-list flush on zero
- requeue without second increment
- `Counter += &Counter`
- priority mapping from config
- `JobHint::Large` exception when `all_jobs_critical_path` is enabled
- `RunContext` construction for normal jobs
- stable job execution hook points

### Does Not Own

- `Builder`
- `CompletionDeferral`
- parallel-for expansion
- dynamic parallel-for element counts
- debugger analysis

### Tests

- job increments accumulate counter before queueing
- job waiting on nonzero counter does not run early
- job waiting on zero counter queues immediately
- direct self-dependency panics
- decrement to nonzero does not flush waiters
- decrement to zero flushes waiters
- flushed waiting jobs are queued without a second increment
- `Counter += &Counter` preserves dependency ordering
- `flush_counter` runs jobs accumulating into the target counter
- `flush_counter` can run eligible higher-priority work while waiting
- `flush_counter` runs trivial jobs
- `flush_counter_render_frame` respects the large-job policy
- flush requeues ineligible jobs without calling `run_job`
- flush rejects reentrant use
- flush rejects or debug-asserts wrong-thread use according to the documented policy
- timeout returns `false` without debugger analysis

This is the first pass where concurrency races matter. The locked recheck rules
from the internals document are mandatory, not optimization details.

---

## Pass 5 — Builder And CompletionDeferral

### Owns

- `Builder`
- `Fence`
- `dispatch_job`
- `dispatch_job_no_fence`
- `dispatch_job_with_hint` if added
- `dispatch_wait`
- `dispatch_fence_explicitly`
- `extract_wait_counter`
- builder final-sync on drop
- runtime panic for no-fence misuse
- zero-accumulator fence behavior
- continuation builder construction from `RunContext`
- continuation final-sync into the parent counter
- extraction behavior for continuation builders
- `CompletionDeferral`
- deferral creation through `Counter`
- deferral finish and drop behavior

### Does Not Own

- parallel-for public methods
- parallel-for epilogues
- dynamic element count overloads

### Tests

- ordered builder dispatch runs in order
- no-fence dispatch allows parallel jobs before explicit fence
- ordered dispatch after no-fence without explicit fence panics
- `dispatch_wait()` after no-fence without explicit fence panics
- dropping a builder with pending no-fence work panics
- explicit fence clears the no-fence guard
- empty accumulator is not rotated on fence
- final-sync discards an empty accumulator
- nonempty accumulator becomes the final wait counter
- `extract_wait_counter()` invalidates the builder
- continuation builder inherits `ctx.continuation.param.priority`
- continuation builder drop extends the parent job lifetime
- continuation builder extraction still links into the parent continuation counter
- `CompletionDeferral::finish()` decrements exactly once
- double finish panics
- drop auto-finishes unfinished deferrals
- finished deferral drop does not panic
- `CompletionDeferral` is move-only and not `Clone`
- `Builder` is not `Send`

This pass should make the public ergonomic API usable for normal single-job
work. No parallel-for shortcuts belong here.

---

## Pass 5.5 — Bevy Render-World Hookup

### Owns

- optional Bevy resource support for `LeetJobSystem`
- `LeetJobPlugin` as the render-world registration boundary
- registering one `LeetJobSystem` resource in the LEET render sub-app
- making that resource accessible to render systems as `Res<LeetJobSystem>`
- claiming the flush thread once at the start of the render schedule
- render-world shutdown guard that calls `LeetJobSystem::shutdown()`
- re-exporting job-system public handles from `leet_render` for engine systems
- replacing old render-side references to the placeholder job crate

### Does Not Own

- render graph execution behavior
- command-handler integration
- GPU concepts inside `leet_jobs2`
- parallel-for
- changing the core job-system crate into a Bevy-owned runtime

### Tests

- render plugin inserts `LeetJobSystem` into the render world
- `LeetJobPlugin` can install the resource layout without the full renderer stack
- render schedule claims the flush thread before render work
- repeated render updates do not reclaim the flush thread
- pipelined render update observes job-system thread index `Some(0)` on the
  render thread
- dropping the render app shuts down the registered job system
- `leet_render` builds against `leet_jobs2`, not the placeholder job crate

This pass is intentionally cross-crate. The job system remains a plain Rust
runtime; Bevy only sees it as a resource at the render-app boundary.

---

## Pass 6 — Parallel-For

### Owns

- `dispatch_parallel_for`
- `dispatch_parallel_for_no_fence`
- `dispatch_parallel_for_with_epilogue`
- `dispatch_parallel_for_with_epilogue_no_fence`
- optional max-batch-size parameter if exposed
- `ParallelForSharedState`
- team-job expansion
- team size calculation:
  - `min(num_elements, num_worker_threads + 1)`
- single-team execution path
- multi-team atomic batch claiming
- C++ batch-size formula
- `RunContext.parallel_for_index`
- zero-element behavior
- epilogue execution exactly once after all chunks finish

### Does Not Own

- dynamic element count overloads
- shared-data setup callbacks
- per-index convenience wrapper unless real call sites require it
- engine-specific helpers such as parallel sort

### Tests

- zero elements with no epilogue still increments and decrements through one empty job
- zero elements with epilogue runs the epilogue once
- single-team path covers the full range
- multi-team path covers every element exactly once
- multi-team path does not process out-of-range elements
- `parallel_for_index` is the team index for chunk work
- epilogue sees `parallel_for_index == -1`
- epilogue runs after all chunks complete
- outer counter does not reach zero before epilogue completes
- `max_batch_size == 0` uses team-size batching
- nonzero max batch size follows the documented C++ formula
- no-fence parallel-for participates in builder fence rules

Parallel-for is last because it relies on every previous invariant: counters,
waiting dependencies, worker execution, builder fences, continuation linking, and
epilogue lifetime.

---

## Pass 7 — Hardening And Audit

### Owns

- stress tests for dependency chains
- stress tests for flush while workers are active
- shutdown behavior under queued work
- docs-vs-code audit
- public API audit
- panic message cleanup
- race-prone test cases around waiting-list rechecks

### Tests

- many counters chained together complete in dependency order
- repeated flushes do not reenter or leak jobs
- concurrent producers can dispatch jobs without losing work
- shutdown during queued work joins cleanly
- no out-of-scope features were introduced
- crate-level docs match the implemented API

This pass is not a place to add new features. It is the point where the system is
treated like production infrastructure and shaken until weak assumptions show up.

---

## Pass Handoff Checklist

Before moving from one pass to the next:

- run `cargo fmt`
- run `cargo test -p leet_jobs2`
- read the tests and confirm they cover the pass invariants
- remove any temporary scaffolding
- ensure unsupported later-pass behavior is absent or loudly unavailable
- update `leet_jobs2_internals.md` if implementation changed the design
- update this file if a pass boundary changed
- confirm no engine-specific concepts entered `leet_jobs2` outside an explicit,
  feature-gated integration boundary

The goal is not to move fast through the checklist. The goal is to avoid building
future behavior on a layer that is already lying.






What I would still treat as v1 limits:

Not performance-tuned like the old lock-free/semaphore design
No CPU affinity / worker priority pinning yet
Shutdown is stop/no-drain, so important graph work must be flushed before teardown
Job closures must not capture borrowed Bevy ECS data
Render graph GPU synchronization still belongs to the render graph, not leet_jobs2