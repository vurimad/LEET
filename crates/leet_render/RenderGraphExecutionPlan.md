# LEET Render Graph - Execution Plan

This file is the implementation plan for the LEET render graph frame resource
allocator.

It exists to prevent "simple for now" allocator code from becoming the
foundation of the renderer. Each pass is a production layer. A later pass may
extend an earlier layer, but it should not need to replace its data model.

The design contract lives in `RenderGraphDesign.md`. This file is the build
order.

---

## Ground Rules

### Production Layers Only

If a behavior is part of the current pass, implement it with final V1 semantics.

If a behavior belongs to a later pass, do one of these:

- leave the public API absent
- keep the function private and unused
- make the unsupported path fail loudly with a clear error

Do not add placeholder behavior that silently acts like a descriptor-to-texture
cache. The allocator shape must remain request-stream, lifetime, timeline, and
pool based from the first pass.

### Pass Boundaries Are Contracts

Each pass has:

- an owned scope
- explicit non-goals
- tests that must pass before moving on
- a clean handoff to the next pass

When a pass exposes public API, that API must have the final V1 meaning even if
some later operation returns `Unsupported` until its layer exists.

If a pass discovers that an earlier layer cannot support RED-style request
replay, timeline lookup, ownership-aware pool reuse, imported resources, or
multi-camera flow spaces, stop and fix the earlier layer before continuing.

### Design And RED Source Authority

Implementation must respect `RenderGraphDesign.md`. If code pressure reveals an
ambiguity, do not guess and do not simplify the behavior to fit the easiest Rust
shape.

When the design document is unclear or two interpretations seem possible,
inspect the relevant RED `.h` and `.cpp` files again, then mirror the behavior in
Rust syntax and Bevy/wgpu boundaries. If the Rust implementation must diverge
from RED for backend reasons, update `RenderGraphDesign.md` with the reason
before continuing.

### No Out-Of-Scope Drift

Do not implement these during the allocator passes unless a pass explicitly says
so:

- material asset loading
- Bevy `Image` asset preparation
- mesh vertex/index buffer ownership
- material bind groups or descriptor sets
- shader pipeline compilation
- scene extraction
- high-level render node algorithms
- physical memory aliasing on wgpu
- replacing Bevy's view-target path before that boundary is decided

The allocator may store raw Bevy/wgpu-facing `Texture`, `TextureView`, and
`Buffer` handles. It must not become the owner of higher-level LEET buffer
wrappers or asset systems.

### Comment Quality

Production comments should explain Rust invariants and allocator correctness:

- why a phase operation is allowed or rejected
- why a request must replay exactly
- why a tag lookup uses current request time
- why an imported or externally swapped resource cannot be recycled
- why a lifetime was extended by queue/fork/join behavior
- why a pooled resource can or cannot be reused

Do not make source comments depend on C++ filenames. RED references belong in
the design document or this execution plan, not ordinary implementation comments.

### Test Placement

Allocator tests should live under the render graph test tree, not inside large
implementation files:

```text
src/render_graph/tests/
  mod.rs
  resources_tags.rs
  resources_desc.rs
  resources_requests.rs
  resources_lifetimes.rs
  resources_pool.rs
  resources_resolve.rs
```

Small unit tests next to tiny pure helper modules are allowed only when they stay
short. Any test that needs scenario setup, multiple requests, pool state, graph
state, or replay validation belongs under `render_graph/tests`.

---

## Common Definition Of Done

Every pass is done only when:

- `cargo fmt` has been run
- `cargo test -p leet_render` passes, or the pass documents why a narrower test
  command is temporarily required
- tests cover invariants and failure paths, not only happy paths
- public API added in the pass has final V1 semantics
- unsupported later-pass behavior is absent or loudly unavailable
- allocator tests for non-trivial behavior live under `render_graph/tests`
- no pass has introduced a public descriptor-to-cached-texture shortcut
- no pass has introduced a texture-only allocator shape
- imports distinguish allocator authority from Rust handle ownership
- implementation follows `RenderGraphDesign.md`, or the design document was
  updated before an intentional divergence
- source comments explain Rust invariants without relying on C++ call paths
- `RenderGraphDesign.md` is updated if implementation changes a locked design
  decision

