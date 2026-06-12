//! Render node implementation context.
//!
//! This is the node-facing layer above `RenderResourceAllocator`. It owns the
//! context-sensitive choices around which flow group requests are recorded into,
//! which flow space named resources belong to, and whether the node is
//! unique/global or camera/view scoped.
//!
//! Resource methods on this type do not allocate GPU memory directly. They record
//! the same allocator request stream during pre-consume and replay the same
//! stream during consume.

use std::sync::Arc;

use super::super::resources::{
    ExternalFrameResourceId, FrameBufferDesc, FrameBufferResource, FrameResourceDesc,
    FrameResourceResult, FrameTextureDesc, FrameTextureResource, ImportedFrameResource,
    QueueSyncKind, RenderFlowGroup, RenderFlowName, RenderFlowNameTag, RenderFlowSpace,
    RenderQueueKind, RenderResourceAllocator, ResourceAllocatorPhase, ResourceRequest,
    ResourceUsage,
};
use super::{RenderGraphError, RenderGraphResult};
use crate::{FrameInput, PreparedFrameSceneData};
use bevy_math::URect;

#[derive(Clone, Copy)]
pub struct RenderNodeFrameContextInit<'a> {
    pub frame: &'a FrameInput,
    pub dispatcher_thread_index: u32,
}

impl<'a> RenderNodeFrameContextInit<'a> {
    pub fn new(frame: &'a FrameInput, dispatcher_thread_index: u32) -> Self {
        Self {
            frame,
            dispatcher_thread_index,
        }
    }
}

/// Camera access permission granted by the node context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderCameraAccess {
    Current { camera_index: u32 },
    Indexed { camera_index: u32 },
    All,
}

/// Frame/runtime hooks that are intentionally outside the resource allocator.
pub trait RenderNodeFrameRuntime {
    fn create_command_recorder(
        &mut self,
        _flow_group: RenderFlowGroup,
        _queue: RenderQueueKind,
        _label: &str,
    ) -> RenderGraphResult<()> {
        Err(RenderGraphError::InvalidState {
            reason: "frame runtime cannot create command recorders",
        })
    }

    fn has_command_recorder(&self, flow_group: RenderFlowGroup) -> RenderGraphResult<bool>;

    fn set_command_recorder_active(
        &mut self,
        flow_group: RenderFlowGroup,
        active: bool,
    ) -> RenderGraphResult<()>;

    fn set_viewport(
        &mut self,
        flow_group: RenderFlowGroup,
        viewport: URect,
    ) -> RenderGraphResult<()>;

    fn record_command_sync(
        &mut self,
        _flow_group: RenderFlowGroup,
        _sync: QueueSyncKind,
        _label: &str,
    ) -> RenderGraphResult<()> {
        Ok(())
    }
}

/// Immutable per-node setup copied into a `RenderNodeImplContext`.
///
/// This is intentionally small and `Copy`: graph execution can derive it from a
/// compiled node step, then build a context around the frame allocator for the
/// actual node call. The `flow_group` controls request ordering, while
/// `flow_space` controls logical resource-name scoping for camera/view nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderNodeImplContextInit {
    flow_group: RenderFlowGroup,
    flow_space: RenderFlowSpace,
    camera_index: Option<u32>,
    node_kind: RenderNodeImplKind,
    dispatcher_thread_index: u32,
}

impl RenderNodeImplContextInit {
    /// Creates init data for a unique/global node.
    ///
    /// Unique nodes are not tied to a camera, so their named resources resolve in
    /// shared flow space. The supplied `flow_group` is still used for request
    /// ordering and consume replay.
    pub fn unique_node(flow_group: RenderFlowGroup) -> Self {
        Self {
            flow_group,
            flow_space: RenderFlowSpace::SHARED,
            camera_index: None,
            node_kind: RenderNodeImplKind::Unique,
            dispatcher_thread_index: u32::MAX,
        }
    }

    /// Creates init data for a camera/view node.
    ///
    /// The `flow_space` should be the camera's render-flow space. Named resources
    /// created through `rt_name_tag` will be scoped to that space, preventing
    /// same-name resources from different cameras from colliding logically.
    pub fn camera_node(flow_group: RenderFlowGroup, flow_space: RenderFlowSpace) -> Self {
        Self {
            flow_group,
            flow_space,
            camera_index: Some(u32::from(flow_space.get())),
            node_kind: RenderNodeImplKind::Camera,
            dispatcher_thread_index: u32::MAX,
        }
    }

