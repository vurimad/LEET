//! Frame renderer boundary behind the render command handler.

use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use crate::{
    PersistentRenderSceneDataRegistry, PreparedFrameCamera, RenderGraphCache,
    RenderGraphCacheEntry, RenderGraphExecutionInput, RenderGraphExecutor, RenderGraphShapeHash,
    RenderGraphShapeHashBuilder, RenderResourceAllocator,
};

use super::{
    FrameCaptureIntent, FrameDebugGraphView, FrameGraphBuildKind, FrameInput, FramePurpose,
    FrameRenderingMode, RenderFrameContext, RenderFrameError, RenderFrameResult,
    RenderGraphBuilder,
};

pub struct RenderProfilerScope {
    _entered: tracing::span::EnteredSpan,
}

impl RenderProfilerScope {
    pub fn render_frame() -> Self {
        Self {
            _entered: tracing::trace_span!("FrameRenderer::render_frame").entered(),
        }
    }
}

#[derive(Default)]
pub struct FrameRenderer {
    graph_cache: RenderGraphCache,
    resource_allocator: RenderResourceAllocator,
    scene_registry: PersistentRenderSceneDataRegistry,
    graph_executor: RenderGraphExecutor,
}

#[derive(Clone)]
pub struct FrameRendererHandle {
    inner: Arc<Mutex<FrameRenderer>>,
}

fn frame_graph_shape_hash(frame: &FrameInput, num_cameras: usize) -> RenderGraphShapeHash {
    let mut hash = RenderGraphShapeHashBuilder::new();

    hash.append_usize(num_cameras);
    hash.append_u32(frame_rendering_mode_key(frame.mode));
    hash.append_u32(frame_purpose_key(frame.purpose));
    hash.append_u32(frame_capture_key(frame.capture));
    hash.append_bool(frame.debug.stable_dissolves);
    hash.append_u32(frame_debug_graph_view_key(frame.debug.graph_view));

    for camera in frame.cameras.iter().take(num_cameras) {
        append_camera_graph_hash(&mut hash, camera);
    }

    hash.finish()
}

fn camera_setup_count(frame: &FrameInput, num_cameras: usize) -> usize {
    if num_cameras == 0 || frame.purpose == FramePurpose::Blank {
        1
    } else {
        num_cameras
    }
}

fn rebuild_cached_graph(
    frame: &FrameInput,
    num_cameras: usize,
    entry: &mut RenderGraphCacheEntry,
) -> RenderFrameResult<()> {
    let mut graph_builder = RenderGraphBuilder::new();
    let mut build_requests = Vec::new();
    let setup_count = camera_setup_count(frame, num_cameras);

    for setup_index in 0..setup_count {
        let build_kind = resolve_frame_graph_build_kind(frame, num_cameras, setup_index);
        build_requests.push((setup_index, build_kind));
    }

    for (camera_index, build_kind) in build_requests {
        let Some(graph) = entry.camera_build_data_mut().get_mut(camera_index) else {
            return Err(RenderFrameError::InvalidFrameInput {
                reason: "frame graph setup index is out of range",
            });
        };
        graph_builder.build(graph, build_kind)?;
    }

    for camera_index in 0..setup_count {
        add_camera_setup_graph_to_frame_graph(entry, camera_index)?;
    }

    let final_graph = entry
        .final_graph_mut()
        .ok_or(RenderFrameError::InvalidFrameInput {
            reason: "rebuilt frame graph has no imported camera setup graphs",
        })?;
    final_graph.graph_mut().build_flow_groups()?;
    entry.post_build_clear();

    Ok(())
}

