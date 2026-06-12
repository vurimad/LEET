//! Renderer-lifetime resource allocator orchestration.

use crate::RenderDevice;

use super::{
    AllocationRequestSource, ExternalFrameResourceId, FrameBufferDesc, FrameBufferResource,
    FrameLifetimeSolution, FrameResource, FrameResourceAllocation, FrameResourceAllocationClass,
    FrameResourceAllocationId, FrameResourceDiagnostics, FrameResourceError, FrameResourcePool,
    FrameResourcePoolPlan, FrameResourceResult, FrameTextureDesc, FrameTextureResource,
    RenderFlowAutoId, RenderFlowGroup, RenderFlowNameTag, RequestGroup, RequestGroupAction,
    RequestTime, ResourceAllocatorPhase, ResourceRequest, MAX_RENDER_FLOW_GROUPS,
};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Clone)]
pub struct RenderResourceAllocator {
    inner: Arc<Mutex<RenderResourceAllocatorState>>,
}

struct RenderResourceAllocatorState {
    phase: ResourceAllocatorPhase,
    request_groups: Vec<RequestGroup>,
    frame_state: RenderResourceFrameState,
    resolution: Option<Arc<FrameResourceResolution>>,
    current_consume_time: Option<RequestTime>,
}

impl RenderResourceAllocator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RenderResourceAllocatorState::new())),
        }
    }

    pub fn reset_for_frame(&self) {
        self.state_mut().reset_for_frame();
    }

    pub fn phase(&self) -> ResourceAllocatorPhase {
        self.state().phase()
    }

    pub fn is_consume_phase(&self) -> bool {
        self.state().is_consume_phase()
    }

    pub fn set_phase(&self, next: ResourceAllocatorPhase) -> FrameResourceResult<()> {
        self.state_mut().set_phase(next)
    }

    pub fn record_request(
        &self,
        flow_group: RenderFlowGroup,
        request: ResourceRequest,
    ) -> FrameResourceResult<RequestGroupAction> {
        self.state_mut().record_request(flow_group, request)
    }

    pub fn prepare_preconsume_groups(&self, group_count: usize) -> FrameResourceResult<()> {
        self.state_mut().prepare_preconsume_groups(group_count)
    }

    pub fn request_is_declared(
        &self,
        flow_group: RenderFlowGroup,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<bool> {
        self.state_mut().request_is_declared(flow_group, tag)
    }

    pub fn request_decision(
        &self,
        flow_group: RenderFlowGroup,
        value: bool,
    ) -> FrameResourceResult<bool> {
        self.state_mut().request_decision(flow_group, value)
    }

    pub fn next_request_auto_id(
        &self,
        flow_group: RenderFlowGroup,
    ) -> FrameResourceResult<RenderFlowAutoId> {
        self.state().next_request_auto_id(flow_group)
    }

    pub fn request_group(&self, flow_group: RenderFlowGroup) -> Option<RequestGroup> {
        self.state().request_group(flow_group).cloned()
    }

    pub fn request_groups(&self) -> Vec<RequestGroup> {
        self.state().request_groups().to_vec()
    }

    pub fn request_group_count(&self) -> usize {
        self.state().request_group_count()
    }

    pub fn current_consume_time(&self) -> Option<RequestTime> {
        self.state().current_consume_time()
    }

    pub fn diagnostics(&self) -> FrameResourceDiagnostics<'_> {
        FrameResourceDiagnostics::new(self)
    }

    pub fn lifetime_solution(&self) -> Option<FrameLifetimeSolution> {
        self.state().lifetime_solution().cloned()
    }

    pub fn pool_plan(&self) -> Option<FrameResourcePoolPlan> {
        self.state().pool_plan().cloned()
    }

    pub fn resource_pool(&self) -> FrameResourcePoolReadGuard<'_> {
        FrameResourcePoolReadGuard {
            guard: self.state(),
        }
    }

    pub fn resource_pool_mut(&self) -> FrameResourcePoolWriteGuard<'_> {
        FrameResourcePoolWriteGuard {
            guard: self.state_mut(),
        }
    }

    pub fn resources_resolved(&self) -> bool {
        self.state().resources_resolved()
    }

    pub fn process_eviction(&self) -> bool {
        self.state().process_eviction()
    }

    pub fn set_process_eviction(&self, process_eviction: bool) {
        self.state_mut().set_process_eviction(process_eviction);
    }

    pub fn resolved_allocation_id(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameResourceAllocationId>> {
        self.state().resolved_allocation_id(tag)
    }

    pub(crate) fn frame_resource_resolution(
        &self,
    ) -> FrameResourceResult<Arc<FrameResourceResolution>> {
        self.state().frame_resource_resolution()
    }

    pub fn register_external_texture(
        &self,
        external_id: ExternalFrameResourceId,
        desc: FrameTextureDesc,
        texture: wgpu::Texture,
        default_view: wgpu::TextureView,
    ) -> FrameResourceResult<()> {
        self.state_mut()
            .register_external_texture(external_id, desc, texture, default_view)
    }

    pub fn register_external_buffer(
        &self,
        external_id: ExternalFrameResourceId,
        desc: FrameBufferDesc,
        buffer: wgpu::Buffer,
    ) -> FrameResourceResult<()> {
        self.state_mut()
            .register_external_buffer(external_id, desc, buffer)
    }

    pub fn resolve_frame_resources(&self, render_device: &RenderDevice) -> FrameResourceResult<()> {
        self.state_mut().resolve_frame_resources(render_device)
    }

    pub fn get_texture(&self, tag: RenderFlowNameTag) -> FrameResourceResult<FrameTextureResource> {
        self.state().get_texture(tag).cloned()
    }

    pub fn try_get_texture(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameTextureResource>> {
        self.state()
            .try_get_texture(tag)
            .map(|resource| resource.cloned())
    }

    pub fn get_buffer(&self, tag: RenderFlowNameTag) -> FrameResourceResult<FrameBufferResource> {
        self.state().get_buffer(tag).cloned()
    }

    pub fn try_get_buffer(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameBufferResource>> {
        self.state()
            .try_get_buffer(tag)
            .map(|resource| resource.cloned())
    }

    pub fn clear_all_caches(&self) -> FrameResourceResult<()> {
        self.state_mut().clear_all_caches()
    }

    pub fn caches_cleared_count(&self) -> u32 {
        self.state().caches_cleared_count()
    }

    pub fn validate_resource_retrieval_phase(&self) -> FrameResourceResult<()> {
        self.state().validate_resource_retrieval_phase()
    }

    fn state(&self) -> MutexGuard<'_, RenderResourceAllocatorState> {
        self.inner
            .lock()
            .expect("render resource allocator state mutex was poisoned")
    }

    fn state_mut(&self) -> MutexGuard<'_, RenderResourceAllocatorState> {
        self.state()
    }
}

