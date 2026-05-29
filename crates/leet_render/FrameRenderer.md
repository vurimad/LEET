# Frame Renderer Notes

The frame renderer is the bridge between Bevy's app world and LEET's RED-style
render graph runtime. The render graph and frame resource allocator can be
correct on their own, but the renderer still needs a clean data path:

```text
Bevy Main World
  components/assets/cameras/windows
        |
        | extract only changed renderer-relevant data
        v
LEET Render World
  RenderScene / GpuScene / AssetStores / CameraViews
        |
        | prepare/upload only dirty ranges
        v
FrameRenderer
  graph cache + frame graph + allocator + jobs
        |
        | nodes consume prepared render-world data
        v
wgpu
```

## Entry Point

RED's frame entry is viewport-driven:

```text
Viewport
  -> submits one CRenderFrame
      -> CRenderFrameInfo
          -> target viewport
          -> scene
          -> issued camera IDs
              -> scene owns CRenderFrameCameraStorage
                  -> selected camera IDs become SCameraData
                      -> graph setup is selected per camera
                          -> per-camera graphs are merged into one final frame graph
```

LEET's entry is render-app driven instead of viewport-object driven:

```text
Bevy app update
  -> RenderApp extraction
      -> Render schedule
          -> Prepare
              -> surfaces/windows/cameras/render data become render-world state
          -> Render
              -> FrameDispatcher::resolve_frames(&mut render_commands)
          -> Cleanup
              -> frame-local cleanup and presentation bookkeeping
```

THIS MUST BE COMPLETED FIRST FOR THE REDNER GRAPH
True Parallel Execution Plan
We mention the current executor is a shell, but we do not have a concrete pass plan for:

parallel preconsume request recording
per-node/per-worker request buffers
deterministic merge into allocator streams
parallel consume over CPU-ready batches
per-node command recorder ownership
final join before cleanup
FrameExecutionRuntime Ownership
We do not yet document the runtime object that replaces passing raw &mut FrameResourceAllocator / &mut FrameCommandRecorders everywhere.

Materialized Resolve In Frame Execution
The graph docs do not yet say where the real resolve_frame_resources(device) happens in the production path, or how external swapchain/camera resources are registered before resolve.

Command Side Completion
The TODO in RenderNodeImplContext is explained locally, but the plan does not yet define the staged path:

recorder/pass controls now/later
real wgpu encoders
render/compute pass abstraction
bind groups/pipelines
why RED BindPSO maps to LEET pass setup, not a literal API
RenderNodeCommandListUsage Enforcement
We document the enum, but we need a completion pass for actual behavior:

Own creates a command recorder
Require fails without an active recorder
Sync routes through sync runtime
None cannot touch command recording
RED Lifecycle Nodes With Real Side Effects
Current system nodes are fixtures. The docs do not yet map when they become real:

StartRender
EndRender
Present
flush texture/buffer grabs
cleanup batch data
end frame
visibility/query lifecycle if we keep that shape
Render Command Handler Equivalent
We need to document whether LEET has:

a RenderCommandHandler
or a FrameCommandRuntime
or both
And what owns command submission, draw-buffer waits, external kickoff, and present.
External Kickoff Counter
RED’s graph execution waits on frame/draw-buffer readiness before releasing graph jobs. We only have a local shell. This needs a real design section.

Graph Cache Production Key
The design says cache keys must include topology-affecting inputs, but the execution plan does not list the actual categories:

camera count/setup
window/swapchain format
enabled features
debug modes that alter graph topology
async compute policy
render path choices
Graph Diagnostics
RED-level graph dumps are not planned as a pass yet:

nodes
CPU/GPU deps
flow groups
command groups/subnodes
cache hit/miss reasons
resource request streams tied back to nodes
Independent Graph Import With Impl Remap
Current import assumes shared impl store. We need a doc/pass for importing camera graphs with separate impl stores and remapping impl ids plus command-group subnodes.

Stable System/Unique Subtype Registry
We use raw RenderNodeSubtype::new(...). The docs need a registry/constants plan so real recipes do not collide by accident.

Resource Dependency Scope Nodes
RED has explicit camera resource dependency scopes. We have resource use ranges, but not the graph recipe/API plan for scope nodes.

Inter-Command-List Sync Nodes
RED has more than basic synchronize: start-frame sync, signal intermediate sync point, wait intermediate sync point. We have not planned those as implementation passes.

So the LEET equivalent of RED's `EngineViewport::SubmitFrame` is not a method
on a viewport object. It should be a render-world system in
`RenderSystems::Render` that collects the already-extracted render-world
state and dispatches it through the render command handler.

The frame entry object should be explicit:

```text
FrameInput
  frame target
  camera views for that target
  extracted window/swapchain state
  prepared render scene data
  frame settings and render intent
  presentation intent
```

`FrameInput` is the LEET-side equivalent of the parts of
`CRenderFrameInfo` that the frame renderer actually needs. It must not borrow
from Bevy's main world. It is assembled from render-world resources such as
`ExtractedWindows`, `ExtractedViews`, sorted cameras, `GpuScene`,
and future prepared mesh/material stores.

The RED `Viewport -> SubmitFrame` role should become a LEET frame-target
submission mechanism:

