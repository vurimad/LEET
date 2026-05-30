//! Concrete render-graph node implementations.

use super::frame_command_lists::FrameCommandListIndex;
use super::render_context::{NodeRecordContext, RenderContext};
use super::render_node::{RenderNode, RenderNodeCommandListUsage, RenderNodeType};
use leet_core::LeetResult;

/// Node that clears the current backbuffer.
pub struct ClearBackbufferNode {
    name: String,
    color: wgpu::Color,
}

impl ClearBackbufferNode {
    pub fn new(color: wgpu::Color) -> Self {
        Self {
            name: "ClearBackbuffer".to_string(),
            color,
        }
    }

    pub fn named(name: impl Into<String>, color: wgpu::Color) -> Self {
        Self {
            name: name.into(),
            color,
        }
    }
}

impl RenderNode for ClearBackbufferNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Own
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        record.encode_backbuffer_pass(Some(self.name()), wgpu::LoadOp::Clear(self.color), |_| {});
        Ok(())
    }
}

/// Frame-scoped start marker for a render graph.
pub struct StartFrameNode {
    name: String,
}

impl StartFrameNode {
    pub fn new() -> Self {
        Self {
            name: "StartFrame".to_string(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Default for StartFrameNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for StartFrameNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> RenderNodeType {
        RenderNodeType::Unique
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn run(&self, _frame: &RenderContext<'_>) -> LeetResult<()> {
        Ok(())
    }
}

/// Root node for the main pass.
pub struct MainPassRootNode {
    name: String,
    clear_color: wgpu::Color,
}

impl MainPassRootNode {
    pub fn new(clear_color: wgpu::Color) -> Self {
        Self {
            name: "MainPassRoot".to_string(),
            clear_color,
        }
    }

    pub fn named(name: impl Into<String>, clear_color: wgpu::Color) -> Self {
        Self {
            name: name.into(),
            clear_color,
        }
    }
}

impl RenderNode for MainPassRootNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Own
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        record
            .encoder()
            .insert_debug_marker(&format!("{}_Begin", self.name()));
        record.encode_backbuffer_pass(
            Some(self.name()),
            wgpu::LoadOp::Clear(self.clear_color),
            |pass| {
                pass.insert_debug_marker(&format!("{}_Pass", self.name()));
            },
        );
        record
            .encoder()
            .insert_debug_marker(&format!("{}_End", self.name()));
        Ok(())
    }
}

/// Placeholder opaque draw list appended into the main pass task.
pub struct OpaqueDrawsNode {
    name: String,
}

impl OpaqueDrawsNode {
    pub fn new() -> Self {
        Self {
            name: "OpaqueDraws".to_string(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Default for OpaqueDrawsNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for OpaqueDrawsNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Require
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        record
            .encoder()
            .insert_debug_marker(&format!("{}_Begin", self.name()));
        record.encode_backbuffer_pass(Some(self.name()), wgpu::LoadOp::Load, |pass| {
            pass.insert_debug_marker(&format!("{}_Pass", self.name()));
        });
        record
            .encoder()
            .insert_debug_marker(&format!("{}_End", self.name()));
        Ok(())
    }
}

/// Placeholder sky draw list appended into the main pass task.
pub struct SkyDrawsNode {
    name: String,
}

impl SkyDrawsNode {
    pub fn new() -> Self {
        Self {
            name: "SkyDraws".to_string(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Default for SkyDrawsNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for SkyDrawsNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Require
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        record
            .encoder()
            .insert_debug_marker(&format!("{}_Begin", self.name()));
        record.encode_backbuffer_pass(Some(self.name()), wgpu::LoadOp::Load, |pass| {
            pass.insert_debug_marker(&format!("{}_Pass", self.name()));
        });
        record
            .encoder()
            .insert_debug_marker(&format!("{}_End", self.name()));
        Ok(())
    }
}

/// Placeholder bloom pass appended as its own task.
pub struct BloomNode {
    name: String,
}

impl BloomNode {
    pub fn new() -> Self {
        Self {
            name: "Bloom".to_string(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Default for BloomNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for BloomNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Own
    }

    fn record(&self, record: &mut NodeRecordContext<'_>) -> LeetResult<()> {
        record
            .encoder()
            .insert_debug_marker(&format!("{}_Begin", self.name()));
        record.encode_backbuffer_pass(Some(self.name()), wgpu::LoadOp::Load, |pass| {
            pass.insert_debug_marker(&format!("{}_Pass", self.name()));
        });
        record
            .encoder()
            .insert_debug_marker(&format!("{}_End", self.name()));
        Ok(())
    }
}

/// Frame-scoped end marker for a render graph.
pub struct EndFrameNode {
    name: String,
}

impl EndFrameNode {
    pub fn new() -> Self {
        Self {
            name: "EndFrame".to_string(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Default for EndFrameNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderNode for EndFrameNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> RenderNodeType {
        RenderNodeType::Unique
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::None
    }

    fn run(&self, _frame: &RenderContext<'_>) -> LeetResult<()> {
        Ok(())
    }
}

/// Node that submits all frame command lists through a target slot.
pub struct SubmitCommandListsNode {
    scope_name: String,
    up_to_slot: FrameCommandListIndex,
}

impl SubmitCommandListsNode {
    pub fn new(scope_name: impl Into<String>, up_to_slot: FrameCommandListIndex) -> Self {
        Self {
            scope_name: scope_name.into(),
            up_to_slot,
        }
    }
}

impl RenderNode for SubmitCommandListsNode {
    fn name(&self) -> &str {
        &self.scope_name
    }

    fn node_type(&self) -> RenderNodeType {
        RenderNodeType::Unique
    }

    fn command_list_usage(&self) -> RenderNodeCommandListUsage {
        RenderNodeCommandListUsage::Sync
    }

    fn run(&self, frame: &RenderContext<'_>) -> LeetResult<()> {
        frame.submit_command_lists(&self.scope_name, self.up_to_slot)
    }
}
