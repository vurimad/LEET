use crate::RenderGraphCameraBuildData;

use super::{RenderFrameError, RenderFrameResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameGraphBuildKind {
    Blank { has_camera: bool },
    Camera { camera_index: usize },
    NoScene { camera_index: usize },
    DebugVisualization { camera_index: usize },
    SafeMode { camera_index: usize },
    GBufferOnly { camera_index: usize },
}

#[derive(Default)]
pub(crate) struct RenderGraphBuilder;

impl RenderGraphBuilder {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn build(
        &mut self,
        graph: &mut RenderGraphCameraBuildData,
        kind: FrameGraphBuildKind,
    ) -> RenderFrameResult<()> {
        match kind {
            FrameGraphBuildKind::Blank { has_camera } => self.build_blank(graph, has_camera),
            FrameGraphBuildKind::Camera { camera_index } => self.build_camera(graph, camera_index),
            FrameGraphBuildKind::NoScene { camera_index } => {
                self.build_no_scene(graph, camera_index)
            }
            FrameGraphBuildKind::DebugVisualization { camera_index } => {
                self.build_debug_visualization(graph, camera_index)
            }
            FrameGraphBuildKind::SafeMode { camera_index } => {
                self.build_safe_mode(graph, camera_index)
            }
            FrameGraphBuildKind::GBufferOnly { camera_index } => {
                self.build_gbuffer_only(graph, camera_index)
            }
        }
    }

    fn build_blank(
        &mut self,
        _graph: &mut RenderGraphCameraBuildData,
        _has_camera: bool,
    ) -> RenderFrameResult<()> {
        Err(RenderFrameError::NotImplemented {
            operation: "build blank render graph",
        })
    }

    fn build_camera(
        &mut self,
        _graph: &mut RenderGraphCameraBuildData,
        _camera_index: usize,
    ) -> RenderFrameResult<()> {
        Err(RenderFrameError::NotImplemented {
            operation: "build camera render graph",
        })
    }

    fn build_no_scene(
        &mut self,
        _graph: &mut RenderGraphCameraBuildData,
        _camera_index: usize,
    ) -> RenderFrameResult<()> {
        Err(RenderFrameError::NotImplemented {
            operation: "build no-scene render graph",
        })
    }

    fn build_gbuffer_only(
        &mut self,
        _graph: &mut RenderGraphCameraBuildData,
        _camera_index: usize,
    ) -> RenderFrameResult<()> {
        Err(RenderFrameError::NotImplemented {
            operation: "build g-buffer-only render graph",
        })
    }

    fn build_debug_visualization(
        &mut self,
        _graph: &mut RenderGraphCameraBuildData,
        _camera_index: usize,
    ) -> RenderFrameResult<()> {
        Err(RenderFrameError::NotImplemented {
            operation: "build debug-visualization render graph",
        })
    }

    fn build_safe_mode(
        &mut self,
        _graph: &mut RenderGraphCameraBuildData,
        _camera_index: usize,
    ) -> RenderFrameResult<()> {
        Err(RenderFrameError::NotImplemented {
            operation: "build safe-mode render graph",
        })
    }
}
