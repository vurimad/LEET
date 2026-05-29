use bytemuck::{Pod, Zeroable};
use pollster::block_on;
use std::{env, error::Error, fmt, mem, sync::mpsc, time::Instant};

const WORKGROUP_SIZE: u32 = 256;

const SHADER: &str = r#"
struct InstancePayload {
    model_0: vec4<f32>,
    model_1: vec4<f32>,
    model_2: vec4<f32>,
    model_3: vec4<f32>,
    prev_0: vec4<f32>,
    prev_1: vec4<f32>,
    prev_2: vec4<f32>,
    prev_3: vec4<f32>,
    packed_meta: vec4<u32>,
};

struct Params {
    count: u32,
    base_offset: u32,
    _pad0: u32,
    _pad1: u32,
};

fn evaluate_instance(instance: InstancePayload) -> vec4<f32> {
    let meta_mix = f32((instance.packed_meta.x ^ instance.packed_meta.y ^ instance.packed_meta.z ^ instance.packed_meta.w) & 1023u);
    return instance.model_0
        + instance.model_1 * 0.5
        + instance.model_2 * 0.25
        + instance.model_3 * 0.125
        + instance.prev_0 * 0.0625
        + instance.prev_1 * 0.03125
        + instance.prev_2 * 0.015625
        + instance.prev_3 * 0.0078125
        + vec4<f32>(meta_mix * 0.0001);
}

@group(0) @binding(0) var<storage, read> direct_instances: array<InstancePayload>;
@group(0) @binding(1) var<storage, read_write> direct_outputs: array<vec4<f32>>;
@group(0) @binding(2) var<uniform> direct_params: Params;

@compute @workgroup_size(256)
fn direct_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let local_index = global_id.x;
    if (local_index >= direct_params.count) {
        return;
    }

    let draw_index = direct_params.base_offset + local_index;
    direct_outputs[draw_index] = evaluate_instance(direct_instances[draw_index]);
}

@group(0) @binding(0) var<storage, read> shared_instances: array<InstancePayload>;
@group(0) @binding(1) var<storage, read> instance_indices: array<u32>;
@group(0) @binding(2) var<storage, read_write> indexed_outputs: array<vec4<f32>>;
@group(0) @binding(3) var<uniform> indexed_params: Params;

@compute @workgroup_size(256)
fn indexed_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let local_index = global_id.x;
    if (local_index >= indexed_params.count) {
        return;
    }

    let draw_index = indexed_params.base_offset + local_index;
    let object_index = instance_indices[draw_index];
    indexed_outputs[draw_index] = evaluate_instance(shared_instances[object_index]);
}
"#;

#[derive(Debug)]
struct BenchError(String);

