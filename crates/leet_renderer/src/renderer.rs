//! Core renderer that owns the instance, adapter, device, and queue.
//!
//! # Initialization sequence
//!
//! ```text
//! Renderer::init()
//!   -> wgpu::Instance::new()
//!   -> instance.request_adapter()
//!   -> adapter.request_device()
//! ```

use crate::render_context::RenderContext;
use crate::render_scene::{RenderSceneId, RenderSceneProxy, RenderSceneRegistry, RenderSceneType};
use crate::render_viewport::RenderViewport;
use crate::scene_gpu::SceneGpuState;
use crate::surface::RenderSurface;
use leet_core::{Leeror, LeetResult};
use leet_log::info;
use std::collections::{BTreeMap, BTreeSet};

// =============================================================================
// Renderer
// =============================================================================

/// Owns the wgpu backend state.
///
/// Phase 1 initializes the backend without any window or surface.
/// Phase 2 happens later when the app asks the renderer to create a viewport.
pub struct Renderer {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    scene_registry: RenderSceneRegistry,
    scene_gpu_states: BTreeMap<RenderSceneId, SceneGpuState>,
    main_viewport: Option<RenderViewport>,
    frame_index: u64,
}

impl Renderer {
    /// Initialize the renderer backend without creating any surface or viewport.
    ///
    /// Blocks the calling thread while the async adapter/device requests
    /// complete (driven by `pollster`).
    pub fn init() -> LeetResult<Self> {
        pollster::block_on(Self::init_async())
    }

    async fn init_async() -> LeetResult<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| Leeror::Init("No suitable wgpu adapter found".to_string()))?;

        info!(
            "[LEET Renderer] Adapter: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend,
        );

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("LEET Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .map_err(|e| Leeror::Init(format!("Failed to create wgpu device: {e}")))?;

        info!("[LEET Renderer] Initialized");

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            scene_registry: RenderSceneRegistry::new(),
            scene_gpu_states: BTreeMap::new(),
            main_viewport: None,
            frame_index: 0,
        })
    }

    /// Create the primary presentation viewport from a window handle.
    ///
    /// The target is generic over `wgpu` window handles so the renderer stays
    /// agnostic to the concrete windowing crate used by the app.
    pub fn create_viewport(
        &mut self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
    ) -> LeetResult<()> {
        if self.main_viewport.is_some() {
            info!("[LEET Renderer] Multi-viewport is not handled yet; ignoring extra viewport request");
            return Ok(());
        }

        let surface =
            RenderSurface::new(&self.instance, target, size, &self.adapter, &self.device)?;
        self.main_viewport = Some(RenderViewport::main_window_with_surface(size, surface));
        Ok(())
    }

    // =========================================================================
    // Resize
    // =========================================================================

    /// Reconfigure the main viewport after a window resize.
    ///
    /// Call this from `on_event` when a `WindowEvent::Resized` arrives.
    pub fn resize(&mut self, new_width: u32, new_height: u32) {
        if let Some(main_viewport) = self.main_viewport.as_mut() {
            // NOTE: this works only because we have one viewport, but eventually we'll need to use the id provided by the event to find the right viewport to resize.
            main_viewport.resize(&self.device, new_width, new_height);
        }
    }

    /// The renderer's primary presentation viewport.
    pub fn main_viewport(&self) -> Option<&RenderViewport> {
        self.main_viewport.as_ref()
    }

    /// Create a renderer-managed scene handle.
    ///
    /// Gameplay/world systems keep the returned proxy and enqueue proxy updates
    /// through it. The renderer later synchronizes the registry plus all
    /// pending scene updates at `dispatch_general_rendering`.
    pub fn create_scene_proxy(&self, scene_type: RenderSceneType) -> LeetResult<RenderSceneProxy> {
        self.scene_registry.create_scene_proxy(scene_type)
    }

    /// Queue removal of a renderer-managed scene handle.
    pub fn remove_scene_proxy(&self, scene_proxy: &RenderSceneProxy) -> LeetResult<()> {
        self.scene_registry.remove_scene_proxy(scene_proxy)
    }

    /// Acquire the next frame and build its shared render context.
    pub fn begin_frame(&mut self, command_list_count: usize) -> LeetResult<RenderContext<'_>> {
        let main_viewport = self
            .main_viewport
            .as_ref()
            .ok_or_else(|| Leeror::Runtime("renderer has no main viewport yet".to_string()))?;
        let frame_index = self.frame_index;
        self.frame_index += 1;

        RenderContext::new(
            &self.device,
            &self.queue,
            main_viewport,
            frame_index,
            command_list_count,
        )
    }

    fn generate_redner_data_for_scene_proxy(
        &self,
        _scene_proxy: &RenderSceneProxy,
        scene_gpu_state: &SceneGpuState,
    ) {
        let _live_instances = scene_gpu_state.live_instance_count();
        let _slot_capacity = scene_gpu_state.slot_capacity();
        let _has_instance_buffer = scene_gpu_state.instance_buffer().is_some();

        // generate a gpu driven representation of the scene proxy's data (render proxies, etc.) that can be consumed by the frame renderer
        //This will be done using GIB (GPU Instance Batcher), GIB will be responsible for packing the instance data depnding on the Mesh/Material of the render proxies, and generating the necessary buffers and bind groups for rendering.
    }

    fn update_scene_proxies(&mut self) -> LeetResult<()> {
        let scene_proxies = self.scene_registry.scene_proxies()?;
        let live_scene_ids: BTreeSet<_> =
            scene_proxies.iter().map(|scene| scene.scene_id()).collect();
        self.scene_gpu_states
            .retain(|scene_id, _| live_scene_ids.contains(scene_id));

        for scene_proxy in &scene_proxies {
            scene_proxy.apply_synced_updates()?;
            {
                let scene_gpu_state = self
                    .scene_gpu_states
                    .entry(scene_proxy.scene_id())
                    .or_default();
                scene_gpu_state.sync_scene(&self.device, &self.queue, scene_proxy)?;
            }

            let scene_gpu_state = self
                .scene_gpu_states
                .get(&scene_proxy.scene_id())
                .expect("scene GPU state should exist after sync");
            self.generate_redner_data_for_scene_proxy(scene_proxy, scene_gpu_state);
        }

        Ok(())
    }

    fn previous_frame_sync(&mut self) -> LeetResult<()> {
        // wait for the previous frame to finish and clean up any resources that are pending deletion
        //TODO:: Call the Wait function here

        self.scene_registry.execute_pending_updates()?;
        let scene_proxies = self.scene_registry.scene_proxies()?;
        for scene_proxy in &scene_proxies {
            scene_proxy.hand_off()?;
        }
        Ok(())
    }

    pub fn dispatch_general_rendering(&mut self) -> LeetResult<()> {
        self.previous_frame_sync()?;

        {
            //ANYTHING IN THIS SCOPE IS RUN ON THE RENDER THREAD
            self.update_scene_proxies()?;
        }

        Ok(())
    }
}