    /// Creates init data for a camera/view node with an explicit camera index.
    pub fn camera_node_with_index(
        flow_group: RenderFlowGroup,
        flow_space: RenderFlowSpace,
        camera_index: u32,
    ) -> Self {
        Self {
            flow_group,
            flow_space,
            camera_index: Some(camera_index),
            node_kind: RenderNodeImplKind::Camera,
            dispatcher_thread_index: u32::MAX,
        }
    }

    /// Stores the dispatcher thread index associated with this node call.
    ///
    /// The resource allocator does not use this value directly. It is carried so
    /// the context shape remains ready for render-node/job integration.
    pub fn with_dispatcher_thread_index(mut self, dispatcher_thread_index: u32) -> Self {
        self.dispatcher_thread_index = dispatcher_thread_index;
        self
    }

    /// Returns the flow group used for allocator request recording.
    ///
    /// Requests recorded by this context are appended to this group's stream.
    /// During consume, the same group cursor is advanced and matched against the
    /// pre-consume stream.
    pub fn flow_group(self) -> RenderFlowGroup {
        self.flow_group
    }

    /// Returns the configured render-flow space.
    ///
    /// For camera nodes this is the camera flow space. For unique nodes it is
    /// shared flow space.
    pub fn flow_space(self) -> RenderFlowSpace {
        self.flow_space
    }

    /// Returns the current camera index carried by camera/view nodes.
    pub fn camera_index(self) -> Option<u32> {
        self.camera_index
    }

    /// Returns the scoping rule used by `rt_name_tag`.
    pub fn node_kind(self) -> RenderNodeImplKind {
        self.node_kind
    }

    /// Returns the dispatcher thread index metadata carried by this init data.
    pub fn dispatcher_thread_index(self) -> u32 {
        self.dispatcher_thread_index
    }
}

/// Node scoping mode used to choose render-flow tag scope.
///
/// This is deliberately separate from existing render-graph node types. It only
/// answers the resource allocator question: should ordinary names be scoped to
/// shared/global flow space or to a camera/view flow space?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderNodeImplKind {
    /// A unique/global node; ordinary names resolve in shared flow space.
    Unique,
    /// A camera/view node; ordinary names resolve in the assigned flow space.
    Camera,
}

/// Node implementation context for resource-facing graph operations.
///
/// Node code should talk to this type instead of constructing allocator requests
/// directly. The context records each request with its configured flow group and
/// creates tags using the correct flow-space rule for the current node.
pub struct RenderNodeImplContext<'a> {
    allocator: &'a mut RenderResourceAllocator,
    frame_runtime: Option<&'a mut dyn RenderNodeFrameRuntime>,
    initialized: bool,
    flow_group: RenderFlowGroup,
    flow_space: RenderFlowSpace,
    camera_index: Option<u32>,
    node_kind: RenderNodeImplKind,
    dispatcher_thread_index: u32,
    frame_scene_data: Option<Arc<PreparedFrameSceneData>>,
}

impl<'a> RenderNodeImplContext<'a> {
    /// Creates a context around a frame resource allocator and per-node init data.
    ///
    /// This does not reset allocator state or change allocator phase. The caller
    /// is responsible for creating contexts at the correct point in pre-consume
    /// or consume execution.
    pub fn new(
        allocator: &'a mut RenderResourceAllocator,
        init: RenderNodeImplContextInit,
    ) -> Self {
        Self {
            allocator,
            frame_runtime: None,
            initialized: true,
            flow_group: init.flow_group,
            flow_space: init.flow_space,
            camera_index: init.camera_index,
            node_kind: init.node_kind,
            dispatcher_thread_index: init.dispatcher_thread_index,
            frame_scene_data: None,
        }
    }

