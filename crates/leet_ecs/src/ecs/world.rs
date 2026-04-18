//! Thin facade wrapper around the Flecs world implementation.

use super::entity::Entity;
use crate::Transform;
use flecs_ecs::prelude::*;
use leet_math::{Mat4, Vec3};
use std::mem;

pub use flecs_ecs::macros::Component;

pub trait Component: ComponentId + DataComponent + Send + Sync + 'static {}

impl<T> Component for T where T: ComponentId + DataComponent + Send + Sync + 'static {}

type PreDespawnHook = Box<dyn FnMut(Entity, &World) + Send>;

pub struct World {
    inner: flecs_ecs::core::World,
    world_index: u32,
    deferred_transform_entities: Vec<Entity>,
    deferred_render_proxy_creates: Vec<Entity>,
    deferred_render_proxy_removes: Vec<Entity>,
    deferred_mesh_renderer_updates: Vec<Entity>,
    pre_despawn_hooks: Vec<PreDespawnHook>,
}

impl World {
    pub(crate) fn new(world_index: u32) -> Self {
        Self {
            inner: flecs_ecs::core::World::new(),
            world_index,
            deferred_transform_entities: Vec::new(),
            deferred_render_proxy_creates: Vec::new(),
            deferred_render_proxy_removes: Vec::new(),
            deferred_mesh_renderer_updates: Vec::new(),
            pre_despawn_hooks: Vec::new(),
        }
    }

    /// Returns the registry slot index that owns this world.
    pub fn world_index(&self) -> u32 {
        self.world_index
    }

    /// Spawns a new entity and returns an engine handle with the owning world index.
    pub fn spawn(&mut self) -> Entity {
        let entity_view = self.inner.entity();
        Entity {
            id: *entity_view.id(),
            world_index: self.world_index,
        }
    }

    /// Despawns the entity. Delegates to flecs.
    pub fn despawn(&mut self, entity: Entity) {
        let mut pre_despawn_hooks = mem::take(&mut self.pre_despawn_hooks);
        for hook in &mut pre_despawn_hooks {
            hook(entity, self);
        }
        self.pre_despawn_hooks = pre_despawn_hooks;
        self.inner.entity_from_id(entity.id).destruct();
    }

    /// Adds a component to an entity.
    pub fn add_component<T: Component>(&mut self, entity: Entity, component: T) {
        self.inner.entity_from_id(entity.id).set(component);
    }

    /// Removes a component from an entity.
    pub fn remove_component<T: Component>(&mut self, entity: Entity) {
        self.inner.entity_from_id(entity.id).remove(T::id());
    }

    /// Returns whether the entity currently has the requested component.
    pub fn has_component<T: Component>(&self, entity: Entity) -> bool {
        self.inner.entity_from_id(entity.id).has(T::id())
    }

