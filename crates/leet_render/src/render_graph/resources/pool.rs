//! Frame resource pool assignment planning.

use crate::RenderDevice;

use super::{
    AllocationRequest, AllocationRequestId, AllocationRequestSource, FrameBufferDesc,
    FrameLifetimeSolution, FrameResourceAllocationId, FrameResourceDesc, FrameResourceError,
    FrameResourceResult, FrameResourceShape, FrameTextureDesc, RequestRange,
};
use std::cmp::Ordering;

pub struct FrameResourcePool {
    allocations: Vec<FrameResourceAllocation>,
    max_unused_age: u32,
}

impl FrameResourcePool {
    pub const DEFAULT_MAX_UNUSED_AGE: u32 = 3;

    pub fn new() -> Self {
        Self::with_max_unused_age(Self::DEFAULT_MAX_UNUSED_AGE)
    }

    pub fn with_max_unused_age(max_unused_age: u32) -> Self {
        Self {
            allocations: Vec::new(),
            max_unused_age,
        }
    }

    pub fn allocations(&self) -> &[FrameResourceAllocation] {
        &self.allocations
    }

    pub fn max_unused_age(&self) -> u32 {
        self.max_unused_age
    }

    pub fn allocation(&self, id: FrameResourceAllocationId) -> Option<&FrameResourceAllocation> {
        self.allocations
            .iter()
            .find(|allocation| allocation.id == id)
    }

    pub fn allocation_mut(
        &mut self,
        id: FrameResourceAllocationId,
    ) -> Option<&mut FrameResourceAllocation> {
        self.allocations
            .iter_mut()
            .find(|allocation| allocation.id == id)
    }

    pub fn planner_candidates(&self) -> Vec<FrameResourcePoolCandidate> {
        self.allocations
            .iter()
            .filter(|allocation| {
                allocation.ownership == FrameResourceOwnership::Owned && allocation.cacheable
            })
            .map(|allocation| {
                FrameResourcePoolCandidate::owned_reusable(allocation.id, allocation.desc.clone())
            })
            .collect()
    }

    pub fn create_owned_texture(
        &mut self,
        id: FrameResourceAllocationId,
        desc: FrameTextureDesc,
        render_device: &RenderDevice,
    ) -> FrameResourceResult<()> {
        self.validate_new_allocation(id)?;
        desc.validate()?;
        let allocation_shape = desc.current_allocation_shape();
        let descriptor = desc.concrete_descriptor_for_shape(allocation_shape)?;
        let allocation_desc = desc.concrete_capacity_desc_for_shape(allocation_shape)?;
        let texture = render_device.0.create_texture(&descriptor);
        let default_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.allocations.push(FrameResourceAllocation {
            id,
            desc: FrameResourceDesc::Texture(allocation_desc),
            resource: FrameResource::Texture(FrameTextureResource {
                texture,
                default_view,
            }),
            ownership: FrameResourceOwnership::Owned,
            used_this_frame: true,
            cacheable: true,
            age: 0,
            overcapacity_age: 0,
        });
        Ok(())
    }

    pub fn create_owned_buffer(
        &mut self,
        id: FrameResourceAllocationId,
        desc: FrameBufferDesc,
        render_device: &RenderDevice,
    ) -> FrameResourceResult<()> {
        self.validate_new_allocation(id)?;
        desc.validate()?;
        let allocation_shape = desc.current_allocation_shape();
        let descriptor = desc.concrete_descriptor_for_shape(allocation_shape)?;
        let allocation_desc = desc.concrete_capacity_desc_for_shape(allocation_shape)?;
        let buffer = render_device.0.create_buffer(&descriptor);
        self.allocations.push(FrameResourceAllocation {
            id,
            desc: FrameResourceDesc::Buffer(allocation_desc),
            resource: FrameResource::Buffer(FrameBufferResource { buffer }),
            ownership: FrameResourceOwnership::Owned,
            used_this_frame: true,
            cacheable: true,
            age: 0,
            overcapacity_age: 0,
        });
        Ok(())
    }

    pub fn import_texture(
        &mut self,
        id: FrameResourceAllocationId,
        desc: FrameTextureDesc,
        texture: wgpu::Texture,
        default_view: wgpu::TextureView,
    ) -> FrameResourceResult<()> {
        self.insert_external_texture(
            id,
            desc,
            texture,
            default_view,
            FrameResourceOwnership::Imported,
        )
    }

