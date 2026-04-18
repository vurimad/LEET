//! Persistent GPU-side scene data.
//!
//! This layer bridges renderer-owned CPU scene state into durable GPU buffers.
//! The important rule is that transient sync work should move indices around,
//! not copy whole instance payload batches. `SceneGpuState` owns the persistent
//! CPU mirror of GPU instance data, and frame sync updates only the dirty
//! entries of that mirror.

use crate::render_proxy::{RenderProxy, RenderProxyKind};
use crate::render_scene::{RenderSceneGpuSyncRequest, RenderSceneId, RenderSceneProxy};
use leet_core::LeetResult;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GpuInstanceData {
    local_to_world: [[f32; 4]; 4],
    debug_color: [f32; 4],
    metadata: [u32; 4],
}

impl GpuInstanceData {
    pub const ZERO: Self = Self {
        local_to_world: [[0.0; 4]; 4],
        debug_color: [0.0; 4],
        metadata: [0; 4],
    };

    pub fn from_proxy(proxy: &RenderProxy) -> Self {
        let local_to_world = proxy.local_to_world().to_cols_array_2d();
        let color = proxy.debug_color();

        Self {
            local_to_world,
            debug_color: [
                color.r as f32,
                color.g as f32,
                color.b as f32,
                color.a as f32,
            ],
            metadata: [
                u32::from(proxy.is_visible()),
                match proxy.kind() {
                    RenderProxyKind::Opaque => 0,
                    RenderProxyKind::Sky => 1,
                },
                0,
                0,
            ],
        }
    }

    pub const fn stride() -> u64 {
        std::mem::size_of::<Self>() as u64
    }

    #[cfg(test)]
    pub fn local_to_world(&self) -> [[f32; 4]; 4] {
        self.local_to_world
    }

    #[cfg(test)]
    pub fn visible(&self) -> bool {
        self.metadata[0] != 0
    }
}

#[derive(Debug, Default)]
pub(crate) struct SceneGpuState {
    instance_buffer: Option<wgpu::Buffer>,
    slot_capacity: usize,
    live_instance_count: usize,
    cpu_slot_image: Vec<GpuInstanceData>,
}

impl SceneGpuState {
    pub fn sync_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &RenderSceneProxy,
    ) -> LeetResult<()> {
        let required_slot_capacity = scene.gpu_slot_capacity()?;
        let requires_full_upload =
            self.instance_buffer.is_none() || self.slot_capacity < required_slot_capacity;
        let sync_request = scene.take_gpu_sync_request(requires_full_upload)?;

        if self.slot_capacity < sync_request.required_slot_capacity()
            || self.instance_buffer.is_none()
        {
            self.recreate_buffer(
                device,
                scene.scene_id(),
                sync_request.required_slot_capacity(),
            );
        }

        self.ensure_cpu_slot_capacity(sync_request.required_slot_capacity());
        self.refresh_cpu_slot_image(scene, &sync_request)?;
        self.upload_sync_request(queue, &sync_request);
        self.live_instance_count = sync_request.live_instance_count();

        Ok(())
    }

    pub fn instance_buffer(&self) -> Option<&wgpu::Buffer> {
        self.instance_buffer.as_ref()
    }

    pub fn slot_capacity(&self) -> usize {
        self.slot_capacity
    }

    pub fn live_instance_count(&self) -> usize {
        self.live_instance_count
    }

    fn recreate_buffer(
        &mut self,
        device: &wgpu::Device,
        scene_id: RenderSceneId,
        slot_capacity: usize,
    ) {
        if slot_capacity == 0 {
            self.instance_buffer = None;
            self.slot_capacity = 0;
            self.cpu_slot_image.clear();
            return;
        }

        let size = slot_capacity as u64 * GpuInstanceData::stride();
        self.instance_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!(
                "LEET Scene Instance Buffer scene={}",
                scene_id.get()
            )),
            size,
            usage: wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        }));
        self.slot_capacity = slot_capacity;
    }

    fn ensure_cpu_slot_capacity(&mut self, slot_capacity: usize) {
        if self.cpu_slot_image.len() < slot_capacity {
            self.cpu_slot_image
                .resize(slot_capacity, GpuInstanceData::ZERO);
        }
    }

    fn refresh_cpu_slot_image(
        &mut self,
        scene: &RenderSceneProxy,
        sync_request: &RenderSceneGpuSyncRequest,
    ) -> LeetResult<()> {
        scene.refresh_gpu_slot_image(sync_request.dirty_slots(), &mut self.cpu_slot_image)
    }

    fn upload_sync_request(
        &mut self,
        queue: &wgpu::Queue,
        sync_request: &RenderSceneGpuSyncRequest,
    ) {
        let Some(instance_buffer) = self.instance_buffer.as_ref() else {
            return;
        };

        if sync_request.full_upload() {
            let slot_capacity = sync_request.required_slot_capacity();
            if slot_capacity != 0 {
                queue.write_buffer(
                    instance_buffer,
                    0,
                    cast_slice_to_bytes(&self.cpu_slot_image[..slot_capacity]),
                );
            }
            return;
        }

        for contiguous_range in build_contiguous_upload_ranges(sync_request.dirty_slots()) {
            let end_slot = contiguous_range.end_slot_exclusive;
            queue.write_buffer(
                instance_buffer,
                contiguous_range.start_slot_index as u64 * GpuInstanceData::stride(),
                cast_slice_to_bytes(
                    &self.cpu_slot_image[contiguous_range.start_slot_index..end_slot],
                ),
            );
        }
    }
}