impl fmt::Display for BenchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for BenchError {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct InstancePayload {
    model: [[f32; 4]; 4],
    previous: [[f32; 4]; 4],
    meta: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ParamsUniform {
    count: u32,
    base_offset: u32,
    padding: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
struct OutputValue {
    value: [f32; 4],
}

#[derive(Clone, Debug)]
struct ScenarioConfig {
    label: String,
    unique_instances: u32,
    phase_count: u32,
    shuffled_indices: bool,
    iterations: u32,
}

impl ScenarioConfig {
    fn draw_count(&self) -> usize {
        self.unique_instances as usize * self.phase_count as usize
    }
}

#[derive(Debug)]
struct ScenarioBuffers {
    direct_output_buffer: wgpu::Buffer,
    indexed_output_buffer: wgpu::Buffer,
    direct_bind_group: wgpu::BindGroup,
    indexed_bind_group: wgpu::BindGroup,
    direct_phase_bind_groups: Vec<wgpu::BindGroup>,
    indexed_phase_bind_groups: Vec<wgpu::BindGroup>,
    direct_separate_phases: Vec<SeparatePhaseResources>,
    indexed_separate_phases: Vec<SeparatePhaseResources>,
    draw_count: u32,
    items_per_phase: u32,
    direct_bytes: u64,
    indexed_bytes: u64,
}

#[derive(Debug)]
struct TimingResult {
    average_ns: f64,
    wall_ms: f64,
    used_timestamps: bool,
}

#[derive(Clone, Copy)]
enum DispatchShape {
    Merged,
    Bounded,
    SeparateBuffers,
}

impl DispatchShape {
    fn label(self) -> &'static str {
        match self {
            Self::Merged => "merged_stream",
            Self::Bounded => "bounded_phases",
            Self::SeparateBuffers => "separate_phase_buffers",
        }
    }
}

#[derive(Debug)]
struct SeparatePhaseResources {
    bind_group: wgpu::BindGroup,
    output_buffer: wgpu::Buffer,
}

struct BenchContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    timestamp_supported: bool,
    direct_bind_group_layout: wgpu::BindGroupLayout,
    indexed_bind_group_layout: wgpu::BindGroupLayout,
    direct_pipeline: wgpu::ComputePipeline,
    indexed_pipeline: wgpu::ComputePipeline,
}

fn main() -> Result<(), Box<dyn Error>> {
    block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn Error>> {
    let scenarios = parse_args(env::args().skip(1).collect())?;
    let context = create_context().await?;

    println!("Adapter: {}", context.device.adapter_info().name);
    println!(
        "Timestamp queries: {}",
        if context.timestamp_supported {
            "enabled"
        } else {
            "not supported, falling back to wall-clock timing"
        }
    );
    println!();

    for scenario in scenarios {
        let buffers = build_scenario_buffers(&context, &scenario)?;
        verify_outputs_match(&context, &buffers)?;
        warm_up(&context, &buffers)?;

        let merged_direct = measure_pipeline(
            &context,
            &buffers,
            PipelineKind::Direct,
            DispatchShape::Merged,
            scenario.iterations,
        )?;
        let merged_indexed = measure_pipeline(
            &context,
            &buffers,
            PipelineKind::Indexed,
            DispatchShape::Merged,
            scenario.iterations,
        )?;
        let bounded_direct = measure_pipeline(
            &context,
            &buffers,
            PipelineKind::Direct,
            DispatchShape::Bounded,
            scenario.iterations,
        )?;
        let bounded_indexed = measure_pipeline(
            &context,
            &buffers,
            PipelineKind::Indexed,
            DispatchShape::Bounded,
            scenario.iterations,
        )?;
        let separate_direct = measure_pipeline(
            &context,
            &buffers,
            PipelineKind::Direct,
            DispatchShape::SeparateBuffers,
            scenario.iterations,
        )?;
        let separate_indexed = measure_pipeline(
            &context,
            &buffers,
            PipelineKind::Indexed,
            DispatchShape::SeparateBuffers,
            scenario.iterations,
        )?;

        print_report(
            &scenario,
            &buffers,
            &merged_direct,
            &merged_indexed,
            &bounded_direct,
            &bounded_indexed,
            &separate_direct,
            &separate_indexed,
        );
    }

    Ok(())
}

async fn create_context() -> Result<BenchContext, Box<dyn Error>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .map_err(|error| BenchError(format!("failed to request adapter: {error}")))?;

    let adapter_features = adapter.features();
    let timestamp_supported = adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY)
        && adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
    let required_features = if timestamp_supported {
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
    } else {
        wgpu::Features::empty()
    };

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("LEET WGPU Microbench Device"),
            required_features,
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|error| BenchError(format!("failed to create device: {error}")))?;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("LEET WGPU Microbench Shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });

    let direct_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Direct Bind Group Layout"),
            entries: &[
                read_only_storage_layout_entry(0),
                storage_layout_entry(1),
                uniform_layout_entry(2),
            ],
        });

    let indexed_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Indexed Bind Group Layout"),
            entries: &[
                read_only_storage_layout_entry(0),
                read_only_storage_layout_entry(1),
                storage_layout_entry(2),
                uniform_layout_entry(3),
            ],
        });

    let direct_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Direct Pipeline Layout"),
        bind_group_layouts: &[Some(&direct_bind_group_layout)],
        immediate_size: 0,
    });

    let indexed_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Indexed Pipeline Layout"),
        bind_group_layouts: &[Some(&indexed_bind_group_layout)],
        immediate_size: 0,
    });

    let direct_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Direct Pipeline"),
        layout: Some(&direct_pipeline_layout),
        module: &shader,
        entry_point: Some("direct_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    let indexed_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Indexed Pipeline"),
        layout: Some(&indexed_pipeline_layout),
        module: &shader,
        entry_point: Some("indexed_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    Ok(BenchContext {
        device,
        queue,
        timestamp_supported,
        direct_bind_group_layout,
        indexed_bind_group_layout,
        direct_pipeline,
        indexed_pipeline,
    })
}

