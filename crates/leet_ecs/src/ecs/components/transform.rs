//! Spatial transform component data shared across engine systems.

use crate::Component;
use leet_math::{Quat, Vec3};

/// Spatial data for an entity. Position, rotation, scale. No behavior.
#[derive(Debug, Clone, Component)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub rotation_euler: Vec3,
    pub scale: Vec3,
    pub dirty_frame_index: u64,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            rotation_euler: Vec3::ZERO,
            scale: Vec3::ONE,
            dirty_frame_index: u64::MAX,
        }
    }
}
