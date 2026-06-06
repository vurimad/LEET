//! Frame renderer error surface.

use std::{error::Error, fmt};

use crate::RenderGraphError;

pub type RenderFrameResult<T> = Result<T, RenderFrameError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderFrameError {
    InvalidFrameInput { reason: &'static str },
    LockPoisoned { resource: &'static str },
    MissingFrameTarget { reason: &'static str },
    NotImplemented { operation: &'static str },
    RenderGraph(RenderGraphError),
}

impl fmt::Display for RenderFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameInput { reason } => {
                write!(f, "invalid frame input: {reason}")
            }
            Self::LockPoisoned { resource } => {
                write!(f, "frame renderer lock was poisoned: {resource}")
            }
            Self::MissingFrameTarget { reason } => {
                write!(f, "missing frame target: {reason}")
            }
            Self::NotImplemented { operation } => {
                write!(
                    f,
                    "frame renderer operation is not implemented yet: {operation}"
                )
            }
            Self::RenderGraph(error) => {
                write!(f, "render graph error during frame rendering: {error}")
            }
        }
    }
}

impl Error for RenderFrameError {}

impl From<RenderGraphError> for RenderFrameError {
    fn from(error: RenderGraphError) -> Self {
        Self::RenderGraph(error)
    }
}
