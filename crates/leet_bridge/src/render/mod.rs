//! ECS-to-renderer bridge types and synchronization helpers.

mod render_bridge;
mod render_proxy_binding;

pub use render_bridge::{RenderBridge, WorldRenderBinding};
pub use render_proxy_binding::RenderProxyBinding;
