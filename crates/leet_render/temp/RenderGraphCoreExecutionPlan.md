# LEET Render Graph Core - Execution Plan

This file is the implementation plan for the LEET render graph core.

The design contract lives in `RenderGraphCoreDesign.md`. The frame resource
allocator contract lives in `RenderGraphDesign.md`. Frame renderer and Bevy
extraction bridge notes live in `FrameRenderer.md`.

This plan starts after the frame resource allocator foundation. It must not
replace the allocator model, and it must not revive the old graph files moved to
`render_graph/badgraph`.

---

## Ground Rules

### Production Layers Only

Every pass is a production layer. A later pass may extend an earlier layer, but
it should not need to replace its data model.

If a behavior belongs to a later pass:

- leave the public API absent
- keep the function private and unused
- or make the unsupported path fail loudly with a clear error

Do not add a serial toy graph and hope to make it parallel later. The graph must
be compatible with RED-style import/merge, CPU/GPU dependency tracks,
command-list groups, render-flow groups, and dependency-counter execution from
the beginning.

### Design And RED Source Authority

Implementation must respect `RenderGraphCoreDesign.md`.

When the design document is unclear, inspect the relevant RED `.h` and `.cpp`
files again before choosing a Rust shape. Mirror RED behavior conceptually, but
adapt to Rust ownership and wgpu pass/command-buffer semantics.

If implementation must intentionally diverge from the design, update
`RenderGraphCoreDesign.md` before continuing.

### Test Placement

Graph-core tests should live under the render graph test tree:

```text
src/render_graph/tests/
  graph_ids.rs
  graph_storage.rs
  graph_dependencies.rs
  graph_merge.rs
  graph_flow_groups.rs
  graph_factory.rs
  graph_command_groups.rs
  graph_execution.rs
  graph_cache.rs
  render_node_impl_context.rs
```

Small pure helper tests may live beside tiny modules only if they stay short.
Scenario tests, graph topology tests, merge tests, execution tests, and context
tests belong under `render_graph/tests`.

### Logging And Errors

Graph-core errors should remain typed inside graph code so tests and callers can
match exact failure kinds. Outer renderer code may convert them into LEET's
central `Leeror`/`LeetResult` surface.

Actual runtime logging must go through `leet_log` macros:

```rust
use leet_log::{debug, error, info, trace, warn};
```

Do not import `tracing` directly from graph-core implementation files. `write!`
inside `Display` implementations is not logging; it is normal Rust formatting
and should stay there.

### No Out-Of-Scope Drift

Do not implement these in the graph-core passes unless a pass explicitly says
so:

- Bevy scene extraction
- material/mesh asset preparation
- shader compilation or pipeline cache
- final renderer batching
- real draw-call generation
- full frame renderer orchestration
- physical memory aliasing
- replacing Bevy's view target path
- concrete RED renderer feature nodes beyond tiny test nodes

The graph core owns topology, dependency semantics, node execution shell,
command recording structure, and integration points. It does not own the final
renderer content pipeline.

### Naming Rules

Use the locked names unless an implementation pass explicitly updates the design:

- `RenderNodeImpl` for the Rust node implementation trait
- `RenderNodeImplContext` for node-facing runtime context
- `GroupEntry` and `GroupExit` for group dependency anchors
- `add_graph(options)` for graph import/merge
- `RenderNodeDependencyKind::{Cpu, Gpu}`

Avoid `Dummy` for LEET structural nodes. RED code quotes may contain dummy names,
but LEET graph anchors use explicit roles.

### Comment Quality

Implementation comments should explain Rust invariants:

- why topology is mutable or frozen
- why ids remain valid or become invalid
- why CPU/GPU dependency graphs are separate
- why a merge/remap step is required
- why a command-list group owns subnode execution
- why request order must be deterministic
- why cleanup runs in the frame execution epilogue

Do not make normal source comments depend on RED filenames. RED references
belong in the design docs or this plan.

---

## Common Definition Of Done

Every pass is done only when:

- `cargo fmt` has been run
- `cargo test -p leet_render` passes, or the pass documents why a narrower
  command is temporarily required
- tests cover invariants and failure paths, not only happy paths
- public API added in the pass has final V1 semantics
- unsupported later-pass behavior is absent or fails loudly
- graph-core tests for non-trivial behavior live under `render_graph/tests`
- no pass introduces a serial-only graph model
- no pass collapses CPU and GPU dependencies into one edge kind
- no pass makes graph import/merge a future rewrite
- no pass hides allocator phase cleanup inside a normal node
- no pass exposes RED-style mutable global binding as the final wgpu API
- `RenderGraphCoreDesign.md` is updated before any intentional design
  divergence