    /// Returns whether the entity currently exists in this world.
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.inner.is_alive(entity.id)
    }

    /// Immutable component access via closure.
    /// &T never escapes; lifetime is contained within the closure.
    pub fn get_component<T: Component, F: FnOnce(&T)>(&self, entity: Entity, f: F) {
        self.inner.entity_from_id(entity.id).get::<&T>(f);
    }

    /// Mutable component access via closure.
    /// &mut T never escapes; mutation is applied in place, borrow released on closure exit.
    pub fn get_component_mut<T: Component, F: FnOnce(&mut T)>(&mut self, entity: Entity, f: F) {
        self.inner.entity_from_id(entity.id).get::<&mut T>(f);
    }

    /// Returns the Flecs parent of the entity if the entity currently has one.
    pub fn parent_of(&self, entity: Entity) -> Option<Entity> {
        self.inner
            .entity_from_id(entity.id)
            .parent()
            .map(|parent| Entity {
                id: parent.0,
                world_index: self.world_index,
            })
    }

    /// Sets the Flecs ChildOf relationship for the entity.
    pub fn set_parent(&mut self, entity: Entity, parent: Entity) {
        self.inner.entity_from_id(entity.id).child_of(parent.id);
    }

    /// Clears the Flecs ChildOf relationship when the entity currently has a parent.
    pub fn clear_parent(&mut self, entity: Entity) {
        let entity_view = self.inner.entity_from_id(entity.id);

        if let Some(parent) = entity_view.parent() {
            entity_view.remove((flecs::ChildOf::ID, *parent));
        }
    }

    /// Returns true when the entity has at least one child in the Flecs hierarchy.
    pub fn has_children(&self, entity: Entity) -> bool {
        self.inner.entity_from_id(entity.id).has_children()
    }

    /// Returns the number of direct children in the Flecs hierarchy.
    pub fn child_count(&self, entity: Entity) -> u32 {
        self.inner.entity_from_id(entity.id).count_children()
    }

    /// Iterates direct Flecs children and maps them back to engine Entity handles.
    pub fn each_child<F: FnMut(Entity)>(&self, entity: Entity, mut f: F) -> bool {
        let world_index = self.world_index;
        self.inner.entity_from_id(entity.id).each_child(|child| {
            f(Entity {
                id: child.0,
                world_index,
            });
        })
    }

    /// Resolves the entity's local-to-world matrix by recursively composing parents.
    pub fn local_to_world_matrix(&self, entity: Entity) -> Mat4 {
        let mut local_matrix = Mat4::IDENTITY;
        self.get_component::<Transform, _>(entity, |transform| {
            local_matrix = Mat4::from_scale_rotation_translation(
                transform.scale,
                transform.rotation,
                transform.position,
            );
        });

        if let Some(parent) = self.parent_of(entity) {
            self.local_to_world_matrix(parent) * local_matrix
        } else {
            local_matrix
        }
    }

    /// Returns the entity's resolved world-space translation.
    pub fn world_position(&self, entity: Entity) -> Vec3 {
        self.local_to_world_matrix(entity)
            .transform_point3(Vec3::ZERO)
    }

    /// Queue an entity for deferred transform-to-render sync later in the frame.
    pub fn defer_transform_sync(&mut self, entity: Entity) {
        self.deferred_transform_entities.push(entity);
    }

    /// Drain the entities whose transforms were dirtied this frame.
    pub fn take_deferred_transform_syncs(&mut self) -> Vec<Entity> {
        std::mem::take(&mut self.deferred_transform_entities)
    }

    /// Queue an entity for deferred render-proxy creation later in the frame.
    pub fn defer_render_proxy_create(&mut self, entity: Entity) {
        self.deferred_render_proxy_creates.push(entity);
    }

    /// Drain the entities whose render proxies should be created this frame.
    pub fn take_deferred_render_proxy_creates(&mut self) -> Vec<Entity> {
        std::mem::take(&mut self.deferred_render_proxy_creates)
    }

    /// Queue an entity for deferred render-proxy removal later in the frame.
    pub fn defer_render_proxy_remove(&mut self, entity: Entity) {
        self.deferred_render_proxy_removes.push(entity);
    }

    /// Drain the entities whose render proxies should be removed this frame.
    pub fn take_deferred_render_proxy_removes(&mut self) -> Vec<Entity> {
        mem::take(&mut self.deferred_render_proxy_removes)
    }

    /// Queue an entity for deferred mesh-renderer-to-proxy sync later in the frame.
    pub fn defer_mesh_renderer_update(&mut self, entity: Entity) {
        self.deferred_mesh_renderer_updates.push(entity);
    }

    /// Drain the entities whose mesh-renderer state was dirtied this frame.
    pub fn take_deferred_mesh_renderer_updates(&mut self) -> Vec<Entity> {
        mem::take(&mut self.deferred_mesh_renderer_updates)
    }

    /// Registers a callback that runs immediately before an entity is despawned.
    pub fn add_pre_despawn_hook<F>(&mut self, hook: F)
    where
        F: FnMut(Entity, &World) + Send + 'static,
    {
        self.pre_despawn_hooks.push(Box::new(hook));
    }
}