fn resolve_frame_graph_build_kind(
    frame: &FrameInput,
    num_cameras: usize,
    camera_index: usize,
) -> FrameGraphBuildKind {
    if num_cameras == 0 {
        return FrameGraphBuildKind::Blank { has_camera: false };
    }

    if frame.purpose == FramePurpose::Blank {
        return FrameGraphBuildKind::Blank { has_camera: true };
    }

    match frame.debug.graph_view {
        FrameDebugGraphView::Visualization => {
            return FrameGraphBuildKind::DebugVisualization { camera_index };
        }
        FrameDebugGraphView::None => {}
    }

    match frame.mode {
        FrameRenderingMode::SafeMode => {
            if frame.scene.live_proxy_count == 0 {
                FrameGraphBuildKind::NoScene { camera_index }
            } else {
                FrameGraphBuildKind::SafeMode { camera_index }
            }
        }
        FrameRenderingMode::GBufferOnly => FrameGraphBuildKind::GBufferOnly { camera_index },
        FrameRenderingMode::Shaded => {
            if frame.scene.live_proxy_count == 0 {
                FrameGraphBuildKind::NoScene { camera_index }
            } else {
                FrameGraphBuildKind::Camera { camera_index }
            }
        }
        FrameRenderingMode::OverlayOnly | FrameRenderingMode::Blank => {
            FrameGraphBuildKind::NoScene { camera_index }
        }
    }
}

fn add_camera_setup_graph_to_frame_graph(
    entry: &mut RenderGraphCacheEntry,
    camera_index: usize,
) -> RenderFrameResult<()> {
    let camera_index =
        u32::try_from(camera_index).map_err(|_| RenderFrameError::InvalidFrameInput {
            reason: "camera setup index exceeded u32 range",
        })?;

    entry
        .import_camera_setup_graph_to_final(camera_index as usize, camera_index)
        .map_err(RenderFrameError::from)
}

fn append_camera_graph_hash(hash: &mut RenderGraphShapeHashBuilder, camera: &PreparedFrameCamera) {
    hash.append_bool(true);
    hash.append_u64(camera.camera.features.bits());
    hash.append_bool(camera.camera.hdr);
    append_debug_hash(hash, &camera.camera.main_pass_texture_format);
    append_debug_hash(hash, &camera.camera.output_mode);
    append_debug_hash(hash, &camera.camera.msaa_writeback);
    append_debug_hash(hash, &camera.camera.compositing_space);
    hash.append_usize(camera.selected_dependencies.len());
    for dependency in &camera.selected_dependencies {
        hash.append_u64(dependency.camera_id.0);
        hash.append_bytes(&[dependency.flags.bits()]);
    }
}

fn append_debug_hash(hash: &mut RenderGraphShapeHashBuilder, value: &impl Debug) {
    hash.append_bytes(format!("{value:?}").as_bytes());
}

const fn frame_rendering_mode_key(mode: FrameRenderingMode) -> u32 {
    match mode {
        FrameRenderingMode::Shaded => 0,
        FrameRenderingMode::OverlayOnly => 1,
        FrameRenderingMode::SafeMode => 2,
        FrameRenderingMode::GBufferOnly => 3,
        FrameRenderingMode::Blank => 4,
    }
}

const fn frame_purpose_key(purpose: FramePurpose) -> u32 {
    match purpose {
        FramePurpose::Normal => 0,
        FramePurpose::Blank => 1,
        FramePurpose::Screenshot => 2,
        FramePurpose::DeferredScreenshot => 3,
        FramePurpose::EnvProbe => 4,
        FramePurpose::GlobalIllumination => 5,
        FramePurpose::GlobalIlluminationBackfaces => 6,
    }
}

const fn frame_capture_key(capture: FrameCaptureIntent) -> u32 {
    match capture {
        FrameCaptureIntent::None => 0,
        FrameCaptureIntent::Color => 1,
        FrameCaptureIntent::Layered => 2,
    }
}

const fn frame_debug_graph_view_key(view: FrameDebugGraphView) -> u32 {
    match view {
        FrameDebugGraphView::None => 0,
        FrameDebugGraphView::Visualization => 1,
    }
}

impl FrameRendererHandle {
    pub fn new(renderer: FrameRenderer) -> Self {
        Self {
            inner: Arc::new(Mutex::new(renderer)),
        }
    }

    pub fn handle_for_job(&self) -> Self {
        self.clone()
    }

