//! App-owned bridge that connects ECS worlds to renderer scene proxies.

use super::RenderProxyBinding;
use leet_core::{EngineClock, Leeror, LeetResult};
use leet_ecs::{Entity, MeshRenderer, Transform, World, WorldRegistry};
use leet_math::Mat4;
use leet_renderer::{
    RenderProxyDescriptor, RenderProxyId, RenderProxyKind, RenderSceneProxy, RenderSceneType,
    Renderer,
};
use std::sync::{Arc, Mutex, Weak};

struct HierarchyTransformUpdater<'a> {
    world: &'a mut World,
    scene: &'a RenderSceneProxy,
    sync_frame: u64,
}

impl<'a> HierarchyTransformUpdater<'a> {
    fn new(world: &'a mut World, scene: &'a RenderSceneProxy) -> Self {
        Self {
            world,
            scene,
            sync_frame: EngineClock::current_frame(),
        }
    }

    fn sync_dirty_transforms(&mut self) -> LeetResult<()> {
        for entity in self.world.take_deferred_transform_syncs() {
            if !self.world.is_alive(entity) {
                continue;
            }

            let parent_local_to_world = if let Some(parent) = self.world.parent_of(entity) {
                self.world.local_to_world_matrix(parent)
            } else {
                Mat4::IDENTITY
            };

            self.sync_subtree(entity, parent_local_to_world)?;
        }

        Ok(())
    }

    fn sync_subtree(&mut self, entity: Entity, parent_local_to_world: Mat4) -> LeetResult<()> {
        if !self.world.is_alive(entity) {
            return Ok(());
        }

        if self.is_entity_already_synced(entity) {
            return Ok(());
        }

        let local_to_world = parent_local_to_world * self.local_matrix(entity);
        self.sync_entity_binding(entity, local_to_world)?;

        let mut children = Vec::new();
        self.world.each_child(entity, |child| children.push(child));
        for child in children {
            self.sync_subtree(child, local_to_world)?;
        }

        Ok(())
    }

    fn is_entity_already_synced(&self, entity: Entity) -> bool {
        if !self.world.has_component::<RenderProxyBinding>(entity) {
            return false;
        }

        let mut already_synced = false;
        self.world
            .get_component::<RenderProxyBinding, _>(entity, |binding| {
                already_synced = binding.is_transform_synced_for_frame(self.sync_frame);
            });
        already_synced
    }

    fn sync_entity_binding(&mut self, entity: Entity, local_to_world: Mat4) -> LeetResult<()> {
        if !self.world.has_component::<RenderProxyBinding>(entity) {
            return Ok(());
        }

        let mut proxy_id = None;
        let mut should_sync = false;
        self.world
            .get_component_mut::<RenderProxyBinding, _>(entity, |binding| {
                should_sync = binding.mark_transform_synced(self.sync_frame);
                if should_sync {
                    proxy_id = Some(binding.proxy_id());
                }
            });

        if !should_sync {
            return Ok(());
        }

        let proxy_id = proxy_id.ok_or_else(|| {
            Leeror::Runtime(format!(
                "entity {} was marked for deferred transform sync without a render proxy binding",
                entity.id
            ))
        })?;

        self.scene.update_proxy_transform(proxy_id, local_to_world)
    }

    fn local_matrix(&self, entity: Entity) -> Mat4 {
        if !self.world.has_component::<Transform>(entity) {
            return Mat4::IDENTITY;
        }

        let mut local_matrix = Mat4::IDENTITY;
        self.world
            .get_component::<Transform, _>(entity, |transform| {
                local_matrix = Mat4::from_scale_rotation_translation(
                    transform.scale,
                    transform.rotation,
                    transform.position,
                );
            });
        local_matrix
    }
}

/// Bridge between one ECS world and one renderer-owned scene proxy.
pub struct WorldRenderBinding {
    world_index: u32,
    scene: RenderSceneProxy,
    deferred_despawn_proxy_removes: Arc<Mutex<Vec<RenderProxyId>>>,
}

impl WorldRenderBinding {
    pub fn new(
        world_index: u32,
        scene: RenderSceneProxy,
        deferred_despawn_proxy_removes: Arc<Mutex<Vec<RenderProxyId>>>,
    ) -> Self {
        Self {
            world_index,
            scene,
            deferred_despawn_proxy_removes,
        }
    }

