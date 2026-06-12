use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::fmt;

use bevy_ecs::prelude::Resource;
use bevy_math::UVec2;

use crate::render_graph::graph::{RenderGraphError, RenderGraphResult};
use crate::{
    FrameDebugIntent, FramePurpose, FrameRenderingMode, FrameTiming, PreparedFrameCamera,
    PreparedFrameViews, RenderCameraId,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RenderSceneId(pub u64);

#[derive(Clone, Copy, Debug)]
pub struct FrameCustomDataPrepareContext {
    pub scene_id: RenderSceneId,
    pub timing: FrameTiming,
    pub mode: FrameRenderingMode,
    pub purpose: FramePurpose,
    pub debug: FrameDebugIntent,
    pub viewport_extent: UVec2,
    pub dispatcher_thread_index: u32,
}

#[derive(Default)]
pub struct PreparedCustomDataSet {
    data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl PreparedCustomDataSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T>(&mut self, data: T)
    where
        T: Send + Sync + 'static,
    {
        self.data.insert(TypeId::of::<T>(), Box::new(data));
    }

    pub fn custom<T>(&self) -> RenderGraphResult<&T>
    where
        T: Send + Sync + 'static,
    {
        self.data
            .get(&TypeId::of::<T>())
            .and_then(|data| data.downcast_ref::<T>())
            .ok_or(RenderGraphError::InvalidState {
                reason: "prepared custom data is not available",
            })
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl fmt::Debug for PreparedCustomDataSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedCustomDataSet")
            .field("len", &self.data.len())
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct PreparedSceneCustomData {
    custom: PreparedCustomDataSet,
}

impl PreparedSceneCustomData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn custom<T>(&self) -> RenderGraphResult<&T>
    where
        T: Send + Sync + 'static,
    {
        self.custom.custom::<T>()
    }

    pub fn custom_mut(&mut self) -> &mut PreparedCustomDataSet {
        &mut self.custom
    }
}

#[derive(Debug)]
pub struct PreparedCameraCustomData {
    pub camera_id: RenderCameraId,
    custom: PreparedCustomDataSet,
}

impl PreparedCameraCustomData {
    pub fn new(camera_id: RenderCameraId) -> Self {
        Self {
            camera_id,
            custom: PreparedCustomDataSet::new(),
        }
    }

    pub fn custom<T>(&self) -> RenderGraphResult<&T>
    where
        T: Send + Sync + 'static,
    {
        self.custom.custom::<T>()
    }

    pub fn custom_mut(&mut self) -> &mut PreparedCustomDataSet {
        &mut self.custom
    }
}

#[derive(Debug)]
pub struct PreparedFrameSceneData {
    pub scene_id: RenderSceneId,
    scene_data: PreparedSceneCustomData,
    cameras: HashMap<RenderCameraId, PreparedCameraCustomData>,
}

impl PreparedFrameSceneData {
    pub fn new(
        scene_id: RenderSceneId,
        scene_data: PreparedSceneCustomData,
        cameras: HashMap<RenderCameraId, PreparedCameraCustomData>,
    ) -> Self {
        Self {
            scene_id,
            scene_data,
            cameras,
        }
    }

    pub fn scene_data(&self) -> &PreparedSceneCustomData {
        &self.scene_data
    }

    pub fn camera(
        &self,
        camera_id: RenderCameraId,
    ) -> RenderGraphResult<&PreparedCameraCustomData> {
        self.cameras
            .get(&camera_id)
            .ok_or(RenderGraphError::InvalidState {
                reason: "prepared camera custom data is not available",
            })
    }

    pub fn is_empty(&self) -> bool {
        self.scene_data.custom.is_empty() && self.cameras.is_empty()
    }
}

pub trait PerCameraStorageCustomData: Send + Sync {
    fn initialize(&mut self) {}

    fn prepare(
        &mut self,
        _ctx: &FrameCustomDataPrepareContext,
        _camera: &PreparedFrameCamera,
        _out: &mut PreparedCustomDataSet,
    ) -> RenderGraphResult<()> {
        Ok(())
    }

    fn evict(&mut self) {}

    fn rendering_ready_blocker(&self) -> Option<&'static str> {
        None
    }
}

pub trait SceneStorageCustomData: Send + Sync {
    fn initialize(&mut self) {}

    fn prepare(
        &mut self,
        _ctx: &FrameCustomDataPrepareContext,
        _cameras: &PreparedFrameViews,
        _out: &mut PreparedCustomDataSet,
    ) -> RenderGraphResult<()> {
        Ok(())
    }

    fn evict(&mut self) {}

    fn rendering_ready_blocker(&self) -> Option<&'static str> {
        None
    }
}