fn build_scenario_buffers(
    context: &BenchContext,
    scenario: &ScenarioConfig,
) -> Result<ScenarioBuffers, Box<dyn Error>> {
    let unique_instances = build_unique_instances(scenario.unique_instances);
    let draw_indices = build_draw_indices(scenario);
    let direct_instances = duplicate_direct_instances(&unique_instances, &draw_indices)?;

    let draw_count = draw_indices.len() as u32;
    let output_template = vec![OutputValue::zeroed(); draw_indices.len()];
    let params = ParamsUniform {
        count: draw_count,
        base_offset: 0,
        padding: [0; 2],
    };

    let direct_instance_buffer = create_buffer_with_data(
        &context.device,
        "Direct Instance Buffer",
        &direct_instances,
        wgpu::BufferUsages::STORAGE,
    );
    let shared_instance_buffer = create_buffer_with_data(
        &context.device,
        "Shared Instance Buffer",
        &unique_instances,
        wgpu::BufferUsages::STORAGE,
    );
    let index_buffer = create_buffer_with_data(
        &context.device,
        "Instance Index Buffer",
        &draw_indices,
        wgpu::BufferUsages::STORAGE,
    );
    let direct_output_buffer = create_buffer_with_data(
        &context.device,
        "Direct Output Buffer",
        &output_template,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let indexed_output_buffer = create_buffer_with_data(
        &context.device,
        "Indexed Output Buffer",
        &output_template,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let direct_params_buffer = create_buffer_with_data(
        &context.device,
        "Direct Params Buffer",
        &[params],
        wgpu::BufferUsages::UNIFORM,
    );
    let indexed_params_buffer = create_buffer_with_data(
        &context.device,
        "Indexed Params Buffer",
        &[params],
        wgpu::BufferUsages::UNIFORM,
    );

    let direct_bind_group = context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Direct Bind Group"),
            layout: &context.direct_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: direct_instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: direct_output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: direct_params_buffer.as_entire_binding(),
                },
            ],
        });

    let direct_phase_bind_groups = build_direct_phase_bind_groups(
        context,
        scenario.phase_count,
        scenario.unique_instances,
        &direct_instance_buffer,
        &direct_output_buffer,
    );

    let indexed_phase_bind_groups = build_indexed_phase_bind_groups(
        context,
        scenario.phase_count,
        scenario.unique_instances,
        &shared_instance_buffer,
        &index_buffer,
        &indexed_output_buffer,
    );
    let direct_separate_phases = build_direct_separate_phase_resources(
        context,
        scenario.phase_count,
        scenario.unique_instances,
        &direct_instances,
    );
    let indexed_separate_phases = build_indexed_separate_phase_resources(
        context,
        scenario.phase_count,
        scenario.unique_instances,
        &unique_instances,
        &draw_indices,
    );

    let indexed_bind_group = context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Indexed Bind Group"),
            layout: &context.indexed_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: shared_instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: index_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: indexed_output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: indexed_params_buffer.as_entire_binding(),
                },
            ],
        });

    let direct_bytes = (direct_instances.len() * mem::size_of::<InstancePayload>()) as u64;
    let indexed_bytes = (unique_instances.len() * mem::size_of::<InstancePayload>()) as u64
        + (draw_indices.len() * mem::size_of::<u32>()) as u64;

    Ok(ScenarioBuffers {
        direct_output_buffer,
        indexed_output_buffer,
        direct_bind_group,
        indexed_bind_group,
        direct_phase_bind_groups,
        indexed_phase_bind_groups,
        direct_separate_phases,
        indexed_separate_phases,
        draw_count,
        items_per_phase: scenario.unique_instances,
        direct_bytes,
        indexed_bytes,
    })
}