```text
FrameDispatcher system
  -> groups SortedCameras by render target
  -> builds one FrameInput per target
  -> calls RenderCommandHandler::render_scene(frame_input)
      -> schedules the frame on the render command path
      -> creates RenderFrameContext for the renderer job
```

This is the missing bridge between Bevy's extracted camera data and the render
graph. A LEET frame target is not the same thing as Bevy's `Viewport`.
`Viewport` is a camera rectangle. A frame target is the destination that receives
one frame: a window swapchain texture, an image target, or a manual
external texture view.

Per-view submission should be represented inside the frame input, not by calling
the renderer once per camera:

```text
FrameTarget
  target id
  target size and format
  output/present mode
  capture/screenshot intent

FrameCameraView
  camera entity
  camera id
  camera order and per-target index
  camera render setup key
  viewport rectangle
  clear/output settings
  extracted camera data
  extracted view transforms
```

Then `RenderCommandHandler::render_scene` can behave like RED's render scene
command: it receives a target frame packet, flushes pending render-side command
queues, schedules the frame render job, builds a `RenderFrameContext`, and calls
the internal frame renderer.

The final dispatcher should look like a render-world system that builds target
frames from prepared render-world resources:

```rust
pub fn dispatch_frames(
    mut render_commands: ResMut<RenderCommandHandler>,
    sorted_cameras: Res<SortedCameras>,
    extracted_cameras: Res<ExtractedCameras>,
    extracted_views: Res<ExtractedViews>,
    windows: Res<ExtractedWindows>,
    manual_texture_views: Res<ManualTextureViews>,
    frame_settings: Res<FrameRenderSettings>,
    capture_requests: Res<FrameCaptureRequests>,
) -> RenderFrameResult<()> {
    let dispatcher = FrameDispatcher::construct(
        &sorted_cameras,
        &extracted_cameras,
        &extracted_views,
        &windows,
        &manual_texture_views,
        &frame_settings,
        &capture_requests,
    )?;

    dispatcher.resolve_frames(&mut render_commands)
}
```

The concrete Bevy system may need a different parameter shape to satisfy
borrowing, but the ownership should stay the same:

```text
FrameDispatcher::construct
  -> captures borrowed render-world inputs for this submit phase
  -> validates that the required frame-side resources exist
  -> stores no long-lived renderer state

FrameDispatcher::resolve_frames
  -> read SortedCameras in already-sorted order
  -> skip camera entries with missing ExtractedCamera or ExtractedView
  -> resolve each camera target into a stable FrameTargetKey
  -> detect target changes while walking the sorted camera list
  -> finish the previous target packet before starting the next target
  -> create FrameTarget for each target
  -> attach all valid FrameCameraView entries for that target
  -> attach frame timing, capture, debug, and presentation intent
  -> submit each finished FrameInput to RenderCommandHandler::render_scene
  -> skip targets that have no acquired output texture or no valid views
```

`FrameDispatcher` should be mostly stateless. It is a packet builder and policy
boundary. It should not own graph caches, GPU resources, or long-lived scene
state. Those belong behind `RenderCommandHandler`, inside `FrameRenderer`,
`RenderGraphCache`, `GpuScene`, and the resource allocator/runtime.

The dispatcher object exists to make the frame-submission phase explicit:

```text
let dispatcher = FrameDispatcher::construct(render_world_inputs)?;
dispatcher.resolve_frames(&mut render_commands)?;
```

That mirrors RED's conceptual split:

```text
BeginFrame / SubmitFrame assemble and submit the render frame packet.
RenderCommandHandler::render_scene consumes the assembled frame packet.
```

Expanded `resolve_frames` shape:

```rust
impl<'w> FrameDispatcher<'w> {
    pub fn resolve_frames(
        self,
        render_commands: &mut RenderCommandHandler,
    ) -> RenderFrameResult<()> {
        for frame_input in self.build_frame_inputs()? {
            render_commands.render_scene(frame_input)?;
        }

        Ok(())
    }
}
```

`build_frame_inputs` is where target grouping lives. `resolve_frames` should stay
boring on purpose: once a `FrameInput` exists, it is handed to the render command
path exactly like RED hands a `CRenderFrame` to `CRenderCommandHandler`.

### Render Command Handler Boundary

RED's important command-handler shape is:

```text
CRenderCommandHandler::RenderScene(frame)
  -> create render-path job builder
  -> wait for previous render-scene flush counter
  -> flush queued render commands
  -> dispatch RenderScene/FlushRenderScene job
      -> SRenderFrameContext ctx(runContext, frame)
      -> GetRenderer()->RenderFrame(ctx)
  -> store the new flush counter
```

LEET should mirror that boundary:

```text
RenderCommandHandler::render_scene(frame_input)
  -> create render-path job builder
  -> wait for previous frame flush counter
  -> flush queued render commands
  -> dispatch RenderScene/RenderFrame job
      -> RenderFrameContext { builder, frame_input, dispatcher_thread_index }
      -> FrameRenderer::render_frame(ctx)
  -> store the new flush counter
```

Planned LEET shape:

