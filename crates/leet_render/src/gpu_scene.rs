use crate::{
    BufferUploader, BufferUsages, GpuOnlyBuffer, RenderApp, SparseBufferUpdateJobs,
    SparseBufferUpdatePipeline,
};
use bevy_app::{App, Plugin};
use bevy_asset::UntypedAssetId;
use bevy_ecs::prelude::Resource;
use bevy_math::Mat4;
use bevy_render::render_resource::ShaderType;
use bevy_render::renderer::{RenderDevice, RenderQueue};
use bytemuck::{Pod, Zeroable};
use std::{collections::BTreeMap, sync::Arc};

/// Stable identifier for a proxy stored in [`GpuScene`].
///
/// The lower 32 bits encode the dense slot index. The upper 32 bits encode a
/// generation so freed slots can be reused safely without stale IDs
/// accidentally mutating a new proxy in the same slot later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RenderProxyId(u64);

impl RenderProxyId {
    const GENERATION_SHIFT: u64 = 32;
    const SLOT_INDEX_MASK: u64 = u32::MAX as u64;

    pub const fn from_parts(slot_index: u32, generation: u32) -> Self {
        Self(((generation as u64) << Self::GENERATION_SHIFT) | slot_index as u64)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn slot_index(self) -> usize {
        (self.0 & Self::SLOT_INDEX_MASK) as usize
    }

    pub const fn generation(self) -> u32 {
        (self.0 >> Self::GENERATION_SHIFT) as u32
    }
}

/// High-level proxy classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RenderProxyKind {
    #[default]
    Opaque,
    Sky,
}

/// High-level render phase buckets.
///
/// This is the important architectural split we want from Bevy: the persistent
/// scene object table is not the same thing as the final submitted draw payload.
/// Final instance data is phase-specific.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum GpuScenePhase {
    #[default]
    Opaque,
    AlphaMask,
    Transparent,
    Shadow,
    Deferred,
    Prepass,
}

/// Geometry slice information for one prepared mesh asset allocation.
///
/// This is asset-level data, not per-instance data. Multiple proxy slots can
/// point at the same asset slice later when the prepared-mesh bridge exists.
///
/// This type is intentionally upload-safe:
/// - `#[repr(C)]`
/// - fixed-width integer fields only
/// - no `bool`, `Option`, or Rust enums with unstable layout
///
/// `flags` uses bit packing so `Self::ZERO` is a valid "no slice bound" state.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable, ShaderType)]
pub struct GpuMeshAssetSlice {
    /// First vertex in the prepared mesh allocation.
    pub first_vertex_index: u32,
    /// First index in the prepared mesh allocation.
    pub first_index_index: u32,
    /// Number of indices to draw when `is_valid()` is true.
    pub index_count: u32,
    /// Bitfield encoded by `FLAG_*` constants.
    pub flags: u32,
}

impl GpuMeshAssetSlice {
    const FLAG_VALID: u32 = 1 << 0;
    const FLAG_INDEXED: u32 = 1 << 1;

    pub const ZERO: Self = Self {
        first_vertex_index: 0,
        first_index_index: 0,
        index_count: 0,
        flags: 0,
    };

    pub const fn new(
        first_vertex_index: u32,
        first_index_index: u32,
        index_count: u32,
        indexed: bool,
    ) -> Self {
        Self {
            first_vertex_index,
            first_index_index,
            index_count,
            flags: Self::FLAG_VALID | if indexed { Self::FLAG_INDEXED } else { 0 },
        }
    }

    pub const fn is_valid(self) -> bool {
        (self.flags & Self::FLAG_VALID) != 0
    }

    pub const fn is_indexed(self) -> bool {
        (self.flags & Self::FLAG_INDEXED) != 0
    }
}

/// GPU-safe flattened asset identifier.
///
/// This intentionally mirrors the *shape* of Bevy's `MeshAssetIdFlat`:
/// a small mode tag plus raw words. We do not store Rust's runtime `TypeId`
/// here because the destination field already defines the semantic domain
/// (`mesh_asset_id` versus `material_asset_id`), and `TypeId` is not a stable
/// GPU-facing representation.
///
/// `mode == 0` is the explicit "no asset bound" sentinel, which keeps the type
/// zero-initializable and POD-safe.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable, ShaderType)]
pub struct GpuFlatAssetId {
    /// `MODE_NONE`, `MODE_INDEX`, or `MODE_UUID`.
    pub mode: u32,
    /// Raw payload words for the chosen mode.
    pub words: [u32; 4],
}

impl GpuFlatAssetId {
    pub const MODE_NONE: u32 = 0;
    pub const MODE_INDEX: u32 = 1;
    pub const MODE_UUID: u32 = 2;

    pub const NONE: Self = Self {
        mode: Self::MODE_NONE,
        words: [0; 4],
    };

