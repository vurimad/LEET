//! Window surface creation, acquisition, and temporary smoke-test presentation.

use crate::{
    RenderAdapter, RenderDevice, RenderInstance, RenderQueue, RenderWindow, RenderWindowRegistry,
};
#[cfg(any(target_os = "macos", target_os = "ios"))]
use bevy_ecs::system::NonSendMarker;
use bevy_ecs::{
    entity::{EntityHashMap, EntityHashSet},
    prelude::{Res, ResMut, Resource},
};
use bevy_window::{CompositeAlphaMode, PresentMode};

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

pub fn window_surface_needs_configuration(
    windows: Res<RenderWindowRegistry>,
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
    windows: Res<RenderWindowRegistry>,
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
    mut windows: ResMut<RenderWindowRegistry>,
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
    mut windows: ResMut<RenderWindowRegistry>,
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

fn select_present_mode(
    window: &RenderWindow,
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
    mut windows: ResMut<RenderWindowRegistry>,
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
