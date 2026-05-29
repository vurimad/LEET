//! Frame resource allocator orchestration.

use crate::RenderDevice;

use super::{
    AllocationRequestSource, ExternalFrameResourceId, FrameBufferDesc, FrameBufferResource,
    FrameLifetimeSolution, FrameResource, FrameResourceAllocationClass, FrameResourceAllocationId,
    FrameResourceDiagnostics, FrameResourceError, FrameResourcePool, FrameResourcePoolPlan,
    FrameResourceResult, FrameTextureDesc, FrameTextureResource, RenderFlowAutoId, RenderFlowGroup,
    RenderFlowNameTag, RequestGroup, RequestGroupAction, RequestTime, ResourceAllocatorPhase,
    ResourceRequest,
};
use std::collections::HashMap;

pub struct FrameResourceAllocator {
    phase: ResourceAllocatorPhase,
    request_groups: Vec<RequestGroup>,
    lifetime_solution: Option<FrameLifetimeSolution>,
    pool_plan: Option<FrameResourcePoolPlan>,
    resource_pool: FrameResourcePool,
    external_resources: HashMap<ExternalFrameResourceId, PendingExternalFrameResource>,
    current_consume_time: Option<RequestTime>,
    resources_resolved: bool,
    caches_cleared_count: u32,
}

