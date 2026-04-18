# Renderer Roadmap

This is the working roadmap for the LEET renderer.

The goal is to move carefully from the current low-level foundation to a real
explicit frame graph without losing determinism, clarity, or the ability to
see visible progress at each step.

## Current State

Completed:

- [x] `wgpu` device, queue, and surface bootstrap
- [x] Frame-scoped `RenderContext`
- [x] Frame command-list registry (`FrameCommandLists`)
- [x] Render node traits and execution-plan runtime
- [x] Declarative `RenderGraph`
- [x] Graph compilation into `RenderExecutionPlan`
- [x] CPU/GPU dependency edges in the graph
- [x] GPU-level compilation and automatic submit insertion
- [x] Blank frame executed through the graph
- [x] First mixed example graph with `Own` + `Require` + later `Own`
- [x] Renderer-owned `RenderScene` with queued cross-thread updates
- [x] Minimal `RenderProxy` model and per-frame `RenderCollector`
- [x] Ping-pong `RenderSceneCommands` inbox for swap-and-drain scene updates
- [x] Minimal `RenderViewport` plus `submit_frame(frame_info, camera_info)` API
- [x] Placeholder `FrameRenderer` consuming viewport submissions plus scene data

Current renderer shape:

- The graph is a DAG of node dependencies.
- `Own` nodes become recording tasks with their own command-list slot.
- `Require` nodes chain into an existing `Own` task through a CPU dependency.
- `GPU` edges move work into a later GPU level.
- Submit barriers can be explicit, and the compiler also inserts automatic
  submits between GPU levels and at end-of-frame.
- The renderer now consumes a coherent per-frame collected scene instead of
  reaching directly into live world state.
- Scene updates can be queued from any thread into `RenderScene`; the renderer
  drains them at snapshot time to build one stable frame view.
- Producer threads now write through a `RenderSceneCommands` handle; snapshot
  swaps the active queue and drains the previous queue privately.
- The viewport layer now has a small `begin_frame/submit_frame`
  contract, and a placeholder `FrameRenderer` now consumes those submissions.
- `Renderer::render_scene(scene)` remains only as a convenience wrapper around
  the current viewport + frame-renderer placeholder path.
- There is not yet a dedicated render thread or a formal main-thread/renderer
  synchronization contract.

Current placeholder limitation:

- `Require` nodes currently share a command encoder / command buffer with their
  `Own` parent, but they do not yet share one already-open render pass.
- Because of that, GPU markers are currently expected to appear as a flat
  command-buffer sequence, not as nested pass markers under one live main pass.
- Real pass-sharing will need a pass-scoped recording API, likely by handing
  child work a `wgpu::RenderPass`-style recorder instead of only a
  `wgpu::CommandEncoder`.

## Target Threading Model

This is the target shape we want to grow into. It is intentionally explicit so
we do not accidentally let the renderer become "whatever thread happens to call
it today."

- The main thread owns app lifecycle, OS window events, input, and gameplay
  world mutation.
- A render thread owns frame orchestration for rendering work:
  acquire/backbuffer decisions, frame kickoff, submit scheduling, and present.
- `leet_jobs` worker threads support the render thread by running extract,
  prepare, cull, and command-recording work in parallel.
- The render graph/compiler decides task groups and submit boundaries; the
  render thread does not manually micromanage every pass.
- Communication from the main thread to the renderer should happen through
  explicit frame messages or extracted frame data, not by sharing mutable game
  state during rendering.
- Communication from the renderer back to the main thread should happen through
  explicit completion/results channels, not ad hoc callbacks into gameplay code.

Main-thread to render-thread sync points we already know we will need:

- Window resize / surface reconfiguration
- Shutdown / suspend / device-lost style transitions
- Resource creation and destruction that must happen on the renderer side
- Screenshot / readback / GPU result delivery
- Optional hot-reload or debug capture requests

The intended contract is:

- main thread produces render input for frame `N`
- render thread owns render execution for frame `N`
- `leet_jobs` workers parallelize sub-work inside that frame
- explicit counters/fences/channels define when data may cross between threads

## Working Rules

These rules are here so we do not rush into a harder architecture too early.

- Prefer explicit graph structure before automatic inference.
- Keep each milestone visible and testable.
- Do not add advanced caching, aliasing, or async compute before the main frame
  path is stable.
- Do not parallelize graph execution until the grouping and submit semantics are
  solid in serial execution.
- Do not let the app thread and renderer mutate the same live frame state
  without an explicit handoff point.
- Keep the architecture explicit and practical, and adapt to `wgpu` instead of
  forcing a one-to-one clone of another engine's internals.

## Next Milestones

### Milestone 1: Real Grouped Pass Example

Status: completed

Goal:

- Prove that one `Own` node plus multiple `Require` nodes really behaves like a
  shared recording task.

Scope:

