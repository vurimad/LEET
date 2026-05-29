//! Frame resource request stream types.

use super::{
    FrameBufferDesc, FrameResourceDesc, FrameResourceError, FrameResourceResult, FrameTextureDesc,
    RenderFlowNameTag, ResourceAllocatorPhase, ResourceRequestId, ResourceUsage,
};

#[derive(Clone, Debug)]
pub enum ResourceRequest {
    Declare {
        tag: RenderFlowNameTag,
        desc: FrameResourceDesc,
    },
    DeclareLike {
        dst: RenderFlowNameTag,
        src: RenderFlowNameTag,
    },
    Import {
        tag: RenderFlowNameTag,
        resource: ImportedFrameResource,
    },
    IsDeclared {
        tag: RenderFlowNameTag,
        declared: bool,
    },
    UseBegin {
        tag: RenderFlowNameTag,
        usage: ResourceUsage,
    },
    UseEnd {
        tag: RenderFlowNameTag,
    },
    Free {
        tag: RenderFlowNameTag,
    },
    Swap {
        a: RenderFlowNameTag,
        b: RenderFlowNameTag,
    },
    SwapWithExternal {
        tag: RenderFlowNameTag,
        resource: ImportedFrameResource,
    },
    BeginQueue {
        queue: RenderQueueKind,
    },
    EndQueue,
    QueueSync {
        sync: QueueSyncKind,
    },
    Decision {
        value: bool,
    },
}

impl ResourceRequest {
    pub fn matches_replay(&self, recorded: &Self) -> bool {
        match (self, recorded) {
            (
                Self::Declare { tag, desc },
                Self::Declare {
                    tag: recorded_tag,
                    desc: recorded_desc,
                },
            ) => tag == recorded_tag && desc.is_exact_match(recorded_desc),
            (
                Self::DeclareLike { dst, src },
                Self::DeclareLike {
                    dst: recorded_dst,
                    src: recorded_src,
                },
            ) => dst == recorded_dst && src == recorded_src,
            (
                Self::Import { tag, resource },
                Self::Import {
                    tag: recorded_tag,
                    resource: recorded_resource,
                },
            ) => tag == recorded_tag && resource.matches_replay(recorded_resource),
            (
                Self::IsDeclared { tag, declared },
                Self::IsDeclared {
                    tag: recorded_tag,
                    declared: recorded_declared,
                },
            ) => tag == recorded_tag && declared == recorded_declared,
            (
                Self::UseBegin { tag, usage },
                Self::UseBegin {
                    tag: recorded_tag,
                    usage: recorded_usage,
                },
            ) => tag == recorded_tag && usage == recorded_usage,
            (Self::UseEnd { tag }, Self::UseEnd { tag: recorded_tag }) => tag == recorded_tag,
            (Self::Free { tag }, Self::Free { tag: recorded_tag }) => tag == recorded_tag,
            (
                Self::Swap { a, b },
                Self::Swap {
                    a: recorded_a,
                    b: recorded_b,
                },
            ) => a == recorded_a && b == recorded_b,
            (
                Self::SwapWithExternal { tag, resource },
                Self::SwapWithExternal {
                    tag: recorded_tag,
                    resource: recorded_resource,
                },
            ) => tag == recorded_tag && resource.matches_replay(recorded_resource),
            (
                Self::BeginQueue { queue },
                Self::BeginQueue {
                    queue: recorded_queue,
                },
            ) => queue == recorded_queue,
            (Self::EndQueue, Self::EndQueue) => true,
            (
                Self::QueueSync { sync },
                Self::QueueSync {
                    sync: recorded_sync,
                },
            ) => sync == recorded_sync,
            (Self::Decision { .. }, Self::Decision { .. }) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RequestGroup {
    requests: Vec<ResourceRequest>,
    consume_cursor: usize,
    touched: bool,
}

impl RequestGroup {
    pub fn new() -> Self {
        Self {
            requests: Vec::new(),
            consume_cursor: 0,
            touched: false,
        }
    }

    pub fn reset_for_preconsume(&mut self) {
        self.requests.clear();
        self.consume_cursor = 0;
        self.touched = false;
    }

    pub fn reset_consume_cursor(&mut self) {
        self.consume_cursor = 0;
        self.touched = false;
    }

    pub fn apply(
        &mut self,
        phase: ResourceAllocatorPhase,
        request: ResourceRequest,
    ) -> FrameResourceResult<RequestGroupAction> {
        match phase {
            ResourceAllocatorPhase::PreConsume => self.record_preconsume(request),
            ResourceAllocatorPhase::Consume => self.replay_consume(request),
            _ => Err(FrameResourceError::InvalidOperation {
                operation: "RequestGroup::apply",
                reason: "resource requests are only valid during pre-consume or consume",
            }),
        }
    }

    pub fn validate_consume_finished(&self) -> FrameResourceResult<()> {
        if self.consume_cursor == self.requests.len() {
            Ok(())
        } else {
            Err(FrameResourceError::InvalidState {
                reason: "consume did not replay every pre-consume resource request",
            })
        }
    }

    pub fn requests(&self) -> &[ResourceRequest] {
        &self.requests
    }

    pub fn consume_cursor(&self) -> usize {
        self.consume_cursor
    }

    pub fn touched(&self) -> bool {
        self.touched
    }

    pub fn is_consume_finished(&self) -> bool {
        self.consume_cursor == self.requests.len()
    }

    fn record_preconsume(
        &mut self,
        request: ResourceRequest,
    ) -> FrameResourceResult<RequestGroupAction> {
        let id = request_id_from_index(self.requests.len())?;
        self.requests.push(request);
        self.touched = true;
        Ok(RequestGroupAction::Recorded { id })
    }

    fn replay_consume(
        &mut self,
        request: ResourceRequest,
    ) -> FrameResourceResult<RequestGroupAction> {
        let Some(recorded) = self.requests.get(self.consume_cursor) else {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RequestGroup::replay_consume",
                reason: "consume replay produced more requests than pre-consume",
            });
        };

        if !request.matches_replay(recorded) {
            return Err(FrameResourceError::InvalidOperation {
                operation: "RequestGroup::replay_consume",
                reason: "consume request did not match pre-consume request",
            });
        }

        let id = request_id_from_index(self.consume_cursor)?;
        self.consume_cursor += 1;
        self.touched = true;
        Ok(RequestGroupAction::Replayed {
            id,
            recorded: recorded.clone(),
        })
    }
}

impl Default for RequestGroup {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub enum RequestGroupAction {
    Recorded {
        id: ResourceRequestId,
    },
    Replayed {
        id: ResourceRequestId,
        recorded: ResourceRequest,
    },
}

impl RequestGroupAction {
    pub fn id(&self) -> ResourceRequestId {
        match self {
            Self::Recorded { id } | Self::Replayed { id, .. } => *id,
        }
    }