fn verify_outputs_match(
    context: &BenchContext,
    buffers: &ScenarioBuffers,
) -> Result<(), Box<dyn Error>> {
    for shape in [
        DispatchShape::Merged,
        DispatchShape::Bounded,
        DispatchShape::SeparateBuffers,
    ] {
        run_single_dispatch(context, buffers, PipelineKind::Direct, shape);
        run_single_dispatch(context, buffers, PipelineKind::Indexed, shape);

        let (direct, indexed) = match shape {
            DispatchShape::Merged | DispatchShape::Bounded => (
                read_storage_buffer::<OutputValue>(
                    &context.device,
                    &context.queue,
                    &buffers.direct_output_buffer,
                    buffers.draw_count as usize,
                )?,
                read_storage_buffer::<OutputValue>(
                    &context.device,
                    &context.queue,
                    &buffers.indexed_output_buffer,
                    buffers.draw_count as usize,
                )?,
            ),
            DispatchShape::SeparateBuffers => (
                read_concatenated_phase_outputs(
                    &context.device,
                    &context.queue,
                    &buffers.direct_separate_phases,
                    buffers.items_per_phase as usize,
                )?,
                read_concatenated_phase_outputs(
                    &context.device,
                    &context.queue,
                    &buffers.indexed_separate_phases,
                    buffers.items_per_phase as usize,
                )?,
            ),
        };

        if direct != indexed {
            return Err(Box::new(BenchError(format!(
                "direct and indexed compute paths produced different results for {} shape",
                shape.label()
            ))));
        }
    }

    Ok(())
}

fn warm_up(context: &BenchContext, buffers: &ScenarioBuffers) -> Result<(), Box<dyn Error>> {
    for shape in [
        DispatchShape::Merged,
        DispatchShape::Bounded,
        DispatchShape::SeparateBuffers,
    ] {
        run_single_dispatch(context, buffers, PipelineKind::Direct, shape);
        run_single_dispatch(context, buffers, PipelineKind::Indexed, shape);
    }
    Ok(())
}

fn run_single_dispatch(
    context: &BenchContext,
    buffers: &ScenarioBuffers,
    kind: PipelineKind,
    shape: DispatchShape,
) {
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Warmup Encoder"),
        });
    encode_dispatch(context, buffers, kind, shape, 1, &mut encoder);
    context.queue.submit(Some(encoder.finish()));
    let _ = context.device.poll(wgpu::PollType::wait_indefinitely());
}

fn measure_pipeline(
    context: &BenchContext,
    buffers: &ScenarioBuffers,
    kind: PipelineKind,
    shape: DispatchShape,
    iterations: u32,
) -> Result<TimingResult, Box<dyn Error>> {
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Benchmark Encoder"),
        });

    let timestamp_query_count = iterations * 2;
    let mut query_set = None;
    let mut query_resolve_buffer = None;
    let mut query_readback_buffer = None;

    if context.timestamp_supported {
        let created_query_set = context.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Timestamp Query Set"),
            ty: wgpu::QueryType::Timestamp,
            count: timestamp_query_count,
        });
        let resolve_size = timestamp_query_count as u64 * mem::size_of::<u64>() as u64;
        let created_query_resolve_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Timestamp Resolve Buffer"),
            size: resolve_size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let created_query_readback_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Timestamp Readback Buffer"),
            size: resolve_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        for iteration in 0..iterations {
            encoder.write_timestamp(&created_query_set, iteration * 2);
            encode_dispatch(context, buffers, kind, shape, 1, &mut encoder);
            encoder.write_timestamp(&created_query_set, iteration * 2 + 1);
        }

        encoder.resolve_query_set(
            &created_query_set,
            0..timestamp_query_count,
            &created_query_resolve_buffer,
            0,
        );
        encoder.copy_buffer_to_buffer(
            &created_query_resolve_buffer,
            0,
            &created_query_readback_buffer,
            0,
            resolve_size,
        );

        query_set = Some(created_query_set);
        query_resolve_buffer = Some(created_query_resolve_buffer);
        query_readback_buffer = Some(created_query_readback_buffer);
    } else {
        encode_dispatch(context, buffers, kind, shape, iterations, &mut encoder);
    }

    let started_at = Instant::now();
    context.queue.submit(Some(encoder.finish()));
    let _ = context.device.poll(wgpu::PollType::wait_indefinitely());
    let wall_ms = started_at.elapsed().as_secs_f64() * 1_000.0;

    if let Some(readback_buffer) = query_readback_buffer {
        let ticks = read_raw_u64_buffer(
            &context.device,
            &readback_buffer,
            timestamp_query_count as usize,
        )?;
        let period_ns = context.queue.get_timestamp_period() as f64;
        let mut total_ns = 0.0;
        for pair in ticks.chunks_exact(2) {
            total_ns += (pair[1] - pair[0]) as f64 * period_ns;
        }

        drop(query_set);
        drop(query_resolve_buffer);

        Ok(TimingResult {
            average_ns: total_ns / iterations as f64,
            wall_ms,
            used_timestamps: true,
        })
    } else {
        let total_ns = wall_ms * 1_000_000.0;
        Ok(TimingResult {
            average_ns: total_ns / iterations as f64,
            wall_ms,
            used_timestamps: false,
        })
    }
}

