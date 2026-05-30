use bevy_camera::{CameraOutputMode, ClearColorConfig, MsaaWriteback};
use bevy_ecs::world::World;
use bevy_math::{Mat4, UVec2, UVec4};
use bevy_transform::components::GlobalTransform;

use crate::{RenderCamera, RenderCameraRegistration};
use bevy_math::URect;

use super::*;

fn camera_registration(raw_id: u64, order: isize, view_index: u32) -> RenderCameraRegistration {
    let mut world = World::new();
    let entity = world.spawn_empty().id();

    RenderCameraRegistration {
        camera_entity: entity,
        camera_id: RenderCameraId(raw_id),
        target_view_index: view_index,
        viewport: URect::new(0, 0, 128, 72),
        camera: RenderCamera {
            target: None,
            physical_viewport_size: Some(UVec2::new(128, 72)),
            physical_target_size: Some(UVec2::new(128, 72)),
            viewport: None,
            clip_from_view: Mat4::IDENTITY,
            world_from_view: GlobalTransform::IDENTITY,
            viewport_rect: UVec4::new(0, 0, 128, 72),
            invert_culling: false,
            main_pass_texture_format: wgpu::TextureFormat::Rgba8UnormSrgb,
            order,
            output_mode: CameraOutputMode::default(),
            msaa_writeback: MsaaWriteback::default(),
            clear_color: ClearColorConfig::Default,
            exposure: 1.0,
            hdr: false,
            compositing_space: None,
        },
    }
}

fn prepare_context(frame_index: u64) -> CameraPrepareContext {
    CameraPrepareContext::new(
        true,
        true,
        false,
        frame_index,
        URect::new(0, 0, 128, 72),
    )
}

#[test]
fn selected_dependency_camera_sorts_before_parent_camera() {
    let main = camera_registration(1, 1, 0);
    let mirror = camera_registration(2, 0, 1);
    let mut storage = RenderCameraStorage::new();

    storage.register_camera(
        main.clone(),
        CameraManagement::Permanent,
        CameraRenderPolicy::OnDemand,
    );
    storage.register_camera(
        mirror.clone(),
        CameraManagement::Permanent,
        CameraRenderPolicy::OnDemand,
    );
    storage
        .add_camera_dependency(
            main.camera_id,
            mirror.camera_id,
            CameraManagement::Permanent,
            CameraDependencyFlags::output_color(),
        )
        .unwrap();

    let prepared = storage
        .prepare_frame_cameras(prepare_context(1), &[main.camera_id])
        .unwrap();

    assert_eq!(
        prepared
            .iter()
            .map(|camera| camera.camera_id)
            .collect::<Vec<_>>(),
        vec![mirror.camera_id, main.camera_id]
    );
    assert_eq!(
        prepared[1].selected_dependencies[0].camera_id,
        mirror.camera_id
    );
}

#[test]
fn dependency_depth_limit_excludes_recursive_grandchild_camera() {
    let main = camera_registration(1, 2, 0);
    let mirror = camera_registration(2, 1, 1);
    let mirror_child = camera_registration(3, 0, 2);
    let mut storage = RenderCameraStorage::new();

    for view in [main.clone(), mirror.clone(), mirror_child.clone()] {
        storage.register_camera(
            view,
            CameraManagement::Permanent,
            CameraRenderPolicy::OnDemand,
        );
    }
    storage
        .add_camera_dependency(
            main.camera_id,
            mirror.camera_id,
            CameraManagement::Permanent,
            CameraDependencyFlags::output_color(),
        )
        .unwrap();
    storage
        .add_camera_dependency(
            mirror.camera_id,
            mirror_child.camera_id,
            CameraManagement::Permanent,
            CameraDependencyFlags::output_color(),
        )
        .unwrap();

    let prepared = storage
        .prepare_frame_cameras(prepare_context(1), &[main.camera_id])
        .unwrap();

    assert_eq!(
        prepared
            .iter()
            .map(|camera| camera.camera_id)
            .collect::<Vec<_>>(),
        vec![mirror.camera_id, main.camera_id]
    );
}

#[test]
fn camera_dependency_rejects_cycles() {
    let first = camera_registration(1, 0, 0);
    let second = camera_registration(2, 1, 1);
    let mut storage = RenderCameraStorage::new();

    for view in [first.clone(), second.clone()] {
        storage.register_camera(
            view,
            CameraManagement::Permanent,
            CameraRenderPolicy::OnDemand,
        );
    }

    assert!(storage
        .add_camera_dependency(
            first.camera_id,
            second.camera_id,
            CameraManagement::Permanent,
            CameraDependencyFlags::empty(),
        )
        .unwrap());
    assert!(!storage
        .add_camera_dependency(
            second.camera_id,
            first.camera_id,
            CameraManagement::Permanent,
            CameraDependencyFlags::empty(),
        )
        .unwrap());
}

#[test]
fn temporal_dependency_is_removed_when_not_requested_again() {
    let main = camera_registration(1, 1, 0);
    let temp = camera_registration(2, 0, 1);
    let mut storage = RenderCameraStorage::new();

    storage.register_camera(
        main.clone(),
        CameraManagement::Permanent,
        CameraRenderPolicy::OnDemand,
    );
    storage.register_camera(
        temp.clone(),
        CameraManagement::Temporal,
        CameraRenderPolicy::OnDemand,
    );
    storage
        .add_camera_dependency(
            main.camera_id,
            temp.camera_id,
            CameraManagement::Temporal,
            CameraDependencyFlags::output_color(),
        )
        .unwrap();

    let first_prepared = storage
        .prepare_frame_cameras(prepare_context(1), &[main.camera_id])
        .unwrap();
    assert_eq!(first_prepared.len(), 2);

    let second_prepared = storage
        .prepare_frame_cameras(prepare_context(2), &[main.camera_id])
        .unwrap();
    assert_eq!(
        second_prepared
            .iter()
            .map(|camera| camera.camera_id)
            .collect::<Vec<_>>(),
        vec![main.camera_id]
    );
}
