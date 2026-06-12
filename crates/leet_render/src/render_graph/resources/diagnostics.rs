//! Frame resource allocator diagnostics.

use super::{
    AllocationRequestSource, FrameResourceAllocationClass, FrameResourceDesc, FrameResourceKind,
    FrameResourceOwnership, FrameResourcePoolPlan, FrameResourceReuseRejectionReason,
    ImportedFrameResource, RenderFlowAutoId, RenderFlowSpace, RenderResourceAllocator, RequestTime,
    ResourceRequest, TagLifetimeEventKind,
};
use std::fmt::Write;

pub struct FrameResourceDiagnostics<'a> {
    allocator: &'a RenderResourceAllocator,
}

impl<'a> FrameResourceDiagnostics<'a> {
    pub fn new(allocator: &'a RenderResourceAllocator) -> Self {
        Self { allocator }
    }

    pub fn dump_frame_summary(&self) -> String {
        let request_count = self
            .allocator
            .request_groups()
            .iter()
            .map(|group| group.requests().len())
            .sum::<usize>();
        let allocation_request_count = self
            .allocator
            .lifetime_solution()
            .map(|solution| solution.allocation_requests().len())
            .unwrap_or(0);
        let pool_assignment_count = self
            .allocator
            .pool_plan()
            .map(|plan| plan.assignments().len())
            .unwrap_or(0);

        let mut out = String::new();
        let _ = writeln!(out, "frame_resource_summary");
        let _ = writeln!(out, "  phase={:?}", self.allocator.phase());
        let _ = writeln!(
            out,
            "  request_groups={}",
            self.allocator.request_group_count()
        );
        let _ = writeln!(out, "  requests={request_count}");
        let _ = writeln!(out, "  allocation_requests={allocation_request_count}");
        let _ = writeln!(out, "  pool_assignments={pool_assignment_count}");
        let _ = writeln!(
            out,
            "  pool_allocations={}",
            self.allocator.resource_pool().allocations().len()
        );
        let _ = writeln!(
            out,
            "  resources_resolved={}",
            self.allocator.resources_resolved()
        );
        let _ = writeln!(
            out,
            "  caches_cleared={}",
            self.allocator.caches_cleared_count()
        );
        out
    }

    pub fn dump_request_stream(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "request_stream phase={:?}", self.allocator.phase());

        for (group_index, group) in self.allocator.request_groups().iter().enumerate() {
            let _ = writeln!(
                out,
                "  group={group_index} requests={} consume_cursor={} touched={} finished={}",
                group.requests().len(),
                group.consume_cursor(),
                group.touched(),
                group.is_consume_finished()
            );
            for (index, request) in group.requests().iter().enumerate() {
                let _ = writeln!(
                    out,
                    "    group={group_index} index={index} kind={} tag={} {}",
                    request_kind(request),
                    request_primary_tag(request),
                    request_detail(request)
                );
            }
        }

