use criterion::{black_box, criterion_group, criterion_main, Criterion};
use leet_ecs::{Entity, Transform, WorldRegistry};
use std::sync::{Once, OnceLock};

const ENTITY_COUNT: usize = 100_000;

static ECS_INIT: Once = Once::new();
static ENTITIES: OnceLock<Vec<Entity>> = OnceLock::new();

fn setup_entities() -> &'static [Entity] {
    ECS_INIT.call_once(|| {
        let _ = std::panic::catch_unwind(leet_log::init);
        WorldRegistry::init();

        let world = WorldRegistry::get_mut().main_world_mut();
        let mut entities = Vec::with_capacity(ENTITY_COUNT);

        for _ in 0..ENTITY_COUNT {
            let entity = world.spawn().add(Transform::default());
            entities.push(entity);
        }

        ENTITIES
            .set(entities)
            .expect("benchmark entities must be initialized only once");
    });

    ENTITIES
        .get()
        .expect("benchmark entities are initialized")
        .as_slice()
}

fn bench_entity_get_once_per_entity_100k(c: &mut Criterion) {
    let entities = setup_entities();

    c.bench_function("entity_get_transform_once_per_entity_100k", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;

            for &entity in entities {
                entity.get::<Transform, _>(|transform| {
                    sum += transform.position.x + transform.scale.x;
                });
            }

            black_box(sum);
        });
    });
}

criterion_group!(benches, bench_entity_get_once_per_entity_100k);
criterion_main!(benches);