    pub const fn is_bound(self) -> bool {
        self.mode != Self::MODE_NONE
    }
}

impl From<Option<UntypedAssetId>> for GpuFlatAssetId {
    fn from(value: Option<UntypedAssetId>) -> Self {
        match value {
            None => Self::NONE,
            Some(UntypedAssetId::Index { index, .. }) => {
                let bits = index.to_bits();
                Self {
                    mode: Self::MODE_INDEX,
                    words: [(bits & 0xffff_ffff) as u32, (bits >> 32) as u32, 0, 0],
                }
            }
            Some(UntypedAssetId::Uuid { uuid, .. }) => {
                let (hi, lo) = uuid.as_u64_pair();
                Self {
                    mode: Self::MODE_UUID,
                    words: [
                        (lo & 0xffff_ffff) as u32,
                        (lo >> 32) as u32,
                        (hi & 0xffff_ffff) as u32,
                        (hi >> 32) as u32,
                    ],
                }
            }
        }
    }
}

/// Builder-style payload used when allocating a new proxy in [`GpuScene`].
#[derive(Clone, Debug)]
pub struct RenderProxyDescriptor {
    pub kind: RenderProxyKind,
    pub visible: bool,
    pub casts_shadows: bool,
    pub mesh_asset_id: Option<UntypedAssetId>,
    pub material_asset_id: Option<UntypedAssetId>,
    pub tag: u32,
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

    pub fn with_local_to_world(mut self, local_to_world: Mat4) -> Self {
        self.local_to_world = local_to_world;
        self
    }

    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn with_casts_shadows(mut self, casts_shadows: bool) -> Self {
        self.casts_shadows = casts_shadows;
        self
    }

    pub fn with_mesh_asset_id(mut self, mesh_asset_id: UntypedAssetId) -> Self {
        self.mesh_asset_id = Some(mesh_asset_id);
        self
    }

    pub fn with_material_asset_id(mut self, material_asset_id: UntypedAssetId) -> Self {
        self.material_asset_id = Some(material_asset_id);
        self
    }

    pub fn with_tag(mut self, tag: u32) -> Self {
        self.tag = tag;
        self
    }

    pub fn with_debug_color(mut self, debug_color: wgpu::Color) -> Self {
        self.debug_color = debug_color;
        self
    }
}

impl Default for RenderProxyDescriptor {
    fn default() -> Self {
        Self {
            kind: RenderProxyKind::Opaque,
            visible: true,
            casts_shadows: true,
            mesh_asset_id: None,
            material_asset_id: None,
            tag: 0,
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

/// Plain CPU-side persistent scene object state.
///
/// This only keeps CPU-side metadata that is still useful outside the strict
/// GPU upload tables, such as asset identities for future prepare/bridge work.
/// Canonical per-instance scene state now lives in [`GpuInstanceInput`].
#[derive(Clone, Debug)]
pub struct RenderProxy {
    id: RenderProxyId,
    mesh_asset_id: Option<UntypedAssetId>,
    material_asset_id: Option<UntypedAssetId>,
}

impl RenderProxy {
    pub fn from_descriptor(id: RenderProxyId, descriptor: &RenderProxyDescriptor) -> Self {
        Self {
            id,
            mesh_asset_id: descriptor.mesh_asset_id,
            material_asset_id: descriptor.material_asset_id,
        }
    }

    pub fn id(&self) -> RenderProxyId {
        self.id
    }

    pub fn mesh_asset_id(&self) -> Option<UntypedAssetId> {
        self.mesh_asset_id
    }

    pub fn material_asset_id(&self) -> Option<UntypedAssetId> {
        self.material_asset_id
    }

    fn set_mesh_asset_id(&mut self, mesh_asset_id: Option<UntypedAssetId>) {
        self.mesh_asset_id = mesh_asset_id;
    }

    fn set_material_asset_id(&mut self, material_asset_id: Option<UntypedAssetId>) {
        self.material_asset_id = material_asset_id;
    }
}

/// Persistent per-instance input record.
///
/// This is the LEET-owned equivalent of Bevy's lighter `MeshInputUniform`
/// layer: one entry per live renderable instance in the scene, stable by slot,
/// before any phase/view-specific expansion happens.
///
/// This type is intentionally strict because it is expected to become a real
/// GPU upload contract:
/// - `#[repr(C)]`
/// - POD / zeroable via `bytemuck`
/// - no `Option`, `bool`, or Rust enum layout dependencies
/// - explicit sentinel encodings for "none"
///
/// Sensitive field encodings:
/// - `mesh_asset_id` / `material_asset_id`: `mode == 0` means "unbound"
/// - `mesh_asset_slice`: `flags & VALID == 0` means "no prepared slice yet"
/// - `previous_input_index_plus_one == 0` means "no previous input"
/// - `kind` is a small explicit integer, not a Rust enum payload
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, ShaderType)]
pub struct GpuInstanceInput {
    /// Object transform copied into a strict GPU upload layout.
    pub world_from_local: [[f32; 4]; 4],
    /// Debug-only instance tint kept as explicit `f32` lanes.
    pub debug_color: [f32; 4],
    /// Flattened mesh asset identifier. `mode == 0` means "no mesh".
    pub mesh_asset_id: GpuFlatAssetId,
    /// Flattened material asset identifier. `mode == 0` means "no material".
    pub material_asset_id: GpuFlatAssetId,
    /// Prepared mesh allocation slice. `is_valid() == false` means "not ready".
    pub mesh_asset_slice: GpuMeshAssetSlice,
    /// Encodes `Option<u32>` as `slot + 1`, with `0` reserved for `None`.
    pub previous_input_index_plus_one: u32,
    /// Explicit render-kind integer, kept separate from flags for shader clarity.
    pub kind: u32,
    /// Visibility / shadowing / future per-instance state bits.
    pub flags: u32,
    /// Stable small tag lane for renderer-side grouping/debugging.
    pub tag: u32,
    /// Reserved extension lane so future changes do not need a breaking repack.
    pub reserved: u32,
}

impl GpuInstanceInput {
    pub const FLAG_VISIBLE: u32 = 1 << 0;
    pub const FLAG_CASTS_SHADOWS: u32 = 1 << 1;
    pub const KIND_OPAQUE: u32 = 0;
    pub const KIND_SKY: u32 = 1;
    pub const ZERO: Self = Self {
        world_from_local: [[0.0; 4]; 4],
        debug_color: [0.0; 4],
        mesh_asset_id: GpuFlatAssetId::NONE,
        material_asset_id: GpuFlatAssetId::NONE,
        mesh_asset_slice: GpuMeshAssetSlice::ZERO,
        previous_input_index_plus_one: 0,
        kind: Self::KIND_OPAQUE,
        flags: 0,
        tag: 0,
        reserved: 0,
    };

