use super::*;

#[test]
fn slot_reuse_bumps_generation() {
    let mut scene = GpuScene::default();
    let first_id = scene.allocate_proxy(RenderProxyDescriptor::default());

    assert!(scene.remove_proxy(first_id));

    let second_id = scene.allocate_proxy(RenderProxyDescriptor::default());
    assert_eq!(first_id.slot_index(), second_id.slot_index());
    assert_ne!(first_id.generation(), second_id.generation());
}

#[test]
fn multiple_updates_preserve_live_proxy_count() {
    let mut scene = GpuScene::default();
    let proxy_id = scene.allocate_proxy(RenderProxyDescriptor::default());

    scene.set_visibility(proxy_id, false);
    scene.set_casts_shadows(proxy_id, false);
    scene.set_kind(proxy_id, RenderProxyKind::Sky);

    assert_eq!(scene.slot_capacity(), 1);
    assert_eq!(scene.live_proxy_count(), 1);
}

#[test]
fn smoke_simulation_zeros_removed_slots() {
    let mut scene = GpuScene::default();
    let proxy_id = scene.allocate_proxy(RenderProxyDescriptor::default());

    let simulated = GpuSceneFakeGpuEmulation::simulate_computed_instances(&scene);
    assert!(simulated[0].visible());

    assert!(scene.remove_proxy(proxy_id));

    let simulated = GpuSceneFakeGpuEmulation::simulate_computed_instances(&scene);
    assert!(!simulated[0].visible());
}

#[test]
fn snapshot_previous_inputs_preserves_slot_indices() {
    let mut scene = GpuScene::default();
    let proxy_id = scene.allocate_proxy(
        RenderProxyDescriptor::default()
            .with_tag(17)
            .with_visible(false),
    );

    scene.snapshot_previous_inputs();

    let previous_input = scene.previous_inputs().get(proxy_id.slot_index() as u32);
    assert_eq!(
        previous_input.previous_input_index(),
        Some(proxy_id.slot_index() as u32)
    );
    assert_eq!(previous_input.tag, 17);
    assert!(!previous_input.is_visible());
}

#[test]
fn mesh_input_uses_explicit_gpu_safe_sentinels() {
    let input = GpuInstanceInput::default();

    assert_eq!(input.previous_input_index(), None);
    assert!(!input.mesh_asset_id.is_bound());
    assert!(!input.material_asset_id.is_bound());
    assert!(!input.mesh_asset_slice.is_valid());
}

#[test]
fn flattened_asset_id_packs_index_and_uuid_modes() {
    use bevy_asset::{AssetId, AssetIndex};
    use bevy_image::Image;

    let index_bits = 0x1234_5678_9abc_def0_u64;
    let indexed_flat = GpuFlatAssetId::from(Some(
        AssetId::<Image>::from(AssetIndex::from_bits(index_bits)).untyped(),
    ));
    assert_eq!(indexed_flat.mode, GpuFlatAssetId::MODE_INDEX);
    assert_eq!(indexed_flat.words[0], (index_bits & 0xffff_ffff) as u32);
    assert_eq!(indexed_flat.words[1], (index_bits >> 32) as u32);

    let uuid = AssetId::<Image>::DEFAULT_UUID;
    let uuid_flat = GpuFlatAssetId::from(Some(AssetId::<Image>::Uuid { uuid }.untyped()));
    let (hi, lo) = uuid.as_u64_pair();
    assert_eq!(uuid_flat.mode, GpuFlatAssetId::MODE_UUID);
    assert_eq!(
        uuid_flat.words,
        [
            (lo & 0xffff_ffff) as u32,
            (lo >> 32) as u32,
            (hi & 0xffff_ffff) as u32,
            (hi >> 32) as u32,
        ]
    );
}

#[test]
fn phase_indices_point_into_shared_computed_slots() {
    let mut scene = GpuScene::default();
    let opaque_visible = scene.allocate_proxy(RenderProxyDescriptor::default().with_visible(true));
    let _opaque_hidden = scene.allocate_proxy(RenderProxyDescriptor::default().with_visible(false));
    let sky_visible = scene.allocate_proxy(
        RenderProxyDescriptor::default()
            .with_visible(true)
            .with_casts_shadows(false)
            .with_tag(2),
    );
    assert!(scene.set_kind(sky_visible, RenderProxyKind::Sky));

    let opaque_phase =
        GpuSceneFakeGpuEmulation::simulate_phase_instance_indices(&scene, GpuScenePhase::Opaque);
    let shadow_phase =
        GpuSceneFakeGpuEmulation::simulate_phase_instance_indices(&scene, GpuScenePhase::Shadow);

    assert_eq!(opaque_phase.len(), 1);
    assert_eq!(
        opaque_phase[0].computed_instance_index,
        opaque_visible.slot_index() as u32
    );
    assert_eq!(shadow_phase.len(), 1);
    assert_eq!(
        shadow_phase[0].computed_instance_index,
        opaque_visible.slot_index() as u32
    );
}
