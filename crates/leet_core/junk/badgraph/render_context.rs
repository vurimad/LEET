//! Per-frame rendering context and recording helpers.

use super::frame_command_lists::{FrameCommandListIndex, FrameCommandLists};
use leet_core::LeetResult;
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
pub struct RenderContext<'device> {
    shared: Arc<RenderFrameShared<'device>>,
    surface_frame: wgpu::SurfaceTexture,
}

impl<'device> RenderContext<'device> {
    pub fn new(
        device: &'device wgpu::Device,
        queue: &'device wgpu::Queue,
        surface_frame: wgpu::SurfaceTexture,
        backbuffer: wgpu::TextureView,
        size: (u32, u32),
        surface_format: wgpu::TextureFormat,
        command_list_count: usize,
    ) -> Self {
        Self {
            shared: Arc::new(RenderFrameShared {
                device,
                queue,
                frame_index: 0,
                size,
                surface_format,
                backbuffer,
                command_lists: FrameCommandLists::prepare_for_frame(command_list_count),
            }),
            surface_frame,
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        self.shared.device()
    }

    pub fn queue(&self) -> &wgpu::Queue {
        self.shared.queue()
    }

    pub fn frame_index(&self) -> u64 {
        self.shared.frame_index()
    }

    pub fn size(&self) -> (u32, u32) {
        self.shared.size()
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.shared.surface_format()
    }

    pub fn backbuffer(&self) -> &wgpu::TextureView {
        self.shared.backbuffer()
    }

    pub fn command_lists(&self) -> &FrameCommandLists {
        self.shared.command_lists()
    }

    pub fn begin_recording(
        &self,
        slot: FrameCommandListIndex,
        label: Option<&str>,
    ) -> LeetResult<NodeRecordContext<'device>> {
        self.command_lists().validate_slot(slot)?;
        Ok(NodeRecordContext::new(self.shared.clone(), slot, label))
    }

    pub fn submit_command_lists(
        &self,
        scope_name: &str,
        up_to_index: FrameCommandListIndex,
    ) -> LeetResult<()> {
        self.command_lists()
            .submit(scope_name, up_to_index, self.queue())
    }

    pub fn present(self) {
        self.surface_frame.present();
    }
}

/// Mutable per-task recording state.
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

    pub fn slot(&self) -> FrameCommandListIndex {
        self.slot
    }

    pub fn device(&self) -> &wgpu::Device {
        self.shared.device()
    }

    pub fn frame_index(&self) -> u64 {
        self.shared.frame_index()
    }

    pub fn size(&self) -> (u32, u32) {
        self.shared.size()
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.shared.surface_format()
    }

    pub fn backbuffer(&self) -> &wgpu::TextureView {
        self.shared.backbuffer()
    }

    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }

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
                view: backbuffer,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        encode(&mut pass);
    }

    pub fn finish(self) -> LeetResult<()> {
        self.shared
            .command_lists()
            .set_command_list(self.slot, self.encoder.finish())
    }
}
