//! Per-frame collection of renderer-owned scene data.
//!
//! This is intentionally small for now. It does not cull or sort aggressively;
//! it only turns a coherent scene snapshot into pass-oriented placeholder lists.

use crate::render_proxy::{RenderProxy, RenderProxyKind};
use crate::render_scene::{RenderSceneProxy, RenderSceneSnapshot};
use leet_core::LeetResult;
use std::sync::Arc;

/// Pass-ready placeholder scene data for one frame.
#[derive(Clone, Debug)]
pub struct CollectedRenderScene {
    clear_color: wgpu::Color,
    opaque_proxies: Vec<RenderProxy>,
    sky_proxies: Vec<RenderProxy>,
}

impl CollectedRenderScene {
    pub fn clear_color(&self) -> wgpu::Color {
        self.clear_color
    }

    pub fn opaque_proxies(&self) -> &[RenderProxy] {
        &self.opaque_proxies
    }

    pub fn sky_proxies(&self) -> &[RenderProxy] {
        &self.sky_proxies
    }

    pub fn total_proxy_count(&self) -> usize {
        self.opaque_proxies.len() + self.sky_proxies.len()
    }
}

/// Minimal scene collector.
#[derive(Default)]
pub struct RenderCollector;

impl RenderCollector {
    pub fn collect(scene: &RenderSceneProxy) -> LeetResult<Arc<CollectedRenderScene>> {
        let snapshot = scene.snapshot()?;
        Ok(Arc::new(Self::collect_snapshot(snapshot)))
    }

    fn collect_snapshot(snapshot: RenderSceneSnapshot) -> CollectedRenderScene {
        let mut opaque_proxies = Vec::new();
        let mut sky_proxies = Vec::new();

        for proxy in snapshot.proxies() {
            if !proxy.is_visible() {
                continue;
            }

            match proxy.kind() {
                RenderProxyKind::Opaque => opaque_proxies.push(proxy.clone()),
                RenderProxyKind::Sky => sky_proxies.push(proxy.clone()),
            }
        }

        CollectedRenderScene {
            clear_color: snapshot.clear_color(),
            opaque_proxies,
            sky_proxies,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_proxy::{RenderProxyDescriptor, RenderProxyKind};

    #[test]
    fn collector_splits_proxies_by_placeholder_pass() {
        let scene = RenderSceneProxy::new();
        scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Opaque).named("Opaque"))
            .unwrap();
        scene
            .add_proxy(RenderProxyDescriptor::new(RenderProxyKind::Sky).named("Sky"))
            .unwrap();
        scene
            .add_proxy(
                RenderProxyDescriptor::new(RenderProxyKind::Opaque)
                    .named("Hidden")
                    .with_visible(false),
            )
            .unwrap();
        scene.hand_off().unwrap();
        scene.apply_synced_updates().unwrap();

        let collected = RenderCollector::collect(&scene).unwrap();

        assert_eq!(collected.opaque_proxies().len(), 1);
        assert_eq!(collected.sky_proxies().len(), 1);
        assert_eq!(collected.total_proxy_count(), 2);
        assert_eq!(collected.opaque_proxies()[0].name(), "Opaque");
        assert_eq!(collected.sky_proxies()[0].name(), "Sky");
    }
}
