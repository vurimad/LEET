mod camera_storage;
mod command_handler;
mod error;
mod frame_context;
mod frame_dispatcher;
mod frame_input;
mod frame_renderer;
mod frame_target_resolver;
mod render_viewport;

pub use camera_storage::{
    camera_render_setup_key, frame_camera_view_from_extracted, sync_render_camera_storage,
    CameraDependencyFlags, CameraManagement, CameraPrepareContext, CameraRenderPolicy,
    PreparedCameraDependency, PreparedCameraHistory, PreparedFrameCamera, RenderCameraStorage,
    MAX_CAMERA_DEPENDENCIES, MAX_CAMERA_DEPENDENCY_DEPTH,
};
pub use command_handler::{
    RenderCommand, RenderCommandHandler, RenderCommandQueueKind, RenderCommandSafety,
};
pub use error::{RenderFrameError, RenderFrameResult};
pub use frame_context::{RenderFrameContext, RenderJobBuilder};
pub use frame_dispatcher::{dispatch_general_rendering, FrameDispatcher};
pub use frame_input::{
    CameraRenderSetupKey, FrameCameraView, FrameCaptureIntent, FrameDebugIntent, FrameInput,
    FrameInputBuilder, FramePurpose, FrameRenderingMode, FrameTarget, FrameTargetKey, FrameTiming,
    PresentationIntent, RenderCameraId, RenderSceneId, ViewClearState,
};
pub use frame_renderer::{FrameRenderer, FrameRendererHandle};
pub use frame_target_resolver::FrameTargetResolver;
pub use render_viewport::RenderViewport;

use crate::{
    window::{ExtractedWindow, ExtractedWindows},
    Render, RenderApp, RenderSystems,
};
use bevy_app::{App, Plugin};
#[cfg(any(target_os = "macos", target_os = "ios"))]
use bevy_ecs::system::NonSendMarker;
use bevy_ecs::{
    entity::{EntityHashMap, EntityHashSet},
    prelude::{Res, ResMut, Resource, With, World},
    query::QueryState,
    schedule::IntoScheduleConfigs,
};
use bevy_tasks::block_on;
use bevy_window::{CompositeAlphaMode, PresentMode, PrimaryWindow, RawHandleWrapperHolder};
use std::{
    error::Error,
    fmt,
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct WgpuWrapper<T>(T);

impl<T> WgpuWrapper<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for WgpuWrapper<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for WgpuWrapper<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Resource, Clone)]
pub struct RenderQueue(pub Arc<WgpuWrapper<wgpu::Queue>>);

#[derive(Resource, Clone, Debug)]
pub struct RenderAdapter(pub Arc<WgpuWrapper<wgpu::Adapter>>);

#[derive(Resource, Clone)]
pub struct RenderInstance(pub Arc<WgpuWrapper<wgpu::Instance>>);

#[derive(Resource, Clone)]
pub struct RenderAdapterInfo(pub WgpuWrapper<wgpu::AdapterInfo>);

#[derive(Resource, Clone)]
pub struct RenderDevice(pub WgpuWrapper<wgpu::Device>);

#[derive(Resource, Clone)]
pub struct WgpuSettings {
    pub backends: wgpu::Backends,
    pub power_preference: wgpu::PowerPreference,
    pub force_fallback_adapter: bool,
    pub allow_headless_adapter_fallback: bool,
    pub required_features: wgpu::Features,
    pub required_limits: wgpu::Limits,
    pub memory_hints: wgpu::MemoryHints,
}

impl Default for WgpuSettings {
    fn default() -> Self {
        Self {
            backends: wgpu::Backends::PRIMARY,
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            allow_headless_adapter_fallback: true,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
        }
    }
}

#[derive(Debug)]
pub enum RendererInitializationError {
    PrimaryWindowHandleLockPoisoned,
    BootstrapSurfaceCreationFailed(String),
    AdapterRequestFailed {
        used_compatible_surface: bool,
        attempted_headless_fallback: bool,
        primary_error: String,
        fallback_error: Option<String>,
    },
    DeviceRequestFailed(String),
}