---

## Target File Map

The graph core should live under `src/render_graph/graph/`:

```text
src/render_graph/graph/
  mod.rs
  error.rs
  ids.rs
  metadata.rs
  node_topology.rs
  storage.rs
  render_node_graph.rs
  node_impl.rs
  render_node_impl_context.rs
  factory.rs
  flow_groups.rs
  cache.rs
  execution.rs
  command_group.rs
  diagnostics.rs
src/render_graph/
  frame_command_recorder.rs
src/render_graph/tests/
  graph_*.rs
  render_node_impl_context.rs
  tinyGraphTest/
```

The exact split may change if implementation pressure justifies it, but the
conceptual boundaries must remain:

- ids are typed and cheap
- storage owns stable graph records and dense usage order
- graph topology records nodes and dependencies
- factory owns authoring helpers and implementation storage
- flow groups freeze/build executable ordering metadata
- cache owns built graph topology and node lifetime
- execution owns process wrappers, job wiring, and immutable execution views
- command recording is frame/runtime state, not allocator state

---

## Pass 0 - Module Skeleton And Error Surface

### Owns

- `render_graph/graph/` module tree
- graph-core public re-exports from `render_graph/mod.rs`
- `RenderGraphError`
- `RenderGraphResult<T>`
- error variants for:
  - invalid id
  - duplicate dependency
  - self dependency
  - graph already built/frozen
  - cycle detected
  - invalid merge
  - invalid command-list group usage
  - invalid execution phase

### Does Not Own

- node/dependency storage
- factory API
- execution
- command recording
- graph cache

### Tests

- crate compiles with the graph module tree
- public graph-core re-exports compile
- graph errors format useful messages

### Handoff

The next pass can add typed ids without changing module layout.

---

## Pass 1 - Typed Ids, Kinds, And Static Metadata

### Owns

- `RenderNodeId`
- `RenderDependencyId`
- `RenderNodeImplId`
- `NodeGroupId`
- invalid/null id sentinels where useful
- `RenderNodeKind`
  - `Stage`
  - `Unique`
  - `SequenceBegin`
  - `SequenceEnd`
  - `Temporary`
- `RenderNodeRole`
  - normal
  - group entry
  - group exit
  - command-list group
  - lifecycle/system where needed
- `RenderNodeDependencyKind`
  - `Cpu`
  - `Gpu`
- `RenderNodeCommandListUsage`
  - `None`
  - `Require`
  - `Own`
  - `Sync`
- stable subtype/debug-name structures

### Does Not Own

- storage allocation
- dependency insertion
- graph import/merge
- execution

### Tests

- node ids and dependency ids are not interchangeable
- invalid ids are distinguishable from valid ids
- dependency kind indexing is stable
- command-list usage semantics are represented without backend handles
- node kind and role are independent

### Handoff

The next pass can store graph records using typed ids.

---

## Pass 2 - Graph Storage And Dense Usage Order

### Owns

- reusable storage container for node/dependency records
- dense live usage order
- free-list or slot reuse policy
- allocation appends to usage order
- free swap-removes from usage order
- id validation
- immutable access by id
- mutable access by id
- reset
- usage-order iteration

### Does Not Own

- dependency graph semantics
- graph build/freeze
- import/remap
- node implementation ownership

### Tests

- allocation returns valid typed ids
- freeing invalidates the id
- usage order is dense over live records
- freeing swap-removes without leaving stale live entries
- iteration uses usage order, not raw slot order
- reset clears all live ids and usage order

### Handoff

The next pass can use storage for node and dependency records.

---

## Pass 3 - Node Topology Data

### Owns

- `graph/node_topology.rs`
- `RenderNodeData`
  - kind
  - role
  - subtype
  - implementation id
  - camera index
  - group id
  - CPU/GPU flow groups
  - first parent dependency per kind
  - first child dependency per kind
- `RenderDependencyData`
  - kind
  - parent node
  - child node
  - next dependency from parent
  - next dependency from child
- `RenderNodeExecutionMetadata` or equivalent graph-owned node metadata
- read-only `RenderNodeView`

### Does Not Own

- insertion/removal algorithms
- graph import/merge
- node implementation trait
- execution context