    pub fn insert_external_swap_texture(
        &mut self,
        id: FrameResourceAllocationId,
        desc: FrameTextureDesc,
        texture: wgpu::Texture,
        default_view: wgpu::TextureView,
    ) -> FrameResourceResult<()> {
        self.insert_external_texture(
            id,
            desc,
            texture,
            default_view,
            FrameResourceOwnership::ExternalSwap,
        )
    }

    pub fn import_buffer(
        &mut self,
        id: FrameResourceAllocationId,
        desc: FrameBufferDesc,
        buffer: wgpu::Buffer,
    ) -> FrameResourceResult<()> {
        self.insert_external_buffer(id, desc, buffer, FrameResourceOwnership::Imported)
    }

    pub fn insert_external_swap_buffer(
        &mut self,
        id: FrameResourceAllocationId,
        desc: FrameBufferDesc,
        buffer: wgpu::Buffer,
    ) -> FrameResourceResult<()> {
        self.insert_external_buffer(id, desc, buffer, FrameResourceOwnership::ExternalSwap)
    }

    pub fn mark_used_this_frame(
        &mut self,
        id: FrameResourceAllocationId,
    ) -> FrameResourceResult<()> {
        let allocation = self
            .allocation_mut(id)
            .ok_or(FrameResourceError::InvalidOperation {
                operation: "FrameResourcePool::mark_used_this_frame",
                reason: "frame resource allocation id is not in the pool",
            })?;
        allocation.used_this_frame = true;
        allocation.overcapacity_age = 0;
        Ok(())
    }

    pub fn mark_used_this_frame_for_request(
        &mut self,
        id: FrameResourceAllocationId,
        requested_desc: &FrameResourceDesc,
    ) -> FrameResourceResult<()> {
        let allocation = self
            .allocation_mut(id)
            .ok_or(FrameResourceError::InvalidOperation {
                operation: "FrameResourcePool::mark_used_this_frame_for_request",
                reason: "frame resource allocation id is not in the pool",
            })?;
        allocation.used_this_frame = true;
        if allocation.ownership == FrameResourceOwnership::Owned
            && allocation.cacheable
            && allocation.desc.can_reuse_for(requested_desc)
            && allocation.desc.current_allocation_shape()
                != requested_desc.current_allocation_shape()
        {
            allocation.overcapacity_age = allocation.overcapacity_age.saturating_add(1);
        } else {
            allocation.overcapacity_age = 0;
        }
        Ok(())
    }

    pub fn mark_non_cacheable(&mut self, id: FrameResourceAllocationId) -> FrameResourceResult<()> {
        let allocation = self
            .allocation_mut(id)
            .ok_or(FrameResourceError::InvalidOperation {
                operation: "FrameResourcePool::mark_non_cacheable",
                reason: "frame resource allocation id is not in the pool",
            })?;
        allocation.cacheable = false;
        Ok(())
    }

    pub fn cleanup_after_frame(&mut self) {
        for allocation in &mut self.allocations {
            if allocation.used_this_frame {
                allocation.age = 0;
                allocation.used_this_frame = false;
            } else {
                allocation.age = allocation.age.saturating_add(1);
            }
        }

        let max_unused_age = self.max_unused_age;
        self.allocations.retain(|allocation| {
            allocation.ownership == FrameResourceOwnership::Owned
                && allocation.cacheable
                && allocation.age <= max_unused_age
                && allocation.overcapacity_age <= max_unused_age
        });
    }

    pub fn clear_all_caches(&mut self) {
        self.allocations.clear();
    }

    pub fn oversized_cached_allocations_for(
        &self,
        requested_desc: &FrameResourceDesc,
    ) -> Vec<FrameResourceAllocationId> {
        self.allocations
            .iter()
            .filter(|allocation| {
                allocation.ownership == FrameResourceOwnership::Owned
                    && allocation.cacheable
                    && allocation.desc.can_reuse_for(requested_desc)
                    && allocation.desc.current_allocation_shape()
                        != requested_desc.current_allocation_shape()
            })
            .map(|allocation| allocation.id)
            .collect()
    }

    fn insert_external_texture(
        &mut self,
        id: FrameResourceAllocationId,
        desc: FrameTextureDesc,
        texture: wgpu::Texture,
        default_view: wgpu::TextureView,
        ownership: FrameResourceOwnership,
    ) -> FrameResourceResult<()> {
        self.validate_new_allocation(id)?;
        desc.validate()?;
        self.allocations.push(FrameResourceAllocation {
            id,
            desc: FrameResourceDesc::Texture(desc),
            resource: FrameResource::Texture(FrameTextureResource {
                texture,
                default_view,
            }),
            ownership,
            used_this_frame: true,
            cacheable: false,
            age: 0,
            overcapacity_age: 0,
        });
        Ok(())
    }

