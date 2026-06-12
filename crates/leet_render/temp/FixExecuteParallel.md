# Fix Execute Parallel

Working brainstorm for turning the render graph executor from a scheduling shell
into true parallel frame execution.

The spelling in the filename is intentionally kept as requested. We can rename
later when the plan stabilizes.

## Problem

We want RED-style graph execution:

- graph topology is already built and flow-grouped
- CPU-ready nodes can run in parallel
- preconsume records resource requests
- resources resolve once in deterministic order
- consume executes real node work
- command recording can happen in parallel where graph rules allow it
- cleanup runs only after all graph work is joined

The current LEET executor exposes pieces of this shape, but it does not yet
parallelize the mutable frame state path.

## Current Audit Snapshot

The important correction from the RED scan is that
`CRenderNodeGraph::ExecuteParallel` is not the whole graph execution. It is the
parallel preconsume pass, where nodes record render-flow/resource requests.

RED then runs the real render-node work through `RunRenderNodeJobs`:

- one job is created per graph node
- CPU dependencies are represented with node counters
- root nodes wait on an external kickoff counter
- terminal nodes are joined before graph cleanup
- each job builds a fresh node implementation context from the job run context

LEET currently has parts of this shape, but they are not wired as two true
parallel tracks yet:

- `RenderGraphCoreRunner` owns the allocator, command recorders, and process
  state directly.
- preconsume still runs through a central mutable `FrameResourceAllocator`.
- resource resolve is still mostly a phase transition shell, not the production
  materialized resolve point.
- consume builds CPU-ready batches, but processes the batch nodes
  sequentially.
- command recorder ownership exists conceptually through
  `RenderNodeCommandListUsage`, but the executor does not yet expose
  per-node/per-slot recorder ownership for parallel consume jobs.

So the missing architecture is not just "make the loop parallel". It is:

1. RED-style parallel preconsume.
2. deterministic request merge.
3. materialized resource resolve.
4. RED-style node jobs with CPU dependency counters.
5. explicit final join and cleanup.

## What Rust Is Actually Blocking

Rust is not blocking parallel graph execution.

Rust is blocking this shape:

```rust
parallel_for_each_node(|node| {
    node.execute(&mut allocator, &mut command_recorders);
});
```

That is good. A single `&mut FrameResourceAllocator` and a single
`&mut FrameCommandRecorders` cannot be shared by many jobs at the same time.

So the problem is not parallelism itself. The problem is central mutable state:

- allocator request stream
- allocator resolved timeline/resource state
- command recorder slot ownership
- command submission list
- node execution context
- cleanup state

We need to split these into:

- shared read-only frame data
- per-node/per-worker write buffers
- deterministic merge points
- central mutation phases that happen after joins

## Why RED Does Not Feel Blocked

RED already paid this design cost:

- jobs receive a real run context
- render-frame execution has an owned frame context
- command-list ownership is a first-class concept
- graph nodes do not freely mutate arbitrary global frame state
- resource request/resolve phases are already structured
- graph flow groups and dependency counters are established
- sync nodes and kickoff counters are part of execution

In C++, unsafe shared mutation is technically easy, so the safety rules live in
engine architecture, asserts, conventions, and job-system contracts.

In Rust, those same rules must become data structures and APIs.

## Non-Blockers

These do not block parallel graph execution:

- real frame renderer completion
- real graph recipes
- real wgpu bind groups/pipelines
- production graph cache key completeness
- graph diagnostics
- lifecycle node side effects
- stable subtype registry
- independent graph impl remap
- inter-command-list sync nodes beyond the basic model

They matter, but the executor can be built and tested with tiny graphs first.

## Real Blockers

### 1. Preconsume Request Recording

Preconsume cannot directly mutate one central allocator request stream from
parallel jobs.

Candidate fix:

```rust
struct NodePreconsumeOutput {
    node_index: usize,
    requests: FrameResourceRequestGroup,
    decisions: RenderNodeDecisionLog,
}
```

Each node records into its own request group. After all ready work is joined,
the runtime merges groups in deterministic graph order.

### 2. Deterministic Merge

Resource request ordering must not depend on OS scheduling.

Merge order should be:

1. graph flattened CPU order, or
2. graph node usage order inside preconsume group order, or
3. explicit execution sequence produced by dependency counters

Preferred: graph flattened CPU order, because it is already deterministic and
dependency-valid.

### 3. Materialized Resource Resolve

Resolve happens after preconsume completes and after external resources are
registered.

Production shape:

