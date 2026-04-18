//! Public ECS facade modules and re-exports.

pub mod components;
pub mod entity;
pub mod registry;
pub mod world;

pub use components::*;
pub use entity::Entity;
pub use registry::WorldRegistry;
pub use world::{Component, World};