### Tests

- node topology data initializes with invalid flow groups
- dependency topology data preserves parent/child/kind
- graph-owned metadata is distinct from `RenderNodeImplContext`
- read-only views cannot mutate graph topology

### Handoff

The next pass can add and remove dependencies safely over the stored node
topology.

---

## Pass 4 - Core Graph Mutation And Dependency Bookkeeping

### Owns

- `RenderNodeGraph`
- add node record
- add dependency
- reject self dependency
- reject duplicate dependency
- `has_dependency`
- remove dependency
- remove node without bridging
- remove node with bridge-parents-to-children mode
- copy parent dependencies
- copy child dependencies
- copy all dependencies
- frozen/built mutation guard
- debug checks that no dependency is reachable from only one side

### Does Not Own

- factory authoring API
- graph import/merge
- flow group build
- execution

### Tests

- dependency insertion links from parent and child
- duplicate dependencies are ignored or reported without duplicating storage
- self dependencies fail
- dependency removal unlinks both directions
- node removal removes all attached dependencies
- bridge removal connects parents to children by the same dependency kind
- bridge removal does not create cross-kind edges
- graph rejects mutation after build/freeze

### Handoff

The next pass can import and merge graphs using reliable mutation primitives.

---

## Pass 5 - Graph Import, Remap, And Special Merge

### Owns

- `AddGraphOptions`
  - camera index override
  - merge special nodes
  - import group behavior
- `add_graph(source, options)`
- source-to-destination node remap
- source-to-destination dependency remap
- reindex node dependency heads
- reindex dependency parent/child/next links
- shared node implementation arena assumption
- unique node merge by subtype
- sequence begin/end pair validation
- sequence merge
- temporary/helper removal with bridging
- final validation that no removable helper nodes remain

### Does Not Own

- graph cache
- factory construction helpers
- flow group build
- execution

### Tests

- imported nodes receive new ids
- imported dependencies receive new ids
- imported dependency links point to remapped ids
- camera index override applies to imported nodes
- unique nodes of same subtype merge into one survivor
- unique merge copies dependencies onto survivor
- broken sequence begin/end pairs fail loudly
- sequence merge preserves ordering
- temporary/helper removal bridges dependencies
- helper removal handles storage usage-order mutation safely
- no helper nodes remain after finalization

### Handoff

The next pass can build render-flow groups on final merged graphs.

---

## Pass 6 - Flow Groups, Flattening, And Cycle Detection

### Owns

- build flow groups for CPU dependency graph
- build flow groups for GPU dependency graph
- O(nodes + dependencies) topological processing
- deterministic same-level ordering by usage order or stable key
- strict cycle detection per dependency kind
- dense unique flow-group assignment
- flattened node arrays by dependency kind
- validation that CPU flattening satisfies CPU dependencies
- validation that GPU flattening satisfies GPU dependencies
- built/frozen graph transition

### Does Not Own

- job execution
- command recording
- graph cache

### Tests

- acyclic graph builds CPU and GPU flow groups
- CPU cycle fails with dependency kind and unresolved nodes
- GPU cycle fails with dependency kind and unresolved nodes
- isolated nodes receive CPU and GPU flow groups
- flow groups are dense and unique
- same-level order is deterministic
- CPU and GPU dependency graphs may differ legally
- built graph rejects topology mutation

### Handoff

The next pass can build authoring/factory APIs over finalized topology behavior.

---

## Pass 7 - Node Implementation Store And Trait Shell

### Owns

- `RenderNodeImpl` trait
  - `name`
  - `command_list_usage`
  - `execute`
  - `uses_child_jobs`
  - `allow_gpu_scope`
  - `global_binding_mod`
  - output/pass metadata hooks where needed
- `RenderNodeImplStore`
- shared implementation arena used by graph import/cache
- nullable implementation id for structural/helper nodes
- tiny test node implementations

### Does Not Own

- full `RenderNodeImplContext`
- process wrapper
- graph factory
- command recording backend

### Tests

- graph nodes can reference implementation ids
- structural nodes can have no implementation
- imported graph nodes keep valid implementation ids through shared store
- node metadata has stable defaults

### Handoff

The factory can create graph-visible nodes and subnodes.

One note for later: RenderNodeImplStore::clear() is okay for factory/cache reset, but it must only be used when graphs holding ids from that store are also discarded/reset. We should keep that invariant explicit when we build the factory/reset pass.