    /// Creates a context around a frame allocator and frame runtime hooks.
    pub fn new_with_runtime(
        allocator: &'a mut RenderResourceAllocator,
        frame_runtime: &'a mut dyn RenderNodeFrameRuntime,
        init: RenderNodeImplContextInit,
    ) -> Self {
        Self {
            allocator,
            frame_runtime: Some(frame_runtime),
            initialized: true,
            flow_group: init.flow_group,
            flow_space: init.flow_space,
            camera_index: init.camera_index,
            node_kind: init.node_kind,
            dispatcher_thread_index: init.dispatcher_thread_index,
            frame_scene_data: None,
        }
    }

    /// Creates a diagnostic context that has not been set up for node execution.
    pub fn uninitialized(allocator: &'a mut RenderResourceAllocator) -> Self {
        Self {
            allocator,
            frame_runtime: None,
            initialized: false,
            flow_group: RenderFlowGroup::new(u16::MAX),
            flow_space: RenderFlowSpace::AUTOGENERATED,
            camera_index: None,
            node_kind: RenderNodeImplKind::Unique,
            dispatcher_thread_index: u32::MAX,
            frame_scene_data: None,
        }
    }

    /// Creates a context for a unique/global node.
    ///
    /// Ordinary resource names created by this context are shared/global. This is
    /// the behavior expected for unique/global nodes, where logical resource
    /// names use shared flow space.
    pub fn unique_node(
        allocator: &'a mut RenderResourceAllocator,
        flow_group: RenderFlowGroup,
    ) -> Self {
        Self::new(
            allocator,
            RenderNodeImplContextInit::unique_node(flow_group),
        )
    }

    /// Creates a context for a camera/view node.
    ///
    /// Ordinary resource names created by this context are scoped to
    /// `flow_space`, so two cameras can both use names like `"scene_color"` while
    /// remaining distinct logical resources.
    pub fn camera_node(
        allocator: &'a mut RenderResourceAllocator,
        flow_group: RenderFlowGroup,
        flow_space: RenderFlowSpace,
    ) -> Self {
        Self::new(
            allocator,
            RenderNodeImplContextInit::camera_node(flow_group, flow_space),
        )
    }

    pub fn with_frame_scene_data(mut self, frame_scene_data: Arc<PreparedFrameSceneData>) -> Self {
        self.frame_scene_data = Some(frame_scene_data);
        self
    }

    pub fn frame_scene_data(&self) -> RenderGraphResult<&PreparedFrameSceneData> {
        self.frame_scene_data
            .as_deref()
            .ok_or(RenderGraphError::InvalidState {
                reason: "prepared frame scene data is not available",
            })
    }

    /// Returns the allocator flow group this context records into.
    ///
    /// The flow group is part of request time. Getters during consume resolve a
    /// tag using this group's current replay cursor, not from the tag alone.
    pub fn render_flow_group(&self) -> RenderFlowGroup {
        self.flow_group
    }

    /// Validates that this context has been initialized for node execution.
    pub fn ensure_setup(&self) -> RenderGraphResult<()> {
        if self.initialized {
            Ok(())
        } else {
            Err(RenderGraphError::InvalidState {
                reason: "render node implementation context was used before node setup",
            })
        }
    }

    /// Returns the allocator phase for node logic that needs phase-aware behavior.
    pub fn resource_phase(&self) -> ResourceAllocatorPhase {
        self.allocator.phase()
    }

    /// Returns the configured render-flow space.
    ///
    /// This may be shared flow space for unique nodes or a camera/view flow space
    /// for camera nodes. Use `rt_name_tag` to get the effective tag scope.
    pub fn render_flow_space(&self) -> RenderFlowSpace {
        self.flow_space
    }

    /// Returns init data suitable for a worker-local copy of this context.
    pub fn init_for_worker(
        &self,
        dispatcher_thread_index: u32,
    ) -> RenderGraphResult<RenderNodeImplContextInit> {
        self.ensure_setup()?;
        let init = if let Some(camera_index) = self.camera_index {
            RenderNodeImplContextInit::camera_node_with_index(
                self.flow_group,
                self.flow_space,
                camera_index,
            )
        } else {
            RenderNodeImplContextInit::unique_node(self.flow_group)
        };
        Ok(init.with_dispatcher_thread_index(dispatcher_thread_index))
    }

    /// Returns dispatcher-thread metadata for future job-aware node execution.
    pub fn dispatcher_thread_index(&self) -> u32 {
        self.dispatcher_thread_index
    }

