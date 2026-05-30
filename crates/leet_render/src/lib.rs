mod app;
mod camera;
mod extract;
mod render_graph;
mod rendering;
mod rhi_wgpu;
mod scene;
mod texture;
mod window;

pub use app::{
    ExtractSchedule, JobPlugin, MainWorld, PipelinedRenderingPlugin, Render, RenderApp,
    RenderAppChannels, RenderAppPlugin, RenderExtractApp, RenderShellPlugins, RenderSystems,
};
pub use camera::{
    sync_render_camera_storage, CameraDependencyFlags, CameraManagement, CameraPlugin,
    CameraPrepareContext, CameraRenderPolicy, PreparedCameraDependency, PreparedCameraHistory,
    PreparedFrameCamera, RenderCamera, RenderCameraRegistration, RenderCameraRegistrationRef,
    RenderCameraStorage, MAX_CAMERA_DEPENDENCIES, MAX_CAMERA_DEPENDENCY_DEPTH,
};
pub use extract::{Extract, ExtractionPlugin};
pub use leet_core::{Leeror, LeetResult};
pub use leet_jobs2::{
    Builder, CompletionDeferral, Counter, JobHint, JobSystemConfig, LeetJobSystem, Priority,
    RunContext, ScheduleParam,
};
pub use render_graph::{
    execute_graph_dependency_counter_consume, execute_graph_sequential_gpu_order, process_node,
    process_node_with_runtime, AddGraphGroupImport, AddGraphOptions, AllocationRequest,
    AllocationRequestId, AllocationRequestSource, BuiltRenderNodeGraph, CommandListGroupNode,
    CommandListGroupStore, ExternalFrameResourceId, FrameBufferDesc, FrameBufferResource,
    FrameCommandPassKind, FrameCommandRecorderSlot, FrameCommandRecorderState,
    FrameCommandRecorders, FrameCommandSubmission, FrameCommandSyncEvent, FrameLifetimeSolution,
    FrameResource, FrameResourceAllocation, FrameResourceAllocationClass,
    FrameResourceAllocationId, FrameResourceAllocator, FrameResourceDesc, FrameResourceError,
    FrameResourceFlowGroup, FrameResourceKind, FrameResourceOwnership, FrameResourcePool,
    FrameResourcePoolAssignment, FrameResourcePoolCandidate, FrameResourcePoolPlan,
    FrameResourceRequestTime, FrameResourceResult, FrameResourceReuseRejection,
    FrameResourceReuseRejectionReason, FrameResourceShape, FrameTextureDesc, FrameTextureResource,
    GraphImportMap, ImportedFrameResource, NodeGroupId, NoopRenderGraphCoreRunnerHooks,
    QueueSyncKind, RenderCameraAccess, RenderDependencyData, RenderDependencyId, RenderFlowAutoId,
    RenderFlowGroup, RenderFlowName, RenderFlowNameTag, RenderFlowSpace, RenderGlobalBindingMask,
    RenderGraphCache, RenderGraphCacheEntry, RenderGraphCacheLookup, RenderGraphCameraBuildData,
    RenderGraphCoreRunReport, RenderGraphCoreRunner, RenderGraphCoreRunnerHooks,
    RenderGraphCoreRunnerState, RenderGraphDependencyCounters,
    RenderGraphDependencyExecutionReport, RenderGraphError, RenderGraphJobNode,
    RenderGraphJobPayload, RenderGraphResult, RenderGraphShapeHash, RenderGraphShapeHashBuilder,
    RenderNodeBeginRenderTargets, RenderNodeCleanupBatchData, RenderNodeCommandListUsage,
    RenderNodeData, RenderNodeDebugName, RenderNodeDeclareResources, RenderNodeDependencyKind,
    RenderNodeEndFrame, RenderNodeEndRender, RenderNodeEndRenderTargets,
    RenderNodeExecutionMetadata, RenderNodeFlushBufferGrabs, RenderNodeFlushTextureGrabs,
    RenderNodeFrameRuntime, RenderNodeGraph, RenderNodeGraphFactory, RenderNodeId, RenderNodeImpl,
    RenderNodeImplContext, RenderNodeImplContextInit, RenderNodeImplId, RenderNodeImplKind,
    RenderNodeImplStore, RenderNodeKind, RenderNodeParameters, RenderNodePresent,
    RenderNodeProcessReport, RenderNodeProcessState, RenderNodeResourceDeclaration, RenderNodeRole,
    RenderNodeStartRender, RenderNodeSubtype, RenderNodeSynchronize, RenderNodeView,
    RenderQueueKind, RequestGroup, RequestGroupAction, RequestRange, ResourceAllocatorPhase,
    ResourceRequest, ResourceRequestId, ResourceUsage, TagLifetime, TagLifetimeEvent,
    TagLifetimeEventKind,
};
pub use rendering::{
    dispatch_general_rendering, render_resources_from_wgpu, AtomicBufferUploader, BufferUploadBase,
    BufferUploadPlugin, BufferUploader, CameraRenderSetupKey, FrameCamera, FrameCaptureIntent,
    FrameDebugIntent, FrameDispatcher, FrameInput, FrameInputBuilder, FramePurpose, FrameRenderer,
    FrameRendererHandle, FrameRenderingMode, FrameTarget, FrameTargetKey, FrameTargetResolver,
    FrameTiming, PresentationIntent, RenderCameraId, RenderCommand, RenderCommandHandler,
    RenderCommandQueueKind, RenderCommandSafety, RenderFrameContext, RenderFrameError,
    RenderFrameResult, RenderJobBuilder, RenderSceneId, RenderViewport,
    SparseBufferUpdateJobs, SparseBufferUpdatePipeline, SparseUploadMetadata,
    SparseUploadStagingBuffers,
};
pub(crate) use rhi_wgpu::RenderResources;
pub use rhi_wgpu::{
    RHIPlugin, RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue,
    RendererInitializationError, WgpuSettings, WgpuWrapper,
};
pub use scene::{
    render_device_from_wgpu, render_queue_from_wgpu, AtomicAppendBuffer, AtomicPod, BufferUsages,
    DynamicStructuredStorageBuffer, FrameAppendBuffer, GpuArrayBufferable, GpuInstance,
    GpuInstanceIndex, GpuInstanceInput, GpuOnlyBuffer, GpuOutputArrayBuffer, GpuScene,
    GpuSceneFakeGpuEmulation, GpuScenePhase, GpuScenePlugin, RawArrayBuffer, RenderProxy,
    RenderProxyDescriptor, RenderProxyId, RenderProxyKind, RenderingPreprocessingPlugin,
    ShaderSize, ShaderType, StructuredStorageBuffer, UniformBuffer, WriteBufferRangeError,
};
pub use texture::{ManualTextureView, ManualTextureViews, TexturePlugin};
pub use window::{
    cleanup_stale_surfaces, create_surfaces, prepare_windows, smoke_test_render_windows,
    window_surface_needs_configuration, RenderWindow, RenderWindowPlugin, RenderWindowRegistry,
    WindowSurfaces,
};
