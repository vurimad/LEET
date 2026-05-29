//! Render-side camera registry and per-frame camera preparation.

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::{Res, ResMut, Resource};

use crate::{
    ExtractedCamera, ExtractedCameras, ExtractedView, ExtractedViews, RenderViewportRect,
    SortedCameras,
};

use super::{
    CameraRenderSetupKey, FrameCameraView, FrameInput, RenderCameraId, RenderFrameError,
    RenderFrameResult, ViewClearState,
};

pub const MAX_CAMERA_DEPENDENCIES: usize = 8;
pub const MAX_CAMERA_DEPENDENCY_DEPTH: u8 = 1;
const TEMPORAL_CAMERA_GRACE_TICKS: u64 = 3;
const TEMPORAL_RENDER_FLOW_SPACE_START: u32 = 8;

pub struct CameraPrepareContext<'a> {
    pub frame: &'a FrameInput,
    pub allow_camera_jitter: bool,
    pub flush_temporaries: bool,
    pub render_viewport: RenderViewportRect,
}

impl<'a> CameraPrepareContext<'a> {
    pub fn from_frame(frame: &'a FrameInput) -> Self {
        Self {
            frame,
            allow_camera_jitter: frame.mode.allows_camera_jitter(),
            flush_temporaries: frame.mode.flushes_temporary_cameras(),
            render_viewport: frame.target.full_rect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedFrameCamera {
    pub camera_id: RenderCameraId,
    pub source_view_index: u32,
    pub render_flow_space: u32,
    pub viewport: RenderViewportRect,
    pub selected_dependencies: Vec<PreparedCameraDependency>,
    pub reset_temporal_history: bool,
    pub previous_frame: Option<PreparedCameraHistory>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedCameraDependency {
    pub camera_id: RenderCameraId,
    pub flags: CameraDependencyFlags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedCameraHistory {
    pub frame_index: u64,
    pub viewport: RenderViewportRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraManagement {
    Permanent,
    Temporal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraRenderPolicy {
    OnDemand,
    Always,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CameraDependencyFlags {
    bits: u8,
}

impl CameraDependencyFlags {
    const TEMPORARY: u8 = 1 << 0;
    const TEMPORARY_REQUESTED: u8 = 1 << 1;
    const OUTPUT_COLOR: u8 = 1 << 2;
    const OUTPUT_FINAL_COLOR: u8 = 1 << 3;

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn output_color() -> Self {
        Self {
            bits: Self::OUTPUT_COLOR,
        }
    }

    pub const fn output_final_color() -> Self {
        Self {
            bits: Self::OUTPUT_FINAL_COLOR,
        }
    }

    pub const fn bits(self) -> u8 {
        self.bits
    }

    pub const fn is_temporary(self) -> bool {
        self.bits & Self::TEMPORARY != 0
    }

    pub const fn was_requested_this_frame(self) -> bool {
        self.bits & Self::TEMPORARY_REQUESTED != 0
    }

    pub fn mark_temporary_requested(&mut self) {
        self.bits |= Self::TEMPORARY | Self::TEMPORARY_REQUESTED;
    }

    pub fn clear_temporary_requested(&mut self) {
        self.bits &= !Self::TEMPORARY_REQUESTED;
    }

    pub fn make_permanent(&mut self) {
        self.bits &= !(Self::TEMPORARY | Self::TEMPORARY_REQUESTED);
    }

    pub fn add_outputs(&mut self, outputs: Self) {
        self.bits |= outputs.bits & (Self::OUTPUT_COLOR | Self::OUTPUT_FINAL_COLOR);
    }
}

#[derive(Default, Resource)]
pub struct RenderCameraStorage {
    update_tick: u64,
    registry: HashMap<RenderCameraId, CameraRegistryEntry>,
    submitted_cameras: Vec<RenderCameraId>,
    selected_cameras: Vec<RenderCameraId>,
}

impl RenderCameraStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update_tick(&self) -> u64 {
        self.update_tick
    }

    pub fn registered_camera_count(&self) -> usize {
        self.registry.len()
    }

    pub fn selected_camera_ids(&self) -> &[RenderCameraId] {
        &self.selected_cameras
    }

    pub fn submitted_camera_ids(&self) -> &[RenderCameraId] {
        &self.submitted_cameras
    }

    pub fn camera_view(&self, camera_id: RenderCameraId) -> Option<&FrameCameraView> {
        self.registry.get(&camera_id).map(|entry| &entry.view)
    }

    pub fn sync_extracted_cameras(
        &mut self,
        sorted_cameras: &SortedCameras,
        extracted_cameras: &ExtractedCameras,
        extracted_views: &ExtractedViews,
    ) {
        let mut live_cameras = HashSet::new();
        self.submitted_cameras.clear();

        for sorted_camera in &sorted_cameras.0 {
            let Some(camera) = extracted_cameras.get(&sorted_camera.entity) else {
                continue;
            };
            let Some(view) = extracted_views.get(&sorted_camera.entity) else {
                continue;
            };

            let camera_view = frame_camera_view_from_extracted(
                sorted_camera.entity,
                camera,
                view,
                camera.sorted_camera_index_for_target as u32,
            );
            live_cameras.insert(camera_view.camera_id);
            self.submitted_cameras.push(camera_view.camera_id);
            self.register_camera(
                camera_view,
                CameraManagement::Permanent,
                CameraRenderPolicy::OnDemand,
            );
        }

        let stale_cameras = self
            .registry
            .iter()
            .filter_map(|(camera_id, entry)| {
                (entry.management == CameraManagement::Permanent
                    && !live_cameras.contains(camera_id))
                .then_some(*camera_id)
            })
            .collect::<Vec<_>>();

        for camera_id in stale_cameras {
            self.unregister_camera(camera_id);
        }
    }

    pub fn register_camera(
        &mut self,
        view: FrameCameraView,
        management: CameraManagement,
        render_policy: CameraRenderPolicy,
    ) -> bool {
        let camera_id = view.camera_id;
        let render_flow_space = self
            .registry
            .get(&camera_id)
            .map(|entry| entry.render_flow_space)
            .unwrap_or_else(|| self.allocate_render_flow_space(management));

        let is_new = !self.registry.contains_key(&camera_id);
        let entry = CameraRegistryEntry {
            view,
            management,
            render_policy,
            render_flow_space,
            enabled: true,
            last_update_tick: self.update_tick,
            min_dependency_depth: MAX_CAMERA_DEPENDENCY_DEPTH + 1,
            dependencies: self
                .registry
                .remove(&camera_id)
                .map(|entry| entry.dependencies)
                .unwrap_or_default(),
            last_prepared: None,
        };
        self.registry.insert(camera_id, entry);
        is_new
    }

    pub fn unregister_camera(&mut self, camera_id: RenderCameraId) -> bool {
        let removed = self.registry.remove(&camera_id).is_some();
        if removed {
            self.selected_cameras
                .retain(|selected| *selected != camera_id);
            self.remove_all_dependencies_to(camera_id);
        }
        removed
    }

    pub fn set_camera_enabled(
        &mut self,
        camera_id: RenderCameraId,
        enabled: bool,
    ) -> RenderFrameResult<()> {
        let entry =
            self.registry
                .get_mut(&camera_id)
                .ok_or(RenderFrameError::InvalidFrameInput {
                    reason: "camera id is not registered",
                })?;
        entry.enabled = enabled;
        Ok(())
    }

    pub fn add_camera_dependency(
        &mut self,
        parent: RenderCameraId,
        child: RenderCameraId,
        management: CameraManagement,
        output_flags: CameraDependencyFlags,
    ) -> RenderFrameResult<bool> {
        if parent == child {
            return Err(RenderFrameError::InvalidFrameInput {
                reason: "camera cannot depend on itself",
            });
        }

        if !self.registry.contains_key(&parent) || !self.registry.contains_key(&child) {
            return Ok(false);
        }

        if self.has_dependency(child, parent, true) {
            return Ok(false);
        }

        let parent_entry =
            self.registry
                .get_mut(&parent)
                .ok_or(RenderFrameError::InvalidFrameInput {
                    reason: "camera id is not registered",
                })?;

        if let Some(existing) = parent_entry
            .dependencies
            .iter_mut()
            .find(|dependency| dependency.camera_id == child)
        {
            match management {
                CameraManagement::Permanent => existing.flags.make_permanent(),
                CameraManagement::Temporal => existing.flags.mark_temporary_requested(),
            }
            existing.flags.add_outputs(output_flags);
            return Ok(true);
        }

        if parent_entry.dependencies.len() >= MAX_CAMERA_DEPENDENCIES {
            return Err(RenderFrameError::InvalidFrameInput {
                reason: "camera dependency limit exceeded",
            });
        }

        let mut flags = output_flags;
        if management == CameraManagement::Temporal {
            flags.mark_temporary_requested();
        }

        parent_entry.dependencies.push(CameraDependency {
            camera_id: child,
            flags,
        });
        Ok(true)
    }

    pub fn remove_camera_dependency(
        &mut self,
        parent: RenderCameraId,
        child: RenderCameraId,
    ) -> bool {
        let Some(parent_entry) = self.registry.get_mut(&parent) else {
            return false;
        };
        let old_len = parent_entry.dependencies.len();
        parent_entry
            .dependencies
            .retain(|dependency| dependency.camera_id != child);
        parent_entry.dependencies.len() != old_len
    }

    pub fn has_dependency(
        &self,
        parent: RenderCameraId,
        child: RenderCameraId,
        recursive: bool,
    ) -> bool {
        let Some(parent_entry) = self.registry.get(&parent) else {
            return false;
        };

        for dependency in &parent_entry.dependencies {
            if dependency.camera_id == child {
                return true;
            }
            if recursive && self.has_dependency(dependency.camera_id, child, true) {
                return true;
            }
        }

        false
    }

    pub fn prepare_frame_cameras(
        &mut self,
        ctx: CameraPrepareContext<'_>,
        requested: &[FrameCameraView],
    ) -> RenderFrameResult<Vec<PreparedFrameCamera>> {
        self.flush_temporal_state(ctx.flush_temporaries);

        let requested_ids = requested
            .iter()
            .map(|view| {
                self.register_camera(
                    view.clone(),
                    CameraManagement::Permanent,
                    CameraRenderPolicy::OnDemand,
                );
                view.camera_id
            })
            .collect::<Vec<_>>();

        self.resolve_dependency_depths(&requested_ids);
        self.select_cameras()?;
        self.sort_selected_cameras();

        let stable_frame = ctx.frame.purpose.requires_stable_dissolves()
            || ctx.frame.debug.stable_dissolves
            || !ctx.allow_camera_jitter;
        let mut prepared = Vec::with_capacity(self.selected_cameras.len());

        for camera_id in self.selected_cameras.iter().copied() {
            let entry =
                self.registry
                    .get_mut(&camera_id)
                    .ok_or(RenderFrameError::InvalidFrameInput {
                        reason: "selected camera id is not registered",
                    })?;

            let previous_frame = entry.last_prepared;
            let reset_temporal_history = stable_frame || previous_frame.is_none();
            let prepared_history = PreparedCameraHistory {
                frame_index: ctx.frame.timing.frame_index,
                viewport: entry.view.viewport,
            };
            entry.last_prepared = Some(prepared_history);

            prepared.push(PreparedFrameCamera {
                camera_id,
                source_view_index: entry.view.target_view_index,
                render_flow_space: entry.render_flow_space,
                viewport: entry.view.viewport,
                selected_dependencies: entry
                    .dependencies
                    .iter()
                    .filter(|dependency| self.selected_cameras.contains(&dependency.camera_id))
                    .map(|dependency| PreparedCameraDependency {
                        camera_id: dependency.camera_id,
                        flags: dependency.flags,
                    })
                    .collect(),
                reset_temporal_history,
                previous_frame,
            });
        }

        self.update_tick += 1;
        Ok(prepared)
    }

    fn flush_temporal_state(&mut self, flush_temporaries: bool) {
        if flush_temporaries {
            for entry in self.registry.values_mut() {
                entry.dependencies.retain_mut(|dependency| {
                    if dependency.flags.is_temporary()
                        && !dependency.flags.was_requested_this_frame()
                    {
                        return false;
                    }
                    dependency.flags.clear_temporary_requested();
                    true
                });
            }
        }

        let stale_cameras = self
            .registry
            .iter()
            .filter_map(|(camera_id, entry)| {
                let stale = entry.management == CameraManagement::Temporal
                    && entry.last_update_tick > 0
                    && entry.last_update_tick + TEMPORAL_CAMERA_GRACE_TICKS < self.update_tick;
                stale.then_some(*camera_id)
            })
            .collect::<Vec<_>>();

        for camera_id in stale_cameras {
            self.unregister_camera(camera_id);
        }
    }

    fn resolve_dependency_depths(&mut self, requested_ids: &[RenderCameraId]) {
        let requested = requested_ids.iter().copied().collect::<HashSet<_>>();
        for (camera_id, entry) in &mut self.registry {
            if entry.enabled
                && (entry.render_policy == CameraRenderPolicy::Always
                    || requested.contains(camera_id))
            {
                entry.min_dependency_depth = 0;
            } else {
                entry.min_dependency_depth = MAX_CAMERA_DEPENDENCY_DEPTH + 1;
            }
        }

        loop {
            let mut changed = false;
            let snapshot = self
                .registry
                .iter()
                .map(|(camera_id, entry)| {
                    (
                        *camera_id,
                        entry.min_dependency_depth,
                        entry
                            .dependencies
                            .iter()
                            .map(|dependency| dependency.camera_id)
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();

            for (_, parent_depth, dependencies) in snapshot {
                let child_depth = parent_depth.saturating_add(1);
                for child_id in dependencies {
                    let Some(child_entry) = self.registry.get_mut(&child_id) else {
                        continue;
                    };
                    if child_depth < child_entry.min_dependency_depth {
                        child_entry.min_dependency_depth = child_depth;
                        changed = true;
                    }
                }
            }

            if !changed {
                break;
            }
        }
    }

    fn select_cameras(&mut self) -> RenderFrameResult<()> {
        self.selected_cameras.clear();

        let selected = self
            .registry
            .iter()
            .filter_map(|(camera_id, entry)| {
                (entry.enabled && entry.min_dependency_depth <= MAX_CAMERA_DEPENDENCY_DEPTH)
                    .then_some(*camera_id)
            })
            .collect::<Vec<_>>();

        for camera_id in selected {
            let entry =
                self.registry
                    .get(&camera_id)
                    .ok_or(RenderFrameError::InvalidFrameInput {
                        reason: "camera id is not registered",
                    })?;
            if entry.dependencies.len() > MAX_CAMERA_DEPENDENCIES {
                return Err(RenderFrameError::InvalidFrameInput {
                    reason: "camera dependency limit exceeded",
                });
            }
            self.selected_cameras.push(camera_id);
        }

        Ok(())
    }

    fn sort_selected_cameras(&mut self) {
        let registry = &self.registry;
        self.selected_cameras.sort_by(|left, right| {
            let left_entry = &registry[left];
            let right_entry = &registry[right];

            if depends_on(left_entry, *right) {
                return std::cmp::Ordering::Greater;
            }
            if depends_on(right_entry, *left) {
                return std::cmp::Ordering::Less;
            }

            left_entry
                .view
                .camera_order
                .cmp(&right_entry.view.camera_order)
                .then(
                    left_entry
                        .view
                        .target_view_index
                        .cmp(&right_entry.view.target_view_index),
                )
                .then(left.0.cmp(&right.0))
        });
    }

    fn remove_all_dependencies_to(&mut self, child: RenderCameraId) {
        for entry in self.registry.values_mut() {
            entry
                .dependencies
                .retain(|dependency| dependency.camera_id != child);
        }
    }

    fn allocate_render_flow_space(&self, management: CameraManagement) -> u32 {
        let start = match management {
            CameraManagement::Permanent => 0,
            CameraManagement::Temporal => TEMPORAL_RENDER_FLOW_SPACE_START,
        };
        let used = self
            .registry
            .values()
            .map(|entry| entry.render_flow_space)
            .collect::<HashSet<_>>();

        let mut candidate = start;
        while used.contains(&candidate) {
            candidate += 1;
        }
        candidate
    }
}

pub fn sync_render_camera_storage(
    sorted_cameras: Res<SortedCameras>,
    extracted_cameras: Res<ExtractedCameras>,
    extracted_views: Res<ExtractedViews>,
    mut camera_storage: ResMut<RenderCameraStorage>,
) {
    camera_storage.sync_extracted_cameras(&sorted_cameras, &extracted_cameras, &extracted_views);
}

#[derive(Clone, Debug)]
struct CameraRegistryEntry {
    view: FrameCameraView,
    management: CameraManagement,
    render_policy: CameraRenderPolicy,
    render_flow_space: u32,
    enabled: bool,
    last_update_tick: u64,
    min_dependency_depth: u8,
    dependencies: Vec<CameraDependency>,
    last_prepared: Option<PreparedCameraHistory>,
}

#[derive(Clone, Copy, Debug)]
struct CameraDependency {
    camera_id: RenderCameraId,
    flags: CameraDependencyFlags,
}

fn depends_on(entry: &CameraRegistryEntry, camera_id: RenderCameraId) -> bool {
    entry
        .dependencies
        .iter()
        .any(|dependency| dependency.camera_id == camera_id)
}

pub fn frame_camera_view_from_extracted(
    camera_entity: bevy_ecs::entity::Entity,
    camera: &ExtractedCamera,
    view: &ExtractedView,
    target_view_index: u32,
) -> FrameCameraView {
    FrameCameraView {
        camera_entity,
        camera_id: RenderCameraId(camera_entity.to_bits()),
        camera_order: camera.order,
        target_view_index,
        viewport: RenderViewportRect::new(
            view.viewport.x,
            view.viewport.y,
            view.viewport.z,
            view.viewport.w,
        ),
        clear: ViewClearState::default(),
        camera: camera.clone(),
        view: view.clone(),
        render_setup: camera_render_setup_key(camera.schedule),
    }
}

pub fn camera_render_setup_key(
    schedule: bevy_ecs::schedule::InternedScheduleLabel,
) -> CameraRenderSetupKey {
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
    };

    let mut hasher = DefaultHasher::new();
    schedule.hash(&mut hasher);
    CameraRenderSetupKey(hasher.finish())
}

#[cfg(test)]
mod tests {
    use bevy_camera::{CameraOutputMode, ClearColorConfig, MsaaWriteback};
    use bevy_ecs::world::World;
    use bevy_math::{Mat4, UVec2, UVec4};
    use bevy_transform::components::GlobalTransform;

    use crate::{
        CameraRenderGraph, ExtractedCamera, ExtractedView, FrameCameraView, FrameInput,
        FrameRenderingMode, FrameTarget, FrameTargetKey, FrameTiming, RenderSceneId,
        RenderViewportRect, ViewClearState,
    };

    use super::*;

    fn camera_view(raw_id: u64, order: isize, view_index: u32) -> FrameCameraView {
        let mut world = World::new();
        let entity = world.spawn_empty().id();

        FrameCameraView {
            camera_entity: entity,
            camera_id: RenderCameraId(raw_id),
            camera_order: order,
            target_view_index: view_index,
            viewport: RenderViewportRect::new(0, 0, 128, 72),
            clear: ViewClearState::default(),
            camera: ExtractedCamera {
                target: None,
                physical_viewport_size: Some(UVec2::new(128, 72)),
                physical_target_size: Some(UVec2::new(128, 72)),
                viewport: None,
                schedule: CameraRenderGraph::default().0,
                order,
                output_mode: CameraOutputMode::default(),
                msaa_writeback: MsaaWriteback::default(),
                clear_color: ClearColorConfig::Default,
                sorted_camera_index_for_target: view_index as usize,
                exposure: 1.0,
                hdr: false,
                compositing_space: None,
            },
            view: ExtractedView {
                clip_from_view: Mat4::IDENTITY,
                world_from_view: GlobalTransform::IDENTITY,
                target_format: wgpu::TextureFormat::Rgba8UnormSrgb,
                viewport: UVec4::new(0, 0, 128, 72),
                invert_culling: false,
            },
            render_setup: crate::CameraRenderSetupKey(raw_id),
        }
    }

    fn frame(camera_views: Vec<FrameCameraView>, frame_index: u64) -> FrameInput {
        FrameInput {
            target: FrameTarget {
                key: FrameTargetKey::External(1),
                extent: UVec2::new(128, 72),
                format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            },
            camera_views,
            scene: RenderSceneId(1),
            timing: FrameTiming {
                frame_index,
                ..FrameTiming::default()
            },
            mode: FrameRenderingMode::Shaded,
            purpose: crate::FramePurpose::Normal,
            presentation: crate::PresentationIntent::NoPresent,
            capture: crate::FrameCaptureIntent::None,
            debug: crate::FrameDebugIntent::default(),
        }
    }

    #[test]
    fn selected_dependency_camera_sorts_before_parent_camera() {
        let main = camera_view(1, 1, 0);
        let mirror = camera_view(2, 0, 1);
        let mut storage = RenderCameraStorage::new();

        storage.register_camera(
            main.clone(),
            CameraManagement::Permanent,
            CameraRenderPolicy::OnDemand,
        );
        storage.register_camera(
            mirror.clone(),
            CameraManagement::Permanent,
            CameraRenderPolicy::OnDemand,
        );
        storage
            .add_camera_dependency(
                main.camera_id,
                mirror.camera_id,
                CameraManagement::Permanent,
                CameraDependencyFlags::output_color(),
            )
            .unwrap();

        let frame = frame(vec![main.clone()], 1);
        let prepared = storage
            .prepare_frame_cameras(
                CameraPrepareContext::from_frame(&frame),
                &frame.camera_views,
            )
            .unwrap();

        assert_eq!(
            prepared
                .iter()
                .map(|camera| camera.camera_id)
                .collect::<Vec<_>>(),
            vec![mirror.camera_id, main.camera_id]
        );
        assert_eq!(
            prepared[1].selected_dependencies[0].camera_id,
            mirror.camera_id
        );
    }

    #[test]
    fn dependency_depth_limit_excludes_recursive_grandchild_camera() {
        let main = camera_view(1, 2, 0);
        let mirror = camera_view(2, 1, 1);
        let mirror_child = camera_view(3, 0, 2);
        let mut storage = RenderCameraStorage::new();

        for view in [main.clone(), mirror.clone(), mirror_child.clone()] {
            storage.register_camera(
                view,
                CameraManagement::Permanent,
                CameraRenderPolicy::OnDemand,
            );
        }
        storage
            .add_camera_dependency(
                main.camera_id,
                mirror.camera_id,
                CameraManagement::Permanent,
                CameraDependencyFlags::output_color(),
            )
            .unwrap();
        storage
            .add_camera_dependency(
                mirror.camera_id,
                mirror_child.camera_id,
                CameraManagement::Permanent,
                CameraDependencyFlags::output_color(),
            )
            .unwrap();

        let frame = frame(vec![main.clone()], 1);
        let prepared = storage
            .prepare_frame_cameras(
                CameraPrepareContext::from_frame(&frame),
                &frame.camera_views,
            )
            .unwrap();

        assert_eq!(
            prepared
                .iter()
                .map(|camera| camera.camera_id)
                .collect::<Vec<_>>(),
            vec![mirror.camera_id, main.camera_id]
        );
    }

    #[test]
    fn camera_dependency_rejects_cycles() {
        let first = camera_view(1, 0, 0);
        let second = camera_view(2, 1, 1);
        let mut storage = RenderCameraStorage::new();

        for view in [first.clone(), second.clone()] {
            storage.register_camera(
                view,
                CameraManagement::Permanent,
                CameraRenderPolicy::OnDemand,
            );
        }

        assert!(storage
            .add_camera_dependency(
                first.camera_id,
                second.camera_id,
                CameraManagement::Permanent,
                CameraDependencyFlags::empty(),
            )
            .unwrap());
        assert!(!storage
            .add_camera_dependency(
                second.camera_id,
                first.camera_id,
                CameraManagement::Permanent,
                CameraDependencyFlags::empty(),
            )
            .unwrap());
    }

    #[test]
    fn temporal_dependency_is_removed_when_not_requested_again() {
        let main = camera_view(1, 1, 0);
        let temp = camera_view(2, 0, 1);
        let mut storage = RenderCameraStorage::new();

        storage.register_camera(
            main.clone(),
            CameraManagement::Permanent,
            CameraRenderPolicy::OnDemand,
        );
        storage.register_camera(
            temp.clone(),
            CameraManagement::Temporal,
            CameraRenderPolicy::OnDemand,
        );
        storage
            .add_camera_dependency(
                main.camera_id,
                temp.camera_id,
                CameraManagement::Temporal,
                CameraDependencyFlags::output_color(),
            )
            .unwrap();

        let first_frame = frame(vec![main.clone()], 1);
        let first_prepared = storage
            .prepare_frame_cameras(
                CameraPrepareContext::from_frame(&first_frame),
                &first_frame.camera_views,
            )
            .unwrap();
        assert_eq!(first_prepared.len(), 2);

        let second_frame = frame(vec![main.clone()], 2);
        let second_prepared = storage
            .prepare_frame_cameras(
                CameraPrepareContext::from_frame(&second_frame),
                &second_frame.camera_views,
            )
            .unwrap();
        assert_eq!(
            second_prepared
                .iter()
                .map(|camera| camera.camera_id)
                .collect::<Vec<_>>(),
            vec![main.camera_id]
        );
    }
}
