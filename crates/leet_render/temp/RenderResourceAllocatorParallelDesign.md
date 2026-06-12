# RenderResourceAllocator Parallel Shape

Goal: keep `RenderResourceAllocator` renderer-owned and clonable for graph jobs without putting the whole allocator behind one mutex.

## Target Ownership

```rust
#[derive(Clone)]
pub struct RenderResourceAllocator {
    inner: Arc<RenderResourceAllocatorInner>,
}

struct RenderResourceAllocatorInner {
    phase: AtomicAllocatorPhase,
    request_groups: RenderFlowRequestGroups,
    frame_state: Mutex<RenderResourceFrameState>,
    resolution: Mutex<Option<Arc<FrameResourceResolution>>>,
}
```

This means:

- Cloning `RenderResourceAllocator` only clones the `Arc`.
- Node jobs can hold allocator handles.
- Request recording is split by render-flow group.
- Frame resource resolution is published once and then read through an `Arc`.
- The persistent pool and external-resource staging are not part of the per-node hot request path.

## Field Responsibilities

`phase: AtomicAllocatorPhase`

Tracks `Startup`, `PreConsume`, `Resolve`, `Consume`, and `Cleanup`.

The phase must be atomic because node jobs may read it while replaying requests. Transitions are still controlled by the executor.

`request_groups: RenderFlowRequestGroups`

Owns fixed render-flow group request streams.

Each flow group is independently mutable. A node job may mutate only its assigned group. This replaces the old growing `Vec<RequestGroup>` guarded by a single allocator mutex.

`frame_state: Mutex<RenderResourceFrameState>`

Owns renderer-lifetime or frame-wide mutable state that is not mutated by every node request:

```rust
struct RenderResourceFrameState {
    resource_pool: FrameResourcePool,
    external_resources: HashMap<ExternalFrameResourceId, PendingExternalFrameResource>,
    caches_cleared_count: u32,
    process_eviction: bool,
}
```

This mutex is acceptable because it is used during setup/resolve/cleanup style operations, not around every `declare/use/get` call in node execution.

`resolution: Mutex<Option<Arc<FrameResourceResolution>>>`

Publishes the resolved frame resource packet.

The mutex is only for swapping or cloning the `Arc`. Node hot-path reads should work from a cloned `Arc<FrameResourceResolution>`, not hold the mutex while resolving tags.

## Request Groups

Target shape:

```rust
struct RenderFlowRequestGroups {
    groups: Box<[UnsafeCell<RequestGroup>]>,
    active_group_count: AtomicUsize,
}
```

Rules:

- Capacity is fixed at `MAX_RENDER_FLOW_GROUPS`.
- `prepare_preconsume_groups(group_count)` sets the active group count and resets only active groups.
- `record_request(flow_group, request)` can mutate only that group.
- The graph/executor is responsible for assigning nodes to flow groups so two parallel jobs do not write to the same group at the same time.
- Diagnostics can snapshot groups by cloning each active group.

## Phase Flow

`Startup`

No request recording.

`PreConsume`

Parallel node jobs record requests into their assigned request groups.

`Resolve`

Single-owner phase. The allocator solves lifetimes from request groups, plans pool assignments, materializes/imports resources, and publishes `Arc<FrameResourceResolution>`.

`Consume`

Parallel node jobs replay the same requests. Resource lookup reads from the published resolution and the node's flow-group request position.

`Cleanup`

Validates every active request group finished consume replay, clears request streams for the frame, clears external-resource staging, and runs pool cleanup/eviction.

## No Global Current Consume Time

The old allocator had:

```rust
current_consume_time: Option<RequestTime>
```

That cannot survive parallel consume, because many nodes can replay different flow groups at once.

Resource lookup must use the caller's flow group:

```rust
allocator.get_texture(tag, flow_group)
allocator.try_get_texture(tag, flow_group)
allocator.get_buffer(tag, flow_group)
allocator.try_get_buffer(tag, flow_group)
allocator.resolved_allocation_id(tag, flow_group)
```

The request time is derived from that flow group's replay cursor, not from allocator-global state.

## RenderNodeImplContext Contract

`RenderNodeImplContext` should carry the node's `RenderFlowGroup`.

Its resource helpers call the explicit-flow allocator API:

```rust
rctx.get_texture(tag)
```

internally becomes:

```rust
allocator.get_texture(tag, rctx.flow_group())
```

The node does not touch global allocator state directly.

## What Must Not Happen

- Do not wrap the whole allocator in `Arc<Mutex<...>>`.
- Do not keep global `current_consume_time`.
- Do not grow request groups dynamically during node execution.
- Do not hold the resolution mutex while doing resource lookup work.
- Do not use a placeholder facade that only works because graph execution is still empty.





Sanity Check

Not ready to delete/rename yet. The new allocator2.rs has the main allocator behavior copied, but a few integration gaps remain.

What looks correct:

No current_consume_time in allocator state.
No allocator-wide Arc<Mutex<...State>>.
Phase is atomic.
Request streams are per-flow group.
Resolution is published as Arc<FrameResourceResolution>.
Pool/external resources are isolated in RenderResourceFrameState.
Lookup APIs are flow-explicit: get_texture_for_flow, resolved_allocation_id_for_flow, etc.
What still needs fixing before swapping:

diagnostics.rs still calls allocator.current_consume_time(). That must be removed or changed to per-group diagnostics.

RenderNodeImplContext still calls:

allocator.get_texture(tag)
allocator.try_get_texture(tag)
allocator.get_buffer(tag)
allocator.try_get_buffer(tag)
Those need to become calls using self.flow_group.

Tests and tiny graph code still call old no-flow APIs like resolved_allocation_id(tag).

allocator2.rs is not module-wired, so it has not been compiler-checked yet.

The unsafe request-group split needs a hard invariant: no two parallel jobs may write the same RenderFlowGroup at once. We should probably add debug claim/release checks per group before trusting it.

So: the reference shape is right, but swapping now would break diagnostics, node context, and tests. The next sane pass is to update RenderNodeImplContext and diagnostics against the new explicit-flow API, then wire/check