    pub fn from_descriptor(descriptor: &RenderProxyDescriptor) -> Self {
        let debug_color = descriptor.debug_color;
        Self {
            world_from_local: descriptor.local_to_world.to_cols_array_2d(),
            debug_color: [
                debug_color.r as f32,
                debug_color.g as f32,
                debug_color.b as f32,
                debug_color.a as f32,
            ],
            mesh_asset_id: descriptor.mesh_asset_id.into(),
            material_asset_id: descriptor.material_asset_id.into(),
            mesh_asset_slice: GpuMeshAssetSlice::ZERO,
            previous_input_index_plus_one: 0,
            kind: match descriptor.kind {
                RenderProxyKind::Opaque => Self::KIND_OPAQUE,
                RenderProxyKind::Sky => Self::KIND_SKY,
            },
            flags: (if descriptor.visible {
                Self::FLAG_VISIBLE
            } else {
                0
            }) | (if descriptor.casts_shadows {
                Self::FLAG_CASTS_SHADOWS
            } else {
                0
            }),
            tag: descriptor.tag,
            reserved: 0,
        }
    }

    pub fn set_local_to_world(&mut self, local_to_world: Mat4) {
        self.world_from_local = local_to_world.to_cols_array_2d();
    }

    pub fn set_debug_color(&mut self, debug_color: wgpu::Color) {
        self.debug_color = [
            debug_color.r as f32,
            debug_color.g as f32,
            debug_color.b as f32,
            debug_color.a as f32,
        ];
    }

    pub fn set_mesh_asset_id(&mut self, mesh_asset_id: Option<UntypedAssetId>) {
        self.mesh_asset_id = mesh_asset_id.into();
    }

    pub fn set_material_asset_id(&mut self, material_asset_id: Option<UntypedAssetId>) {
        self.material_asset_id = material_asset_id.into();
    }

    pub const fn set_visibility(&mut self, visible: bool) {
        if visible {
            self.flags |= Self::FLAG_VISIBLE;
        } else {
            self.flags &= !Self::FLAG_VISIBLE;
        }
    }

    pub const fn set_casts_shadows(&mut self, casts_shadows: bool) {
        if casts_shadows {
            self.flags |= Self::FLAG_CASTS_SHADOWS;
        } else {
            self.flags &= !Self::FLAG_CASTS_SHADOWS;
        }
    }

    pub const fn set_tag(&mut self, tag: u32) {
        self.tag = tag;
    }

    pub const fn set_kind(&mut self, kind: RenderProxyKind) {
        self.kind = match kind {
            RenderProxyKind::Opaque => Self::KIND_OPAQUE,
            RenderProxyKind::Sky => Self::KIND_SKY,
        };
    }

