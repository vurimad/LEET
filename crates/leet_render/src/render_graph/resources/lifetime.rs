//! Allocation requests and tag lifetime timelines.

use super::{
    AllocationRequestId, ExternalFrameResourceId, FrameResourceDesc, FrameResourceError,
    FrameResourceKind, FrameResourceResult, QueueSyncKind, RenderFlowGroup, RenderFlowNameTag,
    RequestGroup, RequestTime, ResourceRequest, ResourceUsage,
};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct FrameLifetimeSolution {
    allocation_requests: Vec<AllocationRequest>,
    tag_lifetimes: Vec<TagLifetime>,
}

impl FrameLifetimeSolution {
    pub fn solve_request_groups(request_groups: &[RequestGroup]) -> FrameResourceResult<Self> {
        LifetimeSolver::new(request_groups).solve()
    }

    pub fn allocation_requests(&self) -> &[AllocationRequest] {
        &self.allocation_requests
    }

    pub fn allocation_request(&self, id: AllocationRequestId) -> Option<&AllocationRequest> {
        self.allocation_requests
            .iter()
            .find(|request| request.id == id)
    }

    pub fn tag_lifetimes(&self) -> &[TagLifetime] {
        &self.tag_lifetimes
    }

    pub fn tag_lifetime(&self, tag: RenderFlowNameTag) -> Option<&TagLifetime> {
        self.tag_lifetimes
            .iter()
            .find(|lifetime| lifetime.tag() == tag)
    }

    pub fn lookup_allocation_for_tag(
        &self,
        tag: RenderFlowNameTag,
        time: RequestTime,
    ) -> FrameResourceResult<Option<AllocationRequestId>> {
        let Some(lifetime) = self.tag_lifetime(tag) else {
            return Err(FrameResourceError::InvalidOperation {
                operation: "FrameLifetimeSolution::lookup_allocation_for_tag",
                reason: "tag has no lifetime timeline",
            });
        };

        lifetime.lookup(time)
    }
}

#[derive(Clone, Debug)]
pub struct AllocationRequest {
    id: AllocationRequestId,
    tag: RenderFlowNameTag,
    desc: FrameResourceDesc,
    lifetime: RequestRange,
    source: AllocationRequestSource,
    can_reuse_same_frame: bool,
    can_cache_across_frames: bool,
}

impl AllocationRequest {
    pub fn id(&self) -> AllocationRequestId {
        self.id
    }

    pub fn tag(&self) -> RenderFlowNameTag {
        self.tag
    }

    pub fn desc(&self) -> &FrameResourceDesc {
        &self.desc
    }

    pub fn lifetime(&self) -> RequestRange {
        self.lifetime
    }

    pub fn source(&self) -> AllocationRequestSource {
        self.source
    }

    pub fn can_reuse_same_frame(&self) -> bool {
        self.can_reuse_same_frame
    }

    pub fn can_cache_across_frames(&self) -> bool {
        self.can_cache_across_frames
    }

