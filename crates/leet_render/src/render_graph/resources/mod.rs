//! Frame resource allocator types.
//!
//! This module is built in production layers. Pass 0 owns only the module map
//! and shared error surface; later passes fill in the resource allocator pieces.

pub mod allocator;
pub mod desc;
pub mod diagnostics;
pub mod error;
pub mod lifetime;
pub mod phase;
pub mod pool;
pub mod request;
pub mod tag;
pub mod usage;

pub use allocator::FrameResourceAllocator;
pub use desc::{FrameBufferDesc, FrameResourceDesc, FrameResourceShape, FrameTextureDesc};
pub use diagnostics::FrameResourceDiagnostics;
pub use error::{FrameResourceError, FrameResourceResult};
pub use lifetime::{
    AllocationRequest, AllocationRequestSource, FrameLifetimeSolution, RequestRange, TagLifetime,
    TagLifetimeEvent, TagLifetimeEventKind,
};
pub use phase::ResourceAllocatorPhase;
pub use pool::{
    FrameBufferResource, FrameResource, FrameResourceAllocation, FrameResourceAllocationClass,
    FrameResourceOwnership, FrameResourcePool, FrameResourcePoolAssignment,
    FrameResourcePoolCandidate, FrameResourcePoolPlan, FrameResourceReuseRejection,
    FrameResourceReuseRejectionReason, FrameTextureResource,
};
pub use request::{
    ExternalFrameResourceId, FrameResourceKind, ImportedFrameResource, QueueSyncKind,
    RenderQueueKind, RequestGroup, RequestGroupAction, ResourceRequest,
};
pub use tag::{
    AllocationRequestId, FrameResourceAllocationId, RenderFlowAutoId, RenderFlowGroup,
    RenderFlowName, RenderFlowNameTag, RenderFlowSpace, RequestTime, ResourceRequestId,
};
pub use usage::ResourceUsage;
