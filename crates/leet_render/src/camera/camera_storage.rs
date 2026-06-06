//! Render-side camera registry and per-frame camera preparation.

use std::collections::{HashMap, HashSet};

use bevy_ecs::{
    entity::{Entity, EntityHashMap},
    prelude::{ResMut, Resource},
};

use crate::{RenderCamera, RenderCameraId, RenderFrameError, RenderFrameResult};
use bevy_math::URect;

pub const MAX_CAMERA_DEPENDENCIES: usize = 8;
pub const MAX_CAMERA_DEPENDENCY_DEPTH: u8 = 1;
const TEMPORAL_CAMERA_GRACE_TICKS: u64 = 3;
const TEMPORAL_RENDER_FLOW_SPACE_START: u32 = 8;

pub struct CameraPrepareContext {
    pub allow_camera_jitter: bool,
    pub flush_temporaries: bool,
    pub stable_dissolves: bool,
    pub frame_index: u64,
    pub render_viewport: URect,
}

impl CameraPrepareContext {
    pub fn new(
        allow_camera_jitter: bool,
        flush_temporaries: bool,
        stable_dissolves: bool,
        frame_index: u64,
        render_viewport: URect,
    ) -> Self {
        Self {
            allow_camera_jitter,
            flush_temporaries,
            stable_dissolves,
            frame_index,
            render_viewport,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderCameraRegistration {
    pub camera_id: RenderCameraId,
    pub camera_entity: Entity,
    pub camera: RenderCamera,
    // TODO(Camera submission): this is still target-submission metadata. Keep
    // it here only until frame assembly gets its own camera submission stage.
    pub target_view_index: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderCameraRegistrationRef<'a> {
    pub camera_id: RenderCameraId,
    pub camera_entity: Entity,
    pub camera: &'a RenderCamera,
    pub target_view_index: u32,
}

#[derive(Clone, Debug)]
pub struct PreparedFrameCamera {
    pub camera_id: RenderCameraId,
    pub camera: RenderCamera,
    pub source_view_index: u32,
    pub render_flow_space: u32,
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
    pub viewport: URect,
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
    extracted_cameras: EntityHashMap<RenderCamera>,
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

    pub fn clear_extracted_cameras(&mut self) {
        self.extracted_cameras.clear();
    }

    pub fn insert_extracted_camera(&mut self, entity: Entity, camera: RenderCamera) {
        self.extracted_cameras.insert(entity, camera);
    }

    pub fn registered_camera(
        &self,
        camera_id: RenderCameraId,
    ) -> Option<RenderCameraRegistrationRef<'_>> {
        self.registry
            .get(&camera_id)
            .map(|entry| RenderCameraRegistrationRef {
                camera_id,
                camera_entity: entry.camera_entity,
                camera: &entry.camera,
                target_view_index: entry.target_view_index,
            })
    }

    pub fn sync_extracted_cameras(&mut self) {
        let mut live_cameras = HashSet::new();
        self.submitted_cameras.clear();

        let mut sorted_entities = self.extracted_cameras.keys().copied().collect::<Vec<_>>();
        sorted_entities.sort_by(|left, right| {
            let left_camera = self
                .extracted_cameras
                .get(left)
                .expect("sorted camera entity came from live camera map");
            let right_camera = self
                .extracted_cameras
                .get(right)
                .expect("sorted camera entity came from live camera map");

            (left_camera.order, &left_camera.target)
                .cmp(&(right_camera.order, &right_camera.target))
                .then(left.to_bits().cmp(&right.to_bits()))
        });

        let mut target_counts = HashMap::new();
        let mut submitted_registrations = Vec::with_capacity(sorted_entities.len());
        for entity in sorted_entities {
            let Some(camera) = self.extracted_cameras.get(&entity) else {
                continue;
            };
            let target_view_index = if let Some(target) = &camera.target {
                let count = target_counts
                    .entry((target.clone(), camera.hdr))
                    .or_insert(0usize);
                let target_view_index = *count as u32;
                *count += 1;
                target_view_index
            } else {
                0
            };

            let camera_id = RenderCameraId(entity.to_bits());
            live_cameras.insert(camera_id);
            self.submitted_cameras.push(camera_id);
            submitted_registrations.push(RenderCameraRegistration {
                camera_id,
                camera_entity: entity,
                camera: camera.clone(),
                target_view_index,
            });
        }

        for registration in submitted_registrations {
            self.register_camera(
                registration,
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
        registration: RenderCameraRegistration,
        management: CameraManagement,
        render_policy: CameraRenderPolicy,
    ) -> bool {
        let camera_id = registration.camera_id;
        let render_flow_space = self
            .registry
            .get(&camera_id)
            .map(|entry| entry.render_flow_space)
            .unwrap_or_else(|| self.allocate_render_flow_space(management));

        let is_new = !self.registry.contains_key(&camera_id);
        let entry = CameraRegistryEntry {
            camera_entity: registration.camera_entity,
            camera: registration.camera,
            target_view_index: registration.target_view_index,
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
        ctx: CameraPrepareContext,
        requested: &[RenderCameraId],
    ) -> RenderFrameResult<Vec<PreparedFrameCamera>> {
        self.flush_temporal_state(ctx.flush_temporaries);

        let requested_ids = requested.to_vec();

        self.resolve_dependency_depths(&requested_ids);
        self.select_cameras()?;
        self.sort_selected_cameras();

        let stable_frame = ctx.stable_dissolves || !ctx.allow_camera_jitter;
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
                frame_index: ctx.frame_index,
                viewport: entry.camera.viewport,
            };
            entry.last_prepared = Some(prepared_history);

            prepared.push(PreparedFrameCamera {
                camera_id,
                camera: entry.camera.clone(),
                source_view_index: entry.target_view_index,
                render_flow_space: entry.render_flow_space,
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
                .camera
                .order
                .cmp(&right_entry.camera.order)
                .then(
                    left_entry
                        .target_view_index
                        .cmp(&right_entry.target_view_index),
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

pub fn sync_render_camera_storage(mut camera_storage: ResMut<RenderCameraStorage>) {
    camera_storage.sync_extracted_cameras();
}

#[derive(Clone, Debug)]
struct CameraRegistryEntry {
    camera_entity: Entity,
    camera: RenderCamera,
    // TODO(Camera submission): remove this from registry storage once the
    // frame dispatcher owns submitted-camera ordering outright.
    target_view_index: u32,
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

#[cfg(test)]
#[path = "../tests/camera/camera_storage.rs"]
mod tests;