---

## Pass 8 - Graph Factory And Authoring API

### Owns

- `RenderNodeGraphFactory`
- explicit `finish` / `reset` semantics
- node implementation store ownership
- `create_node`
- `create_subnode`
- group registration
- group membership at node creation
- direct node-to-node links
- CPU-only node/group links
- group-to-node links
- node-to-group links
- group-to-group links
- `GroupEntry` and `GroupExit` anchor creation
- empty group handling
- no mutation in `Drop`

### Does Not Own

- command-list group internals
- automatic creation-order helpers
- execution

### Tests

- normal node creation returns a node id
- subnode creation without an open command-list group fails
- node creation records group membership
- direct links add dependencies
- group links only create CPU dependencies
- group entry/exit anchors are stable and explicit
- empty group links still preserve dependency structure
- factory finish freezes/builds graph only through explicit call

### Handoff

The next pass can add command-list group authoring.

---

## Pass 9 - Command-List Groups And Ordered Subnodes

### Owns

- `CommandListGroupNode`
- queue kind
  - graphics
  - compute
- begin command-list group
- end command-list group
- ordered subnode storage
- command-list group as graph-visible node
- subnodes not directly linkable by graph dependencies
- no nested command-list groups
- `create_node` while group is open fails
- `create_subnode` only while group is open
- begin/end queue request emission hook through `RenderNodeImplContext`
- preconsume subnode walk in deterministic order
- consume subnode walk in deterministic order

### Does Not Own

- final wgpu command recorder implementation
- child-job splitting inside command-list group
- sync node implementation

### Tests

- command-list group creates one graph-visible parent node
- subnodes are owned by the group and not graph-visible
- subnode order is preserved
- nested groups fail
- direct dependencies target the command-list group node
- begin/end queue requests wrap subnode request streams
- preconsume and consume visit subnodes in the same order

### Handoff

The next pass can add creation-order linking helpers that understand command-list
work.

---

## Pass 10 - Factory Linking Helpers

### Owns

- creation-order link helpers
- `link_gpu`
- `link_cpu`
- CPU-to-later-GPU-work helper
- CPU-from-earlier-GPU-work helper
- predicate-based linking over read-only node views
- duplicate dependency suppression
- helper APIs that accept nodes, groups, and dynamic node lists where needed
- command-list group metadata remap after topology import

### Does Not Own

- render feature graph recipes
- real camera graph building
- execution

### Tests

- creation-order GPU links connect graph-visible GPU work in order
- CPU-to-later-GPU helper creates CPU dependencies only
- CPU-from-earlier-GPU helper creates CPU dependencies only
- helpers skip subnodes because they are not graph-visible
- predicate linking cannot mutate graph while iterating
- duplicate links do not duplicate dependencies
- imported command-list group metadata points at remapped graph-visible parent
  nodes

### Handoff

The next pass can process graph nodes through the runtime wrapper.

---

## Pass 11 - Process Wrapper And Execution Core

### Owns

- `process_node`
- phase-aware node execution shell
- command-list usage handling at metadata level
- begin-node state reset
- process epilogue hook
- GPU scope/profiling hook abstraction
- global binding metadata restoration hook
- no direct calls to `RenderNodeImpl::execute` from graph execution
- sequential GPU-flow-order execution path for early validation

### Does Not Own

- dependency-counter parallel jobs
- final command recorder
- full context surface
- frame renderer orchestration

### Tests

- process wrapper calls execute once
- direct execution path is not used by graph executor
- begin-node state resets for every node
- epilogue runs for command-list nodes during consume
- command-list usage `None` nodes still execute during preconsume when needed
- consume-only side effects can be guarded by context phase

### Handoff

The next pass can complete the node implementation context.

---

## Pass 12 - Full RenderNodeImplContext Surface

### Owns

- frame/runtime handle
- allocator handle
- phase query
- flow group and flow space
- unique/global node flag
- camera/view access split
  - current camera access
  - indexed/all-camera access for unique nodes
- worker index
- descriptor helper APIs
- tag/resource allocator front door integration
- command recorder accessors
- pass/output setup entry points
- binding builder/pass wrapper entry points
- viewport setting through active pass/recorder only
- per-node cleanup state

### Does Not Own

- final renderer data model
- material/mesh prepared stores
- real binding layouts

### Tests

