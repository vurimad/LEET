//! LEET Core - shared error and result types.

use std::{error::Error, fmt};

/// The central LEET error type.
#[derive(Debug)]
pub enum Leeror {
    Init(String),
    Config(String),
    Validation(String),
    MissingResource(String),
    Runtime(String),
    Unexpected(String),
    Io { source: std::io::Error },
}

impl fmt::Display for Leeror {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init(message) => write!(f, "Initialization failed: {message}"),
            Self::Config(message) => write!(f, "Configuration error: {message}"),
            Self::Validation(message) => write!(f, "Validation error: {message}"),
            Self::MissingResource(message) => write!(f, "Missing resource: {message}"),
            Self::Runtime(message) => write!(f, "Runtime error: {message}"),
            Self::Unexpected(message) => write!(f, "Unexpected error: {message}"),
            Self::Io { source } => write!(f, "I/O error: {source}"),
        }
    }
}

impl Error for Leeror {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Leeror {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

/// Engine-wide result alias.
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
