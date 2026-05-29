//! Shared error surface for the frame resource allocator.

use std::{error::Error, fmt};

pub type FrameResourceResult<T> = Result<T, FrameResourceError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameResourceError {
    Unsupported {
        operation: &'static str,
    },
    InvalidOperation {
        operation: &'static str,
        reason: &'static str,
    },
    InvalidState {
        reason: &'static str,
    },
}

impl fmt::Display for FrameResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { operation } => {
                write!(
                    f,
                    "frame resource operation is not supported yet: {operation}"
                )
            }
            Self::InvalidOperation { operation, reason } => {
                write!(
                    f,
                    "invalid frame resource operation `{operation}`: {reason}"
                )
            }
            Self::InvalidState { reason } => {
                write!(f, "invalid frame resource allocator state: {reason}")
            }
        }
    }
}

impl Error for FrameResourceError {}