    /// Returns whether ordinary names are treated as shared/global names.
    pub fn is_unique_node(&self) -> bool {
        self.node_kind == RenderNodeImplKind::Unique
    }

    /// Returns current-camera access for camera/view nodes.
    pub fn current_camera_access(&self) -> RenderGraphResult<RenderCameraAccess> {
        self.ensure_setup()?;
        if self.is_unique_node() {
            return Err(RenderGraphError::InvalidState {
                reason: "current camera access is only valid for camera/view nodes",
            });
        }

        let camera_index = self.camera_index.ok_or(RenderGraphError::InvalidState {
            reason: "camera/view node is missing camera index",
        })?;
        Ok(RenderCameraAccess::Current { camera_index })
    }

    /// Returns indexed camera access for unique/global nodes.
    pub fn indexed_camera_access(
        &self,
        camera_index: u32,
    ) -> RenderGraphResult<RenderCameraAccess> {
        self.ensure_setup()?;
        if !self.is_unique_node() {
            return Err(RenderGraphError::InvalidState {
                reason: "indexed camera access is only valid for unique/global nodes",
            });
        }

        Ok(RenderCameraAccess::Indexed { camera_index })
    }

    /// Returns all-camera access for unique/global nodes.
    pub fn all_camera_access(&self) -> RenderGraphResult<RenderCameraAccess> {
        self.ensure_setup()?;
        if !self.is_unique_node() {
            return Err(RenderGraphError::InvalidState {
                reason: "all-camera access is only valid for unique/global nodes",
            });
        }

        Ok(RenderCameraAccess::All)
    }

    /// Returns whether typed resource retrieval is phase-legal.
    ///
    /// This forwards the allocator phase. It does not imply that a particular tag
    /// has a resolved resource; getters still validate kind, lifetime, and
    /// current consume position.
    pub fn is_consume_phase(&self) -> bool {
        self.allocator.is_consume_phase()
    }

    /// Returns the underlying allocator for read-only diagnostics and inspection.
    ///
    /// The context does not expose mutable allocator access. Node code should use
    /// the context methods so requests keep the correct flow group and flow-space
    /// semantics.
    pub fn resource_allocator(&self) -> &RenderResourceAllocator {
        self.allocator
    }

    /// Returns whether this context has frame runtime hooks attached.
    pub fn has_frame_runtime(&self) -> bool {
        self.frame_runtime.is_some()
    }

    /// Returns whether the frame runtime has a command recorder for this flow group.
    pub fn has_command_recorder(&mut self) -> RenderGraphResult<bool> {
        self.ensure_setup()?;
        let flow_group = self.flow_group;
        self.frame_runtime_mut()?.has_command_recorder(flow_group)
    }

    /// Routes command-recorder activation through the frame runtime.
    pub fn set_command_recorder_active(&mut self, active: bool) -> RenderGraphResult<()> {
        self.ensure_setup()?;
        let flow_group = self.flow_group;
        self.frame_runtime_mut()?
            .set_command_recorder_active(flow_group, active)
    }

    /// Sets the viewport through the active frame command recording runtime.
    pub fn set_viewport(&mut self, viewport: URect) -> RenderGraphResult<()> {
        self.ensure_setup()?;
        let flow_group = self.flow_group;
        self.frame_runtime_mut()?.set_viewport(flow_group, viewport)
    }

    /// Creates a logical tag in this node's effective flow space.
    ///
    /// For camera/view nodes, the tag is scoped to the context's camera flow
    /// space. For unique/global nodes, the tag is scoped to shared flow space.
    /// This only creates the logical name; it does not declare or allocate a
    /// resource.
    pub fn rt_name_tag(&self, name: impl Into<RenderFlowName>) -> RenderFlowNameTag {
        let flow_space = if self.is_unique_node() {
            RenderFlowSpace::SHARED
        } else {
            self.flow_space
        };
        RenderFlowNameTag::new(name.into(), flow_space)
    }

    /// Creates a shared-flow logical tag, bypassing camera scoping.
    ///
    /// Most node code should prefer `rt_name_tag`; explicit shared tags are for
    /// resources that are intentionally global across camera/view flow spaces.
    pub fn rt_shared_name_tag(name: impl Into<RenderFlowName>) -> RenderFlowNameTag {
        RenderFlowNameTag::new(name.into(), RenderFlowSpace::SHARED)
    }