        out
    }

    pub fn replay_mismatch_report(
        &self,
        group_index: usize,
        request_index: usize,
        expected: &ResourceRequest,
        actual: &ResourceRequest,
    ) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "replay_mismatch");
        let _ = writeln!(out, "  group={group_index}");
        let _ = writeln!(out, "  index={request_index}");
        let _ = writeln!(out, "  expected_kind={}", request_kind(expected));
        let _ = writeln!(out, "  expected_tag={}", request_primary_tag(expected));
        let _ = writeln!(out, "  expected={}", request_detail(expected));
        let _ = writeln!(out, "  actual_kind={}", request_kind(actual));
        let _ = writeln!(out, "  actual_tag={}", request_primary_tag(actual));
        let _ = writeln!(out, "  actual={}", request_detail(actual));
        out
    }

    pub fn dump_lifetimes(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "lifetimes");
        let Some(solution) = self.allocator.lifetime_solution() else {
            let _ = writeln!(out, "  unavailable=true");
            return out;
        };

        for request in solution.allocation_requests() {
            let _ = writeln!(
                out,
                "  allocation_request={} tag={} kind={} source={} start={} end={} reuse_same_frame={} cache_across_frames={} desc_kind={}",
                request.id().get(),
                format_tag(request.tag()),
                format_kind(request.kind()),
                format_source(request.source()),
                format_time(request.lifetime().start()),
                format_time(request.lifetime().end()),
                request.can_reuse_same_frame(),
                request.can_cache_across_frames(),
                format_desc_kind(request.desc())
            );
        }

        out
    }

    pub fn dump_tag_timelines(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "tag_timelines");
        let Some(solution) = self.allocator.lifetime_solution() else {
            let _ = writeln!(out, "  unavailable=true");
            return out;
        };

        for timeline in solution.tag_lifetimes() {
            let _ = writeln!(out, "  tag={}", format_tag(timeline.tag()));
            for event in timeline.events() {
                let allocation = event
                    .allocation()
                    .map(|id| id.get().to_string())
                    .unwrap_or_else(|| "none".to_string());
                let _ = writeln!(
                    out,
                    "    time={} kind={} allocation={allocation}",
                    format_time(event.time()),
                    format_event_kind(event.kind())
                );
            }
        }

        out
    }

    pub fn dump_pool(&self) -> String {
        let mut out = String::new();
        let pool = self.allocator.resource_pool();
        let _ = writeln!(
            out,
            "pool allocations={} max_unused_age={}",
            pool.allocations().len(),
            pool.max_unused_age()
        );

        for allocation in pool.allocations() {
            let _ = writeln!(
                out,
                "  allocation={} ownership={} resource_kind={} desc_kind={} used_this_frame={} cacheable={} age={} overcapacity_age={}",
                allocation.id().get(),
                format_ownership(allocation.ownership()),
                format_resource_kind(allocation.resource()),
                format_desc_kind(allocation.desc()),
                allocation.used_this_frame(),
                allocation.cacheable(),
                allocation.age(),
                allocation.overcapacity_age()
            );
        }

        out
    }

    pub fn dump_reuse_decisions(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "reuse_decisions");
        let Some(plan) = self.allocator.pool_plan() else {
            let _ = writeln!(out, "  unavailable=true");
            return out;
        };

        append_pool_plan(&mut out, &plan);
        out
    }

    pub fn dump_eviction_state(&self) -> String {
        let mut out = String::new();
        let pool = self.allocator.resource_pool();
        let _ = writeln!(
            out,
            "eviction_state max_unused_age={}",
            pool.max_unused_age()
        );

        for allocation in pool.allocations() {
            let evicts_on_next_cleanup = allocation.ownership() != FrameResourceOwnership::Owned
                || !allocation.cacheable()
                || (!allocation.used_this_frame() && allocation.age() >= pool.max_unused_age())
                || allocation.overcapacity_age() > pool.max_unused_age();
            let _ = writeln!(
                out,
                "  allocation={} ownership={} cacheable={} used_this_frame={} age={} overcapacity_age={} evicts_on_next_cleanup={evicts_on_next_cleanup}",
                allocation.id().get(),
                format_ownership(allocation.ownership()),
                allocation.cacheable(),
                allocation.used_this_frame(),
                allocation.age(),
                allocation.overcapacity_age()
            );
        }

        out
    }

    pub fn dump_oversized_cached_allocations_for(
        &self,
        requested_desc: &FrameResourceDesc,
    ) -> String {
        let mut out = String::new();
        let oversized = self
            .allocator
            .resource_pool()
            .oversized_cached_allocations_for(requested_desc);
        let _ = writeln!(
            out,
            "oversized_cached_allocations requested_desc_kind={} count={}",
            format_desc_kind(requested_desc),
            oversized.len()
        );
        for id in oversized {
            let _ = writeln!(out, "  allocation={}", id.get());
        }
        out
    }
}