```rust
impl RenderCommandHandler {
    pub fn render_scene(&mut self, frame_input: FrameInput) -> RenderFrameResult<()> {
        let mut builder = self.jobs.create_builder(Priority::RenderPath);

        builder.dispatch_wait(&self.frame_flush_counter);

        self.dispatch_queued_render_commands(&mut builder)?;
        self.dispatch_pending_draw_buffer_flushes(&frame_input, &mut builder)?;

        let renderer = self.renderer.clone_for_job();

        builder.dispatch_job("RenderScene/RenderFrame", move |run_context| {
            let ctx = RenderFrameContext::from_run_context(run_context, frame_input);

            renderer.render_frame(ctx)
        });

        self.frame_flush_counter = builder.extract_wait_counter();

        Ok(())
    }
}
```

This uses the active `leet_jobs2::Builder::dispatch_job` API. Its closure
receives `&RunContext`, and `RenderFrameContext` immediately converts that into
a continuation builder, mirroring RED's `SRenderFrameContext`.

The LEET equivalent of RED's `SRenderFrameContext` should stay small:

```rust
pub struct RenderFrameContext {
    pub builder: RenderJobBuilder,
    pub frame_input: FrameInput,
    pub dispatcher_thread_index: u32,
}

impl RenderFrameContext {
    pub fn from_run_context(run_context: &RunContext, frame_input: FrameInput) -> Self {
        Self {
            builder: run_context.create_builder(),
            frame_input,
            dispatcher_thread_index: run_context.thread_index,
        }
    }
}
```

`RenderCommandHandler` must not build graph inputs, cache graph topology, merge
view graphs, allocate graph resources, or know graph node details. Its job is the
command queue and job boundary. `FrameRenderer::render_frame` owns the renderer
internals behind that boundary.

Initial `RenderCommandHandler` API should be small:

```rust
impl RenderCommandHandler {
    pub fn render_scene(&mut self, frame_input: FrameInput) -> RenderFrameResult<()>;
    pub fn flush_previous_frame_commands_processing(&mut self);
    pub fn sync_with_render_commands(&mut self) -> RenderFrameResult<()>;

    fn commit_command(&mut self, command: RenderCommand);
    fn dispatch_queued_render_commands(
        &mut self,
        builder: &mut RenderJobBuilder,
    ) -> RenderFrameResult<()>;
    fn dispatch_pending_draw_buffer_flushes(
        &mut self,
        frame_input: &FrameInput,
        builder: &mut RenderJobBuilder,
    ) -> RenderFrameResult<()>;
}
```

RED has many public commands on this object, but LEET should not add them all
up front. For the first real pass:

- Add now: `render_scene`, previous-frame flush/sync, internal command commit,
  in-order queue flush, and the frame-render job dispatch.
- Add soon: draw-buffer/debug-buffer submission once LEET has a real draw-buffer
  packet.
- Delay: `render_sub_scene`, screenshots, gameplay/post effects, reflection
  probes, proxy mutation commands, and camera register/update commands.

Camera and scene updates should come from Bevy extraction and prepared
render-world stores first. Do not add RED-style `RegisterCamera` or
`UpdateCameraData` commands until there is a real reason to mutate render-side
camera storage outside extraction.

RED command-handler categories:

```text
Frame execution
  RenderScene, FrameTick, RetirePendingResources
  -> LEET keeps render_scene now; frame tick/resource retirement can stay as
     explicit render-schedule systems until the renderer needs command ordering.

Synchronization
  FlushPreviousFrameCommandsProcessing, SyncWithRenderCommands
  -> LEET should add these with job counters.

Deferred command queue
  CommitCommand, in-order queue, per-proxy parallel queues
  -> LEET should add the queue shape, but start with in-order commands first.
     Per-proxy parallel queues should wait until render proxies are command
     mutated outside extraction.

Subscene and draw-buffer queues
  RenderSubScene, SubmitDrawBuffer
  -> Delay until the debug/draw-buffer packet exists.

Camera commands
  RegisterCamera, UnregisterCamera, UpdateCameraData, camera dependencies
  -> Delay. LEET's first camera path is extracted camera/view data.

Scene/proxy mutation commands
  AddProxyToScene, RemoveProxyFromScene, MoveProxyInSceneVisStructure, etc.
  -> Delay. LEET's first scene path is delta extraction into GpuScene.

Capture and presentation helpers
  TakeScreenshot, ToggleContinuousScreenshot, ResizeRenderSurfaces
  -> Delay. Capture intent lives in FrameInput first.

Gameplay/editor/post-effect commands
  ScreenFade, forced LOD, material overrides, effect contexts, editor preview
  -> Not part of the first renderer core pass.
```

RED's `IRenderCommand` also carries a debug name, an optional proxy pointer, and
a parallel-queue safety class. The LEET equivalent should model the same idea,
but with renderer-owned stable ids instead of raw pointers:

```rust
pub enum RenderCommandQueueKind {
    InOrder,
    ProxyParallel {
        proxy: RenderProxyId,
        safety: RenderCommandSafety,
    },
}

pub enum RenderCommandSafety {
    NotSafeWithProxyAddRemove,
    SafeWithProxyAddRemove,
}
```

For the first pass, it is acceptable for all commands to use `InOrder`. The enum
should still exist so the later per-proxy parallel queue is an extension of the
same design, not a rewrite.

### Render Command Handler Header Pass

RED's `CRenderCommandHandler` header has two layers:

```text
public interface
  frame execution
  frame/resource tick
  camera commands
  scene/proxy commands
  capture commands
  debug/draw-buffer commands
  sync helpers

private machinery
  command queues
  command queue page/free-list storage
  job counters
  queue flush helpers
  commit/consume locks
```