pub struct FrameResourcePoolReadGuard<'a> {
    guard: MutexGuard<'a, RenderResourceAllocatorState>,
}

impl Deref for FrameResourcePoolReadGuard<'_> {
    type Target = FrameResourcePool;

    fn deref(&self) -> &Self::Target {
        &self.guard.frame_state.resource_pool
    }
}

pub struct FrameResourcePoolWriteGuard<'a> {
    guard: MutexGuard<'a, RenderResourceAllocatorState>,
}

impl Deref for FrameResourcePoolWriteGuard<'_> {
    type Target = FrameResourcePool;

    fn deref(&self) -> &Self::Target {
        &self.guard.frame_state.resource_pool
    }
}

impl DerefMut for FrameResourcePoolWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard.frame_state.resource_pool
    }
}

impl RenderResourceAllocatorState {
    fn new() -> Self {
        Self {
            phase: ResourceAllocatorPhase::Startup,
            request_groups: Vec::new(),
            frame_state: RenderResourceFrameState::new(),
            resolution: None,
            current_consume_time: None,
        }
    }

    fn reset_for_frame(&mut self) {
        self.phase = ResourceAllocatorPhase::Startup;
        self.request_groups.clear();
        self.resolution = None;
        self.frame_state.reset_for_frame();
        self.current_consume_time = None;
    }