fn append_pool_plan(out: &mut String, plan: &FrameResourcePoolPlan) {
    let order = plan
        .assignment_order()
        .iter()
        .map(|id| id.get().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let _ = writeln!(out, "  assignment_order=[{order}]");

    for assignment in plan.assignments() {
        let _ = writeln!(
            out,
            "  assignment request={} allocation={} class={} reused_existing={}",
            assignment.request_id().get(),
            assignment.allocation_id().get(),
            format_allocation_class(assignment.class()),
            assignment.reused_existing()
        );
    }

    for rejection in plan.rejections() {
        let _ = writeln!(
            out,
            "  rejection request={} candidate={} reason={}",
            rejection.request_id().get(),
            rejection.candidate_allocation_id().get(),
            format_rejection_reason(rejection.reason())
        );
    }
}

fn request_kind(request: &ResourceRequest) -> &'static str {
    match request {
        ResourceRequest::Declare { .. } => "Declare",
        ResourceRequest::DeclareLike { .. } => "DeclareLike",
        ResourceRequest::Import { .. } => "Import",
        ResourceRequest::IsDeclared { .. } => "IsDeclared",
        ResourceRequest::UseBegin { .. } => "UseBegin",
        ResourceRequest::UseEnd { .. } => "UseEnd",
        ResourceRequest::Free { .. } => "Free",
        ResourceRequest::Swap { .. } => "Swap",
        ResourceRequest::SwapWithExternal { .. } => "SwapWithExternal",
        ResourceRequest::BeginQueue { .. } => "BeginQueue",
        ResourceRequest::EndQueue => "EndQueue",
        ResourceRequest::QueueSync { .. } => "QueueSync",
        ResourceRequest::Decision { .. } => "Decision",
    }
}

fn request_primary_tag(request: &ResourceRequest) -> String {
    match request {
        ResourceRequest::Declare { tag, .. }
        | ResourceRequest::Import { tag, .. }
        | ResourceRequest::IsDeclared { tag, .. }
        | ResourceRequest::UseBegin { tag, .. }
        | ResourceRequest::UseEnd { tag }
        | ResourceRequest::Free { tag }
        | ResourceRequest::SwapWithExternal { tag, .. } => format_tag(*tag),
        ResourceRequest::DeclareLike { dst, .. } => format_tag(*dst),
        ResourceRequest::Swap { a, .. } => format_tag(*a),
        ResourceRequest::BeginQueue { .. }
        | ResourceRequest::EndQueue
        | ResourceRequest::QueueSync { .. }
        | ResourceRequest::Decision { .. } => "-".to_string(),
    }
}

fn request_detail(request: &ResourceRequest) -> String {
    match request {
        ResourceRequest::Declare { tag, desc } => {
            format!(
                "tag={} desc_kind={}",
                format_tag(*tag),
                format_desc_kind(desc)
            )
        }
        ResourceRequest::DeclareLike { dst, src } => {
            format!("dst={} src={}", format_tag(*dst), format_tag(*src))
        }
        ResourceRequest::Import { tag, resource } => {
            format!(
                "tag={} {}",
                format_tag(*tag),
                format_imported_resource(resource)
            )
        }
        ResourceRequest::IsDeclared { tag, declared } => {
            format!("tag={} declared={declared}", format_tag(*tag))
        }
        ResourceRequest::UseBegin { tag, usage } => {
            format!("tag={} usage={usage:?}", format_tag(*tag))
        }
        ResourceRequest::UseEnd { tag } => format!("tag={}", format_tag(*tag)),
        ResourceRequest::Free { tag } => format!("tag={}", format_tag(*tag)),
        ResourceRequest::Swap { a, b } => {
            format!("a={} b={}", format_tag(*a), format_tag(*b))
        }
        ResourceRequest::SwapWithExternal { tag, resource } => {
            format!(
                "tag={} {}",
                format_tag(*tag),
                format_imported_resource(resource)
            )
        }
        ResourceRequest::BeginQueue { queue } => format!("queue={queue:?}"),
        ResourceRequest::EndQueue => String::new(),
        ResourceRequest::QueueSync { sync } => format!("sync={sync:?}"),
        ResourceRequest::Decision { value } => format!("value={value}"),
    }
}