impl FrameResourceAllocator {
    pub fn new() -> Self {
        Self {
            phase: ResourceAllocatorPhase::Startup,
            request_groups: Vec::new(),
            lifetime_solution: None,
            pool_plan: None,
            resource_pool: FrameResourcePool::new(),
            external_resources: HashMap::new(),
            current_consume_time: None,
            resources_resolved: false,
            caches_cleared_count: 0,
        }
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
        let action = self.request_group_mut(flow_group).apply(phase, request)?;
        if phase == ResourceAllocatorPhase::Consume {
            self.current_consume_time = Some(RequestTime::new(flow_group, action.id().get()));
        }
        Ok(action)
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
                    operation: "FrameResourceAllocator::next_request_auto_id",
                    reason: "consume cannot generate a temp tag for an untouched flow group",
                })?,
            _ => {
                return Err(FrameResourceError::InvalidOperation {
                    operation: "FrameResourceAllocator::next_request_auto_id",
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
                operation: "FrameResourceAllocator::next_request_auto_id",
                reason: "temp tag id cannot pack this flow group/request index",
            });
        }

        RenderFlowAutoId::new((group << 16) | (index + 1))
    }

    pub fn request_group(&self, flow_group: RenderFlowGroup) -> Option<&RequestGroup> {
        self.request_groups.get(flow_group.get() as usize)
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

    pub fn diagnostics(&self) -> FrameResourceDiagnostics<'_> {
        FrameResourceDiagnostics::new(self)
    }

    pub fn lifetime_solution(&self) -> Option<&FrameLifetimeSolution> {
        self.lifetime_solution.as_ref()
    }

    pub fn pool_plan(&self) -> Option<&FrameResourcePoolPlan> {
        self.pool_plan.as_ref()
    }

    pub fn resource_pool(&self) -> &FrameResourcePool {
        &self.resource_pool
    }

    pub fn resource_pool_mut(&mut self) -> &mut FrameResourcePool {
        &mut self.resource_pool
    }

    pub fn resources_resolved(&self) -> bool {
        self.resources_resolved
    }

    pub fn resolved_allocation_id(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameResourceAllocationId>> {
        self.try_resolved_allocation_id(tag)
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
                operation: "FrameResourceAllocator::resolve_frame_resources",
                reason: "frame resources can only be materialized during resolve",
            });
        }

        let lifetime_solution =
            self.lifetime_solution
                .as_ref()
                .ok_or(FrameResourceError::InvalidState {
                    reason: "lifetime solution is missing during resolve",
                })?;
        let pool_plan = FrameResourcePoolPlan::plan_with_cached_allocations(
            lifetime_solution,
            &self.resource_pool.planner_candidates(),
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
                self.resource_pool.mark_used_this_frame_for_request(
                    assignment.allocation_id(),
                    allocation_request.desc(),
                )?;
                continue;
            }

            match allocation_request.source() {
                AllocationRequestSource::Owned => match allocation_request.desc() {
                    super::FrameResourceDesc::Texture(desc) => {
                        self.resource_pool.create_owned_texture(
                            assignment.allocation_id(),
                            desc.clone(),
                            render_device,
                        )?;
                    }
                    super::FrameResourceDesc::Buffer(desc) => {
                        self.resource_pool.create_owned_buffer(
                            assignment.allocation_id(),
                            desc.clone(),
                            render_device,
                        )?;
                    }
                },
                AllocationRequestSource::Imported(external_id) => {
                    self.attach_external_resource(
                        assignment.allocation_id(),
                        allocation_request.desc(),
                        external_id,
                        FrameResourceAllocationClass::Imported,
                    )?;
                }
                AllocationRequestSource::ExternalSwap(external_id) => {
                    self.attach_external_resource(
                        assignment.allocation_id(),
                        allocation_request.desc(),
                        external_id,
                        FrameResourceAllocationClass::ExternalSwap,
                    )?;
                }
            }

            if assignment.class() == FrameResourceAllocationClass::OwnedRestricted {
                self.resource_pool
                    .mark_non_cacheable(assignment.allocation_id())?;
            }
        }

        self.pool_plan = Some(pool_plan);
        self.resources_resolved = true;
        Ok(())
    }

    pub fn get_texture(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<&FrameTextureResource> {
        self.try_get_texture(tag)?
            .ok_or(FrameResourceError::InvalidOperation {
                operation: "FrameResourceAllocator::get_texture",
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
                operation: "FrameResourceAllocator::try_get_texture",
                reason: "resolved resource is a buffer, not a texture",
            }),
        }
    }

    pub fn get_buffer(&self, tag: RenderFlowNameTag) -> FrameResourceResult<&FrameBufferResource> {
        self.try_get_buffer(tag)?
            .ok_or(FrameResourceError::InvalidOperation {
                operation: "FrameResourceAllocator::get_buffer",
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
                operation: "FrameResourceAllocator::try_get_buffer",
                reason: "resolved resource is a texture, not a buffer",
            }),
            FrameResource::Buffer(buffer) => Ok(Some(buffer)),
        }
    }

    pub fn clear_all_caches(&mut self) -> FrameResourceResult<()> {
        if self.phase != ResourceAllocatorPhase::Cleanup {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameResourceAllocator::clear_all_caches",
                reason: "cache clearing is only valid during cleanup",
            });
        }

        self.caches_cleared_count += 1;
        self.resource_pool.clear_all_caches();
        Ok(())
    }

    pub fn caches_cleared_count(&self) -> u32 {
        self.caches_cleared_count
    }

    pub fn validate_resource_retrieval_phase(&self) -> FrameResourceResult<()> {
        if self.phase == ResourceAllocatorPhase::Consume {
            Ok(())
        } else {
            Err(FrameResourceError::InvalidOperation {
                operation: "FrameResourceAllocator::validate_resource_retrieval_phase",
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
                operation: "FrameResourceAllocator::set_phase",
                reason: "invalid frame resource allocator phase transition",
            })
        }
    }

    fn begin_preconsume(&mut self) {
        self.lifetime_solution = None;
        self.pool_plan = None;
        self.external_resources.clear();
        self.current_consume_time = None;
        self.resources_resolved = false;
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
        self.lifetime_solution = Some(lifetime_solution);
        self.pool_plan = Some(pool_plan);
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
        self.lifetime_solution = None;
        self.pool_plan = None;
        self.external_resources.clear();
        self.current_consume_time = None;
        self.resources_resolved = false;
        self.resource_pool.cleanup_after_frame();
        Ok(())
    }

    fn request_group_mut(&mut self, flow_group: RenderFlowGroup) -> &mut RequestGroup {
        let index = flow_group.get() as usize;
        while self.request_groups.len() <= index {
            self.request_groups.push(RequestGroup::new());
        }
        &mut self.request_groups[index]
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
                    operation: "FrameResourceAllocator::request_is_declared",
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
        let Some(allocation_id) = self.try_resolved_allocation_id(tag)? else {
            return Ok(None);
        };
        let allocation = self.resource_pool.allocation(allocation_id).ok_or(
            FrameResourceError::InvalidState {
                reason: "resolved pool allocation is missing",
            },
        )?;

        Ok(Some(allocation.resource()))
    }

    fn try_resolved_allocation_id(
        &self,
        tag: RenderFlowNameTag,
    ) -> FrameResourceResult<Option<FrameResourceAllocationId>> {
        self.validate_resource_retrieval_phase()?;
        if !self.resources_resolved {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameResourceAllocator::try_resolved_allocation_id",
                reason: "frame resources have not been materialized",
            });
        }

        let current_time = self
            .current_consume_time
            .ok_or(FrameResourceError::InvalidState {
                reason: "no current consume request time is available for resource retrieval",
            })?;
        let lifetime_solution =
            self.lifetime_solution
                .as_ref()
                .ok_or(FrameResourceError::InvalidState {
                    reason: "lifetime solution is missing during consume",
                })?;
        if lifetime_solution.tag_lifetime(tag).is_none() {
            return Ok(None);
        }
        let Some(allocation_request_id) =
            lifetime_solution.lookup_allocation_for_tag(tag, current_time)?
        else {
            return Ok(None);
        };
        let pool_plan = self
            .pool_plan
            .as_ref()
            .ok_or(FrameResourceError::InvalidState {
                reason: "pool plan is missing during consume",
            })?;
        let assignment = pool_plan
            .assignment_for_request(allocation_request_id)
            .ok_or(FrameResourceError::InvalidState {
                reason: "allocation request was not assigned to a pool allocation",
            })?;

        Ok(Some(assignment.allocation_id()))
    }

    fn register_external_resource(
        &mut self,
        external_id: ExternalFrameResourceId,
        resource: PendingExternalFrameResource,
    ) -> FrameResourceResult<()> {
        if self
            .external_resources
            .insert(external_id, resource)
            .is_some()
        {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameResourceAllocator::register_external_resource",
                reason: "external resource id was registered more than once",
            });
        }

        Ok(())
    }

    fn attach_external_resource(
        &mut self,
        allocation_id: FrameResourceAllocationId,
        expected_desc: &super::FrameResourceDesc,
        external_id: ExternalFrameResourceId,
        class: FrameResourceAllocationClass,
    ) -> FrameResourceResult<()> {
        let resource = self.external_resources.remove(&external_id).ok_or(
            FrameResourceError::InvalidOperation {
                operation: "FrameResourceAllocator::attach_external_resource",
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
                self.resource_pool
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
                self.resource_pool.insert_external_swap_texture(
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
                self.resource_pool
                    .import_buffer(allocation_id, desc, buffer)
            }
            (
                PendingExternalFrameResource::Buffer { desc, buffer },
                super::FrameResourceDesc::Buffer(expected),
                FrameResourceAllocationClass::ExternalSwap,
            ) if super::FrameResourceDesc::Buffer(desc.clone())
                .is_exact_match(&super::FrameResourceDesc::Buffer(expected.clone())) =>
            {
                self.resource_pool
                    .insert_external_swap_buffer(allocation_id, desc, buffer)
            }
            _ => Err(FrameResourceError::InvalidOperation {
                operation: "FrameResourceAllocator::attach_external_resource",
                reason:
                    "registered external resource does not match the requested kind or descriptor",
            }),
        }
    }
}

impl Default for FrameResourceAllocator {
    fn default() -> Self {
        Self::new()
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
