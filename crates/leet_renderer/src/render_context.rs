//! Per-frame rendering context and recording helpers.
//!
//! [`RenderContext`] owns the acquired swap-chain image for one frame and
//! exposes immutable data shared across render jobs.
//! [`NodeRecordContext`] owns one command encoder and finishes into a numbered
//! slot in [`crate::frame_command_lists::FrameCommandLists`].

use crate::frame_command_lists::{FrameCommandListIndex, FrameCommandLists};
use crate::render_viewport::RenderViewport;
use leet_core::{Leeror, LeetResult};
use std::sync::Arc;

struct RenderFrameShared<'device> {
    device: &'device wgpu::Device,
    queue: &'device wgpu::Queue,
    frame_index: u64,
    size: (u32, u32),
    surface_format: wgpu::TextureFormat,
    backbuffer: wgpu::TextureView,
    command_lists: FrameCommandLists,
}

impl<'device> RenderFrameShared<'device> {
    fn device(&self) -> &wgpu::Device {
        self.device
    }

    fn queue(&self) -> &wgpu::Queue {
        self.queue
    }

    fn frame_index(&self) -> u64 {
        self.frame_index
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_format
    }

    fn backbuffer(&self) -> &wgpu::TextureView {
        &self.backbuffer
    }

    fn command_lists(&self) -> &FrameCommandLists {
        &self.command_lists
    }
}

/// Shared frame state used while building and recording a render graph.
///
/// The acquired swap-chain image stays on the outer frame context, while the
/// immutable data needed by recording jobs is stored in an `Arc` so the jobs
/// can finish independently into frame command-list slots.
pub struct RenderContext<'device> {
    shared: Arc<RenderFrameShared<'device>>,
    surface_frame: wgpu::SurfaceTexture,
}

impl<'device> RenderContext<'device> {
    pub(crate) fn new(
        device: &'device wgpu::Device,
        queue: &'device wgpu::Queue,
        viewport: &RenderViewport,
        frame_index: u64,
        command_list_count: usize,
    ) -> LeetResult<Self> {
        let surface = viewport.surface().ok_or_else(|| {
            Leeror::Runtime(format!(
                "viewport '{}' does not own a render surface",
                viewport.name(),
            ))
        })?;
        let surface_frame = surface.acquire()?;
        let backbuffer = surface_frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        Ok(Self {
            shared: Arc::new(RenderFrameShared {
                device,
                queue,
                frame_index,
                size: surface.size,
                surface_format: surface.format(),
                backbuffer,
                command_lists: FrameCommandLists::prepare_for_frame(command_list_count),
            }),
            surface_frame,
        })
    }

    /// The device shared by all recording jobs in this frame.
    pub fn device(&self) -> &wgpu::Device {
        self.shared.device()
    }

    /// The queue used to submit finished command buffers.
    pub fn queue(&self) -> &wgpu::Queue {
        self.shared.queue()
    }

    /// Monotonic frame number assigned by the renderer.
    pub fn frame_index(&self) -> u64 {
        self.shared.frame_index()
    }

    /// Current surface size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.shared.size()
    }

    /// Surface format of the current backbuffer.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.shared.surface_format()
    }

    /// View of the current backbuffer.
    pub fn backbuffer(&self) -> &wgpu::TextureView {
        self.shared.backbuffer()
    }

    /// Frame command-list registry for this frame.
    pub fn command_lists(&self) -> &FrameCommandLists {
        self.shared.command_lists()
    }

    /// Create a new recording context for a specific frame command-list slot.
    pub fn begin_recording(
        &self,
        slot: FrameCommandListIndex,
        label: Option<&str>,
    ) -> LeetResult<NodeRecordContext<'device>> {
        self.command_lists().has_command_list(slot)?;
        Ok(NodeRecordContext::new(self.shared.clone(), slot, label))
    }

    /// Submit all finished command buffers from the current flush cursor
    /// through `up_to_index`.
    pub fn submit_command_lists(
        &self,
        scope_name: &str,
        up_to_index: FrameCommandListIndex,
    ) -> LeetResult<()> {
        self.command_lists()
            .submit(scope_name, up_to_index, self.queue())
    }

    /// Present the frame's swap-chain image.
    pub fn present(self) {
        self.surface_frame.present();
    }
}

/// Mutable per-task recording state.
///
/// Each instance owns one command encoder. When recording completes it stores
/// the finished command buffer into its reserved frame command-list slot.
pub struct NodeRecordContext<'device> {
    shared: Arc<RenderFrameShared<'device>>,
    slot: FrameCommandListIndex,
    encoder: wgpu::CommandEncoder,
}

impl<'device> NodeRecordContext<'device> {
    fn new(
        shared: Arc<RenderFrameShared<'device>>,
        slot: FrameCommandListIndex,
        label: Option<&str>,
    ) -> Self {
        let encoder = shared
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label });

        Self {
            shared,
            slot,
            encoder,
        }
    }

    /// Slot this recorder will finish into.
    pub fn slot(&self) -> FrameCommandListIndex {
        self.slot
    }

    /// The device shared by all recording jobs in this frame.
    pub fn device(&self) -> &wgpu::Device {
        self.shared.device()
    }

    /// Monotonic frame number assigned by the renderer.
    pub fn frame_index(&self) -> u64 {
        self.shared.frame_index()
    }

    /// Current surface size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.shared.size()
    }

    /// Surface format of the current backbuffer.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.shared.surface_format()
    }

    /// View of the current backbuffer.
    pub fn backbuffer(&self) -> &wgpu::TextureView {
        self.shared.backbuffer()
    }

    /// Low-level encoder access for custom passes and copies.
    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }

    /// Convenience helper for passes targeting the current backbuffer.
    pub fn encode_backbuffer_pass<F>(
        &mut self,
        label: Option<&str>,
        load: wgpu::LoadOp<wgpu::Color>,
        encode: F,
    ) where
        F: FnOnce(&mut wgpu::RenderPass<'_>),
    {
        let shared = Arc::clone(&self.shared);
        let backbuffer = shared.backbuffer();
        let mut pass = self.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &backbuffer,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        encode(&mut pass);
    }

    /// Finish recording and store the command buffer in the recorder's slot.
    pub fn finish(self) -> LeetResult<()> {
        self.shared
            .command_lists()
            .set_command_list(self.slot, self.encoder.finish())
    }
}