    pub fn world_index(&self) -> u32 {
        self.world_index
    }

    pub fn scene(&self) -> &RenderSceneProxy {
        &self.scene
    }

    /// Remove proxies that lost renderability or whose entities were despawned.
    pub fn sync_proxy_removals(&mut self) -> LeetResult<()> {
        let world = WorldRegistry::get_mut().world_at_mut(self.world_index);

        for entity in world.take_deferred_render_proxy_removes() {
            let mut proxy_id = None;
            if world.is_alive(entity) && world.has_component::<RenderProxyBinding>(entity) {
                world.get_component::<RenderProxyBinding, _>(entity, |binding| {
                    proxy_id = Some(binding.proxy_id());
                });
                world.remove_component::<RenderProxyBinding>(entity);
            }

            let Some(proxy_id) = proxy_id else {
                continue;
            };

            self.scene.remove_proxy(proxy_id)?;
        }

        let deferred_proxy_removes = std::mem::take(
            &mut *self.deferred_despawn_proxy_removes.lock().map_err(|_| {
                Leeror::Runtime(
                    "render bridge despawn proxy remove queue mutex was poisoned".to_string(),
                )
            })?,
        );
        for proxy_id in deferred_proxy_removes {
            self.scene.remove_proxy(proxy_id)?;
        }

        Ok(())
    }

    /// Create proxies for entities that became renderable this frame.
    pub fn sync_proxy_creations(&mut self) -> LeetResult<()> {
        let current_frame = EngineClock::current_frame();
        let world = WorldRegistry::get_mut().world_at_mut(self.world_index);

        for entity in world.take_deferred_render_proxy_creates() {
            if !world.is_alive(entity)
                || !world.has_component::<Transform>(entity)
                || !world.has_component::<MeshRenderer>(entity)
                || world.has_component::<RenderProxyBinding>(entity)
            {
                continue;
            }

            let mut mesh_handle = 0;
            let mut material_handle = 0;
            let mut visible = true;
            let mut casts_shadows = true;
            world.get_component::<MeshRenderer, _>(entity, |mesh_renderer| {
                mesh_handle = mesh_renderer.mesh_handle;
                material_handle = mesh_renderer.material_handle;
                visible = mesh_renderer.visible;
                casts_shadows = mesh_renderer.casts_shadows;
            });

            let proxy_id = self.scene.add_proxy(
                RenderProxyDescriptor::new(RenderProxyKind::Opaque)
                    .named(format!("Entity {}", entity.id))
                    .with_local_to_world(world.local_to_world_matrix(entity))
                    .with_mesh_handle(mesh_handle)
                    .with_material_handle(material_handle)
                    .with_casts_shadows(casts_shadows)
                    .with_visible(visible),
            )?;

            world.add_component(entity, RenderProxyBinding::new(proxy_id));
            world.get_component_mut::<RenderProxyBinding, _>(entity, |binding| {
                let _ = binding.mark_transform_synced(current_frame);
            });
        }

        Ok(())
    }

    /// Sync mesh/material/shadow-state updates into existing proxies.
    pub fn sync_mesh_renderer_updates(&mut self) -> LeetResult<()> {
        let world = WorldRegistry::get_mut().world_at_mut(self.world_index);

        for entity in world.take_deferred_mesh_renderer_updates() {
            if !world.is_alive(entity)
                || !world.has_component::<RenderProxyBinding>(entity)
                || !world.has_component::<MeshRenderer>(entity)
            {
                continue;
            }

            let mut proxy_id = None;
            world.get_component::<RenderProxyBinding, _>(entity, |binding| {
                proxy_id = Some(binding.proxy_id());
            });

            let mut mesh_handle = 0;
            let mut material_handle = 0;
            let mut visible = true;
            let mut casts_shadows = true;
            world.get_component::<MeshRenderer, _>(entity, |mesh_renderer| {
                mesh_handle = mesh_renderer.mesh_handle;
                material_handle = mesh_renderer.material_handle;
                visible = mesh_renderer.visible;
                casts_shadows = mesh_renderer.casts_shadows;
            });

            if let Some(proxy_id) = proxy_id {
                self.scene.update_proxy_mesh_renderer(
                    proxy_id,
                    mesh_handle,
                    material_handle,
                    casts_shadows,
                    visible,
                )?;
            }
        }

        Ok(())
    }

