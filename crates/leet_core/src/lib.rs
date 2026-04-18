//! LEET Core - Fundamental types, traits, and utilities
//!
//! This crate provides the foundation for the LEET game engine.

pub mod engine_clock;

pub use engine_clock::EngineClock;

use thiserror::Error;

/// The central LEET error type.
///
/// All engine crates return this type (or convert into it).
/// Game developers receive this via `anyhow::Result` in the `leet` prelude.
#[derive(Debug, Error)]
pub enum Leeror {
    #[error("Initialization failed: {0}")]
    Init(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Missing resource: {0}")]
    MissingResource(String),

    #[error("Runtime error: {0}")]
    Runtime(String),

    #[error("Unexpected error: {0}")]
    Unexpected(String),

    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

/// Engine-wide Result alias.
pub type LeetResult<T> = std::result::Result<T, Leeror>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leeror_display() {
        assert_eq!(
            Leeror::Init("bad config".into()).to_string(),
            "Initialization failed: bad config"
        );
        assert_eq!(
            Leeror::Runtime("loop crash".into()).to_string(),
            "Runtime error: loop crash"
        );
    }

    #[test]
    fn test_io_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let leeror: Leeror = io_err.into();
        assert!(leeror.to_string().contains("missing file"));
    }
}