    pub const fn is_visible(&self) -> bool {
        (self.flags & Self::FLAG_VISIBLE) != 0
    }

    pub const fn casts_shadows(&self) -> bool {
        (self.flags & Self::FLAG_CASTS_SHADOWS) != 0
    }

    pub const fn previous_input_index(&self) -> Option<u32> {
        if self.previous_input_index_plus_one == 0 {
            None
        } else {
            Some(self.previous_input_index_plus_one - 1)
        }
    }

    pub const fn kind(&self) -> RenderProxyKind {
        match self.kind {
            Self::KIND_SKY => RenderProxyKind::Sky,
            _ => RenderProxyKind::Opaque,
        }
    }
}

/// Final draw-facing per-instance payload.
///
/// We intentionally keep this LEET-owned instead of reusing Bevy's
/// `MeshUniform`, because we want the same layered model without inheriting the
/// exact PBR/material/skinning contract too early.
///
/// Unlike [`GpuInstanceInput`], this is intended to be the *result* of the
/// preprocessing step, not the persistent scene table itself.
///
/// Long term, this should be produced by GPU preprocessing from
/// [`GpuInstanceInput`] plus prepared mesh/material data. Until that compute
/// path exists, the renderer keeps a CPU emulation helper so the smoke path and
/// tests can exercise the final layout honestly.
///
/// This is another sensitive upload type:
/// - `#[repr(C)]`
/// - POD / zeroable via `bytemuck`
/// - only fixed-width scalars and arrays
/// - explicit named fields instead of opaque Rust packing
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, ShaderType)]
pub struct GpuInstance {
    /// Final transform consumed by drawing for this phase.
    local_to_world: [[f32; 4]; 4],
    /// Final debug tint / test color lane.
    debug_color: [f32; 4],
    /// Resolved geometry slice for draw calls.
    mesh_asset_slice: GpuMeshAssetSlice,
    /// Material indirection after preprocess.
    material_asset_id: GpuFlatAssetId,
    /// `slot + 1` link to previous-frame instance input, if any.
    previous_input_index_plus_one: u32,
    /// Explicit proxy kind integer.
    kind: u32,
    /// Visibility / shadow / future state bits.
    flags: u32,
    /// Stable grouping / debug tag.
    tag: u32,
}

impl GpuInstance {
    pub const ZERO: Self = Self {
        local_to_world: [[0.0; 4]; 4],
        debug_color: [0.0; 4],
        mesh_asset_slice: GpuMeshAssetSlice::ZERO,
        material_asset_id: GpuFlatAssetId::NONE,
        previous_input_index_plus_one: 0,
        kind: 0,
        flags: 0,
        tag: 0,
    };

    /// Temporary CPU mirror of the future GPU preprocessing stage.
    ///
    /// This keeps the final instance layout authoritative even before the real
    /// compute path exists.
    pub fn emulate_gpu_preprocess(input: &GpuInstanceInput) -> Self {
        Self {
            local_to_world: input.world_from_local,
            debug_color: input.debug_color,
            mesh_asset_slice: input.mesh_asset_slice,
            material_asset_id: input.material_asset_id,
            previous_input_index_plus_one: input.previous_input_index_plus_one,
            kind: input.kind,
            flags: input.flags,
            tag: input.tag,
        }
    }

    #[cfg(test)]
    fn visible(&self) -> bool {
        (self.flags & GpuInstanceInput::FLAG_VISIBLE) != 0
    }
}

/// Compact per-phase indirection entry.
///
/// LEET's preferred model is:
/// - one shared computed-instance buffer
/// - one compact index stream per phase
///
/// So this entry points from a phase-local submission list into the shared
/// computed-instance table.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, ShaderType)]
pub struct GpuInstanceIndex {
    pub computed_instance_index: u32,
}

