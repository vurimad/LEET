//! Core runtime systems owned by the application.
//!
//! This is the first engine-side aggregation point for systems that should be
//! accessible throughout the runtime. For now it only owns the renderer, but
//! it is intentionally shaped to grow as more core systems come online.

use leet_bridge::RenderBridge;
use leet_core::LeetResult;
use leet_renderer::{RenderSceneProxy, Renderer};

/// Engine-owned runtime systems that survive for the lifetime of the app.
pub struct CoreSystems {
    pub renderer: Renderer,
    pub render_bridge: RenderBridge,
}

impl CoreSystems {
    /// Initialize all core systems that do not depend on an OS window.
    pub fn init() -> LeetResult<Self> {
        let renderer = Renderer::init()?;
        let render_bridge = RenderBridge::new(&renderer)?;

        Ok(Self {
            renderer,
            render_bridge,
        })
    }

    /// Returns the renderer scene proxy bound to the main ECS world.
    pub fn main_world_scene(&self) -> &RenderSceneProxy {
        self.render_bridge
            .main_world_scene()
            .expect("render bridge should bind the main world scene")
    }

    /// Drain deferred ECS transform updates into their renderer scene queues.
    pub fn sync_worlds_to_renderer(&mut self) -> LeetResult<()> {
        self.render_bridge.sync_worlds_to_renderer()
    }
}