    /// Returns the invalid/null tag used to represent an absent optional resource.
    ///
    /// Optional paths should pair this with `try_get_texture` or `try_get_buffer`.
    /// Ordinary required resources should use real tags and fail loudly if not
    /// declared or resolved.
    pub fn rt_null() -> RenderFlowNameTag {
        RenderFlowNameTag::INVALID
    }

    /// Creates a request-position-derived tag for a nameless temp allocation.
    ///
    /// The debug name is for diagnostics only. Identity comes from the allocator
    /// flow group and the next request position, so the same pre-consume and
    /// consume replay position reproduces the same temp tag. Use this for
    /// per-node scratch resources that should not be addressed by a stable
    /// logical name.
    pub fn temp_resource_tag(
        &self,
        debug_name: impl Into<RenderFlowName>,
    ) -> FrameResourceResult<RenderFlowNameTag> {
        let auto_id = self.allocator.next_request_auto_id(self.flow_group)?;
        Ok(RenderFlowNameTag::autogenerated(
            debug_name.into(),
            RenderFlowSpace::AUTOGENERATED,
            auto_id,
        ))
    }

    /// Records that a logical tag needs a texture or buffer with `desc`.
    ///
    /// During pre-consume this appends a declaration request. During consume it
    /// must replay the matching declaration at the same request position.
    /// Declaration does not create GPU memory; resolve assigns the actual pool
    /// resource later, and getters only read that resolved assignment.
    pub fn declare_resource(
        &mut self,
        tag: RenderFlowNameTag,
        desc: FrameResourceDesc,
    ) -> FrameResourceResult<()> {
        self.record(ResourceRequest::Declare { tag, desc })
    }

    /// Declares a texture resource through the current flow group.
    pub fn declare_texture(
        &mut self,
        tag: RenderFlowNameTag,
        desc: FrameTextureDesc,
    ) -> FrameResourceResult<()> {
        self.declare_resource(tag, FrameResourceDesc::Texture(desc))
    }

    /// Declares a buffer resource through the current flow group.
    pub fn declare_buffer(
        &mut self,
        tag: RenderFlowNameTag,
        desc: FrameBufferDesc,
    ) -> FrameResourceResult<()> {
        self.declare_resource(tag, FrameResourceDesc::Buffer(desc))
    }

    /// Records that `dst` should be declared using `src`'s descriptor.
    ///
    /// The descriptor is resolved by the allocator from the source tag's current
    /// logical resource, preserving request-order semantics around swaps and
    /// imports.
    pub fn declare_resource_like(
        &mut self,
        dst: RenderFlowNameTag,
        src: RenderFlowNameTag,
    ) -> FrameResourceResult<()> {
        self.record(ResourceRequest::DeclareLike { dst, src })
    }

    /// Records an external texture as a graph-tracked resource.
    ///
    /// The allocator tracks lifetime and use ordering for `tag`, but it does not
    /// own, recycle, or cache the external texture. The caller must register the
    /// matching external texture handle with the allocator before materialized
    /// resolve.
    pub fn import_texture(
        &mut self,
        tag: RenderFlowNameTag,
        external_id: ExternalFrameResourceId,
        desc: FrameTextureDesc,
    ) -> FrameResourceResult<()> {
        self.record(ResourceRequest::Import {
            tag,
            resource: ImportedFrameResource::texture(external_id, desc),
        })
    }

    /// Records an external buffer as a graph-tracked resource.
    ///
    /// Imported buffers participate in request replay and lifetime analysis, but
    /// the allocator treats them as non-owned resources and forgets only its
    /// per-frame tracking state during cleanup.
    pub fn import_buffer(
        &mut self,
        tag: RenderFlowNameTag,
        external_id: ExternalFrameResourceId,
        desc: FrameBufferDesc,
    ) -> FrameResourceResult<()> {
        self.record(ResourceRequest::Import {
            tag,
            resource: ImportedFrameResource::buffer(external_id, desc),
        })
    }