impl GpuInstanceIndex {
    fn from_slot_index(slot_index: usize) -> Self {
        Self {
            computed_instance_index: slot_index as u32,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct GpuSceneSlot {
    proxy: Option<RenderProxy>,
}

struct GpuSceneComputedBufferState {
    instance_buffer: GpuOnlyBuffer<GpuInstance>,
}

impl Default for GpuSceneComputedBufferState {
    fn default() -> Self {
        Self {
            instance_buffer: GpuOnlyBuffer::new(BufferUsages::VERTEX | BufferUsages::STORAGE),
        }
    }
}

struct GpuScenePhaseBufferState {
    instance_index_buffer: GpuOnlyBuffer<GpuInstanceIndex>,
}

impl Default for GpuScenePhaseBufferState {
    fn default() -> Self {
        Self {
            instance_index_buffer: GpuOnlyBuffer::new(BufferUsages::STORAGE),
        }
    }
}

#[derive(Debug, Default)]
struct GpuSceneSlotAllocator {
    next_slot_index: u32,
    slot_generations: Vec<u32>,
    allocated_slots: Vec<bool>,
    free_slots: Vec<u32>,
}

impl GpuSceneSlotAllocator {
    fn allocate(&mut self) -> RenderProxyId {
        if let Some(slot_index) = self.free_slots.pop() {
            let slot_index = slot_index as usize;
            self.allocated_slots[slot_index] = true;
            return RenderProxyId::from_parts(slot_index as u32, self.slot_generations[slot_index]);
        }

        let slot_index = self.next_slot_index;
        self.next_slot_index = self
            .next_slot_index
            .checked_add(1)
            .expect("LEET GPU scene slot index overflowed u32");
        self.slot_generations.push(0);
        self.allocated_slots.push(true);
        RenderProxyId::from_parts(slot_index, 0)
    }

    fn release(&mut self, proxy_id: RenderProxyId) -> bool {
        let slot_index = proxy_id.slot_index();
        let Some(current_generation) = self.slot_generations.get_mut(slot_index) else {
            return false;
        };
        let Some(is_allocated) = self.allocated_slots.get_mut(slot_index) else {
            return false;
        };

        if !*is_allocated || *current_generation != proxy_id.generation() {
            return false;
        }

        *is_allocated = false;
        *current_generation = current_generation.wrapping_add(1);
        self.free_slots.push(slot_index as u32);
        true
    }
}

/// Dense renderer-owned scene contract.
///
/// This now explicitly has the same major layers we want from Bevy's advanced
/// path:
/// - persistent proxy slots
/// - persistent current/previous instance-input tables
/// - a shared GPU-only computed-instance buffer target
/// - per-phase submission buffers
///
/// We are not wiring extraction into it yet; this exists so the extraction
/// layer has a concrete target.
#[derive(Resource)]
pub struct GpuScene {
    proxies: Vec<GpuSceneSlot>,
    live_proxy_count: usize,
    current_inputs: BufferUploader<GpuInstanceInput>,
    previous_inputs: BufferUploader<GpuInstanceInput>,
    computed_buffer: GpuSceneComputedBufferState,
    phase_buffers: BTreeMap<GpuScenePhase, GpuScenePhaseBufferState>,
    slot_allocator: GpuSceneSlotAllocator,
}

impl Default for GpuScene {
    fn default() -> Self {
        Self {
            proxies: Vec::new(),
            live_proxy_count: 0,
            current_inputs: BufferUploader::new(
                BufferUsages::STORAGE,
                6,
                Arc::<str>::from("leet gpu scene current inputs"),
            ),
            previous_inputs: BufferUploader::new(
                BufferUsages::STORAGE,
                6,
                Arc::<str>::from("leet gpu scene previous inputs"),
            ),
            computed_buffer: GpuSceneComputedBufferState::default(),
            phase_buffers: BTreeMap::new(),
            slot_allocator: GpuSceneSlotAllocator::default(),
        }
    }
}

impl GpuScene {
    pub fn allocate_proxy(&mut self, descriptor: RenderProxyDescriptor) -> RenderProxyId {
        let proxy_id = self.slot_allocator.allocate();
        let input = GpuInstanceInput::from_descriptor(&descriptor);
        let proxy = RenderProxy::from_descriptor(proxy_id, &descriptor);
        let inserted = self.upsert_proxy(proxy);
        debug_assert!(
            inserted,
            "freshly allocated GPU scene proxy should insert cleanly"
        );
        self.ensure_slot(proxy_id.slot_index());
        self.current_inputs.set(proxy_id.slot_index() as u32, input);
        proxy_id
    }

    pub fn remove_proxy(&mut self, proxy_id: RenderProxyId) -> bool {
        if self.remove_proxy_from_state(proxy_id) {
            self.slot_allocator.release(proxy_id);
            self.clear_current_input_slot(proxy_id.slot_index());
            true
        } else {
            false
        }
    }

    pub fn contains_proxy(&self, proxy_id: RenderProxyId) -> bool {
        self.proxy(proxy_id).is_some()
    }

    pub fn proxy(&self, proxy_id: RenderProxyId) -> Option<&RenderProxy> {
        let slot = self.proxies.get(proxy_id.slot_index())?;
        let proxy = slot.proxy.as_ref()?;
        (proxy.id() == proxy_id).then_some(proxy)
    }

    pub fn live_proxy_count(&self) -> usize {
        self.live_proxy_count
    }

    pub fn slot_capacity(&self) -> usize {
        self.proxies.len()
    }

    pub fn current_inputs(&self) -> &BufferUploader<GpuInstanceInput> {
        &self.current_inputs
    }

    pub fn previous_inputs(&self) -> &BufferUploader<GpuInstanceInput> {
        &self.previous_inputs
    }

    pub fn instance_buffer(&self) -> Option<&wgpu::Buffer> {
        self.computed_instance_buffer()
    }

    pub fn computed_instance_buffer(&self) -> Option<&wgpu::Buffer> {
        self.computed_buffer
            .instance_buffer
            .buffer()
            .map(|buffer| &**buffer)
    }

    /// Compatibility helper while the renderer migrates from per-phase instance
    /// payloads to one shared computed-instance table.
    pub fn phase_instance_buffer(&self, _phase: GpuScenePhase) -> Option<&wgpu::Buffer> {
        self.computed_instance_buffer()
    }

    pub fn phase_instance_index_buffer(&self, phase: GpuScenePhase) -> Option<&wgpu::Buffer> {
        self.phase_buffers
            .get(&phase)
            .and_then(|phase_buffer| phase_buffer.instance_index_buffer.buffer())
            .map(|buffer| &**buffer)
    }

    pub fn snapshot_previous_inputs(&mut self) {
        let current_len = self.current_inputs.len();
        if self.previous_inputs.len() < current_len {
            self.previous_inputs.grow(current_len);
        } else if self.previous_inputs.len() > current_len {
            self.previous_inputs.truncate(current_len);
        }

        for slot_index in 0..current_len as usize {
            let mut input = self.current_inputs.get(slot_index as u32);
            if self
                .proxies
                .get(slot_index)
                .and_then(|slot| slot.proxy.as_ref())
                .is_some()
            {
                input.previous_input_index_plus_one = slot_index as u32 + 1;
            } else {
                input = GpuInstanceInput::ZERO;
            }
            self.previous_inputs.set(slot_index as u32, input);
        }
    }

    pub fn set_transform(&mut self, proxy_id: RenderProxyId, local_to_world: Mat4) -> bool {
        self.update_current_input_slot(proxy_id, |input| input.set_local_to_world(local_to_world))
    }

    pub fn set_mesh_asset_id(
        &mut self,
        proxy_id: RenderProxyId,
        mesh_asset_id: Option<UntypedAssetId>,
    ) -> bool {
        let mut updated = false;
        if let Some(proxy) = self.proxy_mut(proxy_id) {
            proxy.set_mesh_asset_id(mesh_asset_id);
            updated = true;
        }
        if updated {
            self.update_current_input_slot(proxy_id, |input| {
                input.set_mesh_asset_id(mesh_asset_id)
            });
        }
        updated
    }

    pub fn set_material_asset_id(
        &mut self,
        proxy_id: RenderProxyId,
        material_asset_id: Option<UntypedAssetId>,
    ) -> bool {
        let mut updated = false;
        if let Some(proxy) = self.proxy_mut(proxy_id) {
            proxy.set_material_asset_id(material_asset_id);
            updated = true;
        }
        if updated {
            self.update_current_input_slot(proxy_id, |input| {
                input.set_material_asset_id(material_asset_id)
            });
        }
        updated
    }

    pub fn set_visibility(&mut self, proxy_id: RenderProxyId, visible: bool) -> bool {
        self.update_current_input_slot(proxy_id, |input| input.set_visibility(visible))
    }

    pub fn set_casts_shadows(&mut self, proxy_id: RenderProxyId, casts_shadows: bool) -> bool {
        self.update_current_input_slot(proxy_id, |input| input.set_casts_shadows(casts_shadows))
    }

    pub fn set_tag(&mut self, proxy_id: RenderProxyId, tag: u32) -> bool {
        self.update_current_input_slot(proxy_id, |input| input.set_tag(tag))
    }

    pub fn set_kind(&mut self, proxy_id: RenderProxyId, kind: RenderProxyKind) -> bool {
        self.update_current_input_slot(proxy_id, |input| input.set_kind(kind))
    }

    pub fn set_debug_color(&mut self, proxy_id: RenderProxyId, debug_color: wgpu::Color) -> bool {
        self.update_current_input_slot(proxy_id, |input| input.set_debug_color(debug_color))
    }

    fn upsert_proxy(&mut self, proxy: RenderProxy) -> bool {
        let slot_index = proxy.id().slot_index();
        self.ensure_slot(slot_index);

        let slot = &mut self.proxies[slot_index];
        match slot.proxy.as_ref() {
            Some(existing_proxy) if existing_proxy.id() != proxy.id() => {
                return false;
            }
            Some(_) => {}
            None => {
                self.live_proxy_count += 1;
            }
        }

        slot.proxy = Some(proxy);
        true
    }

    fn remove_proxy_from_state(&mut self, proxy_id: RenderProxyId) -> bool {
        let Some(slot) = self.proxies.get_mut(proxy_id.slot_index()) else {
            return false;
        };

        if slot
            .proxy
            .as_ref()
            .is_some_and(|proxy| proxy.id() == proxy_id)
        {
            slot.proxy = None;
            self.live_proxy_count -= 1;
            return true;
        }

        false
    }

    fn proxy_mut(&mut self, proxy_id: RenderProxyId) -> Option<&mut RenderProxy> {
        let slot = self.proxies.get_mut(proxy_id.slot_index())?;
        let proxy = slot.proxy.as_mut()?;
        (proxy.id() == proxy_id).then_some(proxy)
    }

    fn update_current_input_slot(
        &mut self,
        proxy_id: RenderProxyId,
        update: impl FnOnce(&mut GpuInstanceInput),
    ) -> bool {
        let slot_index = proxy_id.slot_index();
        let Some(slot) = self.proxies.get(slot_index) else {
            return false;
        };
        if slot
            .proxy
            .as_ref()
            .is_none_or(|proxy| proxy.id() != proxy_id)
        {
            return false;
        }

        let mut input = self.current_inputs.get(slot_index as u32);
        update(&mut input);
        self.current_inputs.set(slot_index as u32, input);
        true
    }

    fn ensure_slot(&mut self, slot_index: usize) {
        if slot_index >= self.proxies.len() {
            self.proxies
                .resize_with(slot_index + 1, GpuSceneSlot::default);
        }
        if slot_index as u32 >= self.current_inputs.len() {
            self.current_inputs.grow(slot_index as u32 + 1);
        }
        if slot_index as u32 >= self.previous_inputs.len() {
            self.previous_inputs.grow(slot_index as u32 + 1);
        }
    }

    fn clear_current_input_slot(&mut self, slot_index: usize) {
        self.ensure_slot(slot_index);
        self.current_inputs
            .set(slot_index as u32, GpuInstanceInput::ZERO);
    }

    pub(crate) fn write_and_prepare_input_buffers(
        &mut self,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
        sparse_buffer_update_jobs: &mut SparseBufferUpdateJobs,
        sparse_buffer_update_pipeline: &SparseBufferUpdatePipeline,
    ) {
        self.current_inputs.write_and_prepare_buffers(
            render_device,
            render_queue,
            sparse_buffer_update_jobs,
            sparse_buffer_update_pipeline,
        );
        self.previous_inputs.write_and_prepare_buffers(
            render_device,
            render_queue,
            sparse_buffer_update_jobs,
            sparse_buffer_update_pipeline,
        );
    }
}

/// Temporary CPU-side emulation of the future GPU preprocess/submission path.
///
/// This is intentionally a dumb fake-data pumper for bring-up and tests only.
/// It should eventually disappear as RenderGraph-driven GPU passes take over
/// computed instance generation and phase submission building.
pub struct GpuSceneFakeGpuEmulation;

impl GpuSceneFakeGpuEmulation {
    pub fn simulate_computed_instances(scene: &GpuScene) -> Vec<GpuInstance> {
        (0..scene.current_inputs.len())
            .map(|slot_index| {
                let input = scene.current_inputs.get(slot_index);
                GpuInstance::emulate_gpu_preprocess(&input)
            })
            .collect()
    }

    pub fn simulate_phase_instance_indices(
        scene: &GpuScene,
        phase: GpuScenePhase,
    ) -> Vec<GpuInstanceIndex> {
        Self::build_phase_instance_indices(phase, &scene.current_inputs)
    }

    fn build_phase_instance_indices(
        phase: GpuScenePhase,
        current_inputs: &BufferUploader<GpuInstanceInput>,
    ) -> Vec<GpuInstanceIndex> {
        let mut phase_indices = Vec::new();
        for slot_index in 0..current_inputs.len() as usize {
            let input = current_inputs.get(slot_index as u32);
            if Self::proxy_participates_in_phase(phase, input) {
                phase_indices.push(GpuInstanceIndex::from_slot_index(slot_index));
            }
        }
        phase_indices
    }

    fn proxy_participates_in_phase(phase: GpuScenePhase, input: GpuInstanceInput) -> bool {
        match phase {
            GpuScenePhase::Opaque | GpuScenePhase::Deferred | GpuScenePhase::Prepass => {
                input.is_visible() && input.kind() == RenderProxyKind::Opaque
            }
            GpuScenePhase::Shadow => input.is_visible() && input.casts_shadows(),
            GpuScenePhase::AlphaMask | GpuScenePhase::Transparent => false,
        }
    }
}

/// Installs the renderer-owned GPU scene resource into the render app.
#[derive(Default)]
pub struct GpuScenePlugin;

impl Plugin for GpuScenePlugin {
    fn build(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.init_resource::<GpuScene>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_reuse_bumps_generation() {
        let mut scene = GpuScene::default();
        let first_id = scene.allocate_proxy(RenderProxyDescriptor::default());

        assert!(scene.remove_proxy(first_id));

        let second_id = scene.allocate_proxy(RenderProxyDescriptor::default());
        assert_eq!(first_id.slot_index(), second_id.slot_index());
        assert_ne!(first_id.generation(), second_id.generation());
    }

    #[test]
    fn multiple_updates_preserve_live_proxy_count() {
        let mut scene = GpuScene::default();
        let proxy_id = scene.allocate_proxy(RenderProxyDescriptor::default());

        scene.set_visibility(proxy_id, false);
        scene.set_casts_shadows(proxy_id, false);
        scene.set_kind(proxy_id, RenderProxyKind::Sky);

        assert_eq!(scene.slot_capacity(), 1);
        assert_eq!(scene.live_proxy_count(), 1);
    }

    #[test]
    fn smoke_simulation_zeros_removed_slots() {
        let mut scene = GpuScene::default();
        let proxy_id = scene.allocate_proxy(RenderProxyDescriptor::default());

        let simulated = GpuSceneFakeGpuEmulation::simulate_computed_instances(&scene);
        assert!(simulated[0].visible());

        assert!(scene.remove_proxy(proxy_id));

        let simulated = GpuSceneFakeGpuEmulation::simulate_computed_instances(&scene);
        assert!(!simulated[0].visible());
    }

    #[test]
    fn snapshot_previous_inputs_preserves_slot_indices() {
        let mut scene = GpuScene::default();
        let proxy_id = scene.allocate_proxy(
            RenderProxyDescriptor::default()
                .with_tag(17)
                .with_visible(false),
        );

        scene.snapshot_previous_inputs();

        let previous_input = scene.previous_inputs().get(proxy_id.slot_index() as u32);
        assert_eq!(
            previous_input.previous_input_index(),
            Some(proxy_id.slot_index() as u32)
        );
        assert_eq!(previous_input.tag, 17);
        assert!(!previous_input.is_visible());
    }

    #[test]
    fn mesh_input_uses_explicit_gpu_safe_sentinels() {
        let input = GpuInstanceInput::default();

        assert_eq!(input.previous_input_index(), None);
        assert!(!input.mesh_asset_id.is_bound());
        assert!(!input.material_asset_id.is_bound());
        assert!(!input.mesh_asset_slice.is_valid());
    }

    #[test]
    fn flattened_asset_id_packs_index_and_uuid_modes() {
        use bevy_asset::{AssetId, AssetIndex};
        use bevy_image::Image;

        let index_bits = 0x1234_5678_9abc_def0_u64;
        let indexed_flat = GpuFlatAssetId::from(Some(
            AssetId::<Image>::from(AssetIndex::from_bits(index_bits)).untyped(),
        ));
        assert_eq!(indexed_flat.mode, GpuFlatAssetId::MODE_INDEX);
        assert_eq!(indexed_flat.words[0], (index_bits & 0xffff_ffff) as u32);
        assert_eq!(indexed_flat.words[1], (index_bits >> 32) as u32);

        let uuid = AssetId::<Image>::DEFAULT_UUID;
        let uuid_flat = GpuFlatAssetId::from(Some(AssetId::<Image>::Uuid { uuid }.untyped()));
        let (hi, lo) = uuid.as_u64_pair();
        assert_eq!(uuid_flat.mode, GpuFlatAssetId::MODE_UUID);
        assert_eq!(
            uuid_flat.words,
            [
                (lo & 0xffff_ffff) as u32,
                (lo >> 32) as u32,
                (hi & 0xffff_ffff) as u32,
                (hi >> 32) as u32,
            ]
        );
    }

    #[test]
    fn phase_indices_point_into_shared_computed_slots() {
        let mut scene = GpuScene::default();
        let opaque_visible =
            scene.allocate_proxy(RenderProxyDescriptor::default().with_visible(true));
        let _opaque_hidden =
            scene.allocate_proxy(RenderProxyDescriptor::default().with_visible(false));
        let sky_visible = scene.allocate_proxy(
            RenderProxyDescriptor::default()
                .with_visible(true)
                .with_casts_shadows(false)
                .with_tag(2),
        );
        assert!(scene.set_kind(sky_visible, RenderProxyKind::Sky));

        let opaque_phase = GpuSceneFakeGpuEmulation::simulate_phase_instance_indices(
            &scene,
            GpuScenePhase::Opaque,
        );
        let shadow_phase = GpuSceneFakeGpuEmulation::simulate_phase_instance_indices(
            &scene,
            GpuScenePhase::Shadow,
        );

        assert_eq!(opaque_phase.len(), 1);
        assert_eq!(
            opaque_phase[0].computed_instance_index,
            opaque_visible.slot_index() as u32
        );
        assert_eq!(shadow_phase.len(), 1);
        assert_eq!(
            shadow_phase[0].computed_instance_index,
            opaque_visible.slot_index() as u32
        );
    }
}