impl fmt::Display for RendererInitializationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrimaryWindowHandleLockPoisoned => {
                write!(
                    f,
                    "could not lock the primary window handle holder during renderer bootstrap"
                )
            }
            Self::BootstrapSurfaceCreationFailed(error) => {
                write!(f, "failed to create the bootstrap surface: {error}")
            }
            Self::AdapterRequestFailed {
                used_compatible_surface,
                attempted_headless_fallback,
                primary_error,
                fallback_error,
            } => {
                write!(
                    f,
                    "could not find a GPU adapter (compatible_surface={}, attempted_headless_fallback={}, primary_error={}",
                    used_compatible_surface, attempted_headless_fallback, primary_error
                )?;
                if let Some(fallback_error) = fallback_error {
                    write!(f, ", fallback_error={fallback_error}")?;
                }
                write!(f, ")")
            }
            Self::DeviceRequestFailed(error) => {
                write!(f, "could not create a GPU device: {error}")
            }
        }
    }
}

impl Error for RendererInitializationError {}

#[derive(Clone)]
struct RenderResources(
    RenderDevice,
    RenderQueue,
    RenderAdapterInfo,
    RenderAdapter,
    RenderInstance,
);

impl RenderResources {
    fn clone_into_main_world(&self, main_world: &mut World) {
        let RenderResources(device, queue, adapter_info, adapter, _) = self;
        main_world.insert_resource(device.clone());
        main_world.insert_resource(queue.clone());
        main_world.insert_resource(adapter_info.clone());
        main_world.insert_resource(adapter.clone());
    }

    fn move_into_render_world(self, render_world: &mut World) {
        let RenderResources(device, queue, adapter_info, adapter, instance) = self;
        render_world.insert_resource(instance);
        render_world.insert_resource(device);
        render_world.insert_resource(queue);
        render_world.insert_resource(adapter);
        render_world.insert_resource(adapter_info);
    }
}

struct SurfaceData {
    surface: wgpu::Surface<'static>,
    configuration: wgpu::SurfaceConfiguration,
    texture_view_format: Option<wgpu::TextureFormat>,
}

#[derive(Default, Resource)]
pub struct WindowSurfaces {
    surfaces: EntityHashMap<SurfaceData>,
    configured_windows: EntityHashSet,
}

impl WindowSurfaces {
    fn remove(&mut self, window: &bevy_ecs::entity::Entity) {
        self.surfaces.remove(window);
        self.configured_windows.remove(window);
    }
}

pub struct RendererPlugin;

impl Plugin for RendererPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WgpuSettings>();
        let primary_window = get_primary_window_handle_holder(app.world_mut());
        let settings = app.world().resource::<WgpuSettings>().clone();
        let render_resources = block_on(initialize_renderer(settings, primary_window))
            .unwrap_or_else(|error| panic!("LEET renderer initialization failed: {error}"));

        {
            render_resources.clone_into_main_world(app.world_mut());

            let render_app = app
                .get_sub_app_mut(RenderApp)
                .expect("LEET render sub-app missing");
            render_resources.move_into_render_world(render_app.world_mut());
            render_app
                .init_resource::<WindowSurfaces>()
                .add_systems(
                    Render,
                    cleanup_stale_surfaces
                        .in_set(RenderSystems::Prepare)
                        .before(create_surfaces),
                )
                .add_systems(
                    Render,
                    create_surfaces
                        .run_if(need_surface_configuration)
                        .in_set(RenderSystems::Prepare)
                        .before(prepare_windows),
                )
                .add_systems(Render, prepare_windows.in_set(RenderSystems::Prepare))
                .add_systems(
                    Render,
                    smoke_test_render_windows.in_set(RenderSystems::Render),
                );
        }
    }
}

fn get_primary_window_handle_holder(world: &mut World) -> Option<RawHandleWrapperHolder> {
    // This is an explicit main-world query over the tiny window set to mirror Bevy's
    // renderer bootstrap path, which looks up the primary window handle holder once
    // before initializing the render backend.
    let mut query = QueryState::<&RawHandleWrapperHolder, With<PrimaryWindow>>::new(world);
    query.single(world).ok().cloned()
}