fn encode_dispatch(
    context: &BenchContext,
    buffers: &ScenarioBuffers,
    kind: PipelineKind,
    shape: DispatchShape,
    repetitions: u32,
    encoder: &mut wgpu::CommandEncoder,
) {
    for _ in 0..repetitions {
        match shape {
            DispatchShape::Merged => {
                let workgroups = buffers.draw_count.div_ceil(WORKGROUP_SIZE);
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Merged Benchmark Pass"),
                    timestamp_writes: None,
                });
                match kind {
                    PipelineKind::Direct => {
                        pass.set_pipeline(&context.direct_pipeline);
                        pass.set_bind_group(0, &buffers.direct_bind_group, &[]);
                    }
                    PipelineKind::Indexed => {
                        pass.set_pipeline(&context.indexed_pipeline);
                        pass.set_bind_group(0, &buffers.indexed_bind_group, &[]);
                    }
                }
                pass.dispatch_workgroups(workgroups, 1, 1);
            }
            DispatchShape::Bounded => {
                let workgroups = buffers.items_per_phase.div_ceil(WORKGROUP_SIZE);
                let phase_bind_groups = match kind {
                    PipelineKind::Direct => &buffers.direct_phase_bind_groups,
                    PipelineKind::Indexed => &buffers.indexed_phase_bind_groups,
                };

                for bind_group in phase_bind_groups {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Bounded Benchmark Pass"),
                        timestamp_writes: None,
                    });
                    match kind {
                        PipelineKind::Direct => {
                            pass.set_pipeline(&context.direct_pipeline);
                        }
                        PipelineKind::Indexed => {
                            pass.set_pipeline(&context.indexed_pipeline);
                        }
                    }
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.dispatch_workgroups(workgroups, 1, 1);
                }
            }
            DispatchShape::SeparateBuffers => {
                let workgroups = buffers.items_per_phase.div_ceil(WORKGROUP_SIZE);
                let phase_resources = match kind {
                    PipelineKind::Direct => &buffers.direct_separate_phases,
                    PipelineKind::Indexed => &buffers.indexed_separate_phases,
                };

                for phase in phase_resources {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Separate Phase Buffer Pass"),
                        timestamp_writes: None,
                    });
                    match kind {
                        PipelineKind::Direct => {
                            pass.set_pipeline(&context.direct_pipeline);
                        }
                        PipelineKind::Indexed => {
                            pass.set_pipeline(&context.indexed_pipeline);
                        }
                    }
                    pass.set_bind_group(0, &phase.bind_group, &[]);
                    pass.dispatch_workgroups(workgroups, 1, 1);
                }
            }
        }
    }
}

fn build_unique_instances(count: u32) -> Vec<InstancePayload> {
    (0..count)
        .map(|index| {
            let seed = index as f32 + 1.0;
            InstancePayload {
                model: [
                    [seed, seed + 0.1, seed + 0.2, 1.0],
                    [seed + 1.0, seed + 1.1, seed + 1.2, 0.0],
                    [seed + 2.0, seed + 2.1, seed + 2.2, 0.0],
                    [seed + 3.0, seed + 3.1, seed + 3.2, 1.0],
                ],
                previous: [
                    [seed + 4.0, seed + 4.1, seed + 4.2, 1.0],
                    [seed + 5.0, seed + 5.1, seed + 5.2, 0.0],
                    [seed + 6.0, seed + 6.1, seed + 6.2, 0.0],
                    [seed + 7.0, seed + 7.1, seed + 7.2, 1.0],
                ],
                meta: [
                    index,
                    index.wrapping_mul(3),
                    index.wrapping_mul(7),
                    0xDEAD_BEEF,
                ],
            }
        })
        .collect()
}

fn build_draw_indices(scenario: &ScenarioConfig) -> Vec<u32> {
    let unique_count = scenario.unique_instances;
    let mut indices = Vec::with_capacity(scenario.draw_count());

    for phase in 0..scenario.phase_count {
        for local_index in 0..unique_count {
            let object_index = if scenario.shuffled_indices {
                permute_index(local_index, unique_count, phase)
            } else {
                local_index
            };
            indices.push(object_index);
        }
    }

    indices
}

