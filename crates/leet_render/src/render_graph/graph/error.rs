//! Shared error surface for render graph topology and execution.

use std::{error::Error, fmt};

use leet_core::Leeror;

use crate::render_graph::resources::FrameResourceError;

/// Result type used by render graph topology, factory, cache, and execution code.
pub type RenderGraphResult<T> = Result<T, RenderGraphError>;

/// Errors produced by graph-core operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderGraphError {
    /// A graph operation is represented in the API but not implemented yet.
    Unsupported { operation: &'static str },
    /// A typed graph id was invalid for the requested operation.
    InvalidId { kind: &'static str, raw: u32 },
    /// The graph already contains the requested dependency edge.
    DuplicateDependency {
        dependency_kind: &'static str,
        parent: u32,
        child: u32,
    },
    /// A node cannot depend on itself.
    SelfDependency {
        dependency_kind: &'static str,
        node: u32,
    },
    /// The graph was built/frozen, so topology mutation is no longer legal.
    GraphAlreadyBuilt { operation: &'static str },
    /// A CPU or GPU dependency cycle was found while building executable order.
    CycleDetected {
        dependency_kind: &'static str,
        remaining_nodes: usize,
    },
    /// Imported graph topology or special-node merge rules were invalid.
    InvalidMerge { reason: &'static str },
    /// Command-list group authoring was used in an invalid order or scope.
    InvalidCommandListGroupUsage {
        operation: &'static str,
        reason: &'static str,
    },
    /// Frame command recording was used in an invalid order or scope.
    InvalidCommandRecorderUsage {
        operation: &'static str,
        reason: &'static str,
    },
    /// A frame-resource request made through node processing failed.
    FrameResource(FrameResourceError),
    /// A graph operation was called in the wrong execution/build phase.
    InvalidExecutionPhase {
        operation: &'static str,
        phase: &'static str,
        expected: &'static str,
    },
    /// A graph invariant failed that is not tied to a narrower error variant.
    InvalidState { reason: &'static str },
}

impl fmt::Display for RenderGraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { operation } => {
                write!(f, "render graph operation is not supported yet: {operation}")
            }
            Self::InvalidId { kind, raw } => {
                write!(f, "invalid render graph {kind} id: {raw}")
            }
            Self::DuplicateDependency {
                dependency_kind,
                parent,
                child,
            } => write!(
                f,
                "duplicate {dependency_kind} dependency from node {parent} to node {child}"
            ),
            Self::SelfDependency {
                dependency_kind,
                node,
            } => write!(
                f,
                "invalid {dependency_kind} dependency: node {node} cannot depend on itself"
            ),
            Self::GraphAlreadyBuilt { operation } => {
                write!(
                    f,
                    "render graph operation `{operation}` cannot mutate a built graph"
                )
            }
            Self::CycleDetected {
                dependency_kind,
                remaining_nodes,
            } => write!(
                f,
                "{dependency_kind} dependency cycle detected with {remaining_nodes} unresolved nodes"
            ),
            Self::InvalidMerge { reason } => {
                write!(f, "invalid render graph merge: {reason}")
            }
            Self::InvalidCommandListGroupUsage { operation, reason } => write!(
                f,
                "invalid command-list group operation `{operation}`: {reason}"
            ),
            Self::InvalidCommandRecorderUsage { operation, reason } => write!(
                f,
                "invalid frame command recorder operation `{operation}`: {reason}"
            ),
            Self::FrameResource(error) => {
                write!(f, "frame resource error during render graph execution: {error}")
            }
            Self::InvalidExecutionPhase {
                operation,
                phase,
                expected,
            } => write!(
                f,
                "invalid render graph phase for `{operation}`: current phase is {phase}, expected {expected}"
            ),
            Self::InvalidState { reason } => {
                write!(f, "invalid render graph state: {reason}")
            }
        }
    }
}

impl Error for RenderGraphError {}

impl From<FrameResourceError> for RenderGraphError {
    fn from(error: FrameResourceError) -> Self {
        Self::FrameResource(error)
    }
}

impl From<RenderGraphError> for Leeror {
    fn from(error: RenderGraphError) -> Self {
        Self::Validation(format!("Render graph error: {error}"))
    }
}