async fn initialize_renderer(
    settings: WgpuSettings,
    primary_window: Option<RawHandleWrapperHolder>,
) -> Result<RenderResources, RendererInitializationError> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: settings.backends,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });

    let surface = if let Some(wrapper) = primary_window {
        let maybe_handle = wrapper
            .0
            .lock()
            .map_err(|_| RendererInitializationError::PrimaryWindowHandleLockPoisoned)?;
        if let Some(wrapper) = maybe_handle.as_ref() {
            let handle = unsafe { wrapper.get_handle() };
            Some(instance.create_surface(handle).map_err(|error| {
                RendererInitializationError::BootstrapSurfaceCreationFailed(error.to_string())
            })?)
        } else {
            None
        }
    } else {
        None
    };

    let used_compatible_surface = surface.is_some();
    let request_options = wgpu::RequestAdapterOptions {
        power_preference: settings.power_preference,
        compatible_surface: surface.as_ref(),
        force_fallback_adapter: settings.force_fallback_adapter,
    };
    let adapter = match instance.request_adapter(&request_options).await {
        Ok(adapter) => adapter,
        Err(primary_error) => {
            if used_compatible_surface && settings.allow_headless_adapter_fallback {
                let fallback_options = wgpu::RequestAdapterOptions {
                    power_preference: settings.power_preference,
                    compatible_surface: None,
                    force_fallback_adapter: settings.force_fallback_adapter,
                };
                match instance.request_adapter(&fallback_options).await {
                    Ok(adapter) => adapter,
                    Err(fallback_error) => {
                        return Err(RendererInitializationError::AdapterRequestFailed {
                            used_compatible_surface,
                            attempted_headless_fallback: true,
                            primary_error: primary_error.to_string(),
                            fallback_error: Some(fallback_error.to_string()),
                        });
                    }
                }
            } else {
                return Err(RendererInitializationError::AdapterRequestFailed {
                    used_compatible_surface,
                    attempted_headless_fallback: false,
                    primary_error: primary_error.to_string(),
                    fallback_error: None,
                });
            }
        }
    };

    let adapter_info = adapter.get_info();

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("LEET Render Device"),
            required_features: settings.required_features,
            required_limits: settings.required_limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: settings.memory_hints,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|error| RendererInitializationError::DeviceRequestFailed(error.to_string()))?;

    Ok(RenderResources(
        RenderDevice(WgpuWrapper::new(device)),
        RenderQueue(Arc::new(WgpuWrapper::new(queue))),
        RenderAdapterInfo(WgpuWrapper::new(adapter_info)),
        RenderAdapter(Arc::new(WgpuWrapper::new(adapter))),
        RenderInstance(Arc::new(WgpuWrapper::new(instance))),
    ))
}

fn need_surface_configuration(
    windows: Res<ExtractedWindows>,
    window_surfaces: Res<WindowSurfaces>,
) -> bool {
    // This explicitly scans the small window set each frame to decide if any surface
    // needs creation or reconfiguration.
    for window in windows.windows.values() {
        if !window_surfaces.configured_windows.contains(&window.entity)
            || window.handle_changed
            || window.size_changed
            || window.present_mode_changed
            || window.needs_surface_reconfigure
            || window.needs_surface_rebuild
        {
            return true;
        }
    }

    false
}

pub fn cleanup_stale_surfaces(
    windows: Res<ExtractedWindows>,
    mut window_surfaces: ResMut<WindowSurfaces>,
) {
    let live_windows: EntityHashSet = windows.windows.keys().copied().collect();
    let stale_windows: Vec<_> = window_surfaces
        .surfaces
        .keys()
        .copied()
        .filter(|entity| !live_windows.contains(entity))
        .collect();
    for stale_window in stale_windows {
        window_surfaces.remove(&stale_window);
    }
    window_surfaces
        .configured_windows
        .retain(|entity| live_windows.contains(entity));
}