    fn insert_external_buffer(
        &mut self,
        id: FrameResourceAllocationId,
        desc: FrameBufferDesc,
        buffer: wgpu::Buffer,
        ownership: FrameResourceOwnership,
    ) -> FrameResourceResult<()> {
        self.validate_new_allocation(id)?;
        desc.validate()?;
        self.allocations.push(FrameResourceAllocation {
            id,
            desc: FrameResourceDesc::Buffer(desc),
            resource: FrameResource::Buffer(FrameBufferResource { buffer }),
            ownership,
            used_this_frame: true,
            cacheable: false,
            age: 0,
            overcapacity_age: 0,
        });
        Ok(())
    }

    fn validate_new_allocation(&self, id: FrameResourceAllocationId) -> FrameResourceResult<()> {
        if self.allocation(id).is_some() {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameResourcePool::validate_new_allocation",
                reason: "frame resource allocation id already exists in the pool",
            });
        }

        Ok(())
    }
}

impl Default for FrameResourcePool {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FrameResourceAllocation {
    id: FrameResourceAllocationId,
    desc: FrameResourceDesc,
    resource: FrameResource,
    ownership: FrameResourceOwnership,
    used_this_frame: bool,
    cacheable: bool,
    age: u32,
    overcapacity_age: u32,
}

impl FrameResourceAllocation {
    pub fn id(&self) -> FrameResourceAllocationId {
        self.id
    }

    pub fn desc(&self) -> &FrameResourceDesc {
        &self.desc
    }

    pub fn resource(&self) -> &FrameResource {
        &self.resource
    }

    pub fn ownership(&self) -> FrameResourceOwnership {
        self.ownership
    }

    pub fn used_this_frame(&self) -> bool {
        self.used_this_frame
    }

    pub fn cacheable(&self) -> bool {
        self.cacheable
    }

    pub fn age(&self) -> u32 {
        self.age
    }

    pub fn overcapacity_age(&self) -> u32 {
        self.overcapacity_age
    }
}

pub enum FrameResource {
    Texture(FrameTextureResource),
    Buffer(FrameBufferResource),
}

impl FrameResource {
    pub fn as_texture(&self) -> Option<&FrameTextureResource> {
        match self {
            Self::Texture(resource) => Some(resource),
            Self::Buffer(_) => None,
        }
    }

    pub fn as_buffer(&self) -> Option<&FrameBufferResource> {
        match self {
            Self::Texture(_) => None,
            Self::Buffer(resource) => Some(resource),
        }
    }
}

pub struct FrameTextureResource {
    texture: wgpu::Texture,
    default_view: wgpu::TextureView,
}

impl FrameTextureResource {
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn default_view(&self) -> &wgpu::TextureView {
        &self.default_view
    }
}

pub struct FrameBufferResource {
    buffer: wgpu::Buffer,
}

impl FrameBufferResource {
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameResourceOwnership {
    Owned,
    Imported,
    ExternalSwap,
}

#[derive(Clone, Debug)]
pub struct FrameResourcePoolPlan {
    assignments: Vec<FrameResourcePoolAssignment>,
    assignment_order: Vec<AllocationRequestId>,
    rejections: Vec<FrameResourceReuseRejection>,
}

impl FrameResourcePoolPlan {
    pub fn plan(solution: &FrameLifetimeSolution) -> FrameResourceResult<Self> {
        Self::plan_with_cached_allocations(solution, &[])
    }

    pub fn plan_with_cached_allocations(
        solution: &FrameLifetimeSolution,
        cached_allocations: &[FrameResourcePoolCandidate],
    ) -> FrameResourceResult<Self> {
        let mut planner = PoolAssignmentPlanner::new(solution, cached_allocations)?;
        planner.plan()
    }

    pub fn assignments(&self) -> &[FrameResourcePoolAssignment] {
        &self.assignments
    }

    pub fn assignment_order(&self) -> &[AllocationRequestId] {
        &self.assignment_order
    }

    pub fn rejections(&self) -> &[FrameResourceReuseRejection] {
        &self.rejections
    }

    pub fn assignment_for_request(
        &self,
        request_id: AllocationRequestId,
    ) -> Option<&FrameResourcePoolAssignment> {
        self.assignments
            .iter()
            .find(|assignment| assignment.request_id == request_id)
    }
}

#[derive(Clone, Debug)]
pub struct FrameResourcePoolAssignment {
    request_id: AllocationRequestId,
    allocation_id: FrameResourceAllocationId,
    class: FrameResourceAllocationClass,
    reused_existing: bool,
}

impl FrameResourcePoolAssignment {
    pub fn request_id(&self) -> AllocationRequestId {
        self.request_id
    }