LEET should not mirror every public RED command yet. It should mirror the
machinery that makes frame submission ordered, job-backed, and expandable.

Planned first LEET shape:

```rust
pub struct RenderCommandHandler {
    jobs: LeetJobSystem,
    renderer: FrameRenderer,

    frame_flush_counter: Counter,

    command_queues: RenderCommandQueues,
    command_queue_lock: RenderCommandQueueLock,
}
```

Member responsibilities:

```text
jobs
  Active leet_jobs2 system used to create RenderPath builders and flush counters.

renderer
  Owns the real frame renderer behind the command-handler boundary.

frame_flush_counter
  LEET equivalent of RED's m_flushCounter. Every render_scene waits on it, then
  replaces it with the counter extracted from the newly scheduled work.

command_queues
  Deferred render-side commands waiting to be consumed before the next frame
  render job. First version can contain only an in-order queue.

command_queue_lock
  Protects commit/consume so command enqueue and frame flush do not overlap
  incorrectly. RED uses m_commmitConsumeRWLock for this role.
```

First-pass queue shape:

```rust
pub struct RenderCommandQueues {
    in_order: VecDeque<RenderCommand>,
}

pub struct RenderCommand {
    debug_name: &'static str,
    queue_kind: RenderCommandQueueKind,
    execute: RenderCommandFn,
}
```

This mirrors RED's `IRenderCommand` contract:

```text
IRenderCommand
  debug name
  optional proxy pointer
  parallel queue safety
  Execute()
```

LEET should start with all commands routed through `RenderCommandQueueKind::InOrder`.
The proxy-parallel fields can be added without changing the public command shape.

Fields to add later, not now:

```rust
proxy_removal_flush_counter: Counter,
draw_buffers_wait_counter: Counter,
proxy_parallel_queues: [RenderCommandQueue; PARALLEL_QUEUE_KIND_COUNT * PARALLEL_BUCKET_COUNT],
subscene_queue: VecDeque<FrameInput>,
draw_buffer_queue: VecDeque<DrawBufferPacket>,
draw_buffer_command_lists: SmallVec<[CommandBuffer; MAX_DRAW_BUFFERS_IN_BATCH]>,
```

Why they are delayed:

```text
proxy_removal_flush_counter
  RED uses this to keep proxy release jobs from overlapping badly with later
  proxy operations. LEET should wait until proxy mutation commands exist.

draw_buffers_wait_counter / draw_buffer_queue / draw_buffer_command_lists
  RED has explicit draw-buffer submission. LEET does not have that packet yet.

proxy_parallel_queues
  RED buckets commands by proxy pointer and safety class. LEET should use stable
  RenderProxyId buckets later, but extraction currently owns scene/proxy updates.

subscene_queue
  RED can enqueue subscene frames. LEET should delay this until there is a clear
  use case for nested frame submission.
```

RED fields LEET should not copy literally:

```text
QueuePage / free page lists
  RED uses fixed byte pages and free lists for custom allocation. LEET can begin
  with Rust containers and move to arenas only when profiling requires it.

raw proxy pointer bucketing
  LEET should use RenderProxyId, never object addresses.

MaxDrawBuffersInBatch and fixed GPU command-list array
  Delay until draw-buffer submission exists.
```

Planned first-pass functions:

```rust
impl RenderCommandHandler {
    pub fn new(jobs: LeetJobSystem, renderer: FrameRenderer) -> Self;

    pub fn render_scene(&mut self, frame_input: FrameInput) -> RenderFrameResult<()>;
    pub fn flush_previous_frame_commands_processing(&self);
    pub fn sync_with_render_commands(&mut self) -> RenderFrameResult<()>;

    pub fn run_synced_with_commands<F>(
        &mut self,
        name: &'static str,
        function: F,
    ) -> RenderFrameResult<()>
    where
        F: FnOnce(&RunContext) + Send + 'static;

    fn commit_command(&mut self, command: RenderCommand);
    fn dispatch_queued_render_commands(
        &mut self,
        builder: &mut RenderJobBuilder,
    ) -> RenderFrameResult<()>;
    fn flush_in_order_commands(commands: Vec<RenderCommand>);
}
```

Function responsibilities:

```text
new
  Creates empty queues and zero/complete counters.

render_scene
  Mirrors RED RenderScene: wait previous frame, flush deferred commands,
  dispatch FrameRenderer::render_frame as a RenderPath job, store the new
  frame_flush_counter.

flush_previous_frame_commands_processing
  Mirrors RED FlushPreviousFrameCommandsProcessing. The app/frame boundary can
  call this when it must ensure render commands from the previous frame are done.

sync_with_render_commands
  Mirrors RED SyncWithRenderCommands. It waits previous work, flushes queued
  commands, updates frame_flush_counter, then waits for that counter.

run_synced_with_commands
  Mirrors RED RunSyncedWithCommands. It schedules one named job after the current
  render command chain and stores the resulting counter.

commit_command
  Enqueues a deferred render-side command. In pass one it always enters the
  in-order queue.

dispatch_queued_render_commands
  Acquires queued commands and schedules their execution before the frame render
  job.

flush_in_order_commands
  Executes all acquired in-order commands in submit order.
```

Public RED methods not planned for the first LEET command handler:

