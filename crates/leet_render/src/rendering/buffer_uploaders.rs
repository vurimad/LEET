use crate::{
    render_device_from_wgpu, render_queue_from_wgpu, AtomicPod, BufferUsages, RawArrayBuffer,
    Render, RenderApp, RenderDevice, RenderQueue, RenderSystems, UniformBuffer,
};
use bevy_app::{App, Plugin};
use bevy_ecs::{
    prelude::{Res, ResMut, Resource},
    schedule::IntoScheduleConfigs,
};
use bevy_render::{
    render_resource::{BindGroup, BindGroupLayout, Buffer, ComputePipeline},
    renderer::{RenderDevice as BevyRenderDevice, RenderQueue as BevyRenderQueue},
};
use bytemuck::{Pod, Zeroable};
use std::{
    borrow::Cow,
    mem::size_of,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// The fraction of the buffer that may be changed before we fall back to full
/// reupload.
const SPARSE_UPLOAD_THRESHOLD: f64 = 0.15;

/// The WebGPU limit on the number of workgroups that can be dispatched.
const MAX_WORKGROUPS: u32 = 65535;

/// The size of a single workgroup in the sparse buffer shader.
const SPARSE_BUFFER_UPDATE_WORKGROUP_SIZE: u32 = 256;

/// We round all allocations up to the nearest power of this.
const REALLOCATION_FACTOR: f64 = 1.5;
/// We round all allocations up to the nearest multiple of this.
const REALLOCATION_SIZE_MULTIPLE: usize = 256;

/// The number of dirty-page bits packed into each dirty-page word.
const PAGES_PER_DIRTY_WORD: u32 = 64;

/// Exact LEET-local copy of Bevy's sparse buffer scatter shader.
///
/// For now this is executed from LEET's render `Prepare` stage. Once the real
/// RenderGraph exists, execution should move there so sparse uploads compose
/// naturally with the rest of the graph.
///
/// TODO: Move this WGSL out of this Rust source and into LEET's future shader
/// cache / registry path once that system exists.
const LEET_SPARSE_BUFFER_UPDATE_WGSL: &str = r#"
struct SparseBufferUpdateMetadata {
    element_size: u32,
    updated_page_count: u32,
    page_size_log2: u32,
};

@group(0) @binding(0) var<storage, read_write> dest_buffer: array<u32>;
@group(0) @binding(1) var<storage> src_buffer: array<u32>;
@group(0) @binding(2) var<storage> indices: array<u32>;
@group(0) @binding(3) var<uniform> metadata: SparseBufferUpdateMetadata;

@workgroup_size(256, 1, 1)
@compute
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let invocation_index = global_id.x;
    let total_word_count = (metadata.updated_page_count << metadata.page_size_log2) *
        metadata.element_size;
    if (invocation_index >= total_word_count) {
        return;
    }

    let element_index = invocation_index / metadata.element_size;
    let word_index = invocation_index % metadata.element_size;
    let update_index = element_index >> metadata.page_size_log2;
    let element_index_in_page = element_index & ((1u << metadata.page_size_log2) - 1u);

    let page_index = indices[update_index];
    let dest_index = ((page_index << metadata.page_size_log2) + element_index_in_page) *
        metadata.element_size + word_index;
    if (dest_index >= arrayLength(&dest_buffer)) {
        return;
    }

    let src_index = element_index * metadata.element_size + word_index;
    dest_buffer[dest_index] = src_buffer[src_index];
}
"#;

/// Installs the render-app sparse upload compute infrastructure.
#[derive(Default)]
pub struct BufferUploadPlugin;

impl Plugin for BufferUploadPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        let render_device = {
            let render_device = render_app.world().resource::<RenderDevice>();
            render_device_from_wgpu(&render_device.0)
        };

        render_app
            .insert_resource(SparseBufferUpdatePipeline::new(&render_device))
            .init_resource::<SparseBufferUpdateJobs>()
            .add_systems(
                Render,
                run_sparse_buffer_update_jobs.in_set(RenderSystems::Prepare),
            );
    }
}