#[derive(Debug)]
struct ContiguousGpuUploadRange {
    start_slot_index: usize,
    end_slot_exclusive: usize,
}

fn build_contiguous_upload_ranges(dirty_slots: &[usize]) -> Vec<ContiguousGpuUploadRange> {
    if dirty_slots.is_empty() {
        return Vec::new();
    }

    let mut sorted_slots = dirty_slots.to_vec();
    sorted_slots.sort_unstable();
    sorted_slots.dedup();

    let mut ranges = Vec::new();
    let mut current_start = sorted_slots[0];
    let mut previous_slot = current_start;

    for slot_index in sorted_slots.into_iter().skip(1) {
        if slot_index != previous_slot + 1 {
            ranges.push(ContiguousGpuUploadRange {
                start_slot_index: current_start,
                end_slot_exclusive: previous_slot + 1,
            });
            current_start = slot_index;
        }
        previous_slot = slot_index;
    }

    ranges.push(ContiguousGpuUploadRange {
        start_slot_index: current_start,
        end_slot_exclusive: previous_slot + 1,
    });

    ranges
}

fn cast_slice_to_bytes<T>(slice: &[T]) -> &[u8] {
    // SAFETY: The returned byte slice is tied to the lifetime of `slice`, and
    // `GpuInstanceData` is a plain `repr(C)` POD-like struct used only for raw
    // GPU uploads. `wgpu` expects byte views here.
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_upload_ranges_merge_adjacent_slots() {
        let ranges = build_contiguous_upload_ranges(&[5, 1, 2, 7, 8, 10]);

        assert_eq!(ranges.len(), 4);
        assert_eq!(ranges[0].start_slot_index, 1);
        assert_eq!(ranges[0].end_slot_exclusive, 3);
        assert_eq!(ranges[1].start_slot_index, 5);
        assert_eq!(ranges[1].end_slot_exclusive, 6);
        assert_eq!(ranges[2].start_slot_index, 7);
        assert_eq!(ranges[2].end_slot_exclusive, 9);
        assert_eq!(ranges[3].start_slot_index, 10);
        assert_eq!(ranges[3].end_slot_exclusive, 11);
    }

    #[test]
    fn contiguous_upload_ranges_handle_empty_input() {
        let ranges = build_contiguous_upload_ranges(&[]);
        assert!(ranges.is_empty());
    }

    #[test]
    fn contiguous_upload_ranges_deduplicate_slots() {
        let ranges = build_contiguous_upload_ranges(&[3, 3, 4, 4, 9]);

        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start_slot_index, 3);
        assert_eq!(ranges[0].end_slot_exclusive, 5);
        assert_eq!(ranges[1].start_slot_index, 9);
        assert_eq!(ranges[1].end_slot_exclusive, 10);
    }
}