fn format_imported_resource(resource: &ImportedFrameResource) -> String {
    format!(
        "external_id={} kind={} desc_kind={}",
        resource.external_id().get(),
        format_kind(resource.kind()),
        format_desc_kind(resource.desc())
    )
}

fn format_tag(tag: super::RenderFlowNameTag) -> String {
    if !tag.is_valid() {
        return "invalid".to_string();
    }

    let auto_id = if tag.auto_id() == RenderFlowAutoId::NONE {
        "none".to_string()
    } else {
        format!("temp:{}", tag.auto_id().get())
    };
    format!(
        "hash={:#010x} flow_space={} auto_id={auto_id}",
        tag.hash(),
        format_flow_space(tag.flow_space())
    )
}

fn format_flow_space(flow_space: RenderFlowSpace) -> String {
    match flow_space {
        RenderFlowSpace::SHARED => "shared".to_string(),
        RenderFlowSpace::AUTOGENERATED => "autogenerated".to_string(),
        _ => format!("camera:{}", flow_space.get()),
    }
}

fn format_time(time: RequestTime) -> String {
    format!(
        "group:{} index:{}",
        time.flow_group().get(),
        time.index_in_group()
    )
}

fn format_source(source: AllocationRequestSource) -> String {
    match source {
        AllocationRequestSource::Owned => "Owned".to_string(),
        AllocationRequestSource::Imported(id) => format!("Imported({})", id.get()),
        AllocationRequestSource::ExternalSwap(id) => format!("ExternalSwap({})", id.get()),
    }
}

fn format_event_kind(kind: TagLifetimeEventKind) -> &'static str {
    match kind {
        TagLifetimeEventKind::Declare => "Declare",
        TagLifetimeEventKind::DeclareLike => "DeclareLike",
        TagLifetimeEventKind::Import => "Import",
        TagLifetimeEventKind::Free => "Free",
        TagLifetimeEventKind::Swap => "Swap",
        TagLifetimeEventKind::SwapWithExternal => "SwapWithExternal",
    }
}

fn format_kind(kind: FrameResourceKind) -> &'static str {
    match kind {
        FrameResourceKind::Texture => "Texture",
        FrameResourceKind::Buffer => "Buffer",
    }
}

fn format_desc_kind(desc: &FrameResourceDesc) -> &'static str {
    match desc {
        FrameResourceDesc::Texture(_) => "Texture",
        FrameResourceDesc::Buffer(_) => "Buffer",
    }
}

fn format_resource_kind(resource: &super::FrameResource) -> &'static str {
    match resource {
        super::FrameResource::Texture(_) => "Texture",
        super::FrameResource::Buffer(_) => "Buffer",
    }
}

fn format_ownership(ownership: FrameResourceOwnership) -> &'static str {
    match ownership {
        FrameResourceOwnership::Owned => "Owned",
        FrameResourceOwnership::Imported => "Imported",
        FrameResourceOwnership::ExternalSwap => "ExternalSwap",
    }
}

fn format_allocation_class(class: FrameResourceAllocationClass) -> &'static str {
    match class {
        FrameResourceAllocationClass::OwnedReusable => "OwnedReusable",
        FrameResourceAllocationClass::OwnedRestricted => "OwnedRestricted",
        FrameResourceAllocationClass::Imported => "Imported",
        FrameResourceAllocationClass::ExternalSwap => "ExternalSwap",
    }
}

fn format_rejection_reason(reason: FrameResourceReuseRejectionReason) -> &'static str {
    match reason {
        FrameResourceReuseRejectionReason::CandidateNotReusable => "CandidateNotReusable",
        FrameResourceReuseRejectionReason::DescriptorIncompatible => "DescriptorIncompatible",
        FrameResourceReuseRejectionReason::LifetimeOverlaps => "LifetimeOverlaps",
        FrameResourceReuseRejectionReason::RequestNotReusable => "RequestNotReusable",
    }
}