    pub fn kind(&self) -> FrameResourceKind {
        match self.desc {
            FrameResourceDesc::Texture(_) => FrameResourceKind::Texture,
            FrameResourceDesc::Buffer(_) => FrameResourceKind::Buffer,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocationRequestSource {
    Owned,
    Imported(ExternalFrameResourceId),
    ExternalSwap(ExternalFrameResourceId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestRange {
    start: RequestTime,
    end: RequestTime,
}

impl RequestRange {
    pub fn new(start: RequestTime, end: RequestTime) -> FrameResourceResult<Self> {
        if end < start {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RequestRange::new",
                reason: "request range end precedes start",
            });
        }

        Ok(Self { start, end })
    }

    pub fn single(time: RequestTime) -> Self {
        Self {
            start: time,
            end: time,
        }
    }

    pub fn start(self) -> RequestTime {
        self.start
    }

    pub fn end(self) -> RequestTime {
        self.end
    }

    pub fn touches(self, time: RequestTime) -> bool {
        self.start <= time && time <= self.end
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }

    fn extend_to(&mut self, time: RequestTime) {
        if time < self.start {
            self.start = time;
        }
        if time > self.end {
            self.end = time;
        }
    }
}

#[derive(Clone, Debug)]
pub struct TagLifetime {
    tag: RenderFlowNameTag,
    events: Vec<TagLifetimeEvent>,
}

impl TagLifetime {
    pub fn tag(&self) -> RenderFlowNameTag {
        self.tag
    }

    pub fn events(&self) -> &[TagLifetimeEvent] {
        &self.events
    }

    pub fn lookup(&self, time: RequestTime) -> FrameResourceResult<Option<AllocationRequestId>> {
        let mut current = None;
        let mut saw_event = false;

        for event in &self.events {
            if event.time > time {
                break;
            }
            saw_event = true;
            current = event.allocation;
        }

        if saw_event {
            Ok(current)
        } else {
            Err(FrameResourceError::InvalidOperation {
                operation: "TagLifetime::lookup",
                reason: "lookup time precedes the tag lifetime timeline",
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagLifetimeEvent {
    time: RequestTime,
    allocation: Option<AllocationRequestId>,
    kind: TagLifetimeEventKind,
}

impl TagLifetimeEvent {
    pub fn time(self) -> RequestTime {
        self.time
    }

    pub fn allocation(self) -> Option<AllocationRequestId> {
        self.allocation
    }

    pub fn kind(self) -> TagLifetimeEventKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagLifetimeEventKind {
    Declare,
    DeclareLike,
    Import,
    Free,
    Swap,
    SwapWithExternal,
}

#[derive(Clone, Debug)]
struct ResourceInstance {
    original_tag: RenderFlowNameTag,
    desc: FrameResourceDesc,
    range: RequestRange,
    source: AllocationRequestSource,
    touched: bool,
    can_reuse_same_frame: bool,
    can_cache_across_frames: bool,
    open_use_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ResourceInstanceId(usize);

#[derive(Clone, Copy, Debug)]
struct PendingTagEvent {
    tag: RenderFlowNameTag,
    time: RequestTime,
    instance: Option<ResourceInstanceId>,
    kind: TagLifetimeEventKind,
}

struct LifetimeSolver<'a> {
    request_groups: &'a [RequestGroup],
    current: HashMap<RenderFlowNameTag, ResourceInstanceId>,
    instances: Vec<ResourceInstance>,
    events: Vec<PendingTagEvent>,
}

impl<'a> LifetimeSolver<'a> {
    fn new(request_groups: &'a [RequestGroup]) -> Self {
        Self {
            request_groups,
            current: HashMap::new(),
            instances: Vec::new(),
            events: Vec::new(),
        }
    }

    fn solve(mut self) -> FrameResourceResult<FrameLifetimeSolution> {
        for (group_index, request_group) in self.request_groups.iter().enumerate() {
            let flow_group = RenderFlowGroup::new(u16::try_from(group_index).map_err(|_| {
                FrameResourceError::InvalidState {
                    reason: "request group index exceeded u16 range",
                }
            })?);

            for (request_index, request) in request_group.requests().iter().enumerate() {
                let request_index =
                    u32::try_from(request_index).map_err(|_| FrameResourceError::InvalidState {
                        reason: "request index exceeded u32 range",
                    })?;
                self.handle_request(RequestTime::new(flow_group, request_index), request)?;
            }
        }

        for instance in &self.instances {
            if instance.open_use_count != 0 {
                return Err(FrameResourceError::InvalidState {
                    reason: "resource use begin was not balanced by use end",
                });
            }
        }

        self.finish()
    }

    fn handle_request(
        &mut self,
        time: RequestTime,
        request: &ResourceRequest,
    ) -> FrameResourceResult<()> {
        match request {
            ResourceRequest::Declare { tag, desc } => self.declare(
                *tag,
                desc.clone(),
                time,
                TagLifetimeEventKind::Declare,
                AllocationRequestSource::Owned,
                false,
            ),
            ResourceRequest::DeclareLike { dst, src } => {
                let source = self.current_instance(*src)?;
                let desc = self.instances[source.0].desc.clone();
                self.declare(
                    *dst,
                    desc,
                    time,
                    TagLifetimeEventKind::DeclareLike,
                    AllocationRequestSource::Owned,
                    false,
                )
            }
            ResourceRequest::Import { tag, resource } => self.declare(
                *tag,
                resource.desc().clone(),
                time,
                TagLifetimeEventKind::Import,
                AllocationRequestSource::Imported(resource.external_id()),
                true,
            ),
            ResourceRequest::IsDeclared { .. } | ResourceRequest::Decision { .. } => Ok(()),
            ResourceRequest::UseBegin { tag, usage } => self.use_begin(*tag, *usage, time),
            ResourceRequest::UseEnd { tag } => self.use_end(*tag, time),
            ResourceRequest::Free { tag } => self.free(*tag, time),
            ResourceRequest::Swap { a, b } => self.swap(*a, *b, time),
            ResourceRequest::SwapWithExternal { tag, resource } => {
                self.swap_with_external(*tag, resource.external_id(), resource.desc().clone(), time)
            }
            ResourceRequest::BeginQueue { .. } | ResourceRequest::EndQueue => {
                self.extend_open_uses(time);
                Ok(())
            }
            ResourceRequest::QueueSync { sync } => {
                if matches!(
                    sync,
                    QueueSyncKind::Fork | QueueSyncKind::Join | QueueSyncKind::Barrier
                ) {
                    self.extend_open_uses(time);
                }
                Ok(())
            }
        }
    }

    fn declare(
        &mut self,
        tag: RenderFlowNameTag,
        desc: FrameResourceDesc,
        time: RequestTime,
        kind: TagLifetimeEventKind,
        source: AllocationRequestSource,
        starts_touched: bool,
    ) -> FrameResourceResult<()> {
        if !tag.is_valid() {
            return Err(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::declare",
                reason: "cannot declare an invalid resource tag",
            });
        }
        if self.current.contains_key(&tag) {
            return Err(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::declare",
                reason: "resource tag was declared while already alive",
            });
        }

        desc.validate()?;

        let instance = ResourceInstanceId(self.instances.len());
        self.instances.push(ResourceInstance {
            original_tag: tag,
            desc,
            range: RequestRange::single(time),
            source,
            touched: starts_touched,
            can_reuse_same_frame: matches!(source, AllocationRequestSource::Owned),
            can_cache_across_frames: matches!(source, AllocationRequestSource::Owned),
            open_use_count: 0,
        });
        self.current.insert(tag, instance);
        self.events.push(PendingTagEvent {
            tag,
            time,
            instance: Some(instance),
            kind,
        });
        Ok(())
    }

    fn use_begin(
        &mut self,
        tag: RenderFlowNameTag,
        usage: ResourceUsage,
        time: RequestTime,
    ) -> FrameResourceResult<()> {
        if !usage.intersects(ResourceUsage::READ | ResourceUsage::WRITE) {
            return Err(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::use_begin",
                reason: "resource use must specify read or write usage",
            });
        }

        let instance = self.current_instance(tag)?;
        let instance = &mut self.instances[instance.0];
        instance.touched = true;
        instance.range.extend_to(time);
        instance.open_use_count += 1;
        Ok(())
    }

    fn use_end(&mut self, tag: RenderFlowNameTag, time: RequestTime) -> FrameResourceResult<()> {
        let instance = self.current_instance(tag)?;
        let instance = &mut self.instances[instance.0];
        if instance.open_use_count == 0 {
            return Err(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::use_end",
                reason: "resource use end had no matching use begin",
            });
        }

        instance.touched = true;
        instance.range.extend_to(time);
        instance.open_use_count -= 1;
        Ok(())
    }

    fn free(&mut self, tag: RenderFlowNameTag, time: RequestTime) -> FrameResourceResult<()> {
        let instance = self.current_instance(tag)?;
        if self.instances[instance.0].open_use_count != 0 {
            return Err(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::free",
                reason: "cannot free a resource while it has an open use",
            });
        }

        self.instances[instance.0].range.extend_to(time);
        self.current.remove(&tag);
        self.events.push(PendingTagEvent {
            tag,
            time,
            instance: None,
            kind: TagLifetimeEventKind::Free,
        });
        Ok(())
    }

    fn swap(
        &mut self,
        a: RenderFlowNameTag,
        b: RenderFlowNameTag,
        time: RequestTime,
    ) -> FrameResourceResult<()> {
        let a_instance = self.current_instance(a)?;
        let b_instance = self.current_instance(b)?;
        if !self.instances[a_instance.0]
            .desc
            .is_compatible_for_swap(&self.instances[b_instance.0].desc)
        {
            return Err(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::swap",
                reason: "swapped resources have incompatible descriptors",
            });
        }
        if self.instances[a_instance.0].open_use_count != 0
            || self.instances[b_instance.0].open_use_count != 0
        {
            return Err(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::swap",
                reason: "cannot swap resources while either tag has an open use",
            });
        }

        self.instances[a_instance.0].range.extend_to(time);
        self.instances[b_instance.0].range.extend_to(time);
        self.current.insert(a, b_instance);
        self.current.insert(b, a_instance);
        self.events.push(PendingTagEvent {
            tag: a,
            time,
            instance: Some(b_instance),
            kind: TagLifetimeEventKind::Swap,
        });
        self.events.push(PendingTagEvent {
            tag: b,
            time,
            instance: Some(a_instance),
            kind: TagLifetimeEventKind::Swap,
        });
        Ok(())
    }

    fn swap_with_external(
        &mut self,
        tag: RenderFlowNameTag,
        external_id: ExternalFrameResourceId,
        external_desc: FrameResourceDesc,
        time: RequestTime,
    ) -> FrameResourceResult<()> {
        let old_instance = self.current_instance(tag)?;
        if self.instances[old_instance.0].open_use_count != 0 {
            return Err(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::swap_with_external",
                reason: "cannot swap an external resource while the old tag has an open use",
            });
        }

        self.instances[old_instance.0].range.extend_to(time);
        self.instances[old_instance.0].can_reuse_same_frame = false;
        self.instances[old_instance.0].can_cache_across_frames = false;

        external_desc.validate()?;
        let external_instance = ResourceInstanceId(self.instances.len());
        self.instances.push(ResourceInstance {
            original_tag: tag,
            desc: external_desc,
            range: RequestRange::single(time),
            source: AllocationRequestSource::ExternalSwap(external_id),
            touched: true,
            can_reuse_same_frame: false,
            can_cache_across_frames: false,
            open_use_count: 0,
        });
        self.current.insert(tag, external_instance);
        self.events.push(PendingTagEvent {
            tag,
            time,
            instance: Some(external_instance),
            kind: TagLifetimeEventKind::SwapWithExternal,
        });
        Ok(())
    }

    fn extend_open_uses(&mut self, time: RequestTime) {
        for instance in &mut self.instances {
            if instance.open_use_count != 0 {
                instance.touched = true;
                instance.range.extend_to(time);
            }
        }
    }

    fn current_instance(&self, tag: RenderFlowNameTag) -> FrameResourceResult<ResourceInstanceId> {
        self.current
            .get(&tag)
            .copied()
            .ok_or(FrameResourceError::InvalidOperation {
                operation: "LifetimeSolver::current_instance",
                reason: "resource tag is not currently alive",
            })
    }

    fn finish(self) -> FrameResourceResult<FrameLifetimeSolution> {
        let mut instance_to_allocation = vec![None; self.instances.len()];
        let mut allocation_requests = Vec::new();

        for (instance_index, instance) in self.instances.iter().enumerate() {
            if !instance.touched {
                continue;
            }

            let allocation_index = u32::try_from(allocation_requests.len()).map_err(|_| {
                FrameResourceError::InvalidState {
                    reason: "allocation request index exceeded u32 range",
                }
            })?;
            let id = AllocationRequestId::new(allocation_index);
            instance_to_allocation[instance_index] = Some(id);
            allocation_requests.push(AllocationRequest {
                id,
                tag: instance.original_tag,
                desc: instance.desc.clone(),
                lifetime: instance.range,
                source: instance.source,
                can_reuse_same_frame: instance.can_reuse_same_frame,
                can_cache_across_frames: instance.can_cache_across_frames,
            });
        }

        let mut events_by_tag: HashMap<RenderFlowNameTag, Vec<TagLifetimeEvent>> = HashMap::new();
        for event in self.events {
            let allocation = event
                .instance
                .and_then(|instance| instance_to_allocation[instance.0]);
            events_by_tag
                .entry(event.tag)
                .or_default()
                .push(TagLifetimeEvent {
                    time: event.time,
                    allocation,
                    kind: event.kind,
                });
        }

        let mut tag_lifetimes = events_by_tag
            .into_iter()
            .map(|(tag, mut events)| {
                events.sort_by_key(|event| event.time);
                TagLifetime { tag, events }
            })
            .collect::<Vec<_>>();
        tag_lifetimes.sort_by_key(|lifetime| lifetime.tag);

        Ok(FrameLifetimeSolution {
            allocation_requests,
            tag_lifetimes,
        })
    }
}
