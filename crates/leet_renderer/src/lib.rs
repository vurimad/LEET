//! Rendering backend for LEET.
//!
//! Powered by wgpu.
//!
//! # Usage
//!
//! ```ignore
//! // Before the event loop starts:
//! let mut renderer = Renderer::init().expect("renderer init failed");
//!
//! // Inside on_window_ready:
//! renderer
//!     .create_viewport(window.raw_arc(), window.inner_size())
//!     .expect("create viewport failed");
//! ```

pub mod frame_command_lists;
pub mod frame_renderer;
pub mod frame_submission;
pub mod render_collector;
pub mod render_context;
pub mod render_graph;
pub mod render_node;
pub mod render_proxy;
pub mod render_scene;
pub mod render_viewport;
pub mod renderer;
mod scene_gpu;
pub mod surface;

pub use frame_command_lists::{FrameCommandListIndex, FrameCommandLists};
pub use frame_renderer::FrameRenderer;
pub use frame_submission::{
    RenderCameraFrameInfo, RenderFramePurpose, RenderFrameSubmission, ViewportFrameInfo,
};
pub use render_collector::{CollectedRenderScene, RenderCollector};
pub use render_context::{NodeRecordContext, RenderContext};
pub use render_graph::{RenderGraph, RenderGraphDependency, RenderGraphNodeId};
pub use render_node::{
    BloomNode, ClearBackbufferNode, EndFrameNode, MainPassRootNode, OpaqueDrawsNode,
    RenderExecutionPlan, RenderExecutionStep, RenderFrameNodeStep, RenderNode,
    RenderNodeCommandListType, RenderNodeCommandListUsage, RenderNodeDependencyType,
    RenderNodeType, RenderRecordTask, SkyDrawsNode, StartFrameNode, SubmitCommandListsNode,
};
pub use render_proxy::{RenderProxy, RenderProxyDescriptor, RenderProxyId, RenderProxyKind};
pub use render_scene::{
    RenderSceneCommands, RenderSceneId, RenderSceneProxy, RenderSceneRegistry, RenderSceneSnapshot,
    RenderSceneType,
};
pub use render_viewport::{RenderViewport, RenderViewportOwner};
pub use renderer::Renderer;
pub use surface::RenderSurface;