    /// Records and returns whether `tag` is currently declared at this point.
    ///
    /// The boolean becomes part of the request stream. During consume replay the
    /// allocator returns the pre-consume value, so branches depending on this
    /// query cannot silently diverge between phases.
    pub fn is_declared(&mut self, tag: RenderFlowNameTag) -> FrameResourceResult<bool> {
        self.allocator.request_is_declared(self.flow_group, tag)
    }

    /// Records the start of a resource use range.
    ///
    /// `usage` must include read and/or write intent. The request extends the
    /// solved lifetime and is replay-validated during consume. It does not by
    /// itself retrieve the resource.
    pub fn use_begin(
        &mut self,
        tag: RenderFlowNameTag,
        usage: ResourceUsage,
    ) -> FrameResourceResult<()> {
        self.record(ResourceRequest::UseBegin { tag, usage })
    }

    /// Records the end of a resource use range.
    ///
    /// Every successful `use_begin` must be balanced by `use_end` in the same
    /// request stream. The end request is also a lifetime touch for allocator
    /// lifetime analysis.
    pub fn use_end(&mut self, tag: RenderFlowNameTag) -> FrameResourceResult<()> {
        self.record(ResourceRequest::UseEnd { tag })
    }

    /// Records that a logical tag is no longer alive in this frame.
    ///
    /// Free closes the tag's current lifetime mapping. Later uses of the same tag
    /// require a new declaration/import, and consume replay must reproduce the
    /// free at the same request position.
    pub fn free(&mut self, tag: RenderFlowNameTag) -> FrameResourceResult<()> {
        self.record(ResourceRequest::Free { tag })
    }

    /// Records a swap between two live logical tags.
    ///
    /// After the swap, each tag resolves to the other tag's previous allocation
    /// timeline. Descriptor compatibility is validated during lifetime solving.
    pub fn swap(&mut self, a: RenderFlowNameTag, b: RenderFlowNameTag) -> FrameResourceResult<()> {
        self.record(ResourceRequest::Swap { a, b })
    }

    /// Records a swap between a live logical tag and an external texture.
    ///
    /// The old owned resource is made restricted/non-cacheable, and the external
    /// texture becomes the current resource for `tag` from this request position.
    /// The external texture handle must be registered before materialized resolve.
    pub fn swap_external_texture(
        &mut self,
        tag: RenderFlowNameTag,
        external_id: ExternalFrameResourceId,
        desc: FrameTextureDesc,
    ) -> FrameResourceResult<()> {
        self.record(ResourceRequest::SwapWithExternal {
            tag,
            resource: ImportedFrameResource::texture(external_id, desc),
        })
    }

    /// Records a swap between a live logical tag and an external buffer.
    ///
    /// This has the same ownership semantics as `swap_external_texture`, but for
    /// buffer resources. The allocator tracks the external buffer but does not
    /// recycle it.
    pub fn swap_external_buffer(
        &mut self,
        tag: RenderFlowNameTag,
        external_id: ExternalFrameResourceId,
        desc: FrameBufferDesc,
    ) -> FrameResourceResult<()> {
        self.record(ResourceRequest::SwapWithExternal {
            tag,
            resource: ImportedFrameResource::buffer(external_id, desc),
        })
    }

    /// Records a branch decision and stabilizes it across phases.
    ///
    /// During pre-consume, the supplied value is recorded and returned. During
    /// consume, the recorded pre-consume value is returned even if `value` differs,
    /// preventing request-stream divergence from runtime branch drift.
    pub fn decision(&mut self, value: bool) -> FrameResourceResult<bool> {
        self.allocator.request_decision(self.flow_group, value)
    }

    /// Records the start of a command-list queue scope for this node's flow group.
    pub fn begin_queue(&mut self, queue: RenderQueueKind) -> FrameResourceResult<()> {
        self.record(ResourceRequest::BeginQueue { queue })
    }

    /// Records the end of the current command-list queue scope.
    pub fn end_queue(&mut self) -> FrameResourceResult<()> {
        self.record(ResourceRequest::EndQueue)
    }

    /// Records allocator-visible queue synchronization in the request stream.
    ///
    /// This is separate from frame command recorder submission state.
    pub fn queue_sync(&mut self, sync: QueueSyncKind) -> FrameResourceResult<()> {
        self.record(ResourceRequest::QueueSync { sync })
    }

