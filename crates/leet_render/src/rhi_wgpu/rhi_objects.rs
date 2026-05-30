//! Core wgpu resources owned by the LEET render app.

use bevy_ecs::{prelude::Resource, world::World};
use std::{
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

#[derive(Clone)]
pub(crate) struct RenderResources(
    pub(crate) RenderDevice,
    pub(crate) RenderQueue,
    pub(crate) RenderAdapterInfo,
    pub(crate) RenderAdapter,
    pub(crate) RenderInstance,
);

impl RenderResources {
    pub(crate) fn clone_into_main_world(&self, main_world: &mut World) {
        let RenderResources(device, queue, adapter_info, adapter, _) = self;
        main_world.insert_resource(device.clone());
        main_world.insert_resource(queue.clone());
        main_world.insert_resource(adapter_info.clone());
        main_world.insert_resource(adapter.clone());
    }

    pub(crate) fn move_into_render_world(self, render_world: &mut World) {
        let RenderResources(device, queue, adapter_info, adapter, instance) = self;
        render_world.insert_resource(instance);
        render_world.insert_resource(device);
        render_world.insert_resource(queue);
        render_world.insert_resource(adapter);
        render_world.insert_resource(adapter_info);
    }
}
