//! Core render graph topology and execution module.
//!
//! This module owns graph-level concepts: nodes, dependencies, graph import,
//! command-list groups, execution ordering, graph cache, and graph execution
//! harnesses. Frame resource allocation lives in `resources` and is intentionally
//! kept separate from graph topology errors.

mod cache;
mod command_group;
mod command_recorder;
mod error;
mod execution;
mod factory;
mod frame_execution_runtime;
mod graph_executor;
mod ids;
mod metadata;
mod node_impl;
mod node_topology;
mod render_node_graph;
mod render_node_impl_context;
pub(crate) mod storage;
mod system_nodes;

pub use cache::{
    RenderGraphCache, RenderGraphCacheEntry, RenderGraphCacheLookup, RenderGraphCameraBuildData,
    RenderGraphShapeHash, RenderGraphShapeHashBuilder,
};
pub use command_group::{CommandListGroupNode, CommandListGroupStore};
pub use command_recorder::{
    FrameCommandPassKind, FrameCommandRecorderSlot, FrameCommandRecorderState,
    FrameCommandRecorders, FrameCommandSubmission, FrameCommandSyncEvent,
};
pub use error::{RenderGraphError, RenderGraphResult};
pub use execution::{
    execute_graph_dependency_counter_consume, execute_graph_sequential_gpu_order, process_node,
    process_node_with_runtime, RenderGraphDependencyCounters, RenderGraphDependencyExecutionReport,
    RenderGraphJobNode, RenderGraphJobPayload, RenderNodeProcessReport, RenderNodeProcessState,
};
pub use factory::{FinalRenderNodeGraph, RenderNodeGraphFactory};
pub use frame_execution_runtime::FrameExecutionRuntime;
pub use graph_executor::{
    NoopRenderGraphExecutorHooks, RenderGraphExecutionInput, RenderGraphExecutionReport,
    RenderGraphExecutor, RenderGraphExecutorHooks, RenderGraphExecutorState,
};
pub use ids::{NodeGroupId, RenderDependencyId, RenderNodeId, RenderNodeImplId};
pub use metadata::{
    RenderNodeCommandListUsage, RenderNodeDebugName, RenderNodeDependencyKind, RenderNodeKind,
    RenderNodeRole, RenderNodeSubtype,
};
pub use node_impl::{RenderGlobalBindingMask, RenderNodeImpl, RenderNodeImplStore};
pub use node_topology::{
    RenderDependencyData, RenderNodeData, RenderNodeExecutionMetadata, RenderNodeParameters,
    RenderNodeView,
};
pub use render_node_graph::{
    AddGraphGroupImport, AddGraphOptions, GraphImportMap, RenderNodeGraph,
};
pub use render_node_impl_context::{
    RenderCameraAccess, RenderNodeFrameContextInit, RenderNodeFrameRuntime, RenderNodeImplContext,
    RenderNodeImplContextInit, RenderNodeImplKind,
};
pub use system_nodes::{
    RenderNodeBeginRenderTargets, RenderNodeCleanupBatchData, RenderNodeDeclareResources,
    RenderNodeEndFrame, RenderNodeEndRender, RenderNodeEndRenderTargets,
    RenderNodeFlushBufferGrabs, RenderNodeFlushTextureGrabs, RenderNodePresent,
    RenderNodeResourceDeclaration, RenderNodeStartRender, RenderNodeSynchronize,
};
