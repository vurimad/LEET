use bevy_render::{
    render_resource::{
        AtomicRawBufferVec, DynamicStorageBuffer, RawBufferVec, StorageBuffer, UninitBufferVec,
    },
    renderer::{RenderDevice as BevyRenderDevice, RenderQueue as BevyRenderQueue, WgpuWrapper},
};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

/// LEET alias for Bevy's CPU-owned raw GPU array helper.
///
/// This is a good fit when the CPU already owns the final byte layout and we
/// want direct indexed access plus optional range uploads.
pub type RawArrayBuffer<T> = RawBufferVec<T>;

/// LEET alias for Bevy's appendable atomic raw buffer.
///
/// This is useful for append/reset patterns where multiple threads may fill an
/// already-reserved backing buffer.
pub type AtomicAppendBuffer<T> = AtomicRawBufferVec<T>;

/// LEET alias for Bevy's uninitialized GPU-only buffer helper.
///
/// This keeps only GPU allocation state plus logical length/capacity metadata.
/// It does not keep a CPU-side mirror of `T` values.
///
/// This is the right shape for buffers whose contents are written by GPU
/// compute passes rather than fully authored on the CPU.
pub type GpuOnlyBuffer<T> = UninitBufferVec<T>;

/// Backwards-compatible alias while naming settles.
pub type GpuOutputArrayBuffer<T> = GpuOnlyBuffer<T>;

/// LEET alias for Bevy's structured storage buffer helper.
///
/// Prefer this for small or medium structured blocks that are naturally
/// serialized as one value or one compact aggregate.
pub type StructuredStorageBuffer<T> = StorageBuffer<T>;

/// LEET alias for Bevy's dynamic structured storage buffer helper.
pub type DynamicStructuredStorageBuffer<T> = DynamicStorageBuffer<T>;

/// LEET alias for Bevy's structured uniform buffer helper.
pub use bevy_render::render_resource::{
    AtomicPod, Buffer, BufferUsages, GpuArrayBufferable, ShaderSize, ShaderType, UniformBuffer,
    WriteBufferRangeError,
};

/// Cheap bridge from the current LEET raw `wgpu::Device` ownership model to
/// Bevy's render-resource helpers.
pub fn render_device_from_wgpu(device: &wgpu::Device) -> BevyRenderDevice {
    BevyRenderDevice::from(device.clone())
}

/// Cheap bridge from the current LEET raw `wgpu::Queue` ownership model to
/// Bevy's render-resource helpers.
pub fn render_queue_from_wgpu(queue: &wgpu::Queue) -> BevyRenderQueue {
    BevyRenderQueue(Arc::new(WgpuWrapper::new(queue.clone())))
}

/// Append-only per-frame buffer.
///
/// This is a good fit for per-frame append/reset patterns where worker threads
/// fill a pre-reserved backing store and only the used prefix is uploaded.
///
/// Typical uses include:
/// - previous-frame instance inputs
/// - per-view work lists
/// - transient preprocess job streams
pub struct FrameAppendBuffer<T>
where
    T: AtomicPod,
{
    buffer: AtomicAppendBuffer<T>,
    atomic_len: AtomicU32,
}

impl<T> FrameAppendBuffer<T>
where
    T: AtomicPod,
{
    /// Creates a new append-only frame buffer using storage-buffer usage.
    pub fn new() -> Self {
        Self::with_label("leet frame append buffer")
    }

    /// Creates a new previous-instance buffer with a custom debug label.
    pub fn with_label(label: &str) -> Self {
        Self {
            buffer: AtomicAppendBuffer::with_label(BufferUsages::STORAGE, label),
            atomic_len: AtomicU32::new(0),
        }
    }

    /// Returns the GPU buffer if it has been allocated already.
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.buffer()
    }

    /// Returns the number of elements appended since the last reserve/clear.
    pub fn len(&self) -> u32 {
        self.atomic_len.load(Ordering::Relaxed)
    }

    /// Returns true if no elements have been appended this frame.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clears logical contents for a new frame without throwing away capacity.
    pub fn clear(&mut self) {
        self.atomic_len.store(0, Ordering::Relaxed);
    }

    /// Ensures the backing buffer can accept at least `capacity` pushes.
    pub fn reserve(&mut self, capacity: u32) {
        self.buffer.grow(capacity);
        *self.atomic_len.get_mut() = 0;
    }

    /// Thread-safe append. `reserve()` must have provided enough capacity.
    pub fn push(&self, value: T) -> u32 {
        let index = self.atomic_len.fetch_add(1, Ordering::Relaxed);
        debug_assert!(
            index < self.buffer.len(),
            "FrameAppendBuffer overflowed its reserved capacity"
        );
        self.buffer.set(index, value);
        index
    }

    /// Guarantees that at least one element exists so a storage buffer binding
    /// can remain valid even when nothing wrote previous-frame data.
    pub fn ensure_nonempty(&mut self) {
        if self.buffer.is_empty() {
            self.buffer.push(T::default());
        }
    }

    /// Upload only the used prefix to the GPU.
    pub fn write_buffer(
        &mut self,
        render_device: &BevyRenderDevice,
        render_queue: &BevyRenderQueue,
    ) {
        let used_len = self.len().max(1) as usize;
        self.buffer
            .write_buffer_range(0..used_len, render_device, render_queue);
    }
}

impl<T> Default for FrameAppendBuffer<T>
where
    T: AtomicPod,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::{Pod, Zeroable};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Pod, Zeroable)]
    struct TestAtomicInput {
        value: u32,
        tag: u32,
    }

    #[derive(Default)]
    #[repr(transparent)]
    struct TestAtomicInputBlob([AtomicU32; 2]);

    impl AtomicPod for TestAtomicInput {
        type Blob = TestAtomicInputBlob;

        fn read_from_blob(blob: &Self::Blob) -> Self {
            Self {
                value: blob.0[0].load(Ordering::Relaxed),
                tag: blob.0[1].load(Ordering::Relaxed),
            }
        }

        fn write_to_blob(&self, blob: &Self::Blob) {
            blob.0[0].store(self.value, Ordering::Relaxed);
            blob.0[1].store(self.tag, Ordering::Relaxed);
        }
    }

    unsafe impl bevy_render::render_resource::AtomicPodBlob for TestAtomicInputBlob {}

    #[test]
    fn frame_append_buffer_tracks_used_prefix() {
        let mut buffer = FrameAppendBuffer::<TestAtomicInput>::new();
        buffer.reserve(4);

        assert_eq!(buffer.push(TestAtomicInput { value: 3, tag: 7 }), 0);
        assert_eq!(buffer.push(TestAtomicInput { value: 5, tag: 9 }), 1);
        assert_eq!(buffer.len(), 2);

        buffer.clear();
        assert_eq!(buffer.len(), 0);

        buffer.reserve(2);
        assert_eq!(buffer.push(TestAtomicInput { value: 11, tag: 13 }), 0);
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn ensure_nonempty_seeds_backing_storage() {
        let mut buffer = FrameAppendBuffer::<TestAtomicInput>::new();
        buffer.ensure_nonempty();
        assert!(!buffer.buffer.is_empty());
        assert_eq!(buffer.len(), 0);
    }
}
