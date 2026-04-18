use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use flecs_ecs::prelude::*;
use leet_ecs::{Entity as LeetEntity, WorldRegistry};
use std::sync::{Once, OnceLock};

const ENTITY_COUNT: usize = 10_000;

static LEET_ECS_INIT: Once = Once::new();
static LEET_BENCH_DATA: OnceLock<LeetRelationshipBenchData> = OnceLock::new();

#[derive(Debug)]
struct LeetRelationshipBenchData {
    parent: LeetEntity,
    children: Vec<LeetEntity>,
}

fn setup_leet_relationship_entities() -> &'static LeetRelationshipBenchData {
    LEET_ECS_INIT.call_once(|| {
        let _ = std::panic::catch_unwind(leet_log::init);
        WorldRegistry::init();

        let world = WorldRegistry::get_mut().main_world_mut();
        let parent = world.spawn();
        let mut children = Vec::with_capacity(ENTITY_COUNT);

        for _ in 0..ENTITY_COUNT {
            let child = world.spawn();
            child.set_parent(parent);
            children.push(child);
        }

        LEET_BENCH_DATA
            .set(LeetRelationshipBenchData { parent, children })
            .expect("benchmark entities must be initialized only once");
    });

    LEET_BENCH_DATA
        .get()
        .expect("benchmark entities are initialized")
}

fn bench_flecs_entity_view_parent_10k(c: &mut Criterion) {
    let world = World::new();
    let parent = world.entity();
    let children: Vec<_> = (0..ENTITY_COUNT)
        .map(|_| world.entity().child_of(parent))
        .collect();

    c.bench_function("flecs_entity_view_parent_10k", |b| {
        b.iter(|| {
            let mut sum = 0u64;

            for &child in &children {
                let parent = child.parent().expect("benchmark child must have a parent");
                sum ^= parent.0;
            }

            black_box(sum);
        });
    });
}

fn bench_leet_entity_parent_10k(c: &mut Criterion) {
    let children = &setup_leet_relationship_entities().children;

    c.bench_function("leet_entity_parent_10k", |b| {
        b.iter(|| {
            let mut sum = 0u64;

            for &child in children {
                let parent = child.parent().expect("benchmark child must have a parent");
                sum ^= parent.id;
            }

            black_box(sum);
        });
    });
}

fn bench_flecs_entity_view_add_child_10k(c: &mut Criterion) {
    let world = Box::leak(Box::new(World::new()));
    let parent = world.entity();
    let children: Vec<_> = (0..ENTITY_COUNT).map(|_| world.entity()).collect();

    c.bench_function("flecs_entity_view_add_child_10k", |b| {
        b.iter_batched(
            || {
                for &child in &children {
                    child.remove((flecs::ChildOf::ID, *parent));
                }
            },
            |_| {
                let mut sum = 0u64;

                for &child in &children {
                    child.child_of(parent);
                    sum ^= child.0;
                }

                black_box(sum);
            },
            BatchSize::LargeInput,
        );
    });
}

fn bench_flecs_entity_view_remove_child_10k(c: &mut Criterion) {
    let world = Box::leak(Box::new(World::new()));
    let parent = world.entity();
    let children: Vec<_> = (0..ENTITY_COUNT).map(|_| world.entity()).collect();

    c.bench_function("flecs_entity_view_remove_child_10k", |b| {
        b.iter_batched(
            || {
                for &child in &children {
                    child.child_of(parent);
                }
            },
            |_| {
                let mut sum = 0u64;

                for &child in &children {
                    child.remove((flecs::ChildOf::ID, *parent));
                    sum ^= child.0;
                }

                black_box(sum);
            },
            BatchSize::LargeInput,
        );
    });
}

fn bench_leet_entity_add_child_10k(c: &mut Criterion) {
    let bench_data = setup_leet_relationship_entities();
    let parent = bench_data.parent;
    let children = bench_data.children.as_slice();

    c.bench_function("leet_entity_add_child_10k", |b| {
        b.iter_batched(
            || {
                for &child in children {
                    child.clear_parent();
                }
            },
            |_| {
                let mut sum = 0u64;

                for &child in children {
                    parent.add_child(child);
                    sum ^= child.id;
                }

                black_box(sum);
            },
            BatchSize::LargeInput,
        );
    });
}

fn bench_leet_entity_remove_child_10k(c: &mut Criterion) {
    let bench_data = setup_leet_relationship_entities();
    let parent = bench_data.parent;
    let children = bench_data.children.as_slice();

    c.bench_function("leet_entity_remove_child_10k", |b| {
        b.iter_batched(
            || {
                for &child in children {
                    parent.add_child(child);
                }
            },
            |_| {
                let mut sum = 0u64;

                for &child in children {
                    parent.remove_child(child);
                    sum ^= child.id;
                }

                black_box(sum);
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(
    benches,
    bench_flecs_entity_view_parent_10k,
    bench_leet_entity_parent_10k,
    bench_flecs_entity_view_add_child_10k,
    bench_leet_entity_add_child_10k,
    bench_flecs_entity_view_remove_child_10k,
    bench_leet_entity_remove_child_10k
);
criterion_main!(benches);