fn duplicate_direct_instances(
    unique_instances: &[InstancePayload],
    draw_indices: &[u32],
) -> Result<Vec<InstancePayload>, Box<dyn Error>> {
    let mut direct_instances = Vec::with_capacity(draw_indices.len());
    for &index in draw_indices {
        let Some(instance) = unique_instances.get(index as usize) else {
            return Err(Box::new(BenchError(format!(
                "draw index entry {index} was out of range for {} unique instances",
                unique_instances.len()
            ))));
        };
        direct_instances.push(*instance);
    }
    Ok(direct_instances)
}

fn build_direct_phase_bind_groups(
    context: &BenchContext,
    phase_count: u32,
    items_per_phase: u32,
    instance_buffer: &wgpu::Buffer,
    output_buffer: &wgpu::Buffer,
) -> Vec<wgpu::BindGroup> {
    (0..phase_count)
        .map(|phase| {
            let params_buffer = create_buffer_with_data(
                &context.device,
                "Direct Phase Params Buffer",
                &[ParamsUniform {
                    count: items_per_phase,
                    base_offset: phase * items_per_phase,
                    padding: [0; 2],
                }],
                wgpu::BufferUsages::UNIFORM,
            );

            context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Direct Phase Bind Group"),
                    layout: &context.direct_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: instance_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: output_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: params_buffer.as_entire_binding(),
                        },
                    ],
                })
        })
        .collect()
}

fn build_indexed_phase_bind_groups(
    context: &BenchContext,
    phase_count: u32,
    items_per_phase: u32,
    instance_buffer: &wgpu::Buffer,
    index_buffer: &wgpu::Buffer,
    output_buffer: &wgpu::Buffer,
) -> Vec<wgpu::BindGroup> {
    (0..phase_count)
        .map(|phase| {
            let params_buffer = create_buffer_with_data(
                &context.device,
                "Indexed Phase Params Buffer",
                &[ParamsUniform {
                    count: items_per_phase,
                    base_offset: phase * items_per_phase,
                    padding: [0; 2],
                }],
                wgpu::BufferUsages::UNIFORM,
            );

            context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Indexed Phase Bind Group"),
                    layout: &context.indexed_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: instance_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: index_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: output_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: params_buffer.as_entire_binding(),
                        },
                    ],
                })
        })
        .collect()
}

fn build_direct_separate_phase_resources(
    context: &BenchContext,
    phase_count: u32,
    items_per_phase: u32,
    direct_instances: &[InstancePayload],
) -> Vec<SeparatePhaseResources> {
    let items_per_phase_usize = items_per_phase as usize;
    (0..phase_count)
        .map(|phase| {
            let start = phase as usize * items_per_phase_usize;
            let end = start + items_per_phase_usize;
            let phase_instance_buffer = create_buffer_with_data(
                &context.device,
                "Direct Separate Phase Instance Buffer",
                &direct_instances[start..end],
                wgpu::BufferUsages::STORAGE,
            );
            let phase_output_buffer = create_buffer_with_data(
                &context.device,
                "Direct Separate Phase Output Buffer",
                &vec![OutputValue::zeroed(); items_per_phase_usize],
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            let phase_params_buffer = create_buffer_with_data(
                &context.device,
                "Direct Separate Phase Params Buffer",
                &[ParamsUniform {
                    count: items_per_phase,
                    base_offset: 0,
                    padding: [0; 2],
                }],
                wgpu::BufferUsages::UNIFORM,
            );

            let bind_group = context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Direct Separate Phase Bind Group"),
                    layout: &context.direct_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: phase_instance_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: phase_output_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: phase_params_buffer.as_entire_binding(),
                        },
                    ],
                });

            SeparatePhaseResources {
                bind_group,
                output_buffer: phase_output_buffer,
            }
        })
        .collect()
}