/// A pending compute scatter job for one sparse buffer upload.
pub struct SparseBufferUpdateJob {
    bind_group: BindGroup,
    workgroup_count: u32,
    label: Arc<str>,
}

/// Pending sparse buffer updates for the current frame.
#[derive(Resource, Default)]
pub struct SparseBufferUpdateJobs(pub Vec<SparseBufferUpdateJob>);

/// The shared sparse upload pipeline state used by all LEET sparse buffer
/// uploaders.
#[derive(Resource)]
pub struct SparseBufferUpdatePipeline {
    bind_group_layout: Option<BindGroupLayout>,
    compute_pipeline: Option<ComputePipeline>,
}

impl SparseBufferUpdatePipeline {
    pub fn new(render_device: &BevyRenderDevice) -> Self {
        if render_device.limits().max_storage_buffers_per_shader_stage < 3 {
            return Self {
                bind_group_layout: None,
                compute_pipeline: None,
            };
        }

        let bind_group_layout = render_device.create_bind_group_layout(
            "leet sparse buffer update bind group layout",
            &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        );

        let shader_module =
            render_device.create_and_validate_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("leet sparse buffer update shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LEET_SPARSE_BUFFER_UPDATE_WGSL)),
            });

        let pipeline_layout =
            render_device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("leet sparse buffer update pipeline layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });

        let compute_pipeline =
            render_device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("leet sparse buffer update pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader_module,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

        Self {
            bind_group_layout: Some(bind_group_layout),
            compute_pipeline: Some(compute_pipeline),
        }
    }

    pub fn is_supported(&self) -> bool {
        self.bind_group_layout.is_some() && self.compute_pipeline.is_some()
    }
}

pub(crate) fn run_sparse_buffer_update_jobs(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    sparse_buffer_update_pipeline: Res<SparseBufferUpdatePipeline>,
    mut sparse_buffer_update_jobs: ResMut<SparseBufferUpdateJobs>,
) {
    if sparse_buffer_update_jobs.0.is_empty() {
        return;
    }

    let Some(compute_pipeline) = sparse_buffer_update_pipeline.compute_pipeline.as_ref() else {
        sparse_buffer_update_jobs.0.clear();
        return;
    };

    let render_device = render_device_from_wgpu(&render_device.0);
    let render_queue = render_queue_from_wgpu(&render_queue.0);

    let mut command_encoder =
        render_device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("leet sparse buffer update"),
        });

    for sparse_buffer_update_job in sparse_buffer_update_jobs.0.drain(..) {
        let mut sparse_buffer_update_pass =
            command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(&*format!(
                    "leet sparse buffer update ({})",
                    &sparse_buffer_update_job.label
                )),
                timestamp_writes: None,
            });
        sparse_buffer_update_pass.set_pipeline(compute_pipeline);
        sparse_buffer_update_pass.set_bind_group(0, &sparse_buffer_update_job.bind_group, &[]);
        sparse_buffer_update_pass.dispatch_workgroups(
            sparse_buffer_update_job.workgroup_count,
            1,
            1,
        );
    }

    render_queue.submit([command_encoder.finish()]);
}

/// CPU-side metadata that mirrors the payload fed to the sparse update compute
/// shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, bevy_render::render_resource::ShaderType)]
pub struct SparseUploadMetadata {
    /// The size of a single element in 32-bit words.
    pub element_size: u32,
    /// The number of pages that need to be updated.
    pub updated_page_count: u32,
    /// The base-2 logarithm of the page size.
    pub page_size_log2: u32,
    /// Reserved lane to keep the struct 16-byte aligned.
    pub reserved: u32,
}

impl SparseUploadMetadata {
    fn new<T>(page_size_log2: u32) -> Self {
        assert_eq!(size_of::<T>() % 4, 0);
        Self {
            element_size: (size_of::<T>() / 4) as u32,
            updated_page_count: 0,
            page_size_log2,
            reserved: 0,
        }
    }

