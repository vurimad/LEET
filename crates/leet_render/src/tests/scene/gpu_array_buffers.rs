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