fn build_indexed_separate_phase_resources(
    context: &BenchContext,
    phase_count: u32,
    items_per_phase: u32,
    unique_instances: &[InstancePayload],
    draw_indices: &[u32],
) -> Vec<SeparatePhaseResources> {
    let shared_instance_buffer = create_buffer_with_data(
        &context.device,
        "Indexed Separate Shared Instance Buffer",
        unique_instances,
        wgpu::BufferUsages::STORAGE,
    );
    let items_per_phase_usize = items_per_phase as usize;

    (0..phase_count)
        .map(|phase| {
            let start = phase as usize * items_per_phase_usize;
            let end = start + items_per_phase_usize;
            let phase_index_buffer = create_buffer_with_data(
                &context.device,
                "Indexed Separate Phase Index Buffer",
                &draw_indices[start..end],
                wgpu::BufferUsages::STORAGE,
            );
            let phase_output_buffer = create_buffer_with_data(
                &context.device,
                "Indexed Separate Phase Output Buffer",
                &vec![OutputValue::zeroed(); items_per_phase_usize],
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            let phase_params_buffer = create_buffer_with_data(
                &context.device,
                "Indexed Separate Phase Params Buffer",
                &[ParamsUniform {
                    count: items_per_phase,
                    base_offset: 0,
                    padding: [0; 2],
                }],
                wgpu::BufferUsages::UNIFORM,
            );

            let bind_group = context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Indexed Separate Phase Bind Group"),
                    layout: &context.indexed_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: shared_instance_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: phase_index_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: phase_output_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: phase_params_buffer.as_entire_binding(),
                        },
                    ],
                });

            SeparatePhaseResources {
                bind_group,
                output_buffer: phase_output_buffer,
            }
        })
        .collect()
}

fn permute_index(local_index: u32, count: u32, phase: u32) -> u32 {
    if count <= 1 {
        return 0;
    }

    let stride = count.saturating_sub(1).max(1);
    let offset = phase.wrapping_mul(97) % count;
    local_index.wrapping_mul(stride).wrapping_add(offset) % count
}

fn create_buffer_with_data<T: Pod>(
    device: &wgpu::Device,
    label: &str,
    data: &[T],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let bytes = bytemuck::cast_slice(data);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len().max(4) as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: true,
    });

    {
        let mut mapped = buffer.slice(..).get_mapped_range_mut();
        mapped.copy_from_slice(bytes);
    }

    buffer.unmap();
    buffer
}

fn read_storage_buffer<T: Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
    element_count: usize,
) -> Result<Vec<T>, Box<dyn Error>> {
    let size = (element_count * mem::size_of::<T>()) as u64;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Storage Buffer Readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Storage Readback Encoder"),
    });
    encoder.copy_buffer_to_buffer(source, 0, &readback, 0, size);
    queue.submit(Some(encoder.finish()));
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    let bytes = read_buffer_bytes(device, &readback)?;
    Ok(bytemuck::cast_slice::<u8, T>(&bytes).to_vec())
}

fn read_raw_u64_buffer(
    device: &wgpu::Device,
    source: &wgpu::Buffer,
    count: usize,
) -> Result<Vec<u64>, Box<dyn Error>> {
    let bytes = read_buffer_bytes(device, source)?;
    let values = bytemuck::cast_slice::<u8, u64>(&bytes);
    Ok(values[..count].to_vec())
}

fn read_concatenated_phase_outputs(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    phases: &[SeparatePhaseResources],
    items_per_phase: usize,
) -> Result<Vec<OutputValue>, Box<dyn Error>> {
    let mut values = Vec::with_capacity(phases.len() * items_per_phase);
    for phase in phases {
        values.extend(read_storage_buffer::<OutputValue>(
            device,
            queue,
            &phase.output_buffer,
            items_per_phase,
        )?);
    }
    Ok(values)
}

fn read_buffer_bytes(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let slice = buffer.slice(..);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    receiver
        .recv()
        .map_err(|error| {
            Box::new(BenchError(format!(
                "failed to receive buffer map result: {error}"
            ))) as Box<dyn Error>
        })?
        .map_err(|error| {
            Box::new(BenchError(format!("buffer map failed: {error}"))) as Box<dyn Error>
        })?;

    let mapped = slice.get_mapped_range();
    let bytes = mapped.to_vec();
    drop(mapped);
    buffer.unmap();
    Ok(bytes)
}

fn print_report(
    scenario: &ScenarioConfig,
    buffers: &ScenarioBuffers,
    merged_direct: &TimingResult,
    merged_indexed: &TimingResult,
    bounded_direct: &TimingResult,
    bounded_indexed: &TimingResult,
    separate_direct: &TimingResult,
    separate_indexed: &TimingResult,
) {
    println!("Scenario: {}", scenario.label);
    println!(
        "  unique objects: {:>8} | phase count: {:>2} | draw items: {:>8} | shuffled: {}",
        scenario.unique_instances,
        scenario.phase_count,
        scenario.draw_count(),
        scenario.shuffled_indices
    );
    println!(
        "  direct payload: {:>8.2} MiB | shared payload + indices: {:>8.2} MiB",
        bytes_to_mib(buffers.direct_bytes),
        bytes_to_mib(buffers.indexed_bytes)
    );
    print_shape_report("merged stream", merged_direct, merged_indexed);
    print_shape_report("bounded phases", bounded_direct, bounded_indexed);
    print_shape_report("separate phase buffers", separate_direct, separate_indexed);
    println!();
}