    pub fn recorded_request(&self) -> Option<&ResourceRequest> {
        match self {
            Self::Recorded { .. } => None,
            Self::Replayed { recorded, .. } => Some(recorded),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImportedFrameResource {
    kind: FrameResourceKind,
    external_id: ExternalFrameResourceId,
    desc: FrameResourceDesc,
}

impl ImportedFrameResource {
    pub fn texture(external_id: ExternalFrameResourceId, desc: FrameTextureDesc) -> Self {
        Self {
            kind: FrameResourceKind::Texture,
            external_id,
            desc: FrameResourceDesc::Texture(desc),
        }
    }

    pub fn buffer(external_id: ExternalFrameResourceId, desc: FrameBufferDesc) -> Self {
        Self {
            kind: FrameResourceKind::Buffer,
            external_id,
            desc: FrameResourceDesc::Buffer(desc),
        }
    }

    pub fn kind(&self) -> FrameResourceKind {
        self.kind
    }

    pub fn external_id(&self) -> ExternalFrameResourceId {
        self.external_id
    }

    pub fn desc(&self) -> &FrameResourceDesc {
        &self.desc
    }

    fn matches_replay(&self, recorded: &Self) -> bool {
        self.kind == recorded.kind
            && self.external_id == recorded.external_id
            && self.desc.is_exact_match(&recorded.desc)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameResourceKind {
    Texture,
    Buffer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExternalFrameResourceId(u64);

impl ExternalFrameResourceId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderQueueKind {
    Graphics,
    Compute,
    Copy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueueSyncKind {
    Fork,
    Join,
    Barrier,
}

fn request_id_from_index(index: usize) -> FrameResourceResult<ResourceRequestId> {
    let index = u32::try_from(index).map_err(|_| FrameResourceError::InvalidState {
        reason: "resource request group exceeded u32 request id range",
    })?;
    Ok(ResourceRequestId::new(index))
}
