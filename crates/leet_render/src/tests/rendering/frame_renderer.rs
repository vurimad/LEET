use bevy_camera::{CameraOutputMode, ClearColorConfig, MsaaWriteback};
use bevy_math::{Mat4, URect, UVec2};
use bevy_transform::components::GlobalTransform;

use crate::{
    FrameCaptureIntent, FrameDebugGraphView, FrameDebugIntent, FrameGpuScene, FrameInput,
    FrameOutput, FramePurpose, FrameRenderingMode, FrameTiming, PreparedFrameCamera,
    PresentationIntent, RenderCamera, RenderCameraFeatures, RenderCameraId,
    RenderNodeExecutionMetadata, RenderNodeParameters, RenderViewport,
};

use super::{
    add_camera_setup_graph_to_frame_graph, camera_setup_count, frame_graph_shape_hash,
    resolve_frame_graph_build_kind, FrameGraphBuildKind, RenderGraphBuilder,
};

fn frame(
    mode: FrameRenderingMode,
    purpose: FramePurpose,
    cameras: Vec<PreparedFrameCamera>,
) -> FrameInput {
    FrameInput {
        viewport: RenderViewport::targetless(
            UVec2::new(128, 72),
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ),
        output: FrameOutput::Targetless,
        cameras,
        scene: FrameGpuScene::empty(),
        timing: FrameTiming {
            frame_index: 7,
            ..FrameTiming::default()
        },
        mode,
        purpose,
        presentation: PresentationIntent::NoPresent,
        capture: FrameCaptureIntent::None,
        debug: FrameDebugIntent::default(),
    }
}

fn camera(
    id: u64,
    hdr: bool,
    format: wgpu::TextureFormat,
    features: RenderCameraFeatures,
) -> PreparedFrameCamera {
    PreparedFrameCamera {
        camera_id: RenderCameraId(id),
        camera: RenderCamera {
            target: None,
            physical_target_size: Some(UVec2::new(128, 72)),
            clip_from_view: Mat4::IDENTITY,
            world_from_view: GlobalTransform::IDENTITY,
            viewport: URect::new(0, 0, 128, 72),
            invert_culling: false,
            main_pass_texture_format: format,
            order: 0,
            output_mode: CameraOutputMode::default(),
            msaa_writeback: MsaaWriteback::default(),
            clear_color: ClearColorConfig::Default,
            exposure: 1.0,
            hdr,
            features,
            compositing_space: None,
        },
        source_view_index: 0,
        render_flow_space: 0,
        selected_dependencies: Vec::new(),
        reset_temporal_history: false,
        previous_frame: None,
    }
}

#[test]
fn frame_graph_hash_changes_when_rendering_mode_changes() {
    let shaded = frame(
        FrameRenderingMode::Shaded,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );
    let overlay = frame(
        FrameRenderingMode::OverlayOnly,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );

    assert_ne!(
        frame_graph_shape_hash(&shaded, shaded.cameras.len()),
        frame_graph_shape_hash(&overlay, 0)
    );
}

#[test]
fn frame_graph_hash_uses_camera_graph_shape_inputs() {
    let ldr = frame(
        FrameRenderingMode::Shaded,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );
    let hdr = frame(
        FrameRenderingMode::Shaded,
        FramePurpose::Normal,
        vec![camera(
            1,
            true,
            wgpu::TextureFormat::Rgba16Float,
            RenderCameraFeatures::empty(),
        )],
    );

    assert_ne!(
        frame_graph_shape_hash(&ldr, ldr.cameras.len()),
        frame_graph_shape_hash(&hdr, hdr.cameras.len())
    );
}

#[test]
fn blank_or_zero_camera_frames_use_one_camera_setup_slot() {
    let blank = frame(FrameRenderingMode::Blank, FramePurpose::Blank, Vec::new());
    let shaded_no_camera = frame(FrameRenderingMode::Shaded, FramePurpose::Normal, Vec::new());

    assert_eq!(camera_setup_count(&blank, 0), 1);
    assert_eq!(camera_setup_count(&shaded_no_camera, 0), 1);
}

#[test]
fn graph_build_kind_distinguishes_blank_frame_shapes() {
    assert_eq!(
        FrameGraphBuildKind::Blank { has_camera: false },
        FrameGraphBuildKind::Blank { has_camera: false }
    );
    assert_ne!(
        FrameGraphBuildKind::Blank { has_camera: false },
        FrameGraphBuildKind::Blank { has_camera: true }
    );
}

