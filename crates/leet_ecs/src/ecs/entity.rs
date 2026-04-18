//! Lightweight gameplay entity handles resolved through the global registry.

use super::registry::WorldRegistry;
use crate::{Component, MeshRenderer, Transform};
use leet_core::EngineClock;
use leet_math::{Mat4, Vec3};
use std::any::TypeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    pub id: u64,
    pub world_index: u32,
}

impl Entity {
    /// Adds a component and returns the same handle for fluent chaining.
    pub fn add<T: Component>(self, component: T) -> Self {
        let world = WorldRegistry::get_mut().world_at_mut(self.world_index);
        world.add_component(self, component);

        if TypeId::of::<T>() == TypeId::of::<MeshRenderer>() {
            world.defer_render_proxy_create(self);
        }

        self
    }

    /// Removes a component and returns the same handle for fluent chaining.
    pub fn remove<T: Component>(self) -> Self {
        let world = WorldRegistry::get_mut().world_at_mut(self.world_index);

        if TypeId::of::<T>() == TypeId::of::<MeshRenderer>() {
            world.defer_render_proxy_remove(self);
        }

        world.remove_component::<T>(self);
        self
    }

    /// Reads an immutable component reference without letting the borrow escape.
    pub fn get<T: Component, F: FnOnce(&T)>(self, f: F) {
        WorldRegistry::get()
            .world_at(self.world_index)
            .get_component(self, f);
    }

    /// Reads a mutable component reference without letting the borrow escape.
    pub fn get_mut<T: Component, F: FnOnce(&mut T)>(self, f: F) {
        WorldRegistry::get_mut()
            .world_at_mut(self.world_index)
            .get_component_mut(self, f);
    }

    /// Clones a component value into an output slot with exactly one clone.
    pub fn read<T: Component + Clone>(self, out: &mut T) {
        self.get::<T, _>(|component| *out = component.clone());
    }

    /// Despawns the entity and schedules deferred proxy cleanup for the frame.
    pub fn despawn(self) {
        WorldRegistry::get_mut()
            .world_at_mut(self.world_index)
            .despawn(self);
    }

    pub fn local_to_world_matrix(self) -> Mat4 {
        WorldRegistry::get()
            .world_at(self.world_index)
            .local_to_world_matrix(self)
    }

    /// Sets the entity world position and schedules one deferred sync for this frame.
    pub fn set_world_position(self, position: Vec3) -> Self {
        let local_position = if let Some(parent) = self.parent() {
            parent
                .local_to_world_matrix()
                .inverse()
                .transform_point3(position)
        } else {
            position
        };

        let current_frame = EngineClock::current_frame();
        let world = WorldRegistry::get_mut().world_at_mut(self.world_index);

        let mut should_enqueue = false;
        world.get_component_mut::<Transform, _>(self, |transform| {
            transform.position = local_position;

            if transform.dirty_frame_index != current_frame {
                transform.dirty_frame_index = current_frame;
                should_enqueue = true;
            }
        });

        if should_enqueue {
            world.defer_transform_sync(self);
        }

        self
    }

    /// Mutates the entity mesh renderer and schedules one deferred proxy-state sync for this frame.
    pub fn mesh_renderer_mut<F: FnOnce(&mut MeshRenderer)>(self, f: F) {
        let current_frame = EngineClock::current_frame();
        let world = WorldRegistry::get_mut().world_at_mut(self.world_index);

        let mut should_enqueue = false;
        world.get_component_mut::<MeshRenderer, _>(self, |mesh_renderer| {
            f(mesh_renderer);

            if mesh_renderer.dirty_frame_index != current_frame {
                mesh_renderer.dirty_frame_index = current_frame;
                should_enqueue = true;
            }
        });

        if should_enqueue {
            world.defer_mesh_renderer_update(self);
        }
    }

    /// Returns the entity's current Flecs parent, if any.
    pub fn parent(self) -> Option<Entity> {
        WorldRegistry::get()
            .world_at(self.world_index)
            .parent_of(self)
    }

    /// Sets or replaces the entity's Flecs parent.
    pub fn set_parent(self, parent: Entity) -> Self {
        self.assert_same_world(parent, "parent relationships cannot cross worlds");
        WorldRegistry::get_mut()
            .world_at_mut(self.world_index)
            .set_parent(self, parent);

        self
    }

    /// Clears the entity's current Flecs parent, if any.
    pub fn clear_parent(self) -> Self {
        WorldRegistry::get_mut()
            .world_at_mut(self.world_index)
            .clear_parent(self);

        self
    }

    /// Returns true when the entity currently has a parent.
    pub fn has_parent(self) -> bool {
        self.parent().is_some()
    }

    /// Adds a child entity to this entity and sets the child's parent in one call.
    pub fn add_child(self, child: Entity) -> Self {
        self.assert_same_world(child, "parent relationships cannot cross worlds");

        child.set_parent(self);
        self
    }

    /// Removes a child entity from this entity when it is currently parented to it.
    pub fn remove_child(self, child: Entity) -> Self {
        if child.parent() == Some(self) {
            child.clear_parent();
        }

        self
    }

    /// Returns true when the entity currently has the provided child.
    pub fn has_child(self, child: Entity) -> bool {
        self.assert_same_world(child, "parent relationships cannot cross worlds");
        child.parent() == Some(self)
    }

    /// Returns true when the entity currently has at least one child.
    pub fn has_children(self) -> bool {
        WorldRegistry::get()
            .world_at(self.world_index)
            .has_children(self)
    }

    /// Returns true when the entity currently has no children.
    pub fn is_leaf(self) -> bool {
        !self.has_children()
    }

    /// Returns the number of direct children in the Flecs hierarchy.
    pub fn child_count(self) -> u32 {
        WorldRegistry::get()
            .world_at(self.world_index)
            .child_count(self)
    }

    /// Iterates all direct children without exposing Flecs types.
    pub fn each_child<F: FnMut(Entity)>(self, f: F) -> bool {
        WorldRegistry::get()
            .world_at(self.world_index)
            .each_child(self, f)
    }

    /// Collects direct children into a Vec for convenience.
    pub fn children(self) -> Vec<Entity> {
        let mut children = Vec::new();
        self.each_child(|child| children.push(child));
        children
    }

    fn assert_same_world(self, other: Entity, message: &str) {
        if self.world_index != other.world_index {
            leet_log::LeetFatal!(
                "{message}: entity {} belongs to world {}, entity {} belongs to world {}",
                self.id,
                self.world_index,
                other.id,
                other.world_index
            );
        }
    }
}