    /// Drain deferred transform updates from the bound world into the scene queue.
    pub fn sync_dirty_transforms(&mut self) -> LeetResult<()> {
        let world = WorldRegistry::get_mut().world_at_mut(self.world_index);
        let mut updater = HierarchyTransformUpdater::new(world, &self.scene);
        updater.sync_dirty_transforms()
    }
}

/// Runtime-owned collection of bridges between ECS worlds and renderer scenes.
pub struct RenderBridge {
    world_bindings: Vec<WorldRenderBinding>,
}

impl RenderBridge {
    pub fn new(renderer: &Renderer) -> LeetResult<Self> {
        WorldRegistry::init_if_needed();

        let mut bridge = Self {
            world_bindings: Vec::new(),
        };
        let main_world_index = WorldRegistry::get().main_world().world_index();
        let _ = bridge.bind_world(renderer, main_world_index, RenderSceneType::World)?;
        Ok(bridge)
    }

    pub fn world_bindings(&self) -> &[WorldRenderBinding] {
        &self.world_bindings
    }

    pub fn main_world_scene(&self) -> Option<&RenderSceneProxy> {
        self.world_bindings.first().map(WorldRenderBinding::scene)
    }

    pub fn bind_world(
        &mut self,
        renderer: &Renderer,
        world_index: u32,
        scene_type: RenderSceneType,
    ) -> LeetResult<&WorldRenderBinding> {
        if let Some(existing_index) = self
            .world_bindings
            .iter()
            .position(|binding| binding.world_index == world_index)
        {
            return Ok(&self.world_bindings[existing_index]);
        }

        let scene = renderer.create_scene_proxy(scene_type)?;
        let deferred_despawn_proxy_removes = Arc::new(Mutex::new(Vec::new()));
        let weak_despawn_proxy_removes: Weak<Mutex<Vec<RenderProxyId>>> =
            Arc::downgrade(&deferred_despawn_proxy_removes);
        WorldRegistry::get_mut()
            .world_at_mut(world_index)
            .add_pre_despawn_hook(move |entity, world| {
                let Some(deferred_removes) = weak_despawn_proxy_removes.upgrade() else {
                    return;
                };

                if !world.has_component::<RenderProxyBinding>(entity) {
                    return;
                }

                world.get_component::<RenderProxyBinding, _>(entity, |binding| {
                    if let Ok(mut deferred_removes) = deferred_removes.lock() {
                        deferred_removes.push(binding.proxy_id());
                    }
                });
            });
        self.world_bindings.push(WorldRenderBinding::new(
            world_index,
            scene,
            deferred_despawn_proxy_removes,
        ));
        Ok(self
            .world_bindings
            .last()
            .expect("world binding was just pushed"))
    }

    pub fn unbind_world(&mut self, renderer: &Renderer, world_index: u32) -> LeetResult<()> {
        if let Some(index) = self
            .world_bindings
            .iter()
            .position(|binding| binding.world_index == world_index)
        {
            let binding = self.world_bindings.swap_remove(index);
            renderer.remove_scene_proxy(binding.scene())?;
        }

        Ok(())
    }

