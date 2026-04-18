//! Render-facing mesh and material bindings for an entity.

use crate::Component;

/// Render data for an entity. Holds handles to mesh and material assets. No behavior.
#[derive(Debug, Clone, Component)]
pub struct MeshRenderer {
    pub mesh_handle: u64,
    pub material_handle: u64,
    pub visible: bool,
    pub casts_shadows: bool,
    pub dirty_frame_index: u64,
}

impl MeshRenderer {
    pub fn new(mesh_handle: u64, material_handle: u64) -> Self {
        Self {
            mesh_handle,
            material_handle,
            visible: true,
            casts_shadows: true,
            dirty_frame_index: u64::MAX,
        }
    }
}
