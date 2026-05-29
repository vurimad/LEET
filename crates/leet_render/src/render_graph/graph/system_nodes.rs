//! Core system node implementations used by graph execution tests and recipes.
//!
//! These nodes are ordinary `RenderNodeImpl` implementations. They reserve graph
//! slots for lifecycle, synchronization, declaration, and render-target marker
//! behavior without owning renderer feature algorithms.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use leet_jobs2::Builder as RenderJobBuilder;

use super::{RenderGraphResult, RenderNodeCommandListUsage, RenderNodeImpl, RenderNodeImplContext};
use crate::render_graph::resources::{
    FrameBufferDesc, FrameResourceDesc, FrameTextureDesc, QueueSyncKind, RenderFlowName,
    ResourceUsage,
};

macro_rules! lifecycle_node {
    ($type_name:ident, $debug_name:literal, $records_request_stream:expr) => {
        #[derive(Clone, Debug, Default)]
        pub struct $type_name {
            consume_counter: Option<Arc<AtomicU64>>,
        }

        impl $type_name {
            pub fn new() -> Self {
                Self::default()
            }

            pub fn with_consume_counter(mut self, counter: Arc<AtomicU64>) -> Self {
                self.consume_counter = Some(counter);
                self
            }
        }

        impl RenderNodeImpl for $type_name {
            fn name(&self) -> &str {
                $debug_name
            }

            fn command_list_usage(&self) -> RenderNodeCommandListUsage {
                RenderNodeCommandListUsage::None
            }

            fn execute(
                &self,
                rctx: &mut RenderNodeImplContext<'_>,
                _jobs: &mut RenderJobBuilder,
            ) -> RenderGraphResult<()> {
                execute_lifecycle_node(rctx, self.consume_counter.as_ref(), $records_request_stream)
            }
        }
    };
}

lifecycle_node!(RenderNodeStartRender, "StartRender", true);
lifecycle_node!(RenderNodeEndRender, "EndRender", false);
lifecycle_node!(RenderNodePresent, "Present", false);
lifecycle_node!(RenderNodeFlushTextureGrabs, "FlushTextureGrabs", false);
lifecycle_node!(RenderNodeFlushBufferGrabs, "FlushBufferGrabs", false);
lifecycle_node!(RenderNodeCleanupBatchData, "CleanupBatchData", false);
lifecycle_node!(RenderNodeEndFrame, "EndFrame", false);

/// Graph-visible synchronization node.
#[derive(Clone, Debug)]
pub struct RenderNodeSynchronize {
    sync: QueueSyncKind,
    command_label: &'static str,
}

impl RenderNodeSynchronize {
    pub const fn new(sync: QueueSyncKind, command_label: &'static str) -> Self {
        Self {
            sync,
            command_label,
        }
    }

    pub const fn sync(&self) -> QueueSyncKind {
        self.sync
    }
}

impl RenderNodeImpl for RenderNodeSynchronize {
    fn name(&self) -> &str {
        self.command_label
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Sync
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        rctx.queue_sync(self.sync)?;
        if rctx.is_consume_phase() && rctx.has_frame_runtime() {
            rctx.command_sync(self.sync, self.command_label)?;
        }
        Ok(())
    }
}

/// A resource declaration emitted by a declaration system node.
#[derive(Clone, Debug)]
pub struct RenderNodeResourceDeclaration {
    name: RenderFlowName,
    desc: FrameResourceDesc,
}

impl RenderNodeResourceDeclaration {
    pub fn texture(name: &'static str, desc: FrameTextureDesc) -> Self {
        Self {
            name: RenderFlowName::from_static(name),
            desc: FrameResourceDesc::Texture(desc),
        }
    }

    pub fn buffer(name: &'static str, desc: FrameBufferDesc) -> Self {
        Self {
            name: RenderFlowName::from_static(name),
            desc: FrameResourceDesc::Buffer(desc),
        }
    }
}

/// Declaration node fixture for stable resource request streams.
#[derive(Clone, Debug)]
pub struct RenderNodeDeclareResources {
    name: &'static str,
    declarations: Vec<RenderNodeResourceDeclaration>,
}

impl RenderNodeDeclareResources {
    pub fn new(name: &'static str, declarations: Vec<RenderNodeResourceDeclaration>) -> Self {
        Self { name, declarations }
    }

    pub fn declarations(&self) -> &[RenderNodeResourceDeclaration] {
        &self.declarations
    }
}

impl RenderNodeImpl for RenderNodeDeclareResources {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        for declaration in &self.declarations {
            let tag = rctx.rt_name_tag(declaration.name);
            rctx.declare_resource(tag, declaration.desc.clone())?;
        }
        Ok(())
    }
}

/// Render-target setup marker.
#[derive(Clone, Debug)]
pub struct RenderNodeBeginRenderTargets {
    name: &'static str,
    target_name: RenderFlowName,
    usage: ResourceUsage,
}

impl RenderNodeBeginRenderTargets {
    pub fn new(name: &'static str, target_name: &'static str, usage: ResourceUsage) -> Self {
        Self {
            name,
            target_name: RenderFlowName::from_static(target_name),
            usage,
        }
    }
}

impl RenderNodeImpl for RenderNodeBeginRenderTargets {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        let tag = rctx.rt_name_tag(self.target_name);
        rctx.use_begin(tag, self.usage)?;
        Ok(())
    }

    fn binds_render_targets(&self) -> bool {
        true
    }
}

/// Render-target end marker.
#[derive(Clone, Debug)]
pub struct RenderNodeEndRenderTargets {
    name: &'static str,
    target_name: RenderFlowName,
}

impl RenderNodeEndRenderTargets {
    pub fn new(name: &'static str, target_name: &'static str) -> Self {
        Self {
            name,
            target_name: RenderFlowName::from_static(target_name),
        }
    }
}

impl RenderNodeImpl for RenderNodeEndRenderTargets {
    fn name(&self) -> &str {
        self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn execute(
        &self,
        rctx: &mut RenderNodeImplContext<'_>,
        _jobs: &mut RenderJobBuilder,
    ) -> RenderGraphResult<()> {
        let tag = rctx.rt_name_tag(self.target_name);
        rctx.use_end(tag)?;
        Ok(())
    }

    fn binds_render_targets(&self) -> bool {
        true
    }
}

fn execute_lifecycle_node(
    rctx: &mut RenderNodeImplContext<'_>,
    consume_counter: Option<&Arc<AtomicU64>>,
    records_request_stream: bool,
) -> RenderGraphResult<()> {
    if records_request_stream {
        rctx.decision(true)?;
    }

    if rctx.is_consume_phase() {
        if let Some(counter) = consume_counter {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    Ok(())
}