#[test]
fn graph_build_kind_resolves_gbuffer_and_no_scene_modes() {
    let gbuffer = frame(
        FrameRenderingMode::GBufferOnly,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );
    let no_scene = frame(
        FrameRenderingMode::Shaded,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );

    assert_eq!(
        resolve_frame_graph_build_kind(&gbuffer, gbuffer.cameras.len(), 0),
        FrameGraphBuildKind::GBufferOnly { camera_index: 0 }
    );
    assert_eq!(
        resolve_frame_graph_build_kind(&no_scene, no_scene.cameras.len(), 0),
        FrameGraphBuildKind::NoScene { camera_index: 0 }
    );
}

#[test]
fn graph_build_kind_resolves_blank_safe_and_debug_variants() {
    let blank_without_camera = frame(FrameRenderingMode::Shaded, FramePurpose::Normal, Vec::new());
    let blank_with_camera = frame(
        FrameRenderingMode::Shaded,
        FramePurpose::Blank,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );
    let safe_mode = frame(
        FrameRenderingMode::SafeMode,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );
    let mut debug_visualization = frame(
        FrameRenderingMode::Shaded,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );
    debug_visualization.debug.graph_view = FrameDebugGraphView::Visualization;

    assert_eq!(
        resolve_frame_graph_build_kind(&blank_without_camera, 0, 0),
        FrameGraphBuildKind::Blank { has_camera: false }
    );
    assert_eq!(
        resolve_frame_graph_build_kind(&blank_with_camera, blank_with_camera.cameras.len(), 0),
        FrameGraphBuildKind::Blank { has_camera: true }
    );
    assert_eq!(
        resolve_frame_graph_build_kind(&safe_mode, safe_mode.cameras.len(), 0),
        FrameGraphBuildKind::NoScene { camera_index: 0 }
    );
    assert_eq!(
        resolve_frame_graph_build_kind(&debug_visualization, debug_visualization.cameras.len(), 0),
        FrameGraphBuildKind::DebugVisualization { camera_index: 0 }
    );
}

#[test]
fn render_graph_builder_leaves_are_explicitly_unimplemented_but_setup_import_is_real() {
    let mut renderer = crate::FrameRenderer::new();
    let graph_hash = crate::RenderGraphShapeHash::from_raw(1);
    let lookup = renderer
        .graph_cache_mut()
        .get_graph(graph_hash, 1, 1, false)
        .unwrap();
    let mut builder = RenderGraphBuilder::new();
    let graph = lookup.entry.camera_build_data_mut().get_mut(0).unwrap();

    assert!(builder
        .build(graph, FrameGraphBuildKind::Blank { has_camera: false })
        .is_err());

    graph
        .temporary_graph_mut()
        .add_node(
            RenderNodeParameters::stage("camera setup"),
            RenderNodeExecutionMetadata::new(None, None),
        )
        .unwrap();

    add_camera_setup_graph_to_frame_graph(lookup.entry, 0).unwrap();

    let final_graph = lookup.entry.final_graph().unwrap().graph();
    assert_eq!(final_graph.node_count(), 1);
    let node = final_graph
        .node(final_graph.node_ids().next().unwrap())
        .unwrap();
    assert_eq!(node.metadata().camera_index, Some(0));

    lookup
        .entry
        .final_graph_mut()
        .unwrap()
        .graph_mut()
        .build_flow_groups()
        .unwrap();
    assert!(lookup.entry.final_graph().unwrap().graph().is_built());

    assert_eq!(
        lookup.entry.camera_build_data()[0]
            .temporary_graph()
            .node_count(),
        1
    );
    lookup.entry.post_build_clear();
    assert_eq!(
        lookup.entry.camera_build_data()[0]
            .temporary_graph()
            .node_count(),
        0
    );
}

#[test]
fn frame_graph_hash_uses_camera_feature_bits() {
    let no_features = frame(
        FrameRenderingMode::Shaded,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::empty(),
        )],
    );
    let feature_enabled = frame(
        FrameRenderingMode::Shaded,
        FramePurpose::Normal,
        vec![camera(
            1,
            false,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            RenderCameraFeatures::from_bits(1),
        )],
    );

    assert_ne!(
        frame_graph_shape_hash(&no_features, no_features.cameras.len()),
        frame_graph_shape_hash(&feature_enabled, feature_enabled.cameras.len())
    );
}