    /// Records command-runtime synchronization for this node's flow group.
    ///
    /// Sync nodes call this during consume after recording allocator-visible
    /// queue sync. The frame runtime owns how command-side sync is stored.
    pub fn command_sync(&mut self, sync: QueueSyncKind, label: &str) -> RenderGraphResult<()> {
        self.ensure_setup()?;
        let flow_group = self.flow_group;
        self.frame_runtime_mut()?
            .record_command_sync(flow_group, sync, label)
    }

    /// Returns the resolved texture for `tag` at the current consume request time.
    ///
    /// This is valid only during consume after materialized resolve. The lookup is
    /// timeline-aware: after swaps or external swaps, the same tag may resolve to
    /// a different allocation at different consume positions. Errors if the tag is
    /// missing, unresolved, or currently a buffer.
    pub fn get_texture(&self, tag: RenderFlowNameTag) -> FrameResourceResult<FrameTextureResource> {
        self.allocator.get_texture(tag, self.flow_group)
    }

    /// Optionally returns the resolved texture for `tag`.
    ///
    /// This has the same phase and kind checks as `get_texture`, but returns
    /// `None` for optional missing/unresolved tag timelines instead of treating
    /// them as required resources.
    pub fn try_get_texture(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameTextureResource>> {
        self.allocator.try_get_texture(tag, self.flow_group)
    }

    /// Returns the resolved buffer for `tag` at the current consume request time.
    ///
    /// This mirrors `get_texture` for buffer resources. It is phase-gated,
    /// timeline-aware, and errors if the tag is missing, unresolved, or currently
    /// a texture.
    pub fn get_buffer(&self, tag: RenderFlowNameTag) -> FrameResourceResult<FrameBufferResource> {
        self.allocator.get_buffer(tag, self.flow_group)
    }

    /// Optionally returns the resolved buffer for `tag`.
    ///
    /// This has the same phase and kind checks as `get_buffer`, but returns
    /// `None` for optional missing/unresolved tag timelines.
    pub fn try_get_buffer(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameBufferResource>> {
        self.allocator.try_get_buffer(tag, self.flow_group)
    }

    fn record(&mut self, request: ResourceRequest) -> FrameResourceResult<()> {
        self.allocator.record_request(self.flow_group, request)?;
        Ok(())
    }

    fn frame_runtime_mut(&mut self) -> RenderGraphResult<&mut (dyn RenderNodeFrameRuntime + 'a)> {
        self.frame_runtime
            .as_deref_mut()
            .ok_or(RenderGraphError::InvalidState {
                reason: "render node implementation context has no frame runtime",
            })
    }
}

// TODO(RenderNodeImplContext command side):
//
// This file currently covers the resource-flow side of the node implementation
// context: logical render-flow tags, declarations, imports, use ranges, swaps,
// decisions, and typed resolved-resource getters.
//
// The context now exposes command-recorder routing hooks through
// `RenderNodeFrameRuntime`. The remaining binding helpers are intentionally not
// implemented here yet because LEET uses wgpu, where binding is expressed
// through command encoders, render/compute passes, pipelines, bind group
// layouts, and bind groups rather than immediate global binding calls.
//
// The missing command/binding side still needs a real LEET design pass before
// public API is added. The functions to mirror conceptually include:
//
// - pass cleanup / binding-state cleanup:
//   - Unbind(clearRenderTarget)
//
// - sampled texture binding:
//   - BindTexture
//   - BindTextureMip
//   - BindTextureStencil
//   - BindTextures
//
// - sampler binding:
//   - BindSampler
//   - BindSamplers
//
// - buffer binding:
//   - BindBufferSRV
//   - BindConstantBuffer
//
// - storage/UAV-style binding:
//   - BindBufferUAV
//   - BindTextureUAV
//   - BindTextureMipUAV
//   - BindTextureUAVs
//
// - pipeline/render-target setup:
//   - BindPSO
//   - ColorTarget / DepthTarget / NullColorTarget / BlankOutput equivalents
//   - SetViewport
//
// Do not add placeholder versions of these methods that only store loose state
// or pretend to bind resources. They should appear only after LEET has the
// wgpu-native pass/bind-group abstraction that gives them production semantics.