```text
FrameTick / RetirePendingResources
  Keep as explicit render-schedule systems until ordering requires command
  handler ownership.

RenderSubScene
  Delay until nested/subscene frame submission is designed.

RegisterCamera / UpdateCameraData / camera dependencies
  Delay. Bevy extraction should fill frame camera/view data first.

AddProxyToScene / RemoveProxyFromScene / proxy mutation
  Delay. Extraction into GpuScene is the first scene update path.

TakeScreenshot / ToggleContinuousScreenshot / ResizeRenderSurfaces
  Delay. Capture and resize intent should live in FrameInput first.

Gameplay, editor, post-effect, material override, reflection probe commands
  Not renderer-core command-handler work for this pass.
```

### Render Command Handler Implementation Pass

RED's `.cpp` behavior has a few important rules that LEET should preserve:

```text
1. Enqueue commands without running them.
2. At render_scene/sync time, atomically acquire the queued work.
3. Schedule queue flushing as jobs in the same RenderPath builder.
4. Schedule the actual frame render job after those flush jobs.
5. Store the extracted builder counter as the next frame_flush_counter.
```

This means LEET should not flush command queues inline on the caller thread. The
flush is itself render-path work.

RED queue flow:

```text
CommitCommand
  -> if command has proxy pointer:
       push into per-proxy parallel bucket
     else:
       push into in-order queue

DispatchQueueFlushJobs(builder)
  -> acquire in-order queue pages
  -> builder.DispatchJob("RenderScene/FlushInOrder", ...)
  -> gather pending proxy add/remove operations
  -> dispatch pending proxy scene updates
  -> dispatch safe proxy-parallel queues
  -> fence
  -> dispatch non-safe proxy-parallel queues
```

LEET first pass:

```text
commit_command
  -> push RenderCommand into command_queues.in_order

dispatch_queued_render_commands(builder)
  -> acquire and replace command_queues.in_order
  -> if nonempty:
       builder.dispatch_job("RenderScene/FlushInOrder", move |ctx| {
           RenderCommandHandler::flush_in_order_commands(commands, ctx)
       })
```

The acquire step matters. It lets new commands be committed while the acquired
batch is scheduled for flushing, without mutating the batch that is already in
flight.

First-pass implementation sketch:

```rust
fn commit_command(&mut self, command: RenderCommand) {
    let mut queues = self.command_queue_lock.write(&mut self.command_queues);
    queues.in_order.push_back(command);
}

fn dispatch_queued_render_commands(
    &mut self,
    builder: &mut RenderJobBuilder,
) -> RenderFrameResult<()> {
    let commands = {
        let mut queues = self.command_queue_lock.write(&mut self.command_queues);
        queues.take_in_order()
    };

    if !commands.is_empty() {
        builder.dispatch_job("RenderScene/FlushInOrder", move |run_context| {
            Self::flush_in_order_commands(commands, run_context);
        });
    }

    Ok(())
}

fn flush_in_order_commands(commands: Vec<RenderCommand>, run_context: &RunContext) {
    for command in commands {
        command.execute(run_context);
    }
}
```

The concrete lock API can be `Mutex`, `RwLock`, or a small render-specific
wrapper. The important contract is the RED one:

```text
commit and consume cannot race on the same queue storage
```

`render_scene` implementation order:

```rust
pub fn render_scene(&mut self, frame_input: FrameInput) -> RenderFrameResult<()> {
    let mut builder = self.jobs.create_builder(Priority::RenderPath);

    builder.dispatch_wait(&self.frame_flush_counter);

    self.dispatch_queued_render_commands(&mut builder)?;

    let renderer = self.renderer.clone_for_job();
    builder.dispatch_job("RenderScene/RenderFrame", move |run_context| {
        let ctx = RenderFrameContext::from_run_context(run_context, frame_input);

        renderer.render_frame(ctx)
    });

    self.frame_flush_counter = builder.extract_wait_counter();
    Ok(())
}
```

Differences from RED in the first pass:

```text
No DispatchSubsceneRenderingFlushJobs
  LEET does not have subscene frame submission yet.

No DispatchDrawBuffersFlushJobs
  LEET does not have draw-buffer packets yet.

No pending proxy add/remove job
  LEET scene mutation currently comes through extraction into GpuScene.

No parallel queue dispatch
  LEET command mutation is in-order until proxy command mutation exists.

No resource eviction branch
  Add later when renderer resource lifetime/eviction policy exists.
```

`sync_with_render_commands` should mirror RED's dependency shape:

```text
sync_with_render_commands
  -> create RenderPath builder
  -> wait frame_flush_counter
  -> dispatch queued command flush jobs
  -> extract/store frame_flush_counter
  -> flush/wait the extracted counter before returning
```

`run_synced_with_commands` should mirror RED's helper:

```text
run_synced_with_commands(name, f)
  -> wait frame_flush_counter
  -> dispatch named job f
  -> store extracted counter
```

`.cpp` behaviors to delay:

```text
PushIRenderCommandToQueue byte-page storage
  Delay. Rust command storage can start as boxed closures or enum commands.

FlushParallelCommands
  Delay until RenderCommandQueueKind::ProxyParallel is used.

FlushSubsceneCommands
  Delay until nested/subscene frame submission is designed.

FlushDrawBufferCommands
  Delay until DrawBufferPacket and command-list batching exist.

Pending proxy add/remove scene update job
  Delay. The first LEET renderer path should use extraction and prepared
  render-world scene data.
```

