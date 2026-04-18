//! LEET Math - math types and helpers for game development.
//!
//! This crate currently re-exports a curated `glam` surface so the rest of the
//! engine can depend on `leet_math` instead of taking a direct dependency on
//! the backend math crate.

pub use glam::{Affine3A, EulerRot, Mat3, Mat4, Quat, Vec2, Vec3, Vec3A, Vec4};

/// Return the zero vector.
pub const fn vec3_zero() -> Vec3 {
    Vec3::ZERO
}

/// Return the one vector.
pub const fn vec3_one() -> Vec3 {
    Vec3::ONE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexports_vec3() {
        let value = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(value.z, 3.0);
    }
}
