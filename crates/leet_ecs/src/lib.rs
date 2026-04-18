//! LEET ECS facade over the Flecs backend.

pub mod ecs;

pub use ecs::*;

#[cfg(test)]
mod tests {
    use super::*;
    use leet_core::EngineClock;
    use leet_math::{Mat4, Vec3};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[test]
    fn registry_and_entities_work_end_to_end() {
        let _ = catch_unwind(|| leet_log::init());

        EngineClock::reset();
        WorldRegistry::init();

        let main_world_index = WorldRegistry::get().main_world().world_index();
        assert_eq!(main_world_index, 0);

        let entity = WorldRegistry::get_mut().main_world_mut().spawn();
        assert_eq!(entity.world_index, main_world_index);
        assert_ne!(entity.id, 0);

        entity
            .add(Transform::default())
            .add(MeshRenderer::new(7, 11));

        entity
            .set_world_position(Vec3::new(1.0, 2.0, 3.0))
            .set_world_position(Vec3::new(4.0, 5.0, 6.0));

        let deferred_syncs = WorldRegistry::get_mut()
            .main_world_mut()
            .take_deferred_transform_syncs();
        assert_eq!(deferred_syncs, vec![entity]);

        entity.get_mut::<Transform, _>(|transform| {
            transform.scale = Vec3::splat(2.0);
        });

        entity.get::<Transform, _>(|transform| {
            assert_eq!(transform.position, Vec3::new(4.0, 5.0, 6.0));
            assert_eq!(transform.scale, Vec3::splat(2.0));
            assert_eq!(transform.dirty_frame_index, EngineClock::current_frame());
        });

        let mut snapshot = Transform::default();
        entity.read(&mut snapshot);
        assert_eq!(snapshot.position, Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(snapshot.scale, Vec3::splat(2.0));

        EngineClock::advance();
        entity.set_world_position(Vec3::new(7.0, 8.0, 9.0));
        let deferred_syncs = WorldRegistry::get_mut()
            .main_world_mut()
            .take_deferred_transform_syncs();
        assert_eq!(deferred_syncs, vec![entity]);

        entity.remove::<MeshRenderer>();
        let removed_component_access = catch_unwind(AssertUnwindSafe(|| {
            entity.get::<MeshRenderer, _>(|_| {});
        }));
        assert!(removed_component_access.is_err());

        let parent_a = WorldRegistry::get_mut().main_world_mut().spawn();
        let parent_b = WorldRegistry::get_mut().main_world_mut().spawn();
        let child_a = WorldRegistry::get_mut().main_world_mut().spawn();
        let child_b = WorldRegistry::get_mut().main_world_mut().spawn();

        assert_eq!(child_a.parent(), None);
        assert_eq!(child_b.parent(), None);
        assert!(child_a.children().is_empty());
        assert!(child_b.children().is_empty());
        assert!(!child_a.has_parent());
        assert!(!child_b.has_parent());

        child_a.set_parent(parent_a);
        child_b.set_parent(parent_a);
        assert!(child_a.has_parent());
        assert!(child_b.has_parent());
        assert_eq!(child_a.parent(), Some(parent_a));
        assert_eq!(parent_a.child_count(), 2);
        assert!(parent_a.has_child(child_a));
        assert!(parent_a.has_child(child_b));
        assert_eq!(parent_a.children(), vec![child_a, child_b]);

        parent_a.add_child(child_a);
        assert_eq!(parent_a.children(), vec![child_a, child_b]);

        parent_b.add_child(child_a);
        assert_eq!(child_a.parent(), Some(parent_b));
        assert_eq!(parent_a.children(), vec![child_b]);
        assert!(parent_a.has_child(child_b));
        assert!(!parent_a.has_child(child_a));
        assert_eq!(parent_b.children(), vec![child_a]);

        parent_b.add_child(child_b);
        assert!(parent_a.is_leaf());
        assert_eq!(parent_b.child_count(), 2);
        assert!(parent_b.has_child(child_a));
        assert!(parent_b.has_child(child_b));

        parent_b.remove_child(child_a);
        assert!(!child_a.has_parent());
        assert_eq!(child_a.parent(), None);
        assert_eq!(parent_b.children(), vec![child_b]);

        child_b.clear_parent();
        assert!(!child_b.has_parent());
        assert!(parent_b.is_leaf());

        let root = WorldRegistry::get_mut()
            .main_world_mut()
            .spawn()
            .add(Transform {
                position: Vec3::new(5.0, 0.0, 0.0),
                ..Transform::default()
            });
        let mid = WorldRegistry::get_mut()
            .main_world_mut()
            .spawn()
            .add(Transform {
                position: Vec3::new(10.0, 0.0, 0.0),
                ..Transform::default()
            });
        let leaf = WorldRegistry::get_mut()
            .main_world_mut()
            .spawn()
            .add(Transform {
                position: Vec3::new(1.0, 2.0, 3.0),
                ..Transform::default()
            });

        mid.set_parent(root);
        leaf.set_parent(mid);

        assert_eq!(
            leaf.local_to_world_matrix(),
            Mat4::from_translation(Vec3::new(16.0, 2.0, 3.0))
        );

        leaf.set_world_position(Vec3::new(20.0, 4.0, 6.0));

        leaf.get::<Transform, _>(|transform| {
            assert_eq!(transform.position, Vec3::new(5.0, 4.0, 6.0));
        });
        assert_eq!(
            leaf.local_to_world_matrix(),
            Mat4::from_translation(Vec3::new(20.0, 4.0, 6.0))
        );

        let gameplay_world_index = WorldRegistry::get_mut().create_world("gameplay");
        assert_eq!(
            WorldRegistry::get().world("gameplay").world_index(),
            gameplay_world_index
        );

        let gameplay_entity = WorldRegistry::get_mut().world_mut("gameplay").spawn();
        assert_eq!(gameplay_entity.world_index, gameplay_world_index);

        WorldRegistry::get_mut().destroy_world("gameplay");

        let destroyed_slot_access = catch_unwind(AssertUnwindSafe(|| {
            let _ = WorldRegistry::get().world_at(gameplay_world_index);
        }));
        assert!(destroyed_slot_access.is_err());

        let reused_world_index = WorldRegistry::get_mut().create_world("ui");
        assert_eq!(reused_world_index, gameplay_world_index);
        assert_eq!(
            WorldRegistry::get().world("ui").world_index(),
            reused_world_index
        );
    }
}