    fn page_size(&self) -> u32 {
        1 << self.page_size_log2
    }

    fn words_to_update(&self) -> u32 {
        self.updated_page_count * self.page_size() * self.element_size
    }

    fn workgroup_count(&self) -> u32 {
        self.words_to_update()
            .div_ceil(SPARSE_BUFFER_UPDATE_WORKGROUP_SIZE)
    }
}

/// The buffers we use to gather sparse updates before a compute scatter.
pub struct SparseUploadStagingBuffers {
    /// All dirty pages flattened into `u32` words.
    pub source_data: RawArrayBuffer<u32>,
    /// The destination page index for each staged page.
    pub indices: RawArrayBuffer<u32>,
    /// The size of each element in 32-bit words.
    pub element_word_size: u32,
    /// The base-2 logarithm of the page size in elements.
    pub page_size_log2: u32,
}

impl SparseUploadStagingBuffers {
    fn new(label: &str, element_word_size: u32, page_size_log2: u32) -> Self {
        let mut source_data = RawArrayBuffer::new(BufferUsages::COPY_DST | BufferUsages::STORAGE);
        source_data.set_label(Some(&format!("{label} staging buffer")));

        let mut indices = RawArrayBuffer::new(BufferUsages::COPY_DST | BufferUsages::STORAGE);
        indices.set_label(Some(&format!("{label} index buffer")));

        Self {
            source_data,
            indices,
            element_word_size,
            page_size_log2,
        }
    }

    fn page_size(&self) -> usize {
        1 << self.page_size_log2
    }

    fn updated_page_count(&self) -> u32 {
        let element_count = self.source_data.len() / self.element_word_size as usize;
        (element_count / self.page_size()) as u32
    }

    fn clear(&mut self) {
        self.source_data.clear();
        self.indices.clear();
    }

    fn should_perform_full_reupload(&self, changed_page_count: u32, buffer_length: usize) -> bool {
        let total_changed_word_count =
            changed_page_count * self.page_size() as u32 * self.element_word_size;
        if total_changed_word_count > MAX_WORKGROUPS * SPARSE_BUFFER_UPDATE_WORKGROUP_SIZE {
            return true;
        }

        let sparse_upload_fraction =
            changed_page_count as f64 / buffer_length.div_ceil(self.page_size()) as f64;
        sparse_upload_fraction > SPARSE_UPLOAD_THRESHOLD
    }

    fn write_buffers(
        &mut self,
        metadata: &mut SparseUploadMetadata,
        metadata_buffer: &mut UniformBuffer<SparseUploadMetadata>,
        label: &str,
        render_device: &BevyRenderDevice,
        render_queue: &BevyRenderQueue,
    ) {
        metadata.updated_page_count = self.updated_page_count();
        metadata_buffer.set(*metadata);
        metadata_buffer.set_label(Some(&format!("{label} sparse metadata")));
        metadata_buffer.write_buffer(render_device, render_queue);
        self.source_data.write_buffer(render_device, render_queue);
        self.indices.write_buffer(render_device, render_queue);
    }
}

/// Shared GPU upload state used by both thread-safe and non-thread-safe sparse
/// uploaders.
pub struct BufferUploadBase<T> {
    data_buffer: Option<Buffer>,
    metadata_buffer: UniformBuffer<SparseUploadMetadata>,
    staging_buffers: SparseUploadStagingBuffers,
    metadata: SparseUploadMetadata,
    capacity: usize,
    buffer_usages: BufferUsages,
    label: Arc<str>,
    needs_full_reupload: bool,
    sparse_upload_scheduled: bool,
    _marker: std::marker::PhantomData<T>,
}