### Frame Payload Pass

RED's frame payload has four relevant types:

```text
IRenderFrame
  virtual wrapper whose only renderer-facing API is GetFrameInfo().

CRenderFrame
  concrete engine-side owner of CRenderFrameInfo.
  Constructor moves CRenderFrameInfo and calls FinishConfigSetup().

CRenderFrameInfo
  large submitted-frame packet:
    target size and viewport/window context
    rendering mode and frame purpose
    present/capture intent
    scene pointers
    issued camera ids
    timing and environment settings
    debug/capture/readback outputs
    dynamic texture targets

SRenderFrameContext
  render-job context:
    continuation job builder created from RunContext
    frame pointer
    dispatcher thread index
```

RED shape:

```cpp
struct SRenderFrameContext
{
    SRenderFrameContext(const job::RunContext& runCtx,
                        const TRenderPtr<IRenderFrame>& frame);

    job::Builder             m_builder;
    TRenderPtr<IRenderFrame> m_frame;
    Uint32                   m_dispatcherThreadIndex;
};
```

Constructor behavior:

```text
SRenderFrameContext(runCtx, frame)
  -> m_dispatcherThreadIndex = runCtx.dispatcherThreadIndex
  -> m_builder = job::Builder(runCtx)
  -> m_frame = frame
```

LEET equivalent:

```rust
pub struct RenderFrameContext {
    pub builder: RenderJobBuilder,
    pub frame_input: FrameInput,
    pub dispatcher_thread_index: u32,
}
```

This means `FrameRenderer::render_frame` should use `ctx.builder` for all child
work it schedules during the frame. It should not create an unrelated
RenderPath builder, because RED extends the lifetime of the parent render-frame
job through the continuation builder.

Frame input should replace both RED `CRenderFrame` and the useful subset of
`CRenderFrameInfo`:

```rust
pub struct FrameInput {
    pub target: FrameTarget,
    pub camera_views: Vec<FrameCameraView>,
    pub scene: RenderSceneId,
    pub timing: FrameTiming,
    pub mode: FrameRenderingMode,
    pub purpose: FramePurpose,
    pub presentation: PresentationIntent,
    pub capture: FrameCaptureIntent,
    pub debug: FrameDebugIntent,
}
```

```rust
pub struct FrameCameraView {
    pub camera_entity: Entity,
    pub camera_id: RenderCameraId,
    pub camera_order: isize,
    pub target_view_index: u32,
    pub viewport: ViewportRect,
    pub clear: ViewClearState,
    pub camera: ExtractedCameraData,
    pub view: ExtractedViewData,
    pub render_setup: CameraRenderSetupKey,
}
```

First-pass `FrameInput` fields:

```text
target
  Replaces RED viewport/window/dynamic texture target fields. In LEET this is a
  window surface, image target, or manual texture view.

camera_views
  Replaces the submitted camera side of RED m_cameras. These are requested
  frame camera views, not fully prepared render cameras. FrameRenderer still
  needs a camera-storage preparation step before graph selection/execution.

scene
  Replaces RED scene pointer. This should identify prepared render-world scene
  data, not borrow Bevy main-world state.

timing
  Replaces RED frame time, engine time, game time, delta time, simulation time,
  and previous time fields.

mode
  Replaces RED ERenderingMode. Start with normal/shaded plus blank/no-scene or
  debug mode only if needed by the first renderer.

purpose
  Replaces RED EFramePurpose. Start with normal and capture/debug placeholders.

presentation
  Replaces RED m_present and target-present policy.

capture
  Replaces RED CRenderFrameGrab output/capture intent. First pass can be empty
  or contain only color capture intent.

debug
  Replaces non-camera debug drawer and debug preview flags. First pass can be a
  lightweight placeholder.
```

RED frame fields to delay:

```text
environment/weather/lighting settings
  Add when the renderer has real environment systems feeding prepared data.

selection and multilayer capture outputs
  Add when selection/readback rendering exists.

GBuffer/depth/background capture outputs
  Add with screenshot/capture pipeline.

dynamic texture target variants
  Add when render-to-texture cameras/manual targets are implemented.

IMGUI/debug heatmap/debug preview payloads
  Add when those systems exist as render-world prepared packets.

originalScene/nonInteractiveScene
  RED needs these for scene-preview and non-interactive update quirks. LEET
  should not copy them unless the renderer gets the same requirement.
```

`CRenderFrameInfo::FinishConfigSetup` currently clamps and validates RED-specific
shadow config. LEET should keep the concept but not the content:

```text
FrameInputBuilder::finish
  -> validate target size is nonzero
  -> validate target has at least one camera view unless blank/no-scene frame
  -> normalize capture/presentation intent
  -> freeze the frame packet before RenderCommandHandler sees it
```

Frame-render entry should therefore read as:

```text
FrameDispatcher
  -> FrameInputBuilder::finish()
  -> RenderCommandHandler::render_scene(frame_input)
  -> RenderFrameContext::from_run_context(run_context, frame_input)
  -> FrameRenderer::render_frame(ctx)
```

Inside `FrameRenderer::render_frame`, the RED mapping is:

