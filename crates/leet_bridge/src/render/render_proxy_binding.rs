//! ECS component that binds a renderable entity to a renderer-owned proxy.

use leet_ecs::Component;
use leet_renderer::RenderProxyId;

/// Renderer-side ECS binding for entities mirrored by a render proxy.
#[derive(Debug, Clone, Component)]
pub struct RenderProxyBinding {
    proxy_id: RenderProxyId,
    last_transform_dirty_frame: u64,
}

impl RenderProxyBinding {
    pub fn new(proxy_id: RenderProxyId) -> Self {
        Self {
            proxy_id,
            last_transform_dirty_frame: u64::MAX,
        }
    }

    pub fn proxy_id(&self) -> RenderProxyId {
        self.proxy_id
    }

    pub fn last_transform_dirty_frame(&self) -> u64 {
        self.last_transform_dirty_frame
    }

    pub(crate) fn is_transform_synced_for_frame(&self, current_frame: u64) -> bool {
        self.last_transform_dirty_frame == current_frame
    }

    pub(crate) fn mark_transform_synced(&mut self, current_frame: u64) -> bool {
        self.mark_transform_dirty(current_frame)
    }

    /// Mark the transform dirty for the provided frame.
    ///
    /// Returns `true` only the first time this entity is marked dirty for that
    /// frame, which lets callers enqueue it once into a deferred sync list.
    pub fn mark_transform_dirty(&mut self, current_frame: u64) -> bool {
        if self.last_transform_dirty_frame == current_frame {
            false
        } else {
            self.last_transform_dirty_frame = current_frame;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_transform_dirty_once_per_frame() {
        let mut binding = RenderProxyBinding::new(RenderProxyId::new(7));

        assert!(binding.mark_transform_dirty(3));
        assert_eq!(binding.last_transform_dirty_frame(), 3);
        assert!(!binding.mark_transform_dirty(3));
        assert!(binding.mark_transform_dirty(4));
        assert_eq!(binding.last_transform_dirty_frame(), 4);
    }
}