fn print_shape_report(label: &str, direct: &TimingResult, indexed: &TimingResult) {
    let direct_ms = direct.average_ns / 1_000_000.0;
    let indexed_ms = indexed.average_ns / 1_000_000.0;
    let delta_pct = if direct.average_ns > 0.0 {
        ((indexed.average_ns / direct.average_ns) - 1.0) * 100.0
    } else {
        0.0
    };

    println!("  {label}:");
    println!(
        "    direct avg:   {:>8.4} ms/dispatch | wall batch: {:>8.3} ms",
        direct_ms, direct.wall_ms
    );
    println!(
        "    indexed avg:  {:>8.4} ms/dispatch | wall batch: {:>8.3} ms",
        indexed_ms, indexed.wall_ms
    );
    println!(
        "    indexed is {:>7.2}% {} than direct ({})",
        delta_pct.abs(),
        if delta_pct >= 0.0 { "slower" } else { "faster" },
        if direct.used_timestamps && indexed.used_timestamps {
            "GPU timestamps"
        } else {
            "wall-clock fallback"
        }
    );
}

fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn parse_args(args: Vec<String>) -> Result<Vec<ScenarioConfig>, Box<dyn Error>> {
    if args.is_empty() {
        return Ok(vec![
            ScenarioConfig {
                label: "unique_baseline".to_string(),
                unique_instances: 131_072,
                phase_count: 1,
                shuffled_indices: false,
                iterations: 64,
            },
            ScenarioConfig {
                label: "reused_sequential_x4".to_string(),
                unique_instances: 131_072,
                phase_count: 4,
                shuffled_indices: false,
                iterations: 64,
            },
            ScenarioConfig {
                label: "reused_shuffled_x4".to_string(),
                unique_instances: 131_072,
                phase_count: 4,
                shuffled_indices: true,
                iterations: 64,
            },
        ]);
    }

    let mut label = "custom".to_string();
    let mut unique_instances = 131_072u32;
    let mut phase_count = 4u32;
    let mut shuffled_indices = false;
    let mut iterations = 64u32;

    let mut iterator = args.into_iter();
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--label" => {
                label = next_arg_value(&mut iterator, "--label")?;
            }
            "--unique" => {
                unique_instances =
                    next_arg_value(&mut iterator, "--unique")?
                        .parse()
                        .map_err(|error| {
                            Box::new(BenchError(format!("invalid --unique value: {error}")))
                                as Box<dyn Error>
                        })?;
            }
            "--passes" => {
                phase_count =
                    next_arg_value(&mut iterator, "--passes")?
                        .parse()
                        .map_err(|error| {
                            Box::new(BenchError(format!("invalid --passes value: {error}")))
                                as Box<dyn Error>
                        })?;
            }
            "--iterations" => {
                iterations = next_arg_value(&mut iterator, "--iterations")?
                    .parse()
                    .map_err(|error| {
                        Box::new(BenchError(format!("invalid --iterations value: {error}")))
                            as Box<dyn Error>
                    })?;
            }
            "--shuffled" => {
                shuffled_indices = true;
            }
            "--sequential" => {
                shuffled_indices = false;
            }
            other => {
                return Err(Box::new(BenchError(format!(
                    "unknown argument '{other}'. Supported flags: --label, --unique, --passes, --iterations, --shuffled, --sequential"
                ))));
            }
        }
    }

    Ok(vec![ScenarioConfig {
        label,
        unique_instances,
        phase_count,
        shuffled_indices,
        iterations,
    }])
}

fn next_arg_value<I>(iterator: &mut I, flag: &str) -> Result<String, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    iterator.next().ok_or_else(|| {
        Box::new(BenchError(format!("expected a value after {flag}"))) as Box<dyn Error>
    })
}

fn storage_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn read_only_storage_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

#[derive(Clone, Copy)]
enum PipelineKind {
    Direct,
    Indexed,
}