pub fn create_surfaces(
    #[cfg(any(target_os = "macos", target_os = "ios"))] _marker: NonSendMarker,
    mut windows: ResMut<ExtractedWindows>,
    mut window_surfaces: ResMut<WindowSurfaces>,
    render_instance: Res<RenderInstance>,
    render_adapter: Res<RenderAdapter>,
    render_device: Res<RenderDevice>,
) {
    // This explicitly iterates all extracted windows and performs an EntityHashMap
    // lookup/insert per window. That cost is intentional here because window management
    // is bootstrap/presentation code, not the per-draw hot path.
    for window in windows.windows.values_mut() {
        if window.handle_changed || window.needs_surface_rebuild {
            window.clear_swapchain_texture();
            window_surfaces.remove(&window.entity);
        }

        let data = window_surfaces
            .surfaces
            .entry(window.entity)
            .or_insert_with(|| {
                let surface_target = wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(window.handle.get_display_handle()),
                    raw_window_handle: window.handle.get_window_handle(),
                };

                let surface = unsafe {
                    render_instance
                        .0
                        .create_surface_unsafe(surface_target)
                        .expect("LEET renderer failed to create a surface")
                };

                let caps = surface.get_capabilities(&render_adapter.0);
                let present_mode = select_present_mode(window, &caps);

                let mut format = *caps
                    .formats
                    .first()
                    .expect("LEET renderer surface reported no supported formats");
                for available_format in &caps.formats {
                    if *available_format == wgpu::TextureFormat::Rgba8UnormSrgb
                        || *available_format == wgpu::TextureFormat::Bgra8UnormSrgb
                    {
                        format = *available_format;
                        break;
                    }
                }

                let texture_view_format = if !format.is_srgb() {
                    Some(format.add_srgb_suffix())
                } else {
                    None
                };

                let configuration = wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format,
                    width: window.physical_width,
                    height: window.physical_height,
                    present_mode,
                    desired_maximum_frame_latency: window
                        .desired_maximum_frame_latency
                        .map(|latency| latency.get())
                        .unwrap_or(2),
                    alpha_mode: match window.alpha_mode {
                        CompositeAlphaMode::Auto => wgpu::CompositeAlphaMode::Auto,
                        CompositeAlphaMode::Opaque => wgpu::CompositeAlphaMode::Opaque,
                        CompositeAlphaMode::PreMultiplied => {
                            wgpu::CompositeAlphaMode::PreMultiplied
                        }
                        CompositeAlphaMode::PostMultiplied => {
                            wgpu::CompositeAlphaMode::PostMultiplied
                        }
                        CompositeAlphaMode::Inherit => wgpu::CompositeAlphaMode::Inherit,
                    },
                    view_formats: match texture_view_format {
                        Some(format) => vec![format],
                        None => vec![],
                    },
                };

                surface.configure(&render_device.0, &configuration);

                SurfaceData {
                    surface,
                    configuration,
                    texture_view_format,
                }
            });

        if window.size_changed || window.present_mode_changed || window.needs_surface_reconfigure {
            window.clear_swapchain_texture();

            data.configuration.width = window.physical_width;
            data.configuration.height = window.physical_height;
            let caps = data.surface.get_capabilities(&render_adapter.0);
            data.configuration.present_mode = select_present_mode(window, &caps);
            data.surface
                .configure(&render_device.0, &data.configuration);
        }

        window.swap_chain_texture_format = Some(data.configuration.format);
        window.swap_chain_texture_view_format = data.texture_view_format;
        window.handle_changed = false;
        window.needs_surface_rebuild = false;
        window.needs_surface_reconfigure = false;
        window_surfaces.configured_windows.insert(window.entity);
    }
}

