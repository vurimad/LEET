//! Per-frame renderer execution.
//!
//! This layer consumes viewport-produced frame submissions plus a renderer-owned
//! scene, then builds and executes the current placeholder render graph.
//! It exists to keep the `begin_frame/submit_frame` viewport path
//! separate from the lower-level `Renderer`, which owns device/surface state.

use crate::frame_submission::{RenderFramePurpose, RenderFrameSubmission};
use crate::render_collector::{CollectedRenderScene, RenderCollector};
use crate::render_graph::RenderGraph;
use crate::render_node::{
    BloomNode, ClearBackbufferNode, EndFrameNode, MainPassRootNode, OpaqueDrawsNode,
    RenderExecutionPlan, RenderNodeDependencyType, SkyDrawsNode, StartFrameNode,
};
use crate::render_scene::{RenderSceneProxy, RenderSceneType};
use crate::renderer::Renderer;
use leet_core::{Leeror, LeetResult};
use std::sync::Arc;

/// Executes one submitted frame against a [`Renderer`].
#[derive(Default)]
pub struct FrameRenderer;

impl FrameRenderer {
    /// Create a frame renderer.
    pub fn new() -> Self {
        Self
    }

    /// Consume one viewport frame submission and render it.
    pub fn render(
        &mut self,
        renderer: &mut Renderer,
        submission: &RenderFrameSubmission,
        scene: &RenderSceneProxy,
    ) -> LeetResult<()> {
        Self::validate_submission(renderer, submission)?;

        let plan = self.compile_plan(submission, scene)?;
        let frame = renderer.begin_frame(plan.command_list_count())?;
        plan.execute(&frame)?;

        if submission.frame_info().present() {
            frame.present();
        }

        Ok(())
    }

    fn compile_plan(
        &self,
        submission: &RenderFrameSubmission,
        scene: &RenderSceneProxy,
    ) -> LeetResult<RenderExecutionPlan> {
        // The current placeholder renderer does not yet consume the camera data,
        // but the submission boundary is in place for the upcoming camera pass.
        let _camera_info = submission.camera_info();

        let collected_scene = RenderCollector::collect(scene)?;

        match (submission.frame_info().purpose(), scene.scene_type()) {
            (RenderFramePurpose::Blank, _) | (_, RenderSceneType::NoScene) => {
                Self::build_blank_plan(collected_scene.clear_color())
            }
            _ => Self::build_scene_plan(collected_scene),
        }
    }

    fn build_blank_plan(clear_color: wgpu::Color) -> LeetResult<RenderExecutionPlan> {
        let mut graph = RenderGraph::new();
        let clear = graph.add_node(ClearBackbufferNode::new(clear_color));
        let submit = graph.add_submit_barrier("Submit_Blank");
        graph.add_cpu_dependency(submit, clear)?;
        graph.compile()
    }

    fn build_scene_plan(
        collected_scene: Arc<CollectedRenderScene>,
    ) -> LeetResult<RenderExecutionPlan> {
        let mut graph = RenderGraph::new();
        let start = graph.add_node(StartFrameNode::new());
        let main_pass = graph.add_node(MainPassRootNode::for_scene(
            Arc::clone(&collected_scene),
            collected_scene.clear_color(),
        ));
        let opaque = graph.add_node(OpaqueDrawsNode::for_scene(Arc::clone(&collected_scene)));
        let sky = graph.add_node(SkyDrawsNode::for_scene(Arc::clone(&collected_scene)));
        let bloom = graph.add_node(BloomNode::for_scene(collected_scene));
        let end = graph.add_node(EndFrameNode::new());

        graph.add_dependency(main_pass, start, RenderNodeDependencyType::Cpu)?;
        graph.add_dependency(opaque, main_pass, RenderNodeDependencyType::Cpu)?;
        graph.add_dependency(sky, main_pass, RenderNodeDependencyType::Cpu)?;
        graph.add_dependency(bloom, main_pass, RenderNodeDependencyType::Gpu)?;
        graph.add_dependency(end, bloom, RenderNodeDependencyType::Cpu)?;

        graph.compile()
    }

    fn validate_submission(
        renderer: &Renderer,
        submission: &RenderFrameSubmission,
    ) -> LeetResult<()> {
        let frame_info = submission.frame_info();
        let viewport = renderer
            .main_viewport()
            .ok_or_else(|| Leeror::Runtime("renderer has no main viewport yet".to_string()))?;

        if frame_info.viewport_name() != viewport.name() {
            return Err(Leeror::Runtime(format!(
                "submission viewport '{}' does not match renderer viewport '{}'",
                frame_info.viewport_name(),
                viewport.name(),
            )));
        }

        if frame_info.viewport_size() != viewport.size() {
            return Err(Leeror::Runtime(format!(
                "submission viewport size {:?} does not match renderer viewport {:?}",
                frame_info.viewport_size(),
                viewport.size(),
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_submission::RenderCameraFrameInfo;
    use crate::render_node::RenderExecutionStep;
    use crate::render_proxy::{RenderProxyDescriptor, RenderProxyKind};
    use crate::render_viewport::RenderViewport;

    #[test]
    fn compile_plan_uses_blank_path_for_blank_frames() {
        let viewport = RenderViewport::detached("PreviewViewport", (512, 512));
        let frame_info = viewport.begin_frame_for_purpose(RenderFramePurpose::Blank, false);
        let submission = viewport
            .submit_frame(frame_info, RenderCameraFrameInfo::default())
            .unwrap();
        let scene = RenderSceneProxy::new();
        scene.set_clear_color(wgpu::Color::RED).unwrap();
        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();

        let plan = FrameRenderer::new()
            .compile_plan(&submission, &scene)
            .unwrap();

        assert_eq!(plan.command_list_count(), 1);
        assert_eq!(plan.steps().len(), 2);
        assert!(matches!(plan.steps()[0], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[1], RenderExecutionStep::Frame(_)));
    }

    #[test]
    fn compile_plan_uses_blank_path_for_no_scene() {
        let viewport = RenderViewport::main_window((800, 600));
        let submission = viewport
            .submit_frame(viewport.begin_frame(), RenderCameraFrameInfo::default())
            .unwrap();
        let scene = RenderSceneProxy::with_type(RenderSceneType::NoScene);
        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();

        let plan = FrameRenderer::new()
            .compile_plan(&submission, &scene)
            .unwrap();

        assert_eq!(plan.command_list_count(), 1);
        assert_eq!(plan.steps().len(), 2);
    }

    #[test]
    fn compile_plan_uses_scene_path_for_world_scene() {
        let viewport = RenderViewport::main_window((1280, 720));
        let submission = viewport
            .submit_frame(viewport.begin_frame(), RenderCameraFrameInfo::default())
            .unwrap();
        let scene = RenderSceneProxy::new();
        scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Opaque).named("Opaque"))
            .unwrap();
        scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Sky).named("Sky"))
            .unwrap();
        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();

        let plan = FrameRenderer::new()
            .compile_plan(&submission, &scene)
            .unwrap();

        assert_eq!(plan.command_list_count(), 2);
        assert_eq!(plan.steps().len(), 6);
        assert!(matches!(plan.steps()[0], RenderExecutionStep::Frame(_)));
        assert!(matches!(plan.steps()[1], RenderExecutionStep::Record(_)));
        assert!(matches!(plan.steps()[2], RenderExecutionStep::Frame(_)));
        assert!(matches!(plan.steps()[3], RenderExecutionStep::Record(_)));
    }
}