    pub fn phase(&self) -> ResourceAllocatorPhase {
        self.phase
    }

    pub fn is_consume_phase(&self) -> bool {
        self.phase.is_consume()
    }

    pub fn set_phase(&mut self, next: ResourceAllocatorPhase) -> FrameResourceResult<()> {
        self.validate_transition(next)?;

        match next {
            ResourceAllocatorPhase::Startup => {}
            ResourceAllocatorPhase::PreConsume => self.begin_preconsume(),
            ResourceAllocatorPhase::Resolve => self.resolve_request_stream_shell()?,
            ResourceAllocatorPhase::Consume => self.begin_consume(),
            ResourceAllocatorPhase::Cleanup => self.begin_cleanup()?,
        }

        self.phase = next;
        Ok(())
    }

    pub fn record_request(
        &mut self,
        flow_group: RenderFlowGroup,
        request: ResourceRequest,
    ) -> FrameResourceResult<RequestGroupAction> {
        let phase = self.phase;
        let action = self.request_group_mut(flow_group)?.apply(phase, request)?;
        if phase == ResourceAllocatorPhase::Consume {
            self.current_consume_time = Some(RequestTime::new(flow_group, action.id().get()));
        }
        Ok(action)
    }

    pub fn prepare_preconsume_groups(&mut self, group_count: usize) -> FrameResourceResult<()> {
        if self.phase != ResourceAllocatorPhase::PreConsume {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::prepare_preconsume_groups",
                reason: "request groups can only be prepared during pre-consume",
            });
        }
        if group_count > MAX_RENDER_FLOW_GROUPS {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::prepare_preconsume_groups",
                reason: "requested render-flow group count exceeds the allocator limit",
            });
        }

        self.request_groups.clear();
        self.request_groups
            .resize_with(group_count, RequestGroup::new);
        Ok(())
    }

    pub fn request_is_declared(
        &mut self,
        flow_group: RenderFlowGroup,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<bool> {
        let declared = self.is_declared_at_current_request_position(flow_group, tag)?;
        self.record_request(flow_group, ResourceRequest::IsDeclared { tag, declared })?;
        Ok(declared)
    }

    pub fn request_decision(
        &mut self,
        flow_group: RenderFlowGroup,
        value: bool,
    ) -> FrameResourceResult<bool> {
        let action = self.record_request(flow_group, ResourceRequest::Decision { value })?;
        let Some(ResourceRequest::Decision { value }) = action.recorded_request() else {
            return Ok(value);
        };
        Ok(*value)
    }

    pub fn next_request_auto_id(
        &self,
        flow_group: RenderFlowGroup,
    ) -> FrameResourceResult<RenderFlowAutoId> {
        let request_index = match self.phase {
            ResourceAllocatorPhase::PreConsume => self
                .request_group(flow_group)
                .map(|group| group.requests().len())
                .unwrap_or(0),
            ResourceAllocatorPhase::Consume => self
                .request_group(flow_group)
                .map(|group| group.consume_cursor())
                .ok_or(FrameResourceError::InvalidOperation {
                    operation: "RenderResourceAllocator::next_request_auto_id",
                    reason: "consume cannot generate a temp tag for an untouched flow group",
                })?,
            _ => {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "RenderResourceAllocator::next_request_auto_id",
                    reason: "temp tags are only valid during pre-consume or consume",
                });
            }
        };

        let group = u32::from(flow_group.get());
        let index = u32::try_from(request_index).map_err(|_| FrameResourceError::InvalidState {
            reason: "request index exceeded u32 range while generating temp tag",
        })?;
        if group > 0xff || index > 0xffff {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::next_request_auto_id",
                reason: "temp tag id cannot pack this flow group/request index",
            });
        }

        RenderFlowAutoId::new((group << 16) | (index + 1))
    }

    pub fn request_group(&self, flow_group: RenderFlowGroup) -> Option<&RequestGroup> {
        if !flow_group.is_valid() {
            return None;
        }

        self.request_groups.get(flow_group.index())
    }

    pub fn request_groups(&self) -> &[RequestGroup] {
        &self.request_groups
    }

    pub fn request_group_count(&self) -> usize {
        self.request_groups.len()
    }

    pub fn current_consume_time(&self) -> Option<RequestTime> {
        self.current_consume_time
    }

    pub fn lifetime_solution(&self) -> Option<&FrameLifetimeSolution> {
        self.resolution
            .as_ref()
            .map(|resolution| resolution.lifetime_solution())
    }

    pub fn pool_plan(&self) -> Option<&FrameResourcePoolPlan> {
        self.resolution
            .as_ref()
            .map(|resolution| resolution.pool_plan())
    }

    pub fn resources_resolved(&self) -> bool {
        self.resolution
            .as_ref()
            .is_some_and(|resolution| resolution.resources_resolved())
    }

    pub fn process_eviction(&self) -> bool {
        self.frame_state.process_eviction
    }

    pub fn set_process_eviction(&mut self, process_eviction: bool) {
        self.frame_state.process_eviction = process_eviction;
    }

    pub fn resolved_allocation_id(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameResourceAllocationId>> {
        self.try_resolved_allocation_id(tag)
    }

    pub(crate) fn frame_resource_resolution(
        &self,
    ) -> FrameResourceResult<Arc<FrameResourceResolution>> {
        let resolution = self
            .resolution
            .as_ref()
            .ok_or(FrameResourceError::InvalidState {
                reason: "frame resource resolution is missing",
            })?;
        if !resolution.resources_resolved() {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::frame_resource_resolution",
                reason: "frame resources have not been materialized",
            });
        }
        Ok(Arc::clone(resolution))
    }

    pub fn register_external_texture(
        &mut self,
        external_id: ExternalFrameResourceId,
        desc: FrameTextureDesc,
        texture: wgpu::Texture,
        default_view: wgpu::TextureView,
    ) -> FrameResourceResult<()> {
        self.register_external_resource(
            external_id,
            PendingExternalFrameResource::Texture {
                desc,
                texture,
                default_view,
            },
        )
    }

    pub fn register_external_buffer(
        &mut self,
        external_id: ExternalFrameResourceId,
        desc: FrameBufferDesc,
        buffer: wgpu::Buffer,
    ) -> FrameResourceResult<()> {
        self.register_external_resource(
            external_id,
            PendingExternalFrameResource::Buffer { desc, buffer },
        )
    }

    pub fn resolve_frame_resources(
        &mut self,
        render_device: &RenderDevice,
    ) -> FrameResourceResult<()> {
        if self.phase != ResourceAllocatorPhase::Resolve {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::resolve_frame_resources",
                reason: "frame resources can only be materialized during resolve",
            });
        }

        let lifetime_solution = self
            .resolution
            .as_ref()
            .ok_or(FrameResourceError::InvalidState {
                reason: "lifetime solution is missing during resolve",
            })?
            .lifetime_solution()
            .clone();
        let pool_plan = FrameResourcePoolPlan::plan_with_cached_allocations(
            &lifetime_solution,
            &self.frame_state.resource_pool.planner_candidates(),
        )?;
        let allocation_requests = lifetime_solution.allocation_requests().to_vec();

        for assignment in pool_plan.assignments() {
            let allocation_request = allocation_requests
                .iter()
                .find(|request| request.id() == assignment.request_id())
                .ok_or(FrameResourceError::InvalidState {
                    reason: "pool assignment references an unknown allocation request",
                })?;

            if assignment.reused_existing() {
                self.frame_state
                    .resource_pool
                    .mark_used_this_frame_for_request(
                        assignment.allocation_id(),
                        allocation_request.desc(),
                    )?;
                continue;
            }

            match allocation_request.source() {
                AllocationRequestSource::Owned => match allocation_request.desc() {
                    super::FrameResourceDesc::Texture(desc) => {
                        self.frame_state.resource_pool.create_owned_texture(
                            assignment.allocation_id(),
                            desc.clone(),
                            render_device,
                        )?;
                    }
                    super::FrameResourceDesc::Buffer(desc) => {
                        self.frame_state.resource_pool.create_owned_buffer(
                            assignment.allocation_id(),
                            desc.clone(),
                            render_device,
                        )?;
                    }
                },
                AllocationRequestSource::Imported(external_id) => {
                    Self::attach_external_resource(
                        &mut self.frame_state,
                        assignment.allocation_id(),
                        allocation_request.desc(),
                        external_id,
                        FrameResourceAllocationClass::Imported,
                    )?;
                }
                AllocationRequestSource::ExternalSwap(external_id) => {
                    Self::attach_external_resource(
                        &mut self.frame_state,
                        assignment.allocation_id(),
                        allocation_request.desc(),
                        external_id,
                        FrameResourceAllocationClass::ExternalSwap,
                    )?;
                }
            }

            if assignment.class() == FrameResourceAllocationClass::OwnedRestricted {
                self.frame_state
                    .resource_pool
                    .mark_non_cacheable(assignment.allocation_id())?;
            }
        }

        let resolution = self
            .resolution
            .as_ref()
            .ok_or(FrameResourceError::InvalidState {
                reason: "frame resource resolution was lost during resolve",
            })?;
        self.resolution = Some(Arc::new(FrameResourceResolution::resolved(
            resolution.lifetime_solution().clone(),
            pool_plan,
            self.frame_state.resource_pool.allocations().to_vec(),
        )));
        Ok(())
    }

    pub fn get_texture(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<&FrameTextureResource> {
        self.try_get_texture(tag)?
            .ok_or(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::get_texture",
                reason: "tag does not resolve to a texture at the current consume time",
            })
    }

    pub fn try_get_texture(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<&FrameTextureResource>> {
        let Some(resource) = self.try_get_resource(tag)? else {
            return Ok(None);
        };

        match resource {
            FrameResource::Texture(texture) => Ok(Some(texture)),
            FrameResource::Buffer(_) => Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::try_get_texture",
                reason: "resolved resource is a buffer, not a texture",
            }),
        }
    }

    pub fn get_buffer(&self, tag: RenderFlowNameTag) -> FrameResourceResult<&FrameBufferResource> {
        self.try_get_buffer(tag)?
            .ok_or(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::get_buffer",
                reason: "tag does not resolve to a buffer at the current consume time",
            })
    }

    pub fn try_get_buffer(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<&FrameBufferResource>> {
        let Some(resource) = self.try_get_resource(tag)? else {
            return Ok(None);
        };

        match resource {
            FrameResource::Texture(_) => Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::try_get_buffer",
                reason: "resolved resource is a texture, not a buffer",
            }),
            FrameResource::Buffer(buffer) => Ok(Some(buffer)),
        }
    }

    pub fn clear_all_caches(&mut self) -> FrameResourceResult<()> {
        if self.phase != ResourceAllocatorPhase::Cleanup {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::clear_all_caches",
                reason: "cache clearing is only valid during cleanup",
            });
        }

        self.frame_state.caches_cleared_count += 1;
        self.frame_state.resource_pool.clear_all_caches();
        Ok(())
    }

    pub fn caches_cleared_count(&self) -> u32 {
        self.frame_state.caches_cleared_count
    }

    pub fn validate_resource_retrieval_phase(&self) -> FrameResourceResult<()> {
        if self.phase == ResourceAllocatorPhase::Consume {
            Ok(())
        } else {
            Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::validate_resource_retrieval_phase",
                reason: "resource retrieval is only valid during consume",
            })
        }
    }

    fn validate_transition(&self, next: ResourceAllocatorPhase) -> FrameResourceResult<()> {
        let valid = matches!(
            (self.phase, next),
            (
                ResourceAllocatorPhase::Cleanup,
                ResourceAllocatorPhase::Startup
            ) | (
                ResourceAllocatorPhase::Startup,
                ResourceAllocatorPhase::PreConsume
            ) | (
                ResourceAllocatorPhase::PreConsume,
                ResourceAllocatorPhase::Resolve
            ) | (
                ResourceAllocatorPhase::Resolve,
                ResourceAllocatorPhase::Consume
            ) | (
                ResourceAllocatorPhase::Consume,
                ResourceAllocatorPhase::Cleanup
            )
        );

        if valid {
            Ok(())
        } else {
            Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::set_phase",
                reason: "invalid frame resource allocator phase transition",
            })
        }
    }

    fn begin_preconsume(&mut self) {
        self.resolution = None;
        self.frame_state.external_resources.clear();
        self.current_consume_time = None;
        for group in &mut self.request_groups {
            group.reset_for_preconsume();
        }
    }

    fn resolve_request_stream_shell(&mut self) -> FrameResourceResult<()> {
        for group in &self.request_groups {
            for request in group.requests() {
                if let ResourceRequest::Declare { desc, .. } = request {
                    desc.validate()?;
                }
            }
        }

        let lifetime_solution = FrameLifetimeSolution::solve_request_groups(&self.request_groups)?;
        let pool_plan = FrameResourcePoolPlan::plan(&lifetime_solution)?;
        self.resolution = Some(Arc::new(FrameResourceResolution::unresolved(
            lifetime_solution,
            pool_plan,
        )));
        Ok(())
    }

    fn begin_consume(&mut self) {
        self.current_consume_time = None;
        for group in &mut self.request_groups {
            group.reset_consume_cursor();
        }
    }

    fn begin_cleanup(&mut self) -> FrameResourceResult<()> {
        for group in &self.request_groups {
            group.validate_consume_finished()?;
        }

        self.request_groups.clear();
        self.resolution = None;
        self.frame_state.external_resources.clear();
        self.current_consume_time = None;
        self.frame_state
            .resource_pool
            .cleanup_after_frame_with_eviction(self.frame_state.process_eviction);
        Ok(())
    }

    fn request_group_mut(
        &mut self,
        flow_group: RenderFlowGroup,
    ) -> FrameResourceResult<&mut RequestGroup> {
        if !flow_group.is_valid() {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::request_group_mut",
                reason: "render-flow group exceeds the allocator limit",
            });
        }

        let index = flow_group.index();
        while self.request_groups.len() <= index {
            self.request_groups.push(RequestGroup::new());
        }
        Ok(&mut self.request_groups[index])
    }

    fn is_declared_at_current_request_position(
        &self,
        flow_group: RenderFlowGroup,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<bool> {
        let Some(group) = self.request_group(flow_group) else {
            return Ok(false);
        };
        let end = match self.phase {
            ResourceAllocatorPhase::PreConsume => group.requests().len(),
            ResourceAllocatorPhase::Consume => group.consume_cursor(),
            _ => {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "RenderResourceAllocator::request_is_declared",
                    reason: "is-declared requests are only valid during pre-consume or consume",
                });
            }
        };

        Ok(is_tag_declared_after_requests(
            &group.requests()[..end],
            tag,
        ))
    }

    fn try_get_resource(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<&FrameResource>> {
        let resolution = self.resolution_for_consume()?;
        let current_time = self
            .current_consume_time
            .ok_or(FrameResourceError::InvalidState {
                reason: "no current consume request time is available for resource retrieval",
            })?;
        resolution.try_get_resource_at(tag, current_time)
    }

    fn try_resolved_allocation_id(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameResourceAllocationId>> {
        self.validate_resource_retrieval_phase()?;
        if !self.resources_resolved() {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::try_resolved_allocation_id",
                reason: "frame resources have not been materialized",
            });
        }

        let current_time = self
            .current_consume_time
            .ok_or(FrameResourceError::InvalidState {
                reason: "no current consume request time is available for resource retrieval",
            })?;
        self.resolution_for_consume()?
            .resolved_allocation_id_at(tag, current_time)
    }

    fn resolution_for_consume(&self) -> FrameResourceResult<&FrameResourceResolution> {
        self.validate_resource_retrieval_phase()?;
        if !self.resources_resolved() {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::try_resolved_allocation_id",
                reason: "frame resources have not been materialized",
            });
        }

        self.resolution
            .as_deref()
            .ok_or(FrameResourceError::InvalidState {
                reason: "frame resource resolution is missing during consume",
            })
    }

    fn register_external_resource(
        &mut self,
        external_id: ExternalFrameResourceId,
        resource: PendingExternalFrameResource,
    ) -> FrameResourceResult<()> {
        if self
            .frame_state
            .external_resources
            .insert(external_id, resource)
            .is_some()
        {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::register_external_resource",
                reason: "external resource id was registered more than once",
            });
        }

        Ok(())
    }

    fn attach_external_resource(
        frame_state: &mut RenderResourceFrameState,
        allocation_id: FrameResourceAllocationId,
        expected_desc: &super::FrameResourceDesc,
        external_id: ExternalFrameResourceId,
        class: FrameResourceAllocationClass,
    ) -> FrameResourceResult<()> {
        let resource = frame_state.external_resources.remove(&external_id).ok_or(
            FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::attach_external_resource",
                reason: "external resource id was not registered before resolve",
            },
        )?;

        match (resource, expected_desc, class) {
            (
                PendingExternalFrameResource::Texture {
                    desc,
                    texture,
                    default_view,
                },
                super::FrameResourceDesc::Texture(expected),
                FrameResourceAllocationClass::Imported,
            ) if super::FrameResourceDesc::Texture(desc.clone())
                .is_exact_match(&super::FrameResourceDesc::Texture(expected.clone())) =>
            {
                frame_state
                    .resource_pool
                    .import_texture(allocation_id, desc, texture, default_view)
            }
            (
                PendingExternalFrameResource::Texture {
                    desc,
                    texture,
                    default_view,
                },
                super::FrameResourceDesc::Texture(expected),
                FrameResourceAllocationClass::ExternalSwap,
            ) if super::FrameResourceDesc::Texture(desc.clone())
                .is_exact_match(&super::FrameResourceDesc::Texture(expected.clone())) =>
            {
                frame_state.resource_pool.insert_external_swap_texture(
                    allocation_id,
                    desc,
                    texture,
                    default_view,
                )
            }
            (
                PendingExternalFrameResource::Buffer { desc, buffer },
                super::FrameResourceDesc::Buffer(expected),
                FrameResourceAllocationClass::Imported,
            ) if super::FrameResourceDesc::Buffer(desc.clone())
                .is_exact_match(&super::FrameResourceDesc::Buffer(expected.clone())) =>
            {
                frame_state
                    .resource_pool
                    .import_buffer(allocation_id, desc, buffer)
            }
            (
                PendingExternalFrameResource::Buffer { desc, buffer },
                super::FrameResourceDesc::Buffer(expected),
                FrameResourceAllocationClass::ExternalSwap,
            ) if super::FrameResourceDesc::Buffer(desc.clone())
                .is_exact_match(&super::FrameResourceDesc::Buffer(expected.clone())) =>
            {
                frame_state
                    .resource_pool
                    .insert_external_swap_buffer(allocation_id, desc, buffer)
            }
            _ => Err(FrameResourceError::InvalidOperation {
                operation: "RenderResourceAllocator::attach_external_resource",
                reason:
                    "registered external resource does not match the requested kind or descriptor",
            }),
        }
    }
}