```rust
runtime.register_external_frame_resources(frame);
runtime.merge_preconsume_outputs();
runtime.resolve_frame_resources(device);
```

This is a central mutation phase, not parallel node work.

### 4. Consume Execution

Consume can run CPU-ready batches in parallel, but nodes cannot freely mutate
global recorder state.

Each node gets a scoped runtime view:

```rust
struct NodeExecutionRuntime<'a> {
    frame: &'a FrameExecutionRuntime,
    node: RenderNodeId,
    worker_index: u32,
    phase: ExecutionPhase,
}
```

The runtime decides what access is legal for that node.

### 5. Command Recorder Ownership

`RenderNodeCommandListUsage` must become hard behavior:

- `Own`: node gets a dedicated command recorder slot
- `Require`: node must execute inside an existing command-list group recorder
- `Sync`: node routes through sync/runtime command path
- `None`: command recording access fails

This means command recorder mutation cannot be one shared `&mut` passed to all
nodes.

Candidate shape:

```rust
struct FrameCommandRecorderTable {
    slots: Vec<FrameCommandRecorderSlot>,
}

struct NodeCommandAccess {
    node: RenderNodeId,
    slot: Option<FrameCommandRecorderSlotId>,
    usage: RenderNodeCommandListUsage,
}
```

Parallel nodes may own different slots. Shared access requires explicit graph
structure, not ambient mutable state.

### 6. Final Join Before Cleanup

Cleanup must happen after:

- all preconsume jobs complete
- resource resolve completes
- all consume jobs complete
- command recorder submission list is finalized
- GPU submission ordering is resolved

No `Drop`-based cleanup for graph state.

## Candidate Runtime Shape

```rust
struct FrameExecutionRuntime {
    allocator: FrameResourceAllocator,
    command_recorders: FrameCommandRecorders,
    command_submission: FrameCommandSubmission,
    preconsume_outputs: Vec<NodePreconsumeOutput>,
    dependency_counters: RenderGraphDependencyCounters,
    worker_count: usize,
}
```

This object is owned by frame execution. Nodes never get raw mutable access to
the whole runtime.

They get narrow context objects:

```rust
struct RenderNodeImplContext<'a> {
    runtime: NodeRuntimeView<'a>,
    node: RenderNodeId,
    phase: ExecutionPhase,
}
```

## Possible Designs

### Option A: Per-Node Buffers

Each node records preconsume requests into a node-owned buffer.

Pros:

- deterministic merge is easy
- diagnostics are excellent
- no worker-local reordering concerns

Cons:

- many small buffers
- requires indexed node output storage

This is the preferred starting point.

### Option B: Per-Worker Buffers

Each worker records requests into a local buffer.

Pros:

- fewer buffers
- good cache behavior

Cons:

- merge needs node ordering metadata
- diagnostics are slightly harder
- accidental worker-order dependence is easier

Good later optimization, not first implementation.

### Option C: Locked Central Allocator

Wrap allocator request stream in a lock.

Pros:

- easiest to code

Cons:

- ordering depends on scheduling unless every write is sorted later
- hides contention
- easy to accidentally make nondeterministic

Avoid for allocator streams.

### Option D: Actor/Channel Collector

Nodes send requests to a collector job.

Pros:

- clean ownership
- flexible

Cons:

- more machinery
- still needs deterministic ordering

Probably unnecessary.

## Recommended Plan

### Pass 1: Runtime Object

Create `FrameExecutionRuntime` as the owner of:

- allocator
- command recorders
- command submission
- preconsume output storage
- dependency counters

No parallelism yet. Just move ownership out of loose parameter passing.

### Pass 2: Per-Node Preconsume Outputs

Change preconsume so each node records into its own request group.

Then merge request groups in deterministic graph order before resolve.

### Pass 3: Materialized Resolve Point

Add the production resolve point:

```rust
register_external_frame_resources(frame);
merge_preconsume_outputs();
resolve_frame_resources(device);
```

### Pass 4: Parallel Preconsume

Run CPU-ready preconsume batches in parallel.

Still merge deterministically.

### Pass 5: Command Recorder Ownership

Make `RenderNodeCommandListUsage` enforce real access rules.

### Pass 6: Parallel Consume

Run CPU-ready consume batches in parallel.

GPU submission remains ordered by graph GPU order.

### Pass 7: Final Join And Cleanup

Make cleanup a required explicit phase after all graph jobs complete.

## Core Rule

Parallel graph execution is allowed only when node-local mutation is separated
from frame-global mutation.

Frame-global mutation happens at explicit merge, resolve, submit, and cleanup
boundaries.