    /// Drain all bound worlds into their matching renderer scene queues.
    pub fn sync_worlds_to_renderer(&mut self) -> LeetResult<()> {
        for binding in &mut self.world_bindings {
            binding.sync_proxy_removals()?;
            binding.sync_proxy_creations()?;
            binding.sync_mesh_renderer_updates()?;
            binding.sync_dirty_transforms()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leet_core::EngineClock;
    use leet_ecs::{MeshRenderer, Transform, WorldRegistry};
    use leet_math::{Mat4, Vec3};
    use leet_renderer::RenderProxyDescriptor;
    use leet_renderer::RenderProxyKind;
    use std::panic::catch_unwind;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Once;

    static ECS_REGISTRY_INIT: Once = Once::new();
    static TEST_WORLD_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn init_ecs_registry_for_tests() {
        ECS_REGISTRY_INIT.call_once(|| {
            let _ = catch_unwind(|| leet_log::init());
            EngineClock::reset();
            WorldRegistry::init();
        });
    }

    fn unique_test_world_name(prefix: &str) -> &'static str {
        let suffix = TEST_WORLD_COUNTER.fetch_add(1, Ordering::Relaxed);
        Box::leak(format!("{prefix}_{suffix}").into_boxed_str())
    }

    fn syncs_deferred_transform_updates_into_bound_scene() {
        init_ecs_registry_for_tests();

        let mut renderer = Renderer::init().unwrap();
        let mut bridge = RenderBridge {
            world_bindings: Vec::new(),
        };

        let world_name = unique_test_world_name("render_bridge_world");
        let world_index = WorldRegistry::get_mut().create_world(world_name);
        let scene = bridge
            .bind_world(&renderer, world_index, RenderSceneType::World)
            .unwrap()
            .scene()
            .clone();

        let proxy_id = scene
            .add_proxy(
                RenderProxyDescriptor::new(RenderProxyKind::Opaque)
                    .named("BridgeMover")
                    .with_translation(Vec3::ZERO),
            )
            .unwrap();

        let parent = WorldRegistry::get_mut()
            .world_mut(world_name)
            .spawn()
            .add(Transform {
                position: Vec3::new(5.0, 0.0, 0.0),
                ..Transform::default()
            });
        let child = WorldRegistry::get_mut()
            .world_mut(world_name)
            .spawn()
            .add(Transform::default())
            .add(RenderProxyBinding::new(proxy_id));

        child.set_parent(parent);
        child.set_world_position(Vec3::new(20.0, 4.0, 6.0));

        bridge.sync_worlds_to_renderer().unwrap();
        renderer.dispatch_general_rendering().unwrap();

        let snapshot = scene.snapshot().unwrap();
        assert_eq!(snapshot.proxies().len(), 1);
        assert_eq!(
            snapshot.proxies()[0].local_to_world(),
            Mat4::from_translation(Vec3::new(20.0, 4.0, 6.0))
        );

        bridge.unbind_world(&renderer, world_index).unwrap();
        WorldRegistry::get_mut().destroy_world(world_name);
    }

    fn parent_dirty_sync_propagates_to_renderable_children() {
        init_ecs_registry_for_tests();

        let mut renderer = Renderer::init().unwrap();
        let mut bridge = RenderBridge {
            world_bindings: Vec::new(),
        };

        let world_name = unique_test_world_name("render_bridge_parent_dirty");
        let world_index = WorldRegistry::get_mut().create_world(world_name);
        let scene = bridge
            .bind_world(&renderer, world_index, RenderSceneType::World)
            .unwrap()
            .scene()
            .clone();

        let proxy_id = scene
            .add_proxy(
                RenderProxyDescriptor::new(RenderProxyKind::Opaque)
                    .named("BridgeChild")
                    .with_translation(Vec3::ZERO),
            )
            .unwrap();

        let parent = WorldRegistry::get_mut()
            .world_mut(world_name)
            .spawn()
            .add(Transform::default());
        let child = WorldRegistry::get_mut()
            .world_mut(world_name)
            .spawn()
            .add(Transform {
                position: Vec3::new(1.0, 2.0, 3.0),
                ..Transform::default()
            })
            .add(RenderProxyBinding::new(proxy_id));

        child.set_parent(parent);

        bridge.sync_worlds_to_renderer().unwrap();
        renderer.dispatch_general_rendering().unwrap();

        parent.set_world_position(Vec3::new(10.0, 0.0, 0.0));

        bridge.sync_worlds_to_renderer().unwrap();
        renderer.dispatch_general_rendering().unwrap();

        let snapshot = scene.snapshot().unwrap();
        assert_eq!(snapshot.proxies().len(), 1);
        assert_eq!(
            snapshot.proxies()[0].local_to_world(),
            Mat4::from_translation(Vec3::new(11.0, 2.0, 3.0))
        );

        bridge.unbind_world(&renderer, world_index).unwrap();
        WorldRegistry::get_mut().destroy_world(world_name);
    }

    fn creates_proxy_for_new_mesh_renderer_without_scene_scan() {
        init_ecs_registry_for_tests();

        let mut renderer = Renderer::init().unwrap();
        let mut bridge = RenderBridge {
            world_bindings: Vec::new(),
        };

        let world_name = unique_test_world_name("render_bridge_create_proxy");
        let world_index = WorldRegistry::get_mut().create_world(world_name);
        let scene = bridge
            .bind_world(&renderer, world_index, RenderSceneType::World)
            .unwrap()
            .scene()
            .clone();

        let entity = WorldRegistry::get_mut()
            .world_mut(world_name)
            .spawn()
            .add(Transform::default())
            .add(MeshRenderer::new(7, 11));
        entity.set_world_position(Vec3::new(3.0, 4.0, 5.0));

        bridge.sync_worlds_to_renderer().unwrap();
        renderer.dispatch_general_rendering().unwrap();

        let snapshot = scene.snapshot().unwrap();
        assert_eq!(snapshot.proxies().len(), 1);
        let proxy = &snapshot.proxies()[0];
        assert_eq!(proxy.mesh_handle(), 7);
        assert_eq!(proxy.material_handle(), 11);
        assert!(proxy.casts_shadows());
        assert!(proxy.is_visible());
        assert_eq!(
            proxy.local_to_world(),
            Mat4::from_translation(Vec3::new(3.0, 4.0, 5.0))
        );

        bridge.unbind_world(&renderer, world_index).unwrap();
        WorldRegistry::get_mut().destroy_world(world_name);
    }

    fn mesh_renderer_updates_sync_into_existing_proxy() {
        init_ecs_registry_for_tests();

        let mut renderer = Renderer::init().unwrap();
        let mut bridge = RenderBridge {
            world_bindings: Vec::new(),
        };

        let world_name = unique_test_world_name("render_bridge_update_proxy");
        let world_index = WorldRegistry::get_mut().create_world(world_name);
        let scene = bridge
            .bind_world(&renderer, world_index, RenderSceneType::World)
            .unwrap()
            .scene()
            .clone();

        let entity = WorldRegistry::get_mut()
            .world_mut(world_name)
            .spawn()
            .add(Transform::default())
            .add(MeshRenderer::new(7, 11));

        bridge.sync_worlds_to_renderer().unwrap();
        renderer.dispatch_general_rendering().unwrap();

        entity.mesh_renderer_mut(|mesh_renderer| {
            mesh_renderer.mesh_handle = 13;
            mesh_renderer.material_handle = 17;
            mesh_renderer.visible = false;
            mesh_renderer.casts_shadows = false;
        });

        bridge.sync_worlds_to_renderer().unwrap();
        renderer.dispatch_general_rendering().unwrap();

        let snapshot = scene.snapshot().unwrap();
        assert_eq!(snapshot.proxies().len(), 1);
        let proxy = &snapshot.proxies()[0];
        assert_eq!(proxy.mesh_handle(), 13);
        assert_eq!(proxy.material_handle(), 17);
        assert!(!proxy.casts_shadows());
        assert!(!proxy.is_visible());

        bridge.unbind_world(&renderer, world_index).unwrap();
        WorldRegistry::get_mut().destroy_world(world_name);
    }

    fn despawning_renderable_entity_removes_proxy() {
        init_ecs_registry_for_tests();

        let mut renderer = Renderer::init().unwrap();
        let mut bridge = RenderBridge {
            world_bindings: Vec::new(),
        };

        let world_name = unique_test_world_name("render_bridge_despawn_proxy");
        let world_index = WorldRegistry::get_mut().create_world(world_name);
        let scene = bridge
            .bind_world(&renderer, world_index, RenderSceneType::World)
            .unwrap()
            .scene()
            .clone();

        let entity = WorldRegistry::get_mut()
            .world_mut(world_name)
            .spawn()
            .add(Transform::default())
            .add(MeshRenderer::new(7, 11));

        bridge.sync_worlds_to_renderer().unwrap();
        renderer.dispatch_general_rendering().unwrap();
        assert_eq!(scene.snapshot().unwrap().proxies().len(), 1);

        entity.despawn();

        bridge.sync_worlds_to_renderer().unwrap();
        renderer.dispatch_general_rendering().unwrap();

        assert!(scene.snapshot().unwrap().proxies().is_empty());

        bridge.unbind_world(&renderer, world_index).unwrap();
        WorldRegistry::get_mut().destroy_world(world_name);
    }

    #[test]
    fn render_bridge_end_to_end_cases() {
        syncs_deferred_transform_updates_into_bound_scene();
        parent_dirty_sync_propagates_to_renderable_children();
        creates_proxy_for_new_mesh_renderer_without_scene_scan();
        mesh_renderer_updates_sync_into_existing_proxy();
        despawning_renderable_entity_removes_proxy();
    }
}