impl Default for RenderResourceAllocator {
    fn default() -> Self {
        Self::new()
    }
}

struct RenderResourceFrameState {
    resource_pool: FrameResourcePool,
    external_resources: HashMap<ExternalFrameResourceId, PendingExternalFrameResource>,
    caches_cleared_count: u32,
    process_eviction: bool,
}

impl RenderResourceFrameState {
    fn new() -> Self {
        Self {
            resource_pool: FrameResourcePool::new(),
            external_resources: HashMap::new(),
            caches_cleared_count: 0,
            process_eviction: true,
        }
    }

    fn reset_for_frame(&mut self) {
        self.external_resources.clear();
        self.process_eviction = true;
    }
}

pub(crate) struct FrameResourceResolution {
    lifetime_solution: FrameLifetimeSolution,
    pool_plan: FrameResourcePoolPlan,
    allocations: Vec<FrameResourceAllocation>,
    resources_resolved: bool,
}

impl FrameResourceResolution {
    fn unresolved(
        lifetime_solution: FrameLifetimeSolution,
        pool_plan: FrameResourcePoolPlan,
    ) -> Self {
        Self {
            lifetime_solution,
            pool_plan,
            allocations: Vec::new(),
            resources_resolved: false,
        }
    }

    fn resolved(
        lifetime_solution: FrameLifetimeSolution,
        pool_plan: FrameResourcePoolPlan,
        allocations: Vec<FrameResourceAllocation>,
    ) -> Self {
        Self {
            lifetime_solution,
            pool_plan,
            allocations,
            resources_resolved: true,
        }
    }

