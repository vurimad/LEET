use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use flecs_ecs::prelude::*;

const ENTITY_COUNT: usize = 10_000;

#[derive(Component, Clone, Copy, Default)]
struct BenchComponent {
    value: u32,
}

fn draft() {
    let world = Box::leak(Box::new(World::new()));
    let q = world.query::<&BenchComponent>().detect_changes().build();
}

fn bench_flecs_entity_view_remove_component_10k(c: &mut Criterion) {
    let world = Box::leak(Box::new(World::new()));
    let entities: Vec<_> = (0..ENTITY_COUNT).map(|_| world.entity()).collect();

    c.bench_function("flecs_entity_view_remove_component_10k", |b| {
        b.iter_batched(
            || {
                for &entity in &entities {
                    entity.set(BenchComponent { value: 1 });
                }
            },
            |_| {
                let mut sum = 0u64;

                for &entity in &entities {
                    entity.remove(BenchComponent::id());
                    sum ^= entity.0;
                }

                black_box(sum);
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, bench_flecs_entity_view_remove_component_10k);
criterion_main!(benches);