- Add `StartFrameNode` (`None`)
- Add `MainPassRootNode` (`Own`)
- Add `OpaqueDrawsNode` (`Require`)
- Add `SkyDrawsNode` (`Require`)
- Add `BloomNode` (`Own`)
- Add `PresentNode` or equivalent frame step
- Build one mixed graph using both CPU and GPU dependencies

Done when:

- We can point to one graph that demonstrates:
  - shared encoder/task grouping
  - separate `Own` passes
  - a GPU edge causing a later submit level
  - a final present path

### Milestone 2: Parallel Task Execution By GPU Level

Status: planned

Goal:

- Execute record tasks in the same GPU level in parallel using `leet_jobs`.

Scope:

- Group compiled tasks by GPU level
- Dispatch independent `RenderRecordTask`s in parallel
- Wait for all tasks in a level before submit
- Preserve deterministic submit ordering

Done when:

- Multiple `Own` tasks in one level record in parallel
- Submit order stays deterministic
- The serial and parallel paths produce the same frame behavior

### Milestone 3: Render Thread And Main Thread Contract

Status: planned

Goal:

- Introduce a dedicated render-thread coordination model on top of `leet_jobs`
  without losing deterministic frame ownership.

Scope:

- Define the render thread's responsibilities versus the app/main thread
- Add a render command or frame-message channel for main-thread to renderer
  requests
- Define extracted per-frame render data handoff
- Add counters/fences/events for "frame accepted", "frame finished", and
  blocking sync operations
- Identify operations that must force a main-thread/render-thread rendezvous:
  resize, shutdown, readback, and device/surface recreation

Done when:

- We can describe exactly how frame work moves from main thread to render thread
- We have one concrete synchronization path for resize and shutdown
- `leet_jobs` worker usage fits under that contract instead of bypassing it

### Milestone 4: App Integration

Status: planned

Goal:

- Move renderer execution into the actual app lifecycle.

Scope:

- Initialize renderer on window-ready
- Resize render surface on window resize
- Execute graph in the frame loop
- Keep the current blank/mixed graph visible through the app

Done when:

- The app creates, resizes, and runs the renderer end-to-end

### Milestone 5: Camera Lite

Status: planned

Goal:

- Add the smallest useful camera-driven render path.

Scope:

- Camera/frame uniform data
- Depth target
- One camera graph
- One opaque scene pass

Done when:

- A single camera can render a simple scene through the graph

### Milestone 6: First Real Draw

Status: planned

Goal:

- Replace clear-only rendering with real geometry.

Scope:

- Triangle or debug quad first
- Then one mesh path
- Basic pipeline/shader ownership per node or pass

Done when:

- A visible draw call runs through the graph and command-list system

## Medium-Term Roadmap

### Resource Model

Status: planned

Goal:

- Stop thinking only in node dependencies and start modeling named render
  targets/resources.

Scope:

- Named graph resources
- Read/write declarations
- Backbuffer, color targets, and depth targets as graph resources
- Resource allocation and lifetime tracking

Done when:

- Nodes can declare what they read and write
- The graph compiler can reason about resource flow, not only node order

### Automatic Edge Inference

Status: planned

Goal:

- Reduce the amount of manual edge wiring needed by users of the renderer.

Scope:

- Derive GPU dependencies from read/write hazards
- Derive some CPU dependencies from orchestration rules
- Keep manual edges available for explicit control

Done when:

- Most common pass chains can be authored without manually specifying every edge

### ECS Extraction

Status: planned

Goal:

- Feed the renderer from ECS/world data in a stable extracted form on top of
  the new renderer-owned scene boundary.

Scope:

- Mirror ECS/world objects into renderer-owned proxies
- Prepare pass inputs before recording
- Separate world mutation from render consumption
- Reuse the same scene/proxy boundary for cross-thread updates later

Done when:

- The renderer consumes extracted scene data rather than reaching directly into
  live gameplay state

## Long-Term Roadmap

These are real targets, but not immediate targets.

### Graph Caching and Reuse

Status: later

- Cache repeated graph structures where appropriate
- Reuse compiled structure when inputs are compatible

### Multi-Camera / View Merging

Status: later

- Support multiple cameras/views in one frame
- Investigate graph merging where it is worth the complexity

### Async Compute

Status: later

- Use `RenderNodeCommandListType::Compute`
- Make compute/graphics overlap a first-class scheduling problem
- Add explicit fork/join sync points where needed

### Resource Aliasing / Advanced Allocation

Status: later

- Alias temporary render targets safely
- Reduce transient allocation cost

## Explicit Non-Goals For Now

These are intentionally deferred.

- Full graph merging semantics
- Complex resource aliasing
- Async compute overlap
- Multi-camera caching
- Automatic barrier generation beyond the current simple submit model

## Immediate Next Task

Use the new renderer-owned scene boundary to draw the first real visible
geometry, then parallelize execution by GPU level with `leet_jobs` while
keeping submit order deterministic.

The next architectural task after that is to formalize the render-thread/main-
thread handoff:

- frame input handoff
- renderer-owned sync points
- resize/shutdown rendezvous
- result delivery back to the main thread
