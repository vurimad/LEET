use super::*;
use std::sync::atomic::AtomicU32;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Pod, Zeroable)]
struct TestValue {
    a: u32,
    b: u32,
}

#[derive(Default)]
#[repr(transparent)]
struct TestValueBlob([AtomicU32; 2]);

impl AtomicPod for TestValue {
    type Blob = TestValueBlob;

    fn read_from_blob(blob: &Self::Blob) -> Self {
        Self {
            a: blob.0[0].load(Ordering::Relaxed),
            b: blob.0[1].load(Ordering::Relaxed),
        }
    }

    fn write_to_blob(&self, blob: &Self::Blob) {
        blob.0[0].store(self.a, Ordering::Relaxed);
        blob.0[1].store(self.b, Ordering::Relaxed);
    }
}

unsafe impl bevy_render::render_resource::AtomicPodBlob for TestValueBlob {}

#[test]
fn direct_uploader_tracks_sparse_dirty_pages() {
    let mut uploader = BufferUploader::<TestValue>::new(
        BufferUsages::STORAGE,
        2,
        Arc::<str>::from("test uploader"),
    );

    uploader.push(TestValue { a: 1, b: 2 });
    uploader.push(TestValue { a: 3, b: 4 });
    uploader.push(TestValue { a: 5, b: 6 });
    uploader.push(TestValue { a: 7, b: 8 });
    uploader.push(TestValue { a: 9, b: 10 });

    assert_eq!(uploader.dirty_pages.len(), 1);
    assert_ne!(uploader.dirty_pages[0], 0);
}

#[test]
fn atomic_uploader_reads_and_writes_values() {
    let mut uploader = AtomicBufferUploader::<TestValue>::new(
        BufferUsages::STORAGE,
        2,
        Arc::<str>::from("test uploader"),
    );

    let index = uploader.push(TestValue { a: 7, b: 11 });
    assert_eq!(uploader.get(index), TestValue { a: 7, b: 11 });

    uploader.set(index, TestValue { a: 13, b: 17 });
    assert_eq!(uploader.get(index), TestValue { a: 13, b: 17 });
}

#[test]
fn metadata_workgroup_count_matches_total_words() {
    let mut metadata = SparseUploadMetadata::new::<TestValue>(2);
    metadata.updated_page_count = 3;

    let expected_words = 3 * (1 << 2) * 2;
    assert_eq!(metadata.words_to_update(), expected_words);
    assert_eq!(
        metadata.workgroup_count(),
        expected_words.div_ceil(SPARSE_BUFFER_UPDATE_WORKGROUP_SIZE)
    );
}

#[test]
fn allocation_size_rounds_up() {
    assert_eq!(calculate_allocation_size(1), 256);
    assert!(calculate_allocation_size(1000) >= 1000);
}