    fn lifetime_solution(&self) -> &FrameLifetimeSolution {
        &self.lifetime_solution
    }

    fn pool_plan(&self) -> &FrameResourcePoolPlan {
        &self.pool_plan
    }

    fn allocation(&self, id: FrameResourceAllocationId) -> Option<&FrameResourceAllocation> {
        self.allocations
            .iter()
            .find(|allocation| allocation.id() == id)
    }

    fn resolved_allocation_id_at(
        &self,
        tag: RenderFlowNameTag,
        request_time: RequestTime,
    ) -> FrameResourceResult<Option<FrameResourceAllocationId>> {
        if self.lifetime_solution.tag_lifetime(tag).is_none() {
            return Ok(None);
        }

        let Some(allocation_request_id) = self
            .lifetime_solution
            .lookup_allocation_for_tag(tag, request_time)?
        else {
            return Ok(None);
        };
        let assignment = self
            .pool_plan
            .assignment_for_request(allocation_request_id)
            .ok_or(FrameResourceError::InvalidState {
                reason: "allocation request was not assigned to a pool allocation",
            })?;

        Ok(Some(assignment.allocation_id()))
    }

    fn try_get_resource_at(
        &self,
        tag: RenderFlowNameTag,
        request_time: RequestTime,
    ) -> FrameResourceResult<Option<&FrameResource>> {
        let Some(allocation_id) = self.resolved_allocation_id_at(tag, request_time)? else {
            return Ok(None);
        };
        let allocation =
            self.allocation(allocation_id)
                .ok_or(FrameResourceError::InvalidState {
                    reason: "resolved pool allocation is missing",
                })?;

        Ok(Some(allocation.resource()))
    }

    fn resources_resolved(&self) -> bool {
        self.resources_resolved
    }
}

enum PendingExternalFrameResource {
    Texture {
        desc: FrameTextureDesc,
        texture: wgpu::Texture,
        default_view: wgpu::TextureView,
    },
    Buffer {
        desc: FrameBufferDesc,
        buffer: wgpu::Buffer,
    },
}

fn is_tag_declared_after_requests(requests: &[ResourceRequest], tag: RenderFlowNameTag) -> bool {
    let mut declared = false;
    for request in requests {
        match request {
            ResourceRequest::Declare {
                tag: declared_tag, ..
            }
            | ResourceRequest::Import {
                tag: declared_tag, ..
            } if *declared_tag == tag => {
                declared = true;
            }
            ResourceRequest::DeclareLike { dst, .. } if *dst == tag => {
                declared = true;
            }
            ResourceRequest::Free { tag: freed_tag } if *freed_tag == tag => {
                declared = false;
            }
            ResourceRequest::SwapWithExternal {
                tag: swapped_tag, ..
            } if *swapped_tag == tag => {
                declared = true;
            }
            _ => {}
        }
    }

    declared
}
