//! Swap-chain surface management.
//!
//! [`RenderSurface`] owns the [`wgpu::Surface`] and its
//! [`wgpu::SurfaceConfiguration`]. It is created once the OS window is ready
//! and reconfigured whenever the window is resized.

use leet_core::{Leeror, LeetResult};
use leet_log::info;

// =============================================================================
// RenderSurface
// =============================================================================

/// Owns the wgpu swap-chain surface and its configuration.
///
/// Created by [`RenderSurface::new`] after the adapter and device exist.
/// Stored inside a [`crate::render_viewport::RenderViewport`], which drives
/// resize and frame acquisition.
#[derive(Debug)]
pub struct RenderSurface {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub size: (u32, u32),
}

impl RenderSurface {
    /// Create and configure the surface.
    ///
    /// The caller is responsible for providing a compatible `adapter` and the
    /// `device` that will actually use the surface.
    pub fn new(
        instance: &wgpu::Instance,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
    ) -> LeetResult<Self> {
        let surface = instance
            .create_surface(target)
            .map_err(|e| Leeror::Init(format!("Failed to create wgpu surface: {e}")))?;

        let caps = surface.get_capabilities(adapter);

        // Prefer sRGB so colours are correct; fall back to the first available.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0,
            height: size.1,
            present_mode: wgpu::PresentMode::Fifo, // vsync
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(device, &config);

        info!(
            "[LEET Renderer] Surface configured: {}x{}, format={:?}, present={:?}",
            size.0, size.1, format, config.present_mode,
        );

        Ok(Self {
            surface,
            config,
            size,
        })
    }

    // =========================================================================
    // Resize
    // =========================================================================

    /// Reconfigure the surface after a window resize.
    ///
    /// Silently skips if either dimension is zero (minimised window).
    pub fn resize(&mut self, device: &wgpu::Device, new_width: u32, new_height: u32) {
        if new_width == 0 || new_height == 0 {
            return;
        }
        self.size = (new_width, new_height);
        self.config.width = new_width;
        self.config.height = new_height;
        self.surface.configure(device, &self.config);
        info!(
            "[LEET Renderer] Surface resized to {}x{}",
            new_width, new_height
        );
    }

    // =========================================================================
    // Frame acquisition
    // =========================================================================

    /// Acquire the next swap-chain texture for rendering.
    pub fn acquire(&self) -> LeetResult<wgpu::SurfaceTexture> {
        self.surface
            .get_current_texture()
            .map_err(|e| Leeror::Runtime(format!("Failed to acquire swap-chain texture: {e}")))
    }

    /// The texture format the surface is configured with.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }
}
