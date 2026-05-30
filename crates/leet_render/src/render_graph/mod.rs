//! Render graph module.

pub mod graph;
pub mod resources;

#[cfg(test)]
#[path = "../tests/render_graph/mod.rs"]
mod tests;

pub use graph::{
    execute_graph_dependency_counter_consume, execute_graph_sequential_gpu_order, process_node,
    process_node_with_runtime, AddGraphGroupImport, AddGraphOptions, BuiltRenderNodeGraph,
    CommandListGroupNode, CommandListGroupStore, FrameCommandPassKind, FrameCommandRecorderSlot,
    FrameCommandRecorderState, FrameCommandRecorders, FrameCommandSubmission,
    FrameCommandSyncEvent, GraphImportMap, NodeGroupId, NoopRenderGraphCoreRunnerHooks,
    RenderCameraAccess, RenderDependencyData, RenderDependencyId, RenderGlobalBindingMask,
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
};
pub use resources::{
    AllocationRequest, AllocationRequestId, AllocationRequestSource, ExternalFrameResourceId,
    FrameBufferDesc, FrameBufferResource, FrameLifetimeSolution, FrameResource,
    FrameResourceAllocation, FrameResourceAllocationClass, FrameResourceAllocationId,
    FrameResourceAllocator, FrameResourceDesc, FrameResourceError, FrameResourceKind,
    FrameResourceOwnership, FrameResourcePool, FrameResourcePoolAssignment,
    FrameResourcePoolCandidate, FrameResourcePoolPlan, FrameResourceResult,
    FrameResourceReuseRejection, FrameResourceReuseRejectionReason, FrameResourceShape,
    FrameTextureDesc, FrameTextureResource, ImportedFrameResource, QueueSyncKind, RenderFlowAutoId,
    RenderFlowGroup, RenderFlowGroup as FrameResourceFlowGroup, RenderFlowName, RenderFlowNameTag,
    RenderFlowSpace, RenderQueueKind, RequestGroup, RequestGroupAction, RequestRange,
    RequestTime as FrameResourceRequestTime, ResourceAllocatorPhase, ResourceRequest,
    ResourceRequestId, ResourceUsage, TagLifetime, TagLifetimeEvent, TagLifetimeEventKind,
};