- context cannot be used before node setup
- camera-only access fails for unique/global nodes
- indexed camera access fails for ordinary camera nodes
- flow-space selection matches unique/global vs camera/view nodes
- descriptor helpers preserve current/max size distinction
- command recorder access routes through frame runtime
- viewport outside active pass fails loudly
- context copies for workers do not share mutable per-node state

### Handoff

The next pass can add command recording abstractions behind the context.

---

## Pass 13 - Frame Command Recorder Abstraction

### Owns

- frame command recording slots
- command recording storage prepared from graph node count
- command-list usage lowering to wgpu-shaped command recording
- graphics/compute queue kind metadata
- ordered command-buffer submission model
- sync node runtime hooks
- separation of allocator `queue_sync` from command recorder sync
- debug labels / profiler markers
- command recorder cleanup

### Does Not Own

- real render pipelines
- material bind groups
- draw-call generation

### Tests

- command recording storage prepares slots for graph nodes
- `Own` creates/owns a recording slot
- `Require` fails without an available recording slot
- `Sync` does not pretend to be a normal command-list node
- ordered submission follows GPU dependency order
- allocator queue sync can be recorded without owning command recording state
- recorder cleanup happens through explicit execution shell, not context drop

### Handoff

The next pass can run nodes as dependency-counter jobs.

---

## Pass 14 - Dependency-Counter Consume Execution

### Owns

- runtime job node table
- CPU dependency edges wired into job wait counters
- external kickoff dependency
- terminal graph-node completion handle
- job payload with node id, node metadata, implementation id, frame runtime, and
  worker index
- per-node `RenderNodeImplContext` construction
- all jobs scheduled up front
- debug validation that terminal completion is not premature
- counter/handle state lifetime

### Does Not Own

- full frame renderer
- graph cache integration
- real render feature nodes

### Tests

- CPU dependency parent completes before child starts
- independent nodes can run in parallel
- GPU-only dependency does not become a CPU job dependency
- external kickoff gates root nodes
- terminal completion waits for all terminal nodes
- premature terminal completion debug check catches invalid state
- job payload passes frame/runtime context explicitly
- no global static current render frame exists

### Handoff

The next pass can add graph cache and built graph lifetime.

One honest note: Pass 14 is still a scheduling shell, not true parallel job dispatch over mutable frame state. It exposes ready batches that can be parallelized later, but the current executor processes those batches sequentially so allocator/runtime borrowing stays sound.

The real parallel execution fix belongs in the frame execution/orchestration layer, likely around Pass 16 or immediately after it. The reason is that true parallel node execution cannot just be “turn ready batch into jobs” while we still pass &mut FrameResourceAllocator and &mut FrameCommandRecorders directly into each node. Rust is correctly forcing us to answer: who owns per-thread/per-node mutable state?

The shape I expect:

Pre-consume remains sequential or uses per-node recording buffers
Nodes record allocator requests into per-flow-group streams, then merge/validate deterministically.

Consume can parallelize CPU-side node work
Ready CPU batches become real leet_jobs2 jobs, but each job gets isolated context state. Shared frame services need interior partitioning or command queues.

Command recording is per-flow-group/per-thread
Each node records into its own recorder slot. Submission remains ordered by GPU flow groups afterward.

Frame runtime becomes the owner
Instead of passing raw mutable allocator/runtime refs to every node, a FrameExecutionRuntime owns allocator replay, command recorders, job counters, and thread-safe or partitioned access.
---

## Pass 15 - Render Graph Cache

### Owns

- `RenderGraphCache`
- `RenderGraphCacheEntry`
- deterministic graph-shape hash
- per-camera setup graph cache data
- final merged graph storage
- camera build data ownership that outlives temporary graph topology
- cache hit by hash and camera setup count
- oldest-entry eviction/reuse
- post-build clear of temporary topology without dropping node implementations

### Does Not Own

- full frame renderer graph recipe selection
- Bevy extraction
- actual camera render features

### Tests

- cache hit requires same graph-shape hash
- cache hit requires same camera setup count
- cache miss reuses oldest entry
- per-camera node storage remains alive after temporary graph reset
- final merged graph remains executable after camera temp graph topology clears
- cache does not store transient GPU resources

### Handoff

The next pass can orchestrate graph build, preconsume, resolve, consume, and
cleanup.

---

## Pass 16 - Graph Execution Orchestration Harness