---

## Target File Map

The allocator should live under the render graph module:

```text
src/render_graph/resources/
  mod.rs
  error.rs
  phase.rs
  usage.rs
  tag.rs
  desc.rs
  request.rs
  lifetime.rs
  pool.rs
  allocator.rs
  diagnostics.rs
src/render_graph/
  render_node_impl_context.rs
src/render_graph/tests/
  mod.rs
  resources_*.rs
  render_node_impl_context.rs
  tinyGraphTest/
    mod.rs
    *.rs
```

The exact split can change if implementation pressure justifies it, but the
conceptual boundaries must remain clear:

- `tag.rs`: logical names, tags, flow spaces, request time
- `desc.rs`: texture/buffer descriptors and compatibility rules
- `request.rs`: recorded request stream and replay matching
- `lifetime.rs`: allocation requests, tag timelines, lifetime solving
- `pool.rs`: owned/imported GPU resource storage, reuse, eviction
- `allocator.rs`: phase machine and orchestration
- `render_node_impl_context.rs`: node implementation context that mirrors the
  resource-facing parts of RED's `SRenderNodeImplContext`
- `diagnostics.rs`: request, lifetime, and pool dumps

---

## Pass 0 - Module Skeleton And Error Surface

### Owns

- `resources/mod.rs` file map
- `FrameResourceError`
- `FrameResourceResult<T>`
- public re-exports from `resources/mod.rs`
- compile-only module integration into `render_graph/mod.rs`
- feature-free CPU-only test scaffolding

### Does Not Own

- tags
- descriptors
- request recording
- allocator phase behavior
- pool storage
- Bevy/wgpu resource creation

### Tests

- crate compiles with the new module tree
- public re-exports compile from `render_graph::resources`
- error variants can be formatted for diagnostics

### Handoff

The next pass can add types without changing the crate/module layout.

---

## Pass 1 - Static Allocator Types

### Owns

- `ResourceAllocatorPhase`
  - `Startup`
  - `PreConsume`
  - `Resolve`
  - `Consume`
  - `Cleanup`
- `ResourceUsage` bitflags
  - `READ`
  - `WRITE`
  - `NO_DISCARD`
- `RenderFlowName`
- `RenderFlowNameTag`
- invalid/null tag
- autogenerated tag id support
- `RenderFlowSpace`
- `RenderFlowGroup`
- `RequestTime`
- stable id wrappers
  - `ResourceRequestId`
  - `AllocationRequestId`
  - `FrameResourceAllocationId`

### Does Not Own

- resource descriptors
- request payloads
- phase transitions
- replay validation
- lifetime solving
- resource pool storage

### Tests

- usage flags combine and test correctly
- invalid/null tag is distinguishable from real tags
- same name in two flow spaces produces distinct tags
- same name in same flow space produces stable tags
- autogenerated ids produce distinct tags within a flow group
- `RequestTime` sorts by group and request index
- all id wrappers are cheap `Copy` values

### Handoff

The next pass can attach descriptors and requests to stable tag/time/id types.

---

## Pass 2 - Resource Descriptors And Compatibility

### Owns

- `FrameResourceDesc`
  - `Texture(FrameTextureDesc)`
  - `Buffer(FrameBufferDesc)`
- `FrameTextureDesc`
- `FrameBufferDesc`
- explicit current size and max capacity fields
- descriptor validation
- descriptor comparison methods
  - `is_exact_match`
  - `is_equal_ignoring_max_size`
  - `is_compatible_for_swap`
  - `can_reuse_for`
- `current_allocation_shape`
- `max_capacity_shape`
- concrete wgpu descriptor construction from selected allocation shape

### Does Not Own

- request stream recording
- dynamic-resolution policy beyond descriptor shape and validation
- GPU resource creation
- pool reuse
- imported resource handles

### Tests

- exact match detects every creation-relevant difference
- equality ignoring max size still checks resource kind and non-size fields
- swap compatibility is distinct from exact equality
- texture and buffer descriptors never compare compatible across resource kinds
- current size must fit inside max size when max exists
- concrete texture descriptor uses selected allocation size, not two competing
  size authorities
