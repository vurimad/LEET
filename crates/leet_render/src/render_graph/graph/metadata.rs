//! Static render graph vocabulary.

use super::NodeGroupId;

/// Render node kind.
///
/// `Stage` is the ordinary executable graph node. The other variants carry
/// graph-merge semantics and must exist before graph import is implemented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RenderNodeKind {
    #[default]
    Stage,
    Unique,
    SequenceBegin,
    SequenceEnd,
    Temporary,
}

/// Structural role of a graph node.
///
/// This is independent from `RenderNodeKind`: for example, group entry/exit
/// anchors are `Stage` nodes by kind but have explicit roles so they are not
/// mistaken for removable temporary helpers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RenderNodeRole {
    #[default]
    Normal,
    GroupEntry(NodeGroupId),
    GroupExit(NodeGroupId),
    CommandListGroup,
    LifecycleSystem,
}

/// CPU or GPU dependency track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderNodeDependencyKind {
    Cpu,
    Gpu,
}

impl RenderNodeDependencyKind {
    pub const COUNT: usize = 2;
    pub const ALL: [Self; Self::COUNT] = [Self::Cpu, Self::Gpu];

    /// Stable compact index for arrays that store one value per dependency kind.
    pub const fn as_index(self) -> usize {
        match self {
            Self::Cpu => 0,
            Self::Gpu => 1,
        }
    }
}

/// How a node relates to command recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RenderNodeCommandListUsage {
    #[default]
    None,
    Require,
    Own,
    Sync,
}

impl RenderNodeCommandListUsage {
    /// Returns true for nodes that bind/use a normal command-list slot.
    pub const fn uses_command_list(self) -> bool {
        matches!(self, Self::Require | Self::Own)
    }

    /// Returns true when a node expects an existing command list from its owner.
    pub const fn requires_command_list(self) -> bool {
        matches!(self, Self::Require)
    }

    /// Returns true when a node creates/owns a command list for child work.
    pub const fn owns_command_list(self) -> bool {
        matches!(self, Self::Own)
    }

    /// Returns true for sync nodes handled by the graph runtime.
    pub const fn is_sync(self) -> bool {
        matches!(self, Self::Sync)
    }
}

/// Stable subtype used for unique/sequence merge rules and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RenderNodeSubtype(u32);

impl RenderNodeSubtype {
    pub const DEFAULT: Self = Self(0);

    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Human-readable node label used for diagnostics.
///
/// This is not graph identity. Graph identity comes from ids, kind, subtype,
/// role, and implementation/storage state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct RenderNodeDebugName(String);

impl RenderNodeDebugName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