    pub fn allocation_id(&self) -> FrameResourceAllocationId {
        self.allocation_id
    }

    pub fn class(&self) -> FrameResourceAllocationClass {
        self.class
    }

    pub fn reused_existing(&self) -> bool {
        self.reused_existing
    }
}

#[derive(Clone, Debug)]
pub struct FrameResourcePoolCandidate {
    allocation_id: FrameResourceAllocationId,
    desc: FrameResourceDesc,
    class: FrameResourceAllocationClass,
}

impl FrameResourcePoolCandidate {
    pub fn owned_reusable(
        allocation_id: FrameResourceAllocationId,
        desc: FrameResourceDesc,
    ) -> Self {
        Self {
            allocation_id,
            desc,
            class: FrameResourceAllocationClass::OwnedReusable,
        }
    }

    pub fn restricted(allocation_id: FrameResourceAllocationId, desc: FrameResourceDesc) -> Self {
        Self {
            allocation_id,
            desc,
            class: FrameResourceAllocationClass::OwnedRestricted,
        }
    }

    pub fn allocation_id(&self) -> FrameResourceAllocationId {
        self.allocation_id
    }

    pub fn desc(&self) -> &FrameResourceDesc {
        &self.desc
    }

    pub fn class(&self) -> FrameResourceAllocationClass {
        self.class
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameResourceAllocationClass {
    OwnedReusable,
    OwnedRestricted,
    Imported,
    ExternalSwap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameResourceReuseRejection {
    request_id: AllocationRequestId,
    candidate_allocation_id: FrameResourceAllocationId,
    reason: FrameResourceReuseRejectionReason,
}

impl FrameResourceReuseRejection {
    pub fn request_id(&self) -> AllocationRequestId {
        self.request_id
    }

    pub fn candidate_allocation_id(&self) -> FrameResourceAllocationId {
        self.candidate_allocation_id
    }

    pub fn reason(&self) -> FrameResourceReuseRejectionReason {
        self.reason
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameResourceReuseRejectionReason {
    CandidateNotReusable,
    DescriptorIncompatible,
    LifetimeOverlaps,
    RequestNotReusable,
}

#[derive(Clone, Debug)]
struct PlannedSlot {
    allocation_id: FrameResourceAllocationId,
    desc: FrameResourceDesc,
    class: FrameResourceAllocationClass,
    lifetimes: Vec<RequestRange>,
    seeded_from_cache: bool,
}

struct PoolAssignmentPlanner<'a> {
    solution: &'a FrameLifetimeSolution,
    slots: Vec<PlannedSlot>,
    next_allocation_id: u32,
    assignments: Vec<FrameResourcePoolAssignment>,
    assignment_order: Vec<AllocationRequestId>,
    rejections: Vec<FrameResourceReuseRejection>,
}

impl<'a> PoolAssignmentPlanner<'a> {
    fn new(
        solution: &'a FrameLifetimeSolution,
        cached_allocations: &[FrameResourcePoolCandidate],
    ) -> FrameResourceResult<Self> {
        validate_cached_allocation_ids(cached_allocations)?;

        let mut next_allocation_id = 0;
        let slots = cached_allocations
            .iter()
            .map(|candidate| {
                next_allocation_id = next_allocation_id.max(candidate.allocation_id.get() + 1);
                PlannedSlot {
                    allocation_id: candidate.allocation_id,
                    desc: candidate.desc.clone(),
                    class: candidate.class,
                    lifetimes: Vec::new(),
                    seeded_from_cache: true,
                }
            })
            .collect();

        Ok(Self {
            solution,
            slots,
            next_allocation_id,
            assignments: Vec::new(),
            assignment_order: Vec::new(),
            rejections: Vec::new(),
        })
    }

    fn plan(&mut self) -> FrameResourceResult<FrameResourcePoolPlan> {
        let mut requests = self
            .solution
            .allocation_requests()
            .iter()
            .collect::<Vec<_>>();
        requests.sort_by(|a, b| compare_allocation_requests(a, b));

        for request in requests {
            self.assign_request(request)?;
        }

        Ok(FrameResourcePoolPlan {
            assignments: std::mem::take(&mut self.assignments),
            assignment_order: std::mem::take(&mut self.assignment_order),
            rejections: std::mem::take(&mut self.rejections),
        })
    }

    fn assign_request(&mut self, request: &AllocationRequest) -> FrameResourceResult<()> {
        self.assignment_order.push(request.id());
        let class = allocation_class_for_request(request);
        if class != FrameResourceAllocationClass::OwnedReusable {
            self.create_new_slot(request, class, false)?;
            return Ok(());
        }

        for slot_index in 0..self.slots.len() {
            let slot = &self.slots[slot_index];
            if let Some(reason) = reuse_rejection_reason(request, slot) {
                self.rejections.push(FrameResourceReuseRejection {
                    request_id: request.id(),
                    candidate_allocation_id: slot.allocation_id,
                    reason,
                });
                continue;
            }

            let reused_existing = slot.seeded_from_cache || !slot.lifetimes.is_empty();
            let allocation_id = slot.allocation_id;
            self.slots[slot_index].lifetimes.push(request.lifetime());
            self.assignments.push(FrameResourcePoolAssignment {
                request_id: request.id(),
                allocation_id,
                class,
                reused_existing,
            });
            return Ok(());
        }

        self.create_new_slot(request, class, false)
    }

    fn create_new_slot(
        &mut self,
        request: &AllocationRequest,
        class: FrameResourceAllocationClass,
        reused_existing: bool,
    ) -> FrameResourceResult<()> {
        let allocation_id = FrameResourceAllocationId::new(self.next_allocation_id);
        self.next_allocation_id += 1;
        let desc = request
            .desc()
            .concrete_capacity_desc_for_shape(request.desc().current_allocation_shape())?;
        self.slots.push(PlannedSlot {
            allocation_id,
            desc,
            class,
            lifetimes: vec![request.lifetime()],
            seeded_from_cache: false,
        });
        self.assignments.push(FrameResourcePoolAssignment {
            request_id: request.id(),
            allocation_id,
            class,
            reused_existing,
        });
        Ok(())
    }
}

fn allocation_class_for_request(request: &AllocationRequest) -> FrameResourceAllocationClass {
    match request.source() {
        AllocationRequestSource::Owned
            if request.can_reuse_same_frame() && request.can_cache_across_frames() =>
        {
            FrameResourceAllocationClass::OwnedReusable
        }
        AllocationRequestSource::Owned => FrameResourceAllocationClass::OwnedRestricted,
        AllocationRequestSource::Imported(_) => FrameResourceAllocationClass::Imported,
        AllocationRequestSource::ExternalSwap(_) => FrameResourceAllocationClass::ExternalSwap,
    }
}

fn validate_cached_allocation_ids(
    cached_allocations: &[FrameResourcePoolCandidate],
) -> FrameResourceResult<()> {
    for (index, candidate) in cached_allocations.iter().enumerate() {
        if cached_allocations[..index]
            .iter()
            .any(|previous| previous.allocation_id == candidate.allocation_id)
        {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameResourcePoolPlan::plan_with_cached_allocations",
                reason: "cached allocation ids must be unique",
            });
        }
    }

    Ok(())
}

fn reuse_rejection_reason(
    request: &AllocationRequest,
    slot: &PlannedSlot,
) -> Option<FrameResourceReuseRejectionReason> {
    if !request.can_reuse_same_frame() || !request.can_cache_across_frames() {
        return Some(FrameResourceReuseRejectionReason::RequestNotReusable);
    }
    if slot.class != FrameResourceAllocationClass::OwnedReusable {
        return Some(FrameResourceReuseRejectionReason::CandidateNotReusable);
    }
    if !slot.desc.can_reuse_for(request.desc()) {
        return Some(FrameResourceReuseRejectionReason::DescriptorIncompatible);
    }
    if slot
        .lifetimes
        .iter()
        .any(|lifetime| lifetime.overlaps(request.lifetime()))
    {
        return Some(FrameResourceReuseRejectionReason::LifetimeOverlaps);
    }

    None
}

fn compare_allocation_requests(a: &AllocationRequest, b: &AllocationRequest) -> Ordering {
    resource_weight(b.desc())
        .cmp(&resource_weight(a.desc()))
        .then_with(|| a.lifetime().start().cmp(&b.lifetime().start()))
        .then_with(|| a.id().cmp(&b.id()))
}

fn resource_weight(desc: &FrameResourceDesc) -> u128 {
    match desc.current_allocation_shape() {
        FrameResourceShape::Texture {
            size,
            mip_level_count,
        } => {
            size.width as u128
                * size.height as u128
                * size.depth_or_array_layers as u128
                * mip_level_count as u128
        }
        FrameResourceShape::Buffer { size_bytes } => size_bytes as u128,
    }
}