```text
ctx.frame_input
  -> RED renderFrameContext.m_frame->GetFrameInfo()

ctx.builder
  -> RED renderFrameContext.m_builder

ctx.dispatcher_thread_index
  -> RED renderFrameContext.m_dispatcherThreadIndex
```

The dispatcher thread index must flow into `RenderNodeImplContext` init data just
like RED passes it into `SRenderNodeImplContext::SInitData`.

### Render Camera Storage Pass

RED's `CRenderFrameCameraStorage` is more than a camera map. It is the
persistent render-side camera registry plus the per-frame camera preparation
step.

RED stores persistent camera records:

```text
SCameraRegistry
  camera hash
  source CRenderCamera
  camera viewport and sort priority
  permanent/temporal lifetime policy
  on-demand/always-render policy
  dependencies on other cameras
  render-flow space
  optional camera builder for generated cameras
```

Then each frame, `CRenderFrameCameraStorage::AllocateCameraData` turns the
requested camera ids from `CRenderFrameInfo::m_cameras` into the actual cameras
that will render this frame:

```text
AllocateCameraData(ctx, requested_cameras)
  -> flush dynamic camera/dependency requests
  -> clear last frame's selected camera list
  -> remove stale temporal cameras and temporal dependencies
  -> resolve which dependency cameras are close enough to requested cameras
  -> prepare requested and always-render cameras
  -> add selected dependency cameras
  -> allocate/reset per-camera collector runtime data
  -> sort dependency cameras before cameras that depend on them
  -> build generated cameras such as mirror cameras
  -> advance the camera storage tick
```

Concrete examples:

```text
Normal frame
  requested: MainCamera
  selected:  MainCamera

Mirror frame
  requested: MainCamera
  dependency: MainCamera -> MirrorCamera
  selected:  MirrorCamera, then MainCamera

Portal/temporary camera
  frame N:   MainCamera temporarily depends on PortalCamera
  frame N+1: dependency is kept only if requested again
  later:     stale temporary dependency/camera is removed
```

Dependency depth is RED's recursion limiter for camera dependencies:

```text
MainCamera depth 0
MirrorCamera depth 1
MirrorInsideMirrorCamera depth 2
```

RED uses `MAX_DEPENDENCY_DEPTH = 1`, so direct camera dependencies can render,
but runaway recursive cameras are rejected from the selected frame camera list.

`PrepareCamera` fills the per-frame render-camera state:

```text
camera cut detection
last-frame camera data for TAA/motion vectors
temporal jitter
final viewport
feature flags
rendering-plane cameras
dynamic/internal resolution state
camera collector pointers
```

The collector runtime data is not a GPU resource allocation. It is the
per-selected-camera bucket used later to collect visible scene/render data for
that camera. If this frame selects two cameras, storage ensures two collector
slots exist and resets both.

LEET should mirror this as a real renderer layer, not by bloating
`FrameInput`:

```text
FrameInput
  requested frame target
  requested camera views
  scene/timing/mode/capture/debug intent

RenderCameraStorage
  persistent render-side camera records
  temporal camera/dependency lifetime
  camera dependency sorting
  per-frame prepared camera output

PreparedFrameCamera
  actual render camera data
  viewport and resolution state
  previous-frame camera data
  temporal reset/jitter state
  dependency/output flags
  collector slot
```

This changes the `FrameRenderer::render_frame` order:

```text
FrameRenderer::render_frame(ctx)
  -> read ctx.frame_input requested camera views
  -> register/update those views in RenderCameraStorage
  -> prepare selected cameras for this frame
  -> use prepared cameras for graph setup/cache keys
  -> build/merge/execute the frame graph
```

First LEET pass can keep the feature smaller:

```text
Add now
  RenderCameraStorage concept
  requested camera views vs prepared cameras distinction
  stable camera ids
  selected camera ordering
  previous-frame camera data/reset flag

Delay
  generated mirror/portal cameras
  temporary dynamic camera requests
  per-camera custom data
  DRS/internal resolution policy
  collector implementation details until scene collection exists
```

### FrameRenderer Render Frame Walkthrough

This section tracks RED's `CRenderInterface::RenderFrame(SRenderFrameContext&)`
in small slices and maps each slice to the LEET `FrameRenderer::render_frame`
shape.

#### Part 1: Enter Frame Render

RED:

```cpp
ScopedProfilerChannel forcedChannels( PBC_RENDER );

const auto& frame = renderFrameContext.m_frame;
```

Meaning:

```text
enter the render profiler channel
pull the frame packet out of SRenderFrameContext
```

LEET:

```rust
pub fn render_frame(&mut self, mut ctx: RenderFrameContext) -> RenderFrameResult<()> {
    let frame = &ctx.frame_input;
}
```

The profiler channel does not need a dedicated LEET type yet. It can become a
`tracing` span or renderer profiling scope later. The important mapping is:

```text
RED renderFrameContext.m_frame->GetFrameInfo()
LEET ctx.frame_input
```

#### Part 2: Debug Guard And Profiler Scope

RED:

```cpp
#ifdef RED_ASSERTS_ENABLED
    ScopeAtomicRelease scopedAtomic( &GDebugRenderFrame );
#endif

PC_SCOPE( CRenderInterface_RenderFrame );
```

Meaning:

```text
mark that a render frame is active in debug builds
open a profiler scope for RenderFrame
```

LEET first pass:

