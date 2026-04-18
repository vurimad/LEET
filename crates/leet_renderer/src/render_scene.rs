//! Renderer-owned scene state and double-buffered update queues.
//!
//! The scene is the persistent boundary between the live game world and the
//! renderer. The gameplay/main thread enqueues updates through
//! [`RenderSceneCommands`], while the renderer swaps and drains a coherent
//! queue once per frame.

use crate::render_proxy::{RenderProxy, RenderProxyDescriptor, RenderProxyId};
use crate::scene_gpu::GpuInstanceData;
use leet_core::{Leeror, LeetResult};
use leet_math::Mat4;
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

static NEXT_RENDER_SCENE_ID: AtomicU64 = AtomicU64::new(0);

/// Stable identifier for a renderer-owned scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RenderSceneId(u64);

impl RenderSceneId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    fn allocate() -> Self {
        Self::new(NEXT_RENDER_SCENE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// High-level scene classification used by renderer features.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderSceneType {
    World,
    Preview,
    Thumbnail,
    NoScene,
}

impl Default for RenderSceneType {
    fn default() -> Self {
        Self::World
    }
}

#[derive(Clone, Debug)]
struct RenderSceneState {
    clear_color: wgpu::Color,
    proxies: Vec<RenderProxySlot>,
    live_proxy_count: usize,
    dirty_slots: Vec<usize>,
    dirty_slot_marks: Vec<bool>,
}

#[derive(Clone, Debug, Default)]
struct RenderProxySlot {
    proxy: Option<RenderProxy>,
}

impl RenderSceneState {
    fn upsert_proxy(&mut self, proxy: RenderProxy) {
        let slot_index = proxy.id().slot_index();
        self.ensure_slot(slot_index);

        let slot = &mut self.proxies[slot_index];
        if slot.proxy.is_none() {
            self.live_proxy_count += 1;
        }
        slot.proxy = Some(proxy);
        self.mark_slot_dirty(slot_index);
    }

    fn remove_proxy(&mut self, proxy_id: RenderProxyId) {
        let Some(slot) = self.proxies.get_mut(proxy_id.slot_index()) else {
            return;
        };

        if slot
            .proxy
            .as_ref()
            .is_some_and(|proxy| proxy.id() == proxy_id)
        {
            slot.proxy = None;
            self.live_proxy_count -= 1;
            self.mark_slot_dirty(proxy_id.slot_index());
        }
    }

    fn proxy_mut(&mut self, proxy_id: RenderProxyId) -> Option<&mut RenderProxy> {
        let slot = self.proxies.get_mut(proxy_id.slot_index())?;
        let proxy = slot.proxy.as_mut()?;

        if proxy.id() == proxy_id {
            Some(proxy)
        } else {
            None
        }
    }

    fn snapshot(&self) -> RenderSceneSnapshot {
        let mut proxies = Vec::with_capacity(self.live_proxy_count);
        for slot in &self.proxies {
            if let Some(proxy) = &slot.proxy {
                proxies.push(proxy.clone());
            }
        }

        RenderSceneSnapshot {
            clear_color: self.clear_color,
            proxies,
        }
    }

    fn slot_capacity(&self) -> usize {
        self.proxies.len()
    }

    fn take_gpu_sync_request(&mut self, full_upload: bool) -> RenderSceneGpuSyncRequest {
        let dirty_slots = if full_upload {
            let mut all_slots = Vec::with_capacity(self.proxies.len());
            for slot_index in 0..self.proxies.len() {
                all_slots.push(slot_index);
            }
            self.clear_dirty_tracking();
            all_slots
        } else {
            self.take_dirty_slots()
        };

        RenderSceneGpuSyncRequest::new(
            self.proxies.len(),
            self.live_proxy_count,
            full_upload,
            dirty_slots,
        )
    }

    fn ensure_slot(&mut self, slot_index: usize) {
        if slot_index >= self.proxies.len() {
            self.proxies
                .resize_with(slot_index + 1, RenderProxySlot::default);
        }
    }