type PerCameraStorageCustomDataFactory =
    Box<dyn Fn() -> Box<dyn PerCameraStorageCustomData> + Send + Sync>;
type SceneStorageCustomDataFactory = Box<dyn Fn() -> Box<dyn SceneStorageCustomData> + Send + Sync>;

#[derive(Default)]
pub struct PerCameraStorageCustomDataSet {
    data: Vec<Box<dyn PerCameraStorageCustomData>>,
}

impl PerCameraStorageCustomDataSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push<D>(&mut self, mut data: D)
    where
        D: PerCameraStorageCustomData + 'static,
    {
        data.initialize();
        self.data.push(Box::new(data));
    }

    pub fn push_boxed(&mut self, mut data: Box<dyn PerCameraStorageCustomData>) {
        data.initialize();
        self.data.push(data);
    }

    fn from_factories(factories: &[PerCameraStorageCustomDataFactory]) -> Self {
        let mut set = Self::new();
        for factory in factories {
            set.push_boxed(factory());
        }
        set
    }

    pub fn prepare(
        &mut self,
        ctx: &FrameCustomDataPrepareContext,
        camera: &PreparedFrameCamera,
        out: &mut PreparedCustomDataSet,
    ) -> RenderGraphResult<()> {
        for data in &mut self.data {
            data.prepare(ctx, camera, out)?;
        }

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl fmt::Debug for PerCameraStorageCustomDataSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerCameraStorageCustomDataSet")
            .field("len", &self.data.len())
            .finish()
    }
}

#[derive(Default)]
pub struct SceneStorageCustomDataSet {
    data: Vec<Box<dyn SceneStorageCustomData>>,
}

impl SceneStorageCustomDataSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push<D>(&mut self, mut data: D)
    where
        D: SceneStorageCustomData + 'static,
    {
        data.initialize();
        self.data.push(Box::new(data));
    }

    pub fn push_boxed(&mut self, mut data: Box<dyn SceneStorageCustomData>) {
        data.initialize();
        self.data.push(data);
    }

    fn from_factories(factories: &[SceneStorageCustomDataFactory]) -> Self {
        let mut set = Self::new();
        for factory in factories {
            set.push_boxed(factory());
        }
        set
    }

    pub fn prepare(
        &mut self,
        ctx: &FrameCustomDataPrepareContext,
        cameras: &PreparedFrameViews,
        out: &mut PreparedCustomDataSet,
    ) -> RenderGraphResult<()> {
        for data in &mut self.data {
            data.prepare(ctx, cameras, out)?;
        }

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl fmt::Debug for SceneStorageCustomDataSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SceneStorageCustomDataSet")
            .field("len", &self.data.len())
            .finish()
    }
}

#[derive(Debug)]
pub struct PersistentRenderSceneData {
    scene_custom_data: SceneStorageCustomDataSet,
    per_camera_custom_data: HashMap<RenderCameraId, PerCameraStorageCustomDataSet>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PersistentRenderSceneDataSyncReport {
    pub cameras_added: usize,
    pub cameras_removed: usize,
}

impl PersistentRenderSceneData {
    pub fn new() -> Self {
        Self {
            scene_custom_data: SceneStorageCustomDataSet::new(),
            per_camera_custom_data: HashMap::new(),
        }
    }