- concrete buffer descriptor uses selected allocation size, not two competing
  size authorities

### Handoff

The next pass can record declarations by descriptor without needing to know how
pool assignment will happen.

---

## Pass 3 - Request Stream And Phase Replay Shell

### Owns

- `ResourceRequest`
  - declare by descriptor
  - declare like another tag
  - import texture
  - import buffer
  - is-declared query
  - use begin
  - use end
  - free
  - swap
  - swap with external texture
  - swap with external buffer
  - begin queue
  - end queue
  - queue sync
  - decision
- `RequestGroup`
  - pre-consume requests
  - consume cursor
  - touched flag
- request replay matcher
- phase operation validation for request APIs
- deterministic `decision` recording and replay
- deterministic `is_declared` recording and replay
- exact consume replay validation against pre-consume request stream

### Does Not Own

- descriptor copy resolution for declare-like
- lifetime solving
- tag timeline lookup
- pool assignment
- typed resource retrieval
- node-facing `rctx`

### Tests

- requests can be recorded during `PreConsume`
- request APIs reject invalid phases
- consume replay accepts matching requests in order
- consume replay rejects missing, extra, reordered, or mismatched requests
- decision replay rejects branch divergence
- is-declared replay is deterministic and does not query resolved GPU state
- swap-with-external replay validates logical request identity without pointer
  equality
- import replay validates logical request identity without pointer equality
- queue sync replay validates sync type and context

### Handoff

The next pass can trust the recorded stream as the source of truth.

---

## Pass 4 - Allocator Phase Machine

### Owns

- `FrameResourceAllocator` shell
- `set_phase`
- `phase`
- `is_consume_phase`
- startup/preconsume/resolve/consume/cleanup transition validation
- per-frame request group reset
- cleanup replay-cursor assertions
- clear-all-caches phase restrictions
- clear error messages for invalid transitions

### Does Not Own

- lifetime solving
- pool resource creation
- typed getters returning real resources
- render graph integration

### Tests

- valid phase sequence succeeds
- invalid phase transitions fail loudly
- entering `PreConsume` clears per-frame request state
- entering `Consume` is impossible before `Resolve`
- `Cleanup` asserts all consume cursors reached the end of their groups
- request APIs cannot be used in `Resolve`, `Startup`, or `Cleanup`
- retrieval APIs cannot be used before `Consume`
- `clear_all_caches` is only allowed during cleanup or an explicit quiescent path

### Handoff

The next pass can hang lifetime resolution on the `Resolve` transition.

---

## Pass 5 - Lifetime Solver And Tag Timelines

### Owns

- `AllocationRequest`
- `RequestRange`
- `TagLifetime`
  - trivial fast path
  - timeline/event path
- `TagLifetimeEvent`
- per-tag request key collection
- declare-by-descriptor lifetime creation
- declare-like descriptor resolution
- import lifetime creation
- use begin/end lifetime touches
- free lifetime close
- swap timeline events
- swap-with-external timeline events
- queue/fork/join lifetime extension
- declared-but-unused resources produce no allocation request
- getter-time lookup by `(tag, current_request_time)`

### Does Not Own

- assigning pool allocations
- creating wgpu textures or buffers
- returning real `Texture`, `TextureView`, or `Buffer` handles
- node-facing `rctx`

### Tests

- simple declare/use/free produces one trivial lifetime
- declared-but-unused resource records the request but creates no allocation
  request
- use begin and use end both extend the lifetime
- free closes the lifetime
- declare-like copies the source descriptor at the correct request time
- swap closes both previous mappings and creates timeline events for both tags
- swap-with-external marks the old allocation non-cacheable/non-reusable as
  required
- import creates a tracked non-owned lifetime
- queue/fork/join extends lifetimes conservatively
- getter-time lookup returns different allocations for a tag before and after a
  swap
- missing tag lookup fails loudly

### Handoff

The next pass can assign allocation requests to pooled resources using solved
lifetimes.

---

## Pass 6 - Pool Assignment Planner

### Owns

- pool assignment over abstract `FrameResourceAllocationId`
- lifetime overlap test with closed intervals
- largest-first allocation request ordering
- stable start-time tie-breaker
- same-frame reuse eligibility
- cross-frame cache reuse eligibility
- ownership-aware reuse classes
  - owned reusable
  - imported non-owned
  - external-swap restricted
