//! Renderer backend bootstrap and plugin wiring.

use crate::{
    RenderAdapter, RenderAdapterInfo, RenderApp, RenderDevice, RenderInstance, RenderQueue,
    RenderResources, WgpuSettings, WgpuWrapper,
};
use bevy_app::{App, Plugin};
use bevy_ecs::{prelude::With, query::QueryState};
use bevy_tasks::block_on;
use bevy_window::{PrimaryWindow, RawHandleWrapperHolder};
use std::{error::Error, fmt, sync::Arc};

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

pub struct RHIPlugin;

impl Plugin for RHIPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WgpuSettings>();
        let primary_window = get_primary_window_handle_holder(app.world_mut());
        let settings = app.world().resource::<WgpuSettings>().clone();
        let render_resources = block_on(initialize_renderer(settings, primary_window))
            .unwrap_or_else(|error| panic!("LEET renderer initialization failed: {error}"));

        render_resources.clone_into_main_world(app.world_mut());

        let render_app = app
            .get_sub_app_mut(RenderApp)
            .expect("LEET render sub-app missing");
        render_resources.move_into_render_world(render_app.world_mut());
    }
}

fn get_primary_window_handle_holder(
    world: &mut bevy_ecs::world::World,
) -> Option<RawHandleWrapperHolder> {
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