    fn from_scene_factories(factories: &[SceneStorageCustomDataFactory]) -> Self {
        Self {
            scene_custom_data: SceneStorageCustomDataSet::from_factories(factories),
            per_camera_custom_data: HashMap::new(),
        }
    }

    pub fn scene_custom_data(&self) -> &SceneStorageCustomDataSet {
        &self.scene_custom_data
    }

    pub fn scene_custom_data_mut(&mut self) -> &mut SceneStorageCustomDataSet {
        &mut self.scene_custom_data
    }

    pub fn per_camera_custom_data(
        &self,
        camera_id: RenderCameraId,
    ) -> Option<&PerCameraStorageCustomDataSet> {
        self.per_camera_custom_data.get(&camera_id)
    }

    pub fn per_camera_custom_data_mut(
        &mut self,
        camera_id: RenderCameraId,
    ) -> &mut PerCameraStorageCustomDataSet {
        self.per_camera_custom_data.entry(camera_id).or_default()
    }

    pub fn remove_camera(&mut self, camera_id: RenderCameraId) -> bool {
        self.per_camera_custom_data.remove(&camera_id).is_some()
    }

    pub fn prepare(
        &mut self,
        ctx: &FrameCustomDataPrepareContext,
        cameras: &PreparedFrameViews,
    ) -> RenderGraphResult<PreparedFrameSceneData> {
        let mut scene_data = PreparedSceneCustomData::new();
        self.scene_custom_data
            .prepare(ctx, cameras, scene_data.custom_mut())?;

        let mut prepared_cameras = HashMap::with_capacity(cameras.len());
        for camera in cameras.iter() {
            let mut prepared_camera = PreparedCameraCustomData::new(camera.camera_id);
            if let Some(custom_data) = self.per_camera_custom_data.get_mut(&camera.camera_id) {
                custom_data.prepare(ctx, camera, prepared_camera.custom_mut())?;
            }
            prepared_cameras.insert(camera.camera_id, prepared_camera);
        }

        Ok(PreparedFrameSceneData::new(
            ctx.scene_id,
            scene_data,
            prepared_cameras,
        ))
    }

    pub fn sync_cameras<I>(&mut self, live_camera_ids: I) -> PersistentRenderSceneDataSyncReport
    where
        I: IntoIterator<Item = RenderCameraId>,
    {
        self.sync_cameras_with_factories(live_camera_ids, &[])
    }

    fn sync_cameras_with_factories<I>(
        &mut self,
        live_camera_ids: I,
        per_camera_factories: &[PerCameraStorageCustomDataFactory],
    ) -> PersistentRenderSceneDataSyncReport
    where
        I: IntoIterator<Item = RenderCameraId>,
    {
        let live_camera_ids = live_camera_ids.into_iter().collect::<HashSet<_>>();
        let old_len = self.per_camera_custom_data.len();

        self.per_camera_custom_data
            .retain(|camera_id, _| live_camera_ids.contains(camera_id));

        let cameras_removed = old_len - self.per_camera_custom_data.len();
        let mut cameras_added = 0;

        for camera_id in live_camera_ids {
            if !self.per_camera_custom_data.contains_key(&camera_id) {
                self.per_camera_custom_data.insert(
                    camera_id,
                    PerCameraStorageCustomDataSet::from_factories(per_camera_factories),
                );
                cameras_added += 1;
            }
        }

        PersistentRenderSceneDataSyncReport {
            cameras_added,
            cameras_removed,
        }
    }
}

impl Default for PersistentRenderSceneData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default, Resource)]
pub struct PersistentRenderSceneDataRegistry {
    scenes: HashMap<RenderSceneId, PersistentRenderSceneData>,
    scene_custom_data_factories: Vec<SceneStorageCustomDataFactory>,
    per_camera_custom_data_factories: Vec<PerCameraStorageCustomDataFactory>,
}