    fn mark_slot_dirty(&mut self, slot_index: usize) {
        if slot_index >= self.dirty_slot_marks.len() {
            self.dirty_slot_marks.resize(slot_index + 1, false);
        }

        if !self.dirty_slot_marks[slot_index] {
            self.dirty_slot_marks[slot_index] = true;
            self.dirty_slots.push(slot_index);
        }
    }

    fn clear_dirty_tracking(&mut self) {
        for slot_index in std::mem::take(&mut self.dirty_slots) {
            if let Some(mark) = self.dirty_slot_marks.get_mut(slot_index) {
                *mark = false;
            }
        }
    }

    fn take_dirty_slots(&mut self) -> Vec<usize> {
        let dirty_slots = std::mem::take(&mut self.dirty_slots);
        for &slot_index in &dirty_slots {
            if let Some(mark) = self.dirty_slot_marks.get_mut(slot_index) {
                *mark = false;
            }
        }
        dirty_slots
    }
}

impl Default for RenderSceneState {
    fn default() -> Self {
        Self {
            clear_color: wgpu::Color {
                r: 0.1,
                g: 0.1,
                b: 0.1,
                a: 1.0,
            },
            proxies: Vec::new(),
            live_proxy_count: 0,
            dirty_slots: Vec::new(),
            dirty_slot_marks: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
enum RenderSceneUpdate {
    UpsertProxy(RenderProxy),
    RemoveProxy(RenderProxyId),
    SetClearColor(wgpu::Color),
    UpdateProxyTransform(RenderProxyId, Mat4),
    UpdateProxyMeshRenderer {
        proxy_id: RenderProxyId,
        mesh_handle: u64,
        material_handle: u64,
        casts_shadows: bool,
        visible: bool,
    },
    UpdateProxyVisibility(RenderProxyId, bool),
    UpdateProxyDebugColor(RenderProxyId, wgpu::Color),
}

impl RenderSceneUpdate {
    fn apply(self, state: &mut RenderSceneState) {
        match self {
            Self::UpsertProxy(proxy) => {
                state.upsert_proxy(proxy);
            }
            Self::RemoveProxy(proxy_id) => {
                state.remove_proxy(proxy_id);
            }
            Self::SetClearColor(color) => {
                state.clear_color = color;
            }
            Self::UpdateProxyTransform(proxy_id, local_to_world) => {
                let mut updated = false;
                if let Some(proxy) = state.proxy_mut(proxy_id) {
                    proxy.set_local_to_world(local_to_world);
                    updated = true;
                }
                if updated {
                    state.mark_slot_dirty(proxy_id.slot_index());
                }
            }
            Self::UpdateProxyMeshRenderer {
                proxy_id,
                mesh_handle,
                material_handle,
                casts_shadows,
                visible,
            } => {
                let mut updated = false;
                if let Some(proxy) = state.proxy_mut(proxy_id) {
                    proxy.set_mesh_renderer(mesh_handle, material_handle, casts_shadows, visible);
                    updated = true;
                }
                if updated {
                    state.mark_slot_dirty(proxy_id.slot_index());
                }
            }
            Self::UpdateProxyVisibility(proxy_id, visible) => {
                let mut updated = false;
                if let Some(proxy) = state.proxy_mut(proxy_id) {
                    proxy.set_visible(visible);
                    updated = true;
                }
                if updated {
                    state.mark_slot_dirty(proxy_id.slot_index());
                }
            }
            Self::UpdateProxyDebugColor(proxy_id, debug_color) => {
                let mut updated = false;
                if let Some(proxy) = state.proxy_mut(proxy_id) {
                    proxy.set_debug_color(debug_color);
                    updated = true;
                }
                if updated {
                    state.mark_slot_dirty(proxy_id.slot_index());
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct RenderSceneUpdateBuffer {
    updates: UnsafeCell<Vec<RenderSceneUpdate>>,
}

impl RenderSceneUpdateBuffer {
    // SAFETY: Access is serialized by the handoff protocol in
    // `RenderSceneUpdateHandoff`: only the main-thread producer touches the
    // active producer buffer, and only the render thread touches the drained
    // buffer after a synchronized swap.
    unsafe fn push(&self, update: RenderSceneUpdate) {
        (*self.updates.get()).push(update);
    }

    // SAFETY: Called only by the render thread after the handoff swaps buffer
    // ownership and waits for any in-flight producer write to finish.
    unsafe fn take(&self) -> Vec<RenderSceneUpdate> {
        std::mem::take(&mut *self.updates.get())
    }
}

unsafe impl Sync for RenderSceneUpdateBuffer {}

#[derive(Debug)]
struct RenderSceneUpdateHandoff {
    current_frame_updates: RenderSceneUpdateBuffer,
    previous_frame_updates: RenderSceneUpdateBuffer,
    sync_mutex: Mutex<()>,
}

impl Default for RenderSceneUpdateHandoff {
    fn default() -> Self {
        Self {
            current_frame_updates: RenderSceneUpdateBuffer::default(),
            previous_frame_updates: RenderSceneUpdateBuffer::default(),
            sync_mutex: Mutex::new(()),
        }
    }
}

impl RenderSceneUpdateHandoff {
    fn push(&self, update: RenderSceneUpdate) -> LeetResult<()> {
        unsafe {
            self.current_frame_updates.push(update);
        }
        Ok(())
    }

    fn swap_and_take_drain_queue(&self) -> LeetResult<Vec<RenderSceneUpdate>> {
        let _sync_guard = self.sync_mutex.lock().map_err(|_| {
            Leeror::Runtime("render scene update sync mutex was poisoned".to_string())
        })?;

        // SAFETY: This is the one frame-boundary handoff point between the
        // main/game thread and the render thread. The contract is that the
        // producer is quiescent while the render thread performs this swap.
        unsafe {
            std::mem::swap(
                &mut *self.current_frame_updates.updates.get(),
                &mut *self.previous_frame_updates.updates.get(),
            );
            Ok(self.previous_frame_updates.take())
        }
    }
}

#[derive(Debug)]
struct RenderSceneInner {
    scene_id: RenderSceneId,
    scene_type: RenderSceneType,
    state: RwLock<RenderSceneState>,
    update_handoff: RenderSceneUpdateHandoff,
    pending_render_updates: Mutex<Vec<RenderSceneUpdate>>,
    next_proxy_id: AtomicU64,
}

/// Immutable scene snapshot consumed by one frame.
#[derive(Clone, Debug)]
pub struct RenderSceneSnapshot {
    clear_color: wgpu::Color,
    proxies: Vec<RenderProxy>,
}

impl RenderSceneSnapshot {
    pub fn clear_color(&self) -> wgpu::Color {
        self.clear_color
    }

    pub fn proxies(&self) -> &[RenderProxy] {
        &self.proxies
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RenderSceneGpuSyncRequest {
    required_slot_capacity: usize,
    live_instance_count: usize,
    full_upload: bool,
    dirty_slots: Vec<usize>,
}

impl RenderSceneGpuSyncRequest {
    fn new(
        required_slot_capacity: usize,
        live_instance_count: usize,
        full_upload: bool,
        dirty_slots: Vec<usize>,
    ) -> Self {
        Self {
            required_slot_capacity,
            live_instance_count,
            full_upload,
            dirty_slots,
        }
    }

    pub fn required_slot_capacity(&self) -> usize {
        self.required_slot_capacity
    }

    pub fn live_instance_count(&self) -> usize {
        self.live_instance_count
    }

    pub fn full_upload(&self) -> bool {
        self.full_upload
    }

    pub fn dirty_slots(&self) -> &[usize] {
        &self.dirty_slots
    }
}

/// Producer-side handle for scene updates.
///
/// Clone this handle freely on the gameplay/main thread. Each method appends
/// directly into the current-frame update queue with no per-update locking.
/// The renderer later performs one frame-boundary handoff to swap queues and
/// drain the previous frame's updates.
#[derive(Clone)]
pub struct RenderSceneCommands {
    inner: Arc<RenderSceneInner>,
}

impl RenderSceneCommands {
    /// Queue a clear-color change for the next snapshot.
    pub fn set_clear_color(&self, color: wgpu::Color) -> LeetResult<()> {
        self.enqueue_update(RenderSceneUpdate::SetClearColor(color))
    }

    /// Allocate a new proxy id and queue insertion of the described proxy.
    pub fn spawn_proxy(&self, descriptor: RenderProxyDescriptor) -> LeetResult<RenderProxyId> {
        let proxy_id = RenderProxyId::new(self.inner.next_proxy_id.fetch_add(1, Ordering::Relaxed));
        let proxy = RenderProxy::from_descriptor(proxy_id, descriptor);
        self.enqueue_update(RenderSceneUpdate::UpsertProxy(proxy))?;
        Ok(proxy_id)
    }

    /// Queue replacement of an existing proxy.
    pub fn upsert_proxy(&self, proxy: RenderProxy) -> LeetResult<()> {
        self.enqueue_update(RenderSceneUpdate::UpsertProxy(proxy))
    }

    /// Queue removal of a proxy.
    pub fn remove_proxy(&self, proxy_id: RenderProxyId) -> LeetResult<()> {
        self.enqueue_update(RenderSceneUpdate::RemoveProxy(proxy_id))
    }

    /// Queue a transform update for an existing proxy.
    pub fn update_proxy_transform(
        &self,
        proxy_id: RenderProxyId,
        local_to_world: Mat4,
    ) -> LeetResult<()> {
        self.enqueue_update(RenderSceneUpdate::UpdateProxyTransform(
            proxy_id,
            local_to_world,
        ))
    }

    /// Queue a mesh/material/shadow-state update for an existing proxy.
    pub fn update_proxy_mesh_renderer(
        &self,
        proxy_id: RenderProxyId,
        mesh_handle: u64,
        material_handle: u64,
        casts_shadows: bool,
        visible: bool,
    ) -> LeetResult<()> {
        self.enqueue_update(RenderSceneUpdate::UpdateProxyMeshRenderer {
            proxy_id,
            mesh_handle,
            material_handle,
            casts_shadows,
            visible,
        })
    }

    /// Queue a visibility update for an existing proxy.
    pub fn update_proxy_visibility(
        &self,
        proxy_id: RenderProxyId,
        visible: bool,
    ) -> LeetResult<()> {
        self.enqueue_update(RenderSceneUpdate::UpdateProxyVisibility(proxy_id, visible))
    }

    /// Queue a debug-color update for an existing proxy.
    pub fn update_proxy_debug_color(
        &self,
        proxy_id: RenderProxyId,
        debug_color: wgpu::Color,
    ) -> LeetResult<()> {
        self.enqueue_update(RenderSceneUpdate::UpdateProxyDebugColor(
            proxy_id,
            debug_color,
        ))
    }

    fn enqueue_update(&self, update: RenderSceneUpdate) -> LeetResult<()> {
        self.inner.update_handoff.push(update)
    }
}

/// Renderer-owned scene handle.
///
/// Cloning this value is cheap. Gameplay uses it to enqueue scene updates on
/// the producer thread, while the renderer synchronizes it once per frame.
#[derive(Clone, Debug)]
pub struct RenderSceneProxy {
    inner: Arc<RenderSceneInner>,
}

impl RenderSceneProxy {
    pub fn new() -> Self {
        Self::with_type(RenderSceneType::World)
    }

    pub fn with_type(scene_type: RenderSceneType) -> Self {
        Self {
            inner: Arc::new(RenderSceneInner {
                scene_id: RenderSceneId::allocate(),
                scene_type,
                state: RwLock::new(RenderSceneState::default()),
                update_handoff: RenderSceneUpdateHandoff::default(),
                pending_render_updates: Mutex::new(Vec::new()),
                next_proxy_id: AtomicU64::new(0),
            }),
        }
    }

    pub fn scene_id(&self) -> RenderSceneId {
        self.inner.scene_id
    }

    pub fn scene_type(&self) -> RenderSceneType {
        self.inner.scene_type
    }

    /// Return a cloneable producer handle for gameplay/main-thread scene updates.
    pub fn commands(&self) -> RenderSceneCommands {
        RenderSceneCommands {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Queue a clear-color change for the next snapshot.
    pub fn set_clear_color(&self, color: wgpu::Color) -> LeetResult<()> {
        self.commands().set_clear_color(color)
    }

    /// Allocate a new proxy id and queue insertion of the described proxy.
    pub fn add_proxy(&self, descriptor: RenderProxyDescriptor) -> LeetResult<RenderProxyId> {
        self.commands().spawn_proxy(descriptor)
    }

    /// Queue replacement of an existing proxy.
    pub fn upsert_proxy(&self, proxy: RenderProxy) -> LeetResult<()> {
        self.commands().upsert_proxy(proxy)
    }

    /// Queue removal of a proxy.
    pub fn remove_proxy(&self, proxy_id: RenderProxyId) -> LeetResult<()> {
        self.commands().remove_proxy(proxy_id)
    }

    /// Queue a transform update for an existing proxy.
    pub fn update_proxy_transform(
        &self,
        proxy_id: RenderProxyId,
        local_to_world: Mat4,
    ) -> LeetResult<()> {
        self.commands()
            .update_proxy_transform(proxy_id, local_to_world)
    }

    /// Queue a mesh/material/shadow-state update for an existing proxy.
    pub fn update_proxy_mesh_renderer(
        &self,
        proxy_id: RenderProxyId,
        mesh_handle: u64,
        material_handle: u64,
        casts_shadows: bool,
        visible: bool,
    ) -> LeetResult<()> {
        self.commands().update_proxy_mesh_renderer(
            proxy_id,
            mesh_handle,
            material_handle,
            casts_shadows,
            visible,
        )
    }

    /// Queue a visibility update for an existing proxy.
    pub fn update_proxy_visibility(
        &self,
        proxy_id: RenderProxyId,
        visible: bool,
    ) -> LeetResult<()> {
        self.commands().update_proxy_visibility(proxy_id, visible)
    }

    /// Queue a debug-color update for an existing proxy.
    pub fn update_proxy_debug_color(
        &self,
        proxy_id: RenderProxyId,
        debug_color: wgpu::Color,
    ) -> LeetResult<()> {
        self.commands()
            .update_proxy_debug_color(proxy_id, debug_color)
    }

    /// Read the current renderer-owned state without performing a queue sync.
    pub fn snapshot(&self) -> LeetResult<RenderSceneSnapshot> {
        self.current_snapshot()
    }

    /// Perform the frame-boundary handoff from gameplay updates to the render
    /// thread, without applying them yet.
    ///
    /// This is the one point where the gameplay/main thread and render thread
    /// are allowed to synchronize. Producers must be quiescent while the render
    /// thread performs this handoff.
    pub(crate) fn hand_off(&self) -> LeetResult<()> {
        let pending_updates = self.swap_update_queues()?;
        if pending_updates.is_empty() {
            return Ok(());
        }

        let mut render_updates = self.inner.pending_render_updates.lock().map_err(|_| {
            Leeror::Runtime("render scene pending render updates lock was poisoned".to_string())
        })?;
        render_updates.extend(pending_updates);
        Ok(())
    }

    pub(crate) fn apply_synced_updates(&self) -> LeetResult<()> {
        let pending_updates = {
            let mut render_updates = self.inner.pending_render_updates.lock().map_err(|_| {
                Leeror::Runtime("render scene pending render updates lock was poisoned".to_string())
            })?;
            std::mem::take(&mut *render_updates)
        };

        if pending_updates.is_empty() {
            return Ok(());
        }

        let mut state = self
            .inner
            .state
            .write()
            .map_err(|_| Leeror::Runtime("render scene state lock was poisoned".to_string()))?;
        for update in pending_updates {
            update.apply(&mut state);
        }

        Ok(())
    }

    pub(crate) fn current_snapshot(&self) -> LeetResult<RenderSceneSnapshot> {
        let state = self
            .inner
            .state
            .read()
            .map_err(|_| Leeror::Runtime("render scene state lock was poisoned".to_string()))?;

        Ok(state.snapshot())
    }

    pub(crate) fn gpu_slot_capacity(&self) -> LeetResult<usize> {
        let state = self
            .inner
            .state
            .read()
            .map_err(|_| Leeror::Runtime("render scene state lock was poisoned".to_string()))?;

        Ok(state.slot_capacity())
    }

    pub(crate) fn refresh_gpu_slot_image(
        &self,
        dirty_slots: &[usize],
        cpu_slot_image: &mut [GpuInstanceData],
    ) -> LeetResult<()> {
        let state = self
            .inner
            .state
            .read()
            .map_err(|_| Leeror::Runtime("render scene state lock was poisoned".to_string()))?;

        for &slot_index in dirty_slots {
            cpu_slot_image[slot_index] = state
                .proxies
                .get(slot_index)
                .and_then(|slot| slot.proxy.as_ref())
                .map(GpuInstanceData::from_proxy)
                .unwrap_or(GpuInstanceData::ZERO);
        }

        Ok(())
    }

    pub(crate) fn take_gpu_sync_request(
        &self,
        full_upload: bool,
    ) -> LeetResult<RenderSceneGpuSyncRequest> {
        let mut state = self
            .inner
            .state
            .write()
            .map_err(|_| Leeror::Runtime("render scene state lock was poisoned".to_string()))?;

        Ok(state.take_gpu_sync_request(full_upload))
    }

    fn swap_update_queues(&self) -> LeetResult<Vec<RenderSceneUpdate>> {
        self.inner.update_handoff.swap_and_take_drain_queue()
    }
}

impl Default for RenderSceneProxy {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
enum RenderSceneRegistryUpdate {
    Register(RenderSceneProxy),
    Unregister(RenderSceneId),
}

#[derive(Debug)]
struct RenderSceneRegistryUpdateInbox {
    queues: [Vec<RenderSceneRegistryUpdate>; 2],
    write_queue_index: usize,
}

impl Default for RenderSceneRegistryUpdateInbox {
    fn default() -> Self {
        Self {
            queues: [Vec::new(), Vec::new()],
            write_queue_index: 0,
        }
    }
}

impl RenderSceneRegistryUpdateInbox {
    fn push(&mut self, update: RenderSceneRegistryUpdate) {
        self.queues[self.write_queue_index].push(update);
    }

    fn swap_and_take_drain_queue(&mut self) -> Vec<RenderSceneRegistryUpdate> {
        let drain_queue_index = self.write_queue_index;
        self.write_queue_index = 1 - self.write_queue_index;
        std::mem::take(&mut self.queues[drain_queue_index])
    }
}

#[derive(Default, Debug)]
struct RenderSceneRegistryState {
    scenes: BTreeMap<RenderSceneId, RenderSceneProxy>,
}

/// Renderer-owned registry of scene proxies.
///
/// Scene registration uses the same swap-and-drain sync model as per-scene
/// proxy updates: producers enqueue create/destroy requests, and the renderer
/// applies them at one explicit sync point before frame work continues.
#[derive(Default, Debug)]
pub struct RenderSceneRegistry {
    state: RwLock<RenderSceneRegistryState>,
    update_inbox: Mutex<RenderSceneRegistryUpdateInbox>,
}

impl RenderSceneRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_scene_proxy(&self, scene_type: RenderSceneType) -> LeetResult<RenderSceneProxy> {
        let proxy = RenderSceneProxy::with_type(scene_type);
        self.enqueue_update(RenderSceneRegistryUpdate::Register(proxy.clone()))?;
        Ok(proxy)
    }

    pub fn remove_scene_proxy(&self, scene_proxy: &RenderSceneProxy) -> LeetResult<()> {
        self.remove_scene(scene_proxy.scene_id())
    }

    pub fn remove_scene(&self, scene_id: RenderSceneId) -> LeetResult<()> {
        self.enqueue_update(RenderSceneRegistryUpdate::Unregister(scene_id))
    }

    pub(crate) fn execute_pending_updates(&self) -> LeetResult<()> {
        self.apply_registry_updates()
    }

    pub fn scene_proxies(&self) -> LeetResult<Vec<RenderSceneProxy>> {
        let state: std::sync::RwLockReadGuard<'_, RenderSceneRegistryState> =
            self.state.read().map_err(|_| {
                Leeror::Runtime("render scene registry state lock was poisoned".to_string())
            })?;
        Ok(state.scenes.values().cloned().collect())
    }

    fn apply_registry_updates(&self) -> LeetResult<()> {
        let pending_updates = self.swap_update_queues()?;
        if pending_updates.is_empty() {
            return Ok(());
        }

        let mut state = self.state.write().map_err(|_| {
            Leeror::Runtime("render scene registry state lock was poisoned".to_string())
        })?;

        for update in pending_updates {
            match update {
                RenderSceneRegistryUpdate::Register(proxy) => {
                    state.scenes.insert(proxy.scene_id(), proxy);
                }
                RenderSceneRegistryUpdate::Unregister(scene_id) => {
                    state.scenes.remove(&scene_id);
                }
            }
        }

        Ok(())
    }

    fn enqueue_update(&self, update: RenderSceneRegistryUpdate) -> LeetResult<()> {
        let mut inbox = self.update_inbox.lock().map_err(|_| {
            Leeror::Runtime("render scene registry update inbox was poisoned".to_string())
        })?;
        inbox.push(update);
        Ok(())
    }

    fn swap_update_queues(&self) -> LeetResult<Vec<RenderSceneRegistryUpdate>> {
        let mut inbox = self.update_inbox.lock().map_err(|_| {
            Leeror::Runtime("render scene registry update inbox was poisoned".to_string())
        })?;
        Ok(inbox.swap_and_take_drain_queue())
    }
}

/// Cloneable scene handle used by gameplay/app code to refer to a renderer-owned scene.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_proxy::{RenderProxyDescriptor, RenderProxyKind};
    use leet_math::{Mat4, Vec3};

    fn debug_color(r: f64, g: f64, b: f64) -> wgpu::Color {
        wgpu::Color { r, g, b, a: 1.0 }
    }

    #[test]
    fn snapshot_applies_queued_updates() {
        let scene = RenderSceneProxy::with_type(RenderSceneType::Preview);
        scene
            .set_clear_color(wgpu::Color {
                r: 0.2,
                g: 0.3,
                b: 0.4,
                a: 1.0,
            })
            .unwrap();
        let first_id = scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Opaque).named("Opaque"))
            .unwrap();
        let second_id = scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Sky).named("Sky"))
            .unwrap();
        scene.remove_proxy(first_id).unwrap();

        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();
        let snapshot = scene.snapshot().unwrap();

        assert_eq!(scene.scene_type(), RenderSceneType::Preview);
        assert_eq!(snapshot.clear_color().g, 0.3);
        assert_eq!(snapshot.proxies().len(), 1);
        assert_eq!(snapshot.proxies()[0].id(), second_id);
        assert_eq!(snapshot.proxies()[0].name(), "Sky");
    }

    #[test]
    fn commands_handle_updates_proxy_fields() {
        let scene = RenderSceneProxy::new();
        let commands = scene.commands();
        let proxy_id = commands
            .spawn_proxy(
                RenderProxyDescriptor::new(RenderProxyKind::Opaque)
                    .named("Mover")
                    .with_translation(Vec3::ZERO)
                    .with_debug_color(debug_color(0.2, 0.2, 0.2)),
            )
            .unwrap();

        commands
            .update_proxy_transform(proxy_id, Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)))
            .unwrap();
        commands
            .update_proxy_mesh_renderer(proxy_id, 7, 11, false, false)
            .unwrap();
        commands.update_proxy_visibility(proxy_id, false).unwrap();
        commands
            .update_proxy_debug_color(proxy_id, debug_color(0.9, 0.1, 0.3))
            .unwrap();

        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();
        let snapshot = scene.snapshot().unwrap();
        let proxy = &snapshot.proxies()[0];

        assert_eq!(proxy.translation(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(
            proxy.local_to_world(),
            Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0))
        );
        assert!(!proxy.is_visible());
        assert_eq!(proxy.mesh_handle(), 7);
        assert_eq!(proxy.material_handle(), 11);
        assert!(!proxy.casts_shadows());
        assert_eq!(proxy.debug_color().r, 0.9);
        assert_eq!(proxy.debug_color().g, 0.1);
        assert_eq!(proxy.debug_color().b, 0.3);
    }

    #[test]
    fn update_inbox_swaps_write_queues() {
        let inbox = RenderSceneUpdateHandoff::default();

        inbox
            .push(RenderSceneUpdate::SetClearColor(debug_color(0.1, 0.2, 0.3)))
            .unwrap();
        let first_drain = inbox.swap_and_take_drain_queue().unwrap();
        inbox
            .push(RenderSceneUpdate::RemoveProxy(RenderProxyId::new(7)))
            .unwrap();
        let second_drain = inbox.swap_and_take_drain_queue().unwrap();

        assert_eq!(first_drain.len(), 1);
        assert!(matches!(
            first_drain[0],
            RenderSceneUpdate::SetClearColor(_)
        ));
        assert_eq!(second_drain.len(), 1);
        assert!(matches!(
            second_drain[0],
            RenderSceneUpdate::RemoveProxy(proxy_id) if proxy_id == RenderProxyId::new(7)
        ));
    }

    #[test]
    fn snapshot_keeps_proxy_order_stable_by_id() {
        let scene = RenderSceneProxy::new();
        let first = scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Opaque).named("First"))
            .unwrap();
        let second = scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Opaque).named("Second"))
            .unwrap();

        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();
        let snapshot = scene.snapshot().unwrap();

        assert_eq!(snapshot.proxies()[0].id(), first);
        assert_eq!(snapshot.proxies()[1].id(), second);
    }

    #[test]
    fn gpu_sync_request_tracks_dirty_slots() {
        let scene = RenderSceneProxy::new();
        let proxy_id = scene
            .add_proxy(
                RenderProxyDescriptor::new(RenderProxyKind::Opaque)
                    .named("Mover")
                    .with_translation(Vec3::new(1.0, 2.0, 3.0)),
            )
            .unwrap();

        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();

        let full_upload = scene.take_gpu_sync_request(true).unwrap();
        assert_eq!(full_upload.required_slot_capacity(), 1);
        assert_eq!(full_upload.live_instance_count(), 1);
        assert!(full_upload.full_upload());
        assert_eq!(full_upload.dirty_slots(), &[0]);

        let no_changes = scene.take_gpu_sync_request(false).unwrap();
        assert!(!no_changes.full_upload());
        assert!(no_changes.dirty_slots().is_empty());

        scene
            .update_proxy_transform(proxy_id, Mat4::from_translation(Vec3::new(4.0, 5.0, 6.0)))
            .unwrap();
        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();

        let dirty_upload = scene.take_gpu_sync_request(false).unwrap();
        assert_eq!(dirty_upload.required_slot_capacity(), 1);
        assert_eq!(dirty_upload.live_instance_count(), 1);
        assert!(!dirty_upload.full_upload());
        assert_eq!(dirty_upload.dirty_slots(), &[0]);

        let mut cpu_slot_image = vec![GpuInstanceData::ZERO];
        scene
            .refresh_gpu_slot_image(dirty_upload.dirty_slots(), &mut cpu_slot_image)
            .unwrap();
        assert_eq!(
            cpu_slot_image[0].local_to_world(),
            Mat4::from_translation(Vec3::new(4.0, 5.0, 6.0)).to_cols_array_2d()
        );
        assert!(cpu_slot_image[0].visible());
    }

    #[test]
    fn registry_syncs_registered_scenes() {
        let registry = RenderSceneRegistry::new();
        let scene = registry.create_scene_proxy(RenderSceneType::World).unwrap();
        scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Opaque).named("Opaque"))
            .unwrap();

        registry.execute_pending_updates().unwrap();
        let synced_scenes = registry.scene_proxies().unwrap();

        assert_eq!(synced_scenes.len(), 1);
        assert_eq!(synced_scenes[0].scene_id(), scene.scene_id());
        synced_scenes[0].hand_off().unwrap();
        synced_scenes[0].apply_synced_updates().unwrap();
        let snapshot = synced_scenes[0].snapshot().unwrap();
        assert_eq!(snapshot.proxies().len(), 1);
        assert_eq!(snapshot.proxies()[0].name(), "Opaque");
    }

    #[test]
    fn registry_remove_scene_unregisters_it_at_sync_point() {
        let registry = RenderSceneRegistry::new();
        let scene = registry
            .create_scene_proxy(RenderSceneType::Preview)
            .unwrap();

        registry.execute_pending_updates().unwrap();
        registry.remove_scene_proxy(&scene).unwrap();
        registry.execute_pending_updates().unwrap();
        let synced_scenes = registry.scene_proxies().unwrap();

        assert!(synced_scenes.is_empty());
    }
}