- descriptor/capacity checks for reuse
- debug reasons for reuse rejection

### Does Not Own

- actual Bevy/wgpu resource creation
- eviction of real GPU handles
- render graph integration
- physical memory aliasing on wgpu

### Tests

- overlapping lifetimes never share the same owned allocation
- touching endpoints count as overlap
- non-overlapping lifetimes may reuse the same owned allocation
- same-frame reuse requires previous lifetime to be completely over
- different resource kinds never reuse the same allocation
- incompatible descriptors do not reuse
- larger cached capacity can satisfy smaller current size
- current size larger than cached capacity rejects reuse
- imported allocations are never recycled
- external-swap allocations follow stricter reuse or no-reuse policy
- largest-first ordering is deterministic
- diagnostics explain why a candidate was rejected

### Handoff

The next pass can attach actual texture/buffer handles to planned allocations.

---

## Pass 7 - FrameResourcePool Storage And Eviction

### Owns

- `FrameResourcePool`
- `FrameResourceAllocation`
- `FrameResource`
  - texture resource
  - buffer resource
- `FrameResourceOwnership`
  - owned
  - imported
  - external swap
- owned texture creation through Bevy `RenderDevice`
- owned buffer creation through Bevy `RenderDevice`
- default texture view creation
- imported texture storage
- imported buffer storage
- cache age tracking
- non-cacheable cleanup
- age-threshold eviction
- dynamic-resolution shrink eviction policy hooks
- dropping owned wgpu handles as release action
- forgetting imported/external records without destroying resources

### Does Not Own

- render-node API ergonomics
- Bevy view-target replacement decision
- physical memory aliasing on wgpu
- material/mesh/asset buffer ownership

### Tests

- owned texture allocation stores texture and default view
- owned buffer allocation stores buffer handle
- imported texture allocation stores texture and view without marking ownership
  as owned
- imported buffer allocation stores buffer without marking ownership as owned
- cleanup resets per-frame tracking on reusable owned allocations
- non-cacheable owned allocation is released on cleanup
- aged owned allocation is evicted after threshold
- imported allocation cleanup only forgets allocator state
- external-swap allocation cleanup never destroys an external resource
- dynamic-resolution shrink hook can identify oversized cached allocations

### Handoff

The next pass can resolve real frame resources and expose typed getters.

---

## Pass 8 - Resolve And Typed Retrieval

### Owns

- `Resolve` implementation
- request stream validation before solving
- lifetime solving orchestration
- pool assignment orchestration
- real pool allocation creation/reuse/import attachment
- resolved tag-to-allocation mapping
- `get_texture`
- `try_get_texture`
- `get_buffer`
- `try_get_buffer`
- current consume request time tracking
- retrieval validation
  - consume phase only
  - declared/resolved tag only
  - correct resource kind only
  - correct timeline event for current request time

### Does Not Own

- node-facing helper ergonomics
- full render graph execution integration
- replacing Bevy's view-target path

### Tests

- resolve assigns resources for touched declarations
- resolve does not allocate declared-but-unused resources
- getters fail before consume
- getters fail for wrong resource kind
- getters fail for unresolved tags
- `try_get_*` returns `None` only for optional missing/unresolved cases allowed
  by the API contract
- getter after swap returns the resource for the current consume cursor
- consume replay plus getter works for texture and buffer resources
- cleanup after consume succeeds only when every request was replayed

### Handoff

The allocator is internally usable. The next pass can wrap it with node-facing
context.

---

## Pass 9 - Node-Facing Resource Context

### Owns

- `rctx` resource helpers
- `rt_name_tag`
- temp resource tag helper
- current node flow-space selection
- shared/global flow-space handling for unique nodes
- camera/view flow-space handling for view nodes
- node-facing declare/import/use/free/swap wrappers
- node-facing typed getters
- node-facing errors that preserve allocator diagnostics

### Does Not Own

- full graph compiler rewrite
- custom render node implementations beyond test nodes
- Bevy view-target replacement decision

### Tests