impl fmt::Debug for PersistentRenderSceneDataRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistentRenderSceneDataRegistry")
            .field("scene_count", &self.scenes.len())
            .field(
                "scene_custom_data_factory_count",
                &self.scene_custom_data_factories.len(),
            )
            .field(
                "per_camera_custom_data_factory_count",
                &self.per_camera_custom_data_factories.len(),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PersistentRenderSceneDataRegistrySyncReport {
    pub scene_added: bool,
    pub cameras_added: usize,
    pub cameras_removed: usize,
}

impl PersistentRenderSceneDataRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_scene_custom_data<D>(&mut self)
    where
        D: SceneStorageCustomData + Default + 'static,
    {
        self.scene_custom_data_factories
            .push(Box::new(|| Box::new(D::default())));
    }

    pub fn register_per_camera_custom_data<D>(&mut self)
    where
        D: PerCameraStorageCustomData + Default + 'static,
    {
        self.per_camera_custom_data_factories
            .push(Box::new(|| Box::new(D::default())));
    }

    pub fn register_scene_custom_data_factory<F>(&mut self, factory: F)
    where
        F: Fn() -> Box<dyn SceneStorageCustomData> + Send + Sync + 'static,
    {
        self.scene_custom_data_factories.push(Box::new(factory));
    }

    pub fn register_per_camera_custom_data_factory<F>(&mut self, factory: F)
    where
        F: Fn() -> Box<dyn PerCameraStorageCustomData> + Send + Sync + 'static,
    {
        self.per_camera_custom_data_factories
            .push(Box::new(factory));
    }

    pub fn scene(&self, scene_id: RenderSceneId) -> Option<&PersistentRenderSceneData> {
        self.scenes.get(&scene_id)
    }

    pub fn scene_mut(&mut self, scene_id: RenderSceneId) -> &mut PersistentRenderSceneData {
        self.scenes.entry(scene_id).or_insert_with(|| {
            PersistentRenderSceneData::from_scene_factories(&self.scene_custom_data_factories)
        })
    }

    pub fn remove_scene(&mut self, scene_id: RenderSceneId) -> Option<PersistentRenderSceneData> {
        self.scenes.remove(&scene_id)
    }

    pub fn remove_camera(&mut self, scene_id: RenderSceneId, camera_id: RenderCameraId) -> bool {
        self.scenes
            .get_mut(&scene_id)
            .map(|scene| scene.remove_camera(camera_id))
            .unwrap_or(false)
    }

    pub fn sync_scene_cameras<I>(
        &mut self,
        scene_id: RenderSceneId,
        live_camera_ids: I,
    ) -> PersistentRenderSceneDataRegistrySyncReport
    where
        I: IntoIterator<Item = RenderCameraId>,
    {
        let scene_added = !self.scenes.contains_key(&scene_id);
        let scene = self.scenes.entry(scene_id).or_insert_with(|| {
            PersistentRenderSceneData::from_scene_factories(&self.scene_custom_data_factories)
        });
        let report = scene
            .sync_cameras_with_factories(live_camera_ids, &self.per_camera_custom_data_factories);

        PersistentRenderSceneDataRegistrySyncReport {
            scene_added,
            cameras_added: report.cameras_added,
            cameras_removed: report.cameras_removed,
        }
    }

    pub fn prepare(
        &mut self,
        scene_id: RenderSceneId,
        ctx: &FrameCustomDataPrepareContext,
        cameras: &PreparedFrameViews,
    ) -> RenderGraphResult<PreparedFrameSceneData> {
        let scene = self
            .scenes
            .get_mut(&scene_id)
            .ok_or(RenderGraphError::InvalidState {
                reason: "render scene data is not registered",
            })?;
        scene.prepare(ctx, cameras)
    }
}