This is a graph-core execution harness, not the production `FrameRenderer`.
Its job is to prove that a built graph can drive allocator phases, command
recording preparation, node-job gating, terminal completion, and cleanup in the
correct order. It is similar in spirit to `tinyGraphTest`: production semantics,
small controlled surface.

Implementation module: `graph/core_runner.rs`. The name is intentional: this is
the core graph lifecycle runner, not the future renderer-facing frame runner.

### Owns

- immutable/exclusive execution view
- no overlapping `RenderFrame` in V1
- graph build/import/merge hook
- render-flow group build after final merge
- command recording storage preparation
- camera/frame custom data preparation hook
- allocator phase sequence:
  - Startup
  - PreConsume
  - Resolve
  - Consume
  - Cleanup
- preconsume shared/base context traversal
- resolve barrier before consume jobs start
- node-job kickoff gate release after resolve
- terminal completion wait
- frame execution epilogue
- allocator cleanup phase in frame epilogue, not hidden inside a node

### Does Not Own

- real frame renderer data extraction
- production camera graph recipes
- draw calls
- final `FrameRenderer` ownership or public orchestration API

### Tests

- graph cannot be mutated while execution view is active
- recursive/concurrent frame execution fails loudly
- preconsume runs before resolve
- consume jobs cannot start before resolve gate releases
- allocator cleanup runs exactly once in frame epilogue
- terminal completion is awaited before cleanup
- command recording storage prepares after graph build and before consume
- per-node consume contexts are distinct from preconsume base context

### Handoff

The core runtime is usable by tiny production-like graph tests without claiming
to be the final frame renderer.

---

## Pass 17 - Core System Nodes

Implementation module: `graph/system_nodes.rs`. These are graph-core fixture
implementations, not renderer feature algorithms. Public node types stay
explicit and recipe-readable: `RenderNodeStartRender`, `RenderNodeEndRender`,
`RenderNodePresent`, `RenderNodeSynchronize`, declaration nodes, and
render-target begin/end nodes are separate structs.

### Owns

- lifecycle/system node implementations as graph-core fixtures:
  - start render
  - end render
  - present/control placeholder
  - flush texture grabs placeholder
  - flush buffer grabs placeholder
  - cleanup batch-data placeholder
  - end frame lifecycle node
- sync node implementation shape
- declaration node test implementation shape
- render-target setup/end node test shape
- ordinary `RenderNodeImpl` implementations for all system nodes

### Does Not Own

- actual renderer feature algorithms
- real readback/grab systems
- real batch allocator

### Tests

- lifecycle nodes participate in dependencies like normal nodes
- consume-only side effects are guarded
- declaration/system nodes can have `CommandListUsage::None`
- sync node records allocator queue sync in both phases and command recorder sync
  during consume
- end-frame lifecycle node does not own allocator cleanup phase

### Handoff

The next pass can use tiny graph tests to validate realistic topology.

---

## Pass 18 - Tiny Graph Core Integration Tests

This pass extends `tinyGraphTest` only as an integration harness. It is not the
production graph recipe system.

### Owns

- graph factory plus allocator execution in one test harness
- command-list group with subnodes
- CPU and GPU dependency split scenario
- graph import/merge scenario
- unique node merge scenario
- graph cache scenario
- preconsume/resolve/consume/cleanup full path
- simple transient texture and buffer declaration nodes

### Does Not Own

- full renderer frame graph
- Bevy scene extraction
- material/mesh pipelines

### Tests

- imported camera graph merges into final graph
- render-flow groups build after import
- preconsume request streams match consume replay
- command-list group subnodes record requests in order
- CPU dependency controls job readiness
- GPU dependency controls command recording/submission order
- graph cache hit executes with retained node storage
- allocator cleanup happens in frame epilogue

### Handoff

Graph core is ready for frame renderer planning and real renderer graph recipes.

---

## Open Decisions Before Coding

These must be answered before their pass starts:

- final Rust name for graph-owned node metadata:
  - `RenderNodeContext`
  - `RenderNodeExecutionMetadata`
  - another explicit name
- exact command recording abstraction names:
  - `FrameCommandRecorder`
  - `FrameCommandListId`
  - `CommandRecordingSlot`
- graph cache hash type and hashing policy
- whether graph storage ids use generation counters or debug-only stale-id checks
- how much of the existing `render_node_impl_context.rs` is kept versus reshaped
  during Pass 12
- how the graph execution harness plugs into the current render app tests without
  committing to the full `FrameRenderer`

Do not block independent earlier passes on later frame-renderer decisions.