pub fn prepare_windows(
    mut windows: ResMut<ExtractedWindows>,
    window_surfaces: Res<WindowSurfaces>,
    render_device: Res<RenderDevice>,
) {
    // This explicitly iterates all extracted windows and does one surface-table lookup
    // per window to acquire the current frame.
    for window in windows.windows.values_mut() {
        let Some(surface_data) = window_surfaces.surfaces.get(&window.entity) else {
            continue;
        };

        if window.has_swapchain_texture() && !window.size_changed && !window.present_mode_changed {
            continue;
        }

        match surface_data.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(surface_texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(surface_texture) => {
                window.set_swapchain_texture(surface_texture);
                window.swap_chain_texture_format = Some(surface_data.configuration.format);
                window.swap_chain_texture_view_format = surface_data.texture_view_format;
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                window.clear_swapchain_texture();
                surface_data
                    .surface
                    .configure(&render_device.0, &surface_data.configuration);
                match surface_data.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(surface_texture)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(surface_texture) => {
                        window.set_swapchain_texture(surface_texture);
                        window.swap_chain_texture_format = Some(surface_data.configuration.format);
                        window.swap_chain_texture_view_format = surface_data.texture_view_format;
                    }
                    _ => {
                        window.needs_surface_reconfigure = true;
                        continue;
                    }
                }
            }
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Validation => {
                window.clear_swapchain_texture();
                window.needs_surface_rebuild = true;
                continue;
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                window.clear_swapchain_texture();
                continue;
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                window.clear_swapchain_texture();
                continue;
            }
        }
    }
}

/// Temporary smoke-test render path.
///
/// This pass exists only to prove that LEET's render sub-app can:
/// - bootstrap wgpu
/// - extract Bevy-managed windows
/// - create/configure surfaces
/// - acquire a frame
/// - submit commands
/// - present to the window
///
/// It is intentionally *not* the beginning of LEET's real renderer architecture.
/// Future rendering work should replace this pass with the real render graph / frame
/// execution path rather than extending this clear-only implementation.
pub fn smoke_test_render_windows(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut windows: ResMut<ExtractedWindows>,
) {
    let mut rendered_any = false;
    let mut encoder = render_device
        .0
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("LEET Render Encoder"),
        });

    // This explicitly iterates every prepared window each frame and records one clear
    // pass per swapchain view for the smoke-test path only.
    for window in windows.windows.values() {
        let Some(view) = window.swap_chain_texture_view.as_ref() else {
            continue;
        };

        rendered_any = true;

        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("LEET Clear Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 1.00,
                        g: 0.07,
                        b: 0.10,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    if rendered_any {
        render_queue.0.submit([encoder.finish()]);
    }

    // Presentation is an explicit second pass over the window set so that all command recording
    // has finished before surface textures are presented.
    for window in windows.windows.values_mut() {
        if window.has_swapchain_texture() || window.needs_initial_present {
            window.present();
            window.needs_initial_present = false;
        }
    }
}

fn select_present_mode(
    window: &ExtractedWindow,
    caps: &wgpu::SurfaceCapabilities,
) -> wgpu::PresentMode {
    let requested = match window.present_mode {
        PresentMode::Fifo => wgpu::PresentMode::Fifo,
        PresentMode::FifoRelaxed => wgpu::PresentMode::FifoRelaxed,
        PresentMode::Mailbox => wgpu::PresentMode::Mailbox,
        PresentMode::Immediate => wgpu::PresentMode::Immediate,
        PresentMode::AutoVsync => wgpu::PresentMode::AutoVsync,
        PresentMode::AutoNoVsync => wgpu::PresentMode::AutoNoVsync,
    };

    let fallbacks: &[wgpu::PresentMode] = match requested {
        wgpu::PresentMode::AutoVsync => &[wgpu::PresentMode::FifoRelaxed, wgpu::PresentMode::Fifo],
        wgpu::PresentMode::AutoNoVsync => &[
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Fifo,
        ],
        wgpu::PresentMode::Mailbox => &[
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Fifo,
        ],
        other => &[other, wgpu::PresentMode::Fifo],
    };

    *fallbacks
        .iter()
        .find(|candidate| caps.present_modes.contains(candidate))
        .expect("LEET renderer could not choose a present mode")
}