    pub fn with<R>(&self, f: impl FnOnce(&FrameRenderer) -> R) -> RenderFrameResult<R> {
        let renderer = self
            .inner
            .lock()
            .map_err(|_| RenderFrameError::LockPoisoned {
                resource: "FrameRenderer",
            })?;
        Ok(f(&renderer))
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut FrameRenderer) -> R) -> RenderFrameResult<R> {
        let mut renderer = self
            .inner
            .lock()
            .map_err(|_| RenderFrameError::LockPoisoned {
                resource: "FrameRenderer",
            })?;
        Ok(f(&mut renderer))
    }

    pub fn render_frame(&self, ctx: RenderFrameContext) {
        let Ok(mut renderer) = self.inner.lock() else {
            return;
        };

        renderer.render_frame(ctx);
    }
}

impl From<FrameRenderer> for FrameRendererHandle {
    fn from(renderer: FrameRenderer) -> Self {
        Self::new(renderer)
    }
}

impl FrameRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn graph_cache(&self) -> &RenderGraphCache {
        &self.graph_cache
    }

    pub fn graph_cache_mut(&mut self) -> &mut RenderGraphCache {
        &mut self.graph_cache
    }

    pub fn resource_allocator(&self) -> &RenderResourceAllocator {
        &self.resource_allocator
    }

    pub fn resource_allocator_mut(&mut self) -> &mut RenderResourceAllocator {
        &mut self.resource_allocator
    }

    pub fn scene_registry(&self) -> &PersistentRenderSceneDataRegistry {
        &self.scene_registry
    }

    pub fn scene_registry_mut(&mut self) -> &mut PersistentRenderSceneDataRegistry {
        &mut self.scene_registry
    }

    pub fn graph_executor(&self) -> &RenderGraphExecutor {
        &self.graph_executor
    }

    pub fn graph_executor_mut(&mut self) -> &mut RenderGraphExecutor {
        &mut self.graph_executor
    }

    pub fn render_frame(&mut self, ctx: RenderFrameContext) {
        let _scope = RenderProfilerScope::render_frame();
        let frame = ctx.frame_input;

        let _viewport = &frame.viewport;
        let _scene = &frame.scene;
        let _camera_storage = frame.cameras.as_slice();
        let _rendering_mode = frame.mode;
        let _frame_purpose = frame.purpose;

        if frame.viewport.extent().x == 0 || frame.viewport.extent().y == 0 {
            return;
        }

        if frame.purpose.requires_stable_dissolves() || frame.debug.stable_dissolves {
            // Not implemented: finish scene dissolve synchronization.
        }

        let num_cameras = if frame.mode == FrameRenderingMode::OverlayOnly {
            0
        } else {
            frame.cameras.len()
        };
        let graph_hash = frame_graph_shape_hash(&frame, num_cameras);
        let camera_setup_count = camera_setup_count(&frame, num_cameras);
        let force_graph_rebuild = false;
        let Ok(graph_lookup) = self.graph_cache.get_graph(
            graph_hash,
            camera_setup_count,
            frame.timing.frame_index,
            force_graph_rebuild,
        ) else {
            return;
        };

        if graph_lookup.needs_rebuild
            && rebuild_cached_graph(&frame, num_cameras, graph_lookup.entry).is_err()
        {
            return;
        }

        let Some(final_graph) = graph_lookup.entry.final_graph() else {
            return;
        };

        self.scene_registry.sync_scene_cameras(
            frame.scene_id,
            frame.cameras.iter().map(|camera| camera.camera_id),
        );

        let execution_input = RenderGraphExecutionInput {
            graph: final_graph,
            frame: &frame,
            dispatcher_thread_index: ctx.dispatcher_thread_index,
            allocator: &mut self.resource_allocator,
            scene_registry: &mut self.scene_registry,
            scene_id: frame.scene_id,
            builder: ctx.builder, // moved
            external_kickoff_wait_counter: None,
        };

        if self.graph_executor.execute(execution_input).is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[path = "../tests/rendering/frame_renderer.rs"]
mod tests;