- camera node creates tags in camera flow space
- unique/global node creates tags in shared flow space
- two cameras with the same logical name do not collide
- temp tags are unique by request position
- node wrappers record the same request stream as allocator-facing calls
- explicit use-begin/use-end records the intended range in both preconsume and
  consume paths
- node getter is unavailable during preconsume

### Handoff

The next pass can run allocator-backed resource declarations through a small
render graph path.

---

## Pass 10 - Render Graph Integration Slice

Pass 10 is a test-only integration harness, not the real render graph.

The harness must live under:

```text
src/render_graph/tests/tinyGraphTest/
```

The intentionally awkward name marks the folder as a small graph-shaped test
driver. Do not add production graph architecture here, and do not revive the
old files in `render_graph/badgraph`.

### Owns

- minimal test-only graph execution hook for allocator phases
- preconsume test-node walk
- resolve barrier between preconsume and consume
- consume test-node walk
- cleanup after test graph execution
- allocator resource injection points for external view targets
- first test render nodes
  - transient texture producer/consumer
  - transient buffer producer/consumer
  - import texture path
  - import buffer path
  - swap path

### Does Not Own

- public render graph compiler/runtime API
- production render graph file layout
- production render node traits
- command-list scheduling
- replacing the full renderer
- final production render node library
- Bevy core view-target replacement unless explicitly selected before this pass

### Tests

- graph preconsume and consume walks produce matching request streams
- mismatched runtime branch fails through `decision`
- transient texture producer/consumer retrieves the same resolved texture
  through the intended lifetime
- transient buffer producer/consumer retrieves the same resolved buffer through
  the intended lifetime
- imported texture participates in lifetime tracking without allocator ownership
- imported buffer participates in lifetime tracking without allocator ownership
- swap path retrieves different resources at different consume positions
- cleanup leaves allocator ready for the next frame

### Handoff

The allocator has a real graph path and can be exercised by production render
features.

---

## Pass 11 - Diagnostics And Debug Dumps

### Owns

- request stream dump
- replay mismatch report
- lifetime dump
- tag timeline dump
- pool allocation dump
- reuse decision dump
- eviction dump
- per-frame summary counters
- clear formatting for camera flow spaces and temp tags

### Does Not Own

- editor UI
- profiling integration
- GPU capture tooling

### Tests

- request dump contains group, index, request kind, and tag
- replay mismatch report identifies expected and actual request
- lifetime dump shows start/end request times
- timeline dump shows swap/import events
- pool dump distinguishes owned/imported/external-swap resources
- reuse decision dump includes rejection reasons
- diagnostics do not depend on raw pointer values for identity

### Handoff

The allocator is inspectable enough to debug real render graph features.

---

## Pass 12 - Multi-Camera And Dynamic-Resolution Validation

### Owns

- first real multi-camera allocator test
- main camera plus lower-resolution mirror/reflection camera scenario
- per-camera flow-space validation
- dynamic-resolution growth validation
- dynamic-resolution shrink/cache validation
- same-frame reuse validation across non-overlapping camera/resource lifetimes
- no-collision validation for same logical names in different flow spaces

### Does Not Own

- complete reflection renderer
- final dynamic-resolution policy tuning
- editor-facing resource visualizer

### Tests

- `scene_color` for main camera and mirror camera are distinct logical tags
- different current sizes do not incorrectly share an allocation
- compatible non-overlapping resources may reuse the pool safely
- growing beyond cached capacity allocates a larger resource
- shrinking can keep larger cache until eviction policy removes it
- imported camera target does not get recycled by the allocator

### Handoff

The allocator is ready for broader render-node adoption.

---

## Open Implementation Decisions Before Coding

These decisions should be answered before their pass begins:

- whether tag hashing preserves RED's exact XOR layout or uses a stronger
  internal key with the same semantic fields
- whether the first allocator instance is a render-world resource or a
  frame-local object owned by graph execution
- whether LEET keeps Bevy `prepare_view_targets` or replaces it for the first
  allocator-backed camera path
- which first render nodes exercise transient textures and transient buffers
- richer memory-budget thresholds for buffer and texture cache eviction beyond
  the V1 age threshold

Do not block earlier independent passes on later integration decisions.
