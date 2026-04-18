//! Renderer-owned proxy definitions.
//!
//! A render proxy is the renderer-facing representation of an object that may
//! contribute to one or more passes. The goal is to keep this data plain and
//! stable, so it can be queued from gameplay threads and consumed later by the
//! renderer without reaching back into the live world.

use leet_math::{Mat4, Vec3};

/// Stable identifier for a render proxy inside a renderer-owned scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RenderProxyId(u64);

impl RenderProxyId {
    const SLOT_INDEX_MASK: u64 = u32::MAX as u64;

    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn slot_index(self) -> usize {
        (self.0 & Self::SLOT_INDEX_MASK) as usize
    }

    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

/// Placeholder classification used by the current collector and pass setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderProxyKind {
    Opaque,
    Sky,
}

impl Default for RenderProxyKind {
    fn default() -> Self {
        Self::Opaque
    }
}

/// Builder-style data used when spawning or replacing a proxy.
#[derive(Clone, Debug)]
pub struct RenderProxyDescriptor {
    pub name: String,
    pub kind: RenderProxyKind,
    pub visible: bool,
    pub mesh_handle: u64,
    pub material_handle: u64,
    pub casts_shadows: bool,
    pub local_to_world: Mat4,
    pub debug_color: wgpu::Color,
}

impl RenderProxyDescriptor {
    pub fn new(kind: RenderProxyKind) -> Self {
        Self {
            kind,
            ..Default::default()
        }
    }

    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_translation(mut self, translation: Vec3) -> Self {
        self.local_to_world = Mat4::from_translation(translation);
        self
    }

    pub fn with_local_to_world(mut self, local_to_world: Mat4) -> Self {
        self.local_to_world = local_to_world;
        self
    }

    pub fn with_debug_color(mut self, debug_color: wgpu::Color) -> Self {
        self.debug_color = debug_color;
        self
    }

    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn with_mesh_handle(mut self, mesh_handle: u64) -> Self {
        self.mesh_handle = mesh_handle;
        self
    }

    pub fn with_material_handle(mut self, material_handle: u64) -> Self {
        self.material_handle = material_handle;
        self
    }

    pub fn with_casts_shadows(mut self, casts_shadows: bool) -> Self {
        self.casts_shadows = casts_shadows;
        self
    }
}

impl Default for RenderProxyDescriptor {
    fn default() -> Self {
        Self {
            name: "RenderProxy".to_string(),
            kind: RenderProxyKind::Opaque,
            visible: true,
            mesh_handle: 0,
            material_handle: 0,
            casts_shadows: true,
            local_to_world: Mat4::IDENTITY,
            debug_color: wgpu::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
        }
    }
}

/// Plain renderer-owned proxy data.
#[derive(Clone, Debug)]
pub struct RenderProxy {
    id: RenderProxyId,
    name: String,
    kind: RenderProxyKind,
    visible: bool,
    mesh_handle: u64,
    material_handle: u64,
    casts_shadows: bool,
    local_to_world: Mat4,
    debug_color: wgpu::Color,
}

impl RenderProxy {
    pub fn from_descriptor(id: RenderProxyId, descriptor: RenderProxyDescriptor) -> Self {
        Self {
            id,
            name: descriptor.name,
            kind: descriptor.kind,
            visible: descriptor.visible,
            mesh_handle: descriptor.mesh_handle,
            material_handle: descriptor.material_handle,
            casts_shadows: descriptor.casts_shadows,
            local_to_world: descriptor.local_to_world,
            debug_color: descriptor.debug_color,
        }
    }

    pub fn id(&self) -> RenderProxyId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> RenderProxyKind {
        self.kind
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn mesh_handle(&self) -> u64 {
        self.mesh_handle
    }

    pub fn material_handle(&self) -> u64 {
        self.material_handle
    }

    pub fn casts_shadows(&self) -> bool {
        self.casts_shadows
    }

    pub fn translation(&self) -> Vec3 {
        self.local_to_world.w_axis.truncate()
    }

    pub fn local_to_world(&self) -> Mat4 {
        self.local_to_world
    }

    pub fn debug_color(&self) -> wgpu::Color {
        self.debug_color
    }

    pub(crate) fn set_local_to_world(&mut self, local_to_world: Mat4) {
        self.local_to_world = local_to_world;
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub(crate) fn set_mesh_renderer(
        &mut self,
        mesh_handle: u64,
        material_handle: u64,
        casts_shadows: bool,
        visible: bool,
    ) {
        self.mesh_handle = mesh_handle;
        self.material_handle = material_handle;
        self.casts_shadows = casts_shadows;
        self.visible = visible;
    }

    pub(crate) fn set_debug_color(&mut self, debug_color: wgpu::Color) {
        self.debug_color = debug_color;
    }
}