```rust
pub fn render_frame(&mut self, mut ctx: RenderFrameContext) -> RenderFrameResult<()> {
    let _span = tracing::trace_span!("FrameRenderer::render_frame").entered();

    let frame = &ctx.frame_input;
}
```

Do not add a debug-only active-frame guard yet unless an actual invariant needs
it. This slice is diagnostics only.

### RED Engine Viewport Mapping

The RED entry point worth mirroring is `EngineViewport::BeginFrame` plus
`EngineViewport::SubmitFrame`, but only as a frame-submission contract. LEET
should not copy RED's viewport/window ownership because Bevy already owns
windows, surfaces, input focus, and resize events.

RED's start-frame shape is:

```text
EngineViewport::BeginFrame
  -> choose rendering mode
  -> create CRenderFrameSetup from viewport
  -> create CRenderCameraFrameInfo
  -> create CRenderFrameInfo
  -> attach last-frame timing/debug state

EngineViewport::SubmitFrame
  -> enqueue debug/overlay work
  -> apply frame pacing
  -> apply capture/screenshot requests
  -> adjust viewport size for capture
  -> add fallback non-camera debug drawer
  -> wrap CRenderFrameInfo into CRenderFrame
  -> call RenderCommandHandler::RenderScene(frame)
```

LEET should split those responsibilities like this:

```text
FrameDispatcher system
  -> groups extracted cameras by target
  -> builds FrameTarget
  -> builds FrameCameraView entries
  -> attaches frame timing/capture/debug intent
  -> calls RenderCommandHandler::render_scene(frame_input)

RenderCommandHandler::render_scene
  -> waits and flushes queued render commands
  -> dispatches the render-frame job
  -> creates RenderFrameContext

FrameRenderer::render_frame
  -> receives RenderFrameContext
  -> updates/imports per-view camera data
  -> builds or fetches graph recipes
  -> merges per-view graph slices into one target graph
  -> resolves frame resources
  -> executes graph work
  -> submits command buffers
  -> presents or resolves the target
```

The core renderer should own only the data that affects rendering. The following
RED viewport responsibilities become external LEET services:

- window creation, focus, and foreground handling: Bevy/window backend
- frame pacing/FPS clamp: app runner or render schedule policy
- debug overlay and UI extraction: debug/overlay systems before submission
- screenshot mode decisions: capture service feeding `FrameInput`
- custom capture resolution: frame target override, not window ownership

The important invariant is:

```text
LEET starts rendering from an already-extracted frame target, not from
the Bevy main world and not from a renderer-owned window viewport.
```

The production call shape should stay narrow:

```text
FrameRenderer::render_frame(render_frame_context)
```

Where:

- `render_frame_context.builder` is the continuation builder for child frame jobs.
- `render_frame_context.frame_input` describes the frame packet.
- `render_frame_context.dispatcher_thread_index` is carried into node contexts.

Graph cache keys, resource allocation, command recording, and node execution are
derived inside `FrameRenderer`. They should not leak into `FrameDispatcher` or
`RenderCommandHandler`.

This keeps RED's important separation:

```text
Frame entry chooses what to render.
Graph recipes choose which nodes exist.
Graph execution runs the selected graph.
Render nodes consume prepared render-world data.
```

Do not copy RED's global `CRenderNodeJob::SetJobsRenderFrame` pattern. LEET
should pass frame runtime state explicitly through `FrameExecutionRuntime` and
`RenderNodeImplContext`.

Important boundary:

```text
render graph nodes should not query Bevy app-world data
```

Nodes should consume LEET render-world resources that were already extracted and
prepared before graph execution.

Bevy owns the source-side authoring data:

- ECS components
- `Mesh`, `Image`, material assets as source data
- asset events and component change detection
- camera/window user-facing components

LEET owns the draw-facing renderer contracts:

- render proxy IDs and stable slots
- GPU scene tables
- mesh/material prepared metadata
- phase lists
- indirect draw argument buffers
- frame resource allocator
- render graph
- frame renderer orchestration
- pipeline and bind layout policy

Use Bevy abstractions where they save time before the hot renderer boundary:
reading asset data, camera/window components, transforms, extraction schedules,
and maybe shader asset loading. Once data becomes draw-facing, LEET owns the
layout and lifetime.

The extraction bridge should be delta-based:

```text
Entity -> RenderProxyId map

Added renderable component
  -> allocate proxy

Changed transform / visibility / mesh handle / material handle
  -> update only that proxy slot

Removed renderable component
  -> remove proxy / clear slot

AssetEvent<Mesh>
  -> update prepared mesh asset store

AssetEvent<Material/Image/Shader>
  -> update corresponding prepared store
```

Current `GpuScene` already points in this direction: stable proxy slots,
generation IDs, current/previous input tables, and sparse dirty-page uploads.
The missing layer is the real Bevy bridge that fills it from changed queries,
removed components, and asset events.

Future frame renderer shape:

```text
FrameRenderer
  owns frame orchestration

RenderGraphCache
  caches graph topology

FrameCommandLists / command recorder
  owns command encoders/buffers/submission order

GpuScene + prepared asset stores
  own render data

Batcher / GPU preprocessing
  builds phase lists and indirect args
```

The guiding rule:

```text
Use Bevy as the source and extraction framework, but do not let Bevy's renderer
architecture dictate LEET's draw-facing contracts.
```


TO BE CONTINUED 