impl<T> BufferUploadBase<T> {
    fn new(buffer_usages: BufferUsages, page_size_log2: u32, label: Arc<str>) -> Self {
        assert_eq!(size_of::<T>() % 4, 0);
        let element_word_size = (size_of::<T>() / 4) as u32;

        Self {
            data_buffer: None,
            metadata_buffer: {
                let mut buffer =
                    UniformBuffer::from(SparseUploadMetadata::new::<T>(page_size_log2));
                buffer.set_label(Some(&format!("{label} sparse metadata")));
                buffer
            },
            staging_buffers: SparseUploadStagingBuffers::new(
                &label,
                element_word_size,
                page_size_log2,
            ),
            metadata: SparseUploadMetadata::new::<T>(page_size_log2),
            capacity: 0,
            buffer_usages: buffer_usages | BufferUsages::COPY_DST,
            label,
            needs_full_reupload: false,
            sparse_upload_scheduled: false,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        self.data_buffer.as_ref()
    }

    pub fn metadata_buffer(&self) -> Option<&Buffer> {
        self.metadata_buffer.buffer()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn metadata(&self) -> SparseUploadMetadata {
        self.metadata
    }

    pub fn sparse_upload_scheduled(&self) -> bool {
        self.sparse_upload_scheduled
    }

    pub fn page_size(&self) -> u32 {
        self.metadata.page_size()
    }

    fn reserve(&mut self, new_capacity: usize, render_device: &BevyRenderDevice) {
        reserve_buffer(
            new_capacity,
            &mut self.capacity,
            &self.label,
            &mut self.data_buffer,
            self.buffer_usages,
            &mut self.needs_full_reupload,
            size_of::<T>(),
            render_device,
        );
    }

    fn should_perform_full_reupload(
        &self,
        changed_page_count: u32,
        buffer_length: usize,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) -> bool {
        if self.needs_full_reupload || !sparse_buffer_update_pipeline.is_supported() {
            return true;
        }

        self.staging_buffers
            .should_perform_full_reupload(changed_page_count, buffer_length)
    }

    fn finish_full_reupload(&mut self) {
        self.needs_full_reupload = false;
        self.sparse_upload_scheduled = false;
        self.staging_buffers.clear();
        self.metadata.updated_page_count = 0;
    }

    fn mark_sparse_upload_scheduled(&mut self) {
        self.sparse_upload_scheduled = self.metadata.updated_page_count != 0;
    }

    fn finish_sparse_job_preparation(&mut self) {
        self.staging_buffers.clear();
        self.needs_full_reupload = false;
        self.sparse_upload_scheduled = false;
        self.metadata.updated_page_count = 0;
    }

    fn prepare_sparse_update_job(
        &mut self,
        render_device: &BevyRenderDevice,
        sparse_buffer_update_jobs: &mut SparseBufferUpdateJobs,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) {
        if !self.sparse_upload_scheduled {
            return;
        }

        let (
            Some(data_buffer),
            Some(metadata_buffer),
            Some(source_data_staging_buffer),
            Some(indices_staging_buffer),
            Some(bind_group_layout),
        ) = (
            self.data_buffer.as_ref(),
            self.metadata_buffer.buffer(),
            self.staging_buffers.source_data.buffer(),
            self.staging_buffers.indices.buffer(),
            sparse_buffer_update_pipeline.bind_group_layout.as_ref(),
        )
        else {
            self.finish_sparse_job_preparation();
            return;
        };

        let entries = [
            wgpu::BindGroupEntry {
                binding: 0,
                resource: data_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: source_data_staging_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: indices_staging_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: metadata_buffer.as_entire_binding(),
            },
        ];
        let bind_group = render_device.create_bind_group(
            Some(&*format!("{} sparse upload bind group", self.label)),
            bind_group_layout,
            &entries,
        );

        sparse_buffer_update_jobs.0.push(SparseBufferUpdateJob {
            bind_group,
            workgroup_count: self.metadata.workgroup_count(),
            label: self.label.clone(),
        });

        self.finish_sparse_job_preparation();
    }
}

/// A sparse GPU uploader for plain CPU-owned POD values.
pub struct BufferUploader<T>
where
    T: Pod + Default + Send + Sync + 'static,
{
    base: BufferUploadBase<T>,
    values: Vec<T>,
    dirty_pages: Vec<u64>,
}

impl<T> BufferUploader<T>
where
    T: Pod + Default + Send + Sync + 'static,
{
    pub fn new(buffer_usages: BufferUsages, page_size_log2: u32, label: Arc<str>) -> Self {
        Self {
            base: BufferUploadBase::new(buffer_usages, page_size_log2, label),
            values: Vec::new(),
            dirty_pages: Vec::new(),
        }
    }

    pub fn len(&self) -> u32 {
        self.values.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        self.base.buffer()
    }

    pub fn metadata_buffer(&self) -> Option<&Buffer> {
        self.base.metadata_buffer()
    }

    pub fn capacity(&self) -> usize {
        self.base.capacity()
    }

    pub fn get(&self, index: u32) -> T {
        self.values[index as usize]
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub fn set(&mut self, index: u32, value: T) {
        self.values[index as usize] = value;
        self.note_changed_index(index);
    }

    pub fn push(&mut self, value: T) -> u32 {
        let index = self.values.len() as u32;
        self.values.push(value);

        let page_word = (self.index_to_page(index) / PAGES_PER_DIRTY_WORD) as usize;
        while self.dirty_pages.len() < page_word + 1 {
            self.dirty_pages.push(0);
        }
        self.note_changed_index(index);
        index
    }

    fn note_changed_index(&mut self, index: u32) {
        let page = self.index_to_page(index);
        let (page_word, page_in_word) = (page / PAGES_PER_DIRTY_WORD, page % PAGES_PER_DIRTY_WORD);
        self.dirty_pages[page_word as usize] |= 1 << page_in_word;
    }

    fn index_to_page(&self, index: u32) -> u32 {
        index / self.base.page_size()
    }

    pub fn reserve(&mut self, new_capacity: usize, render_device: &BevyRenderDevice) {
        self.base.reserve(new_capacity, render_device);
    }

    pub fn grow(&mut self, new_len: u32) {
        let old_len = self.values.len() as u32;
        if old_len >= new_len {
            return;
        }

        self.values.reserve(new_len as usize - old_len as usize);
        self.values.resize_with(new_len as usize, T::default);

        let old_final_page = self.index_to_page(old_len);
        let old_final_page_word_index = old_final_page / PAGES_PER_DIRTY_WORD;
        let old_final_page_in_word = old_final_page % PAGES_PER_DIRTY_WORD;

        if old_final_page_in_word != 0 {
            if let Some(old_final_page_word) =
                self.dirty_pages.get_mut(old_final_page_word_index as usize)
            {
                *old_final_page_word |= !((1u64 << old_final_page_in_word) - 1);
            }
        }

        let new_page_count = new_len.div_ceil(self.base.page_size());
        self.dirty_pages.resize(
            (new_page_count as usize).div_ceil(PAGES_PER_DIRTY_WORD as usize),
            u64::MAX,
        );
    }

    pub fn truncate(&mut self, len: u32) {
        self.values.truncate(len as usize);

        let page = len.div_ceil(self.base.page_size());
        self.dirty_pages
            .truncate(page.div_ceil(PAGES_PER_DIRTY_WORD) as usize);
    }

    pub fn write_buffers(
        &mut self,
        render_device: &BevyRenderDevice,
        render_queue: &BevyRenderQueue,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) {
        if self.values.is_empty() {
            return;
        }

        let good_size = calculate_allocation_size(self.values.len());
        self.reserve(good_size, render_device);

        let changed_page_count: u32 = self
            .dirty_pages
            .iter()
            .map(|page_word| page_word.count_ones())
            .sum();

        if self.base.should_perform_full_reupload(
            changed_page_count,
            self.values.len(),
            sparse_buffer_update_pipeline,
        ) {
            self.write_entire_buffer(render_queue);
        } else {
            self.prepare_sparse_upload(render_device, render_queue);
        }
    }

    pub fn prepare_to_populate_buffers(
        &mut self,
        render_device: &BevyRenderDevice,
        sparse_buffer_update_jobs: &mut SparseBufferUpdateJobs,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) {
        self.base.prepare_sparse_update_job(
            render_device,
            sparse_buffer_update_jobs,
            sparse_buffer_update_pipeline,
        );
    }

    pub fn write_and_prepare_buffers(
        &mut self,
        render_device: &BevyRenderDevice,
        render_queue: &BevyRenderQueue,
        sparse_buffer_update_jobs: &mut SparseBufferUpdateJobs,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) {
        self.write_buffers(render_device, render_queue, sparse_buffer_update_pipeline);
        self.prepare_to_populate_buffers(
            render_device,
            sparse_buffer_update_jobs,
            sparse_buffer_update_pipeline,
        );
    }

    fn write_entire_buffer(&mut self, render_queue: &BevyRenderQueue) {
        let Some(ref data_buffer) = self.base.data_buffer else {
            return;
        };

        render_queue.write_buffer(data_buffer, 0, bytemuck::cast_slice(self.values.as_slice()));

        for page_word in &mut self.dirty_pages {
            *page_word = 0;
        }
        self.base.finish_full_reupload();
    }

    fn prepare_sparse_upload(
        &mut self,
        render_device: &BevyRenderDevice,
        render_queue: &BevyRenderQueue,
    ) {
        for (page_word_index, page_word) in self.dirty_pages.iter_mut().enumerate() {
            let dirty_bits = *page_word;
            for page_index_in_word in BitIter::new(dirty_bits) {
                let page = page_word_index as u32 * PAGES_PER_DIRTY_WORD + page_index_in_word;
                self.base.staging_buffers.indices.push(page);

                let page_size = self.base.staging_buffers.page_size();
                let page_start = page as usize * page_size;
                let page_end = page_start + page_size;
                for value_index in page_start..page_end {
                    if let Some(value) = self.values.get(value_index) {
                        self.base
                            .staging_buffers
                            .source_data
                            .extend(bytemuck::cast_slice(&[*value]).iter().copied());
                    } else {
                        self.base
                            .staging_buffers
                            .source_data
                            .extend(std::iter::repeat_n(
                                0,
                                self.base.staging_buffers.element_word_size as usize,
                            ));
                    }
                }
            }

            *page_word = 0;
        }

        if self.base.staging_buffers.source_data.is_empty() {
            self.base.finish_full_reupload();
            return;
        }

        self.base.staging_buffers.write_buffers(
            &mut self.base.metadata,
            &mut self.base.metadata_buffer,
            &self.base.label,
            render_device,
            render_queue,
        );
        self.base.mark_sparse_upload_scheduled();
    }
}

/// A sparse GPU uploader for atomically updatable POD values.
pub struct AtomicBufferUploader<T>
where
    T: AtomicPod,
{
    base: BufferUploadBase<T>,
    values: Vec<T::Blob>,
    dirty_pages: Vec<AtomicU64>,
}

impl<T> AtomicBufferUploader<T>
where
    T: AtomicPod,
{
    pub fn new(buffer_usages: BufferUsages, page_size_log2: u32, label: Arc<str>) -> Self {
        Self {
            base: BufferUploadBase::new(buffer_usages, page_size_log2, label),
            values: vec![],
            dirty_pages: vec![],
        }
    }

    pub fn len(&self) -> u32 {
        self.values.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        self.base.buffer()
    }

    pub fn metadata_buffer(&self) -> Option<&Buffer> {
        self.base.metadata_buffer()
    }

    pub fn capacity(&self) -> usize {
        self.base.capacity()
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub fn get(&self, index: u32) -> T {
        T::read_from_blob(&self.values[index as usize])
    }

    pub fn set(&self, index: u32, value: T) {
        value.write_to_blob(&self.values[index as usize]);
        self.note_changed_index(index);
    }

    pub fn push(&mut self, value: T) -> u32 {
        let index = self.values.len() as u32;
        self.values.push(T::Blob::default());
        value.write_to_blob(&self.values[index as usize]);

        let page_word = (self.index_to_page(index) / PAGES_PER_DIRTY_WORD) as usize;
        while self.dirty_pages.len() < page_word + 1 {
            self.dirty_pages.push(AtomicU64::default());
        }
        self.note_changed_index(index);
        index
    }

    fn note_changed_index(&self, index: u32) {
        let page = self.index_to_page(index);
        let (page_word, page_in_word) = (page / PAGES_PER_DIRTY_WORD, page % PAGES_PER_DIRTY_WORD);
        self.dirty_pages[page_word as usize].fetch_or(1 << page_in_word, Ordering::Relaxed);
    }

    fn index_to_page(&self, index: u32) -> u32 {
        index / self.base.page_size()
    }

    pub fn reserve(&mut self, new_capacity: usize, render_device: &BevyRenderDevice) {
        self.base.reserve(new_capacity, render_device);
    }

    pub fn grow(&mut self, new_len: u32) {
        let old_len = self.values.len() as u32;
        if old_len >= new_len {
            return;
        }

        self.values.reserve(new_len as usize - old_len as usize);
        self.values.resize_with(new_len as usize, T::Blob::default);

        let old_final_page = self.index_to_page(old_len);
        let old_final_page_word_index = old_final_page / PAGES_PER_DIRTY_WORD;
        let old_final_page_in_word = old_final_page % PAGES_PER_DIRTY_WORD;

        if old_final_page_in_word != 0 {
            if let Some(old_final_page_word) =
                self.dirty_pages.get_mut(old_final_page_word_index as usize)
            {
                *old_final_page_word.get_mut() |= !((1u64 << old_final_page_in_word) - 1);
            }
        }

        let new_page_count = new_len.div_ceil(self.base.page_size());
        self.dirty_pages.resize_with(
            (new_page_count as usize).div_ceil(PAGES_PER_DIRTY_WORD as usize),
            || AtomicU64::new(u64::MAX),
        );
    }

    pub fn truncate(&mut self, len: u32) {
        self.values.truncate(len as usize);

        let page = len.div_ceil(self.base.page_size());
        self.dirty_pages
            .truncate(page.div_ceil(PAGES_PER_DIRTY_WORD) as usize);
    }

    pub fn write_buffers(
        &mut self,
        render_device: &BevyRenderDevice,
        render_queue: &BevyRenderQueue,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) {
        if self.values.is_empty() {
            return;
        }

        let good_size = calculate_allocation_size(self.values.len());
        self.reserve(good_size, render_device);

        let changed_page_count: u32 = self
            .dirty_pages
            .iter()
            .map(|page_word| page_word.load(Ordering::Relaxed).count_ones())
            .sum();

        if self.base.should_perform_full_reupload(
            changed_page_count,
            self.values.len(),
            sparse_buffer_update_pipeline,
        ) {
            self.write_entire_buffer(render_queue);
        } else {
            self.prepare_sparse_upload(render_device, render_queue);
        }
    }

    pub fn prepare_to_populate_buffers(
        &mut self,
        render_device: &BevyRenderDevice,
        sparse_buffer_update_jobs: &mut SparseBufferUpdateJobs,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) {
        self.base.prepare_sparse_update_job(
            render_device,
            sparse_buffer_update_jobs,
            sparse_buffer_update_pipeline,
        );
    }

    pub fn write_and_prepare_buffers(
        &mut self,
        render_device: &BevyRenderDevice,
        render_queue: &BevyRenderQueue,
        sparse_buffer_update_jobs: &mut SparseBufferUpdateJobs,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) {
        self.write_buffers(render_device, render_queue, sparse_buffer_update_pipeline);
        self.prepare_to_populate_buffers(
            render_device,
            sparse_buffer_update_jobs,
            sparse_buffer_update_pipeline,
        );
    }

    fn write_entire_buffer(&mut self, render_queue: &BevyRenderQueue) {
        let Some(ref data_buffer) = self.base.data_buffer else {
            return;
        };

        unsafe {
            render_queue.write_buffer(
                data_buffer,
                0,
                std::slice::from_raw_parts(
                    self.values.as_ptr().cast::<u8>(),
                    self.values.len() * size_of::<T::Blob>(),
                ),
            );
        }

        for page_word in &self.dirty_pages {
            page_word.store(0, Ordering::Relaxed);
        }
        self.base.finish_full_reupload();
    }

    fn prepare_sparse_upload(
        &mut self,
        render_device: &BevyRenderDevice,
        render_queue: &BevyRenderQueue,
    ) {
        for (page_word_index, page_word) in self.dirty_pages.iter().enumerate() {
            let dirty_bits = page_word.load(Ordering::Relaxed);
            for page_index_in_word in BitIter::new(dirty_bits) {
                let page = page_word_index as u32 * PAGES_PER_DIRTY_WORD + page_index_in_word;
                self.base.staging_buffers.indices.push(page);

                let page_size = self.base.staging_buffers.page_size();
                let page_start = page as usize * page_size;
                let page_end = page_start + page_size;
                for value_index in page_start..page_end {
                    match self.values.get(value_index) {
                        Some(blob) => {
                            let value = T::read_from_blob(blob);
                            self.base
                                .staging_buffers
                                .source_data
                                .extend(bytemuck::cast_slice(&[value]).iter().copied());
                        }
                        None => {
                            self.base
                                .staging_buffers
                                .source_data
                                .extend(std::iter::repeat_n(
                                    0,
                                    self.base.staging_buffers.element_word_size as usize,
                                ));
                        }
                    }
                }
            }

            page_word.store(0, Ordering::Relaxed);
        }

        if self.base.staging_buffers.source_data.is_empty() {
            self.base.finish_full_reupload();
            return;
        }

        self.base.staging_buffers.write_buffers(
            &mut self.base.metadata,
            &mut self.base.metadata_buffer,
            &self.base.label,
            render_device,
            render_queue,
        );
        self.base.mark_sparse_upload_scheduled();
    }
}

/// Cheap bridge from LEET's raw `wgpu` ownership model into the Bevy-backed
/// helpers used by the uploaders.
pub fn render_resources_from_wgpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (BevyRenderDevice, BevyRenderQueue) {
    (
        render_device_from_wgpu(device),
        render_queue_from_wgpu(queue),
    )
}

fn reserve_buffer(
    new_capacity: usize,
    capacity: &mut usize,
    label: &str,
    data_buffer: &mut Option<Buffer>,
    buffer_usages: BufferUsages,
    needs_full_reupload: &mut bool,
    element_size: usize,
    render_device: &BevyRenderDevice,
) {
    if new_capacity == 0 || new_capacity <= *capacity {
        return;
    }

    *capacity = new_capacity;
    *data_buffer = Some(render_device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: element_size as u64 * new_capacity as u64,
        usage: buffer_usages,
        mapped_at_creation: false,
    }));
    *needs_full_reupload = true;
}

struct BitIter(u64);

impl BitIter {
    fn new(bits: u64) -> Self {
        Self(bits)
    }
}

impl Iterator for BitIter {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        let trailing_zeros = self.0.trailing_zeros();
        if trailing_zeros == 64 {
            return None;
        }
        self.0 &= !(1 << trailing_zeros);
        Some(trailing_zeros)
    }
}

fn calculate_allocation_size(length: usize) -> usize {
    let exponent = (length as f64).log(REALLOCATION_FACTOR).ceil();
    let size = REALLOCATION_FACTOR.powf(exponent) as usize;
    size.next_multiple_of(REALLOCATION_SIZE_MULTIPLE)
}

#[cfg(test)]
#[path = "../tests/rendering/buffer_uploaders.rs"]
mod tests;
