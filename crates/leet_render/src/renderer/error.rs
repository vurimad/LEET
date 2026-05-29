//! Frame renderer error surface.

use std::{error::Error, fmt};

pub type RenderFrameResult<T> = Result<T, RenderFrameError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderFrameError {
    InvalidFrameInput { reason: &'static str },
    LockPoisoned { resource: &'static str },
    MissingFrameTarget { reason: &'static str },
    NotImplemented { operation: &'static str },
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
        }
    }
}

impl Error for RenderFrameError {}
