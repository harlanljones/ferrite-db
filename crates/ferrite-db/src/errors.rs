//! Errors — the error taxonomy and its types.
//!
//! Every public fallible API returns these types (AGENTS.md §4, gate 1):
//! conditions the caller can fix map to [`ErrorClass::CallerFixable`]
//! variants; operational failures inside Ferrite DB or its storage map to
//! [`ErrorClass::Operational`]. No panic crosses the public API boundary.
//!
//! Owned by ROADMAP FDB-010.

use std::fmt;
use std::io;

/// Classification of an [`Error`]: who can act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// The caller can correct the condition and retry (bad input, unknown
    /// name, shed load).
    CallerFixable,
    /// Operational failure inside Ferrite DB or its storage; retrying with
    /// unchanged input will not help.
    Operational,
}

/// The error taxonomy for every public fallible Ferrite DB API.
///
/// ```
/// use ferrite_db::errors::{Error, ErrorClass};
///
/// let e = Error::DimensionMismatch {
///     expected: 512,
///     actual: 384,
/// };
/// assert_eq!(e.class(), ErrorClass::CallerFixable);
/// assert!(e.to_string().contains("512"));
/// ```
#[derive(Debug)]
pub enum Error {
    /// Search admission shed this call instead of queueing it (ADR 0007).
    /// Retryable after backoff; never a queue wait.
    Busy,
    /// The supplied vector's dimensionality does not match the Table's fixed
    /// dimension.
    DimensionMismatch {
        /// Dimension declared at Table creation.
        expected: u32,
        /// Dimension of the rejected vector or batch.
        actual: u32,
    },
    /// The input violates the Table's declared Metadata Schema.
    SchemaViolation {
        /// What was violated, e.g. unknown column or duplicate declaration.
        reason: String,
    },
    /// No Table with the given name exists in this process.
    TableNotFound {
        /// Name that was requested.
        name: String,
    },
    /// An underlying filesystem operation failed.
    Io(
        /// The source error from the filesystem layer.
        io::Error,
    ),
    /// A Segment file failed validation before use.
    CorruptSegment {
        /// What failed validation and where.
        detail: String,
    },
}

impl Error {
    /// Reports whether the condition is [`ErrorClass::CallerFixable`] or
    /// [`ErrorClass::Operational`].
    pub fn class(&self) -> ErrorClass {
        match self {
            Error::Busy
            | Error::DimensionMismatch { .. }
            | Error::SchemaViolation { .. }
            | Error::TableNotFound { .. } => ErrorClass::CallerFixable,
            Error::Io(_) | Error::CorruptSegment { .. } => ErrorClass::Operational,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Busy => write!(f, "search admission shed this call; retry later"),
            Error::DimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "dimension mismatch: table expects {expected}, got {actual}"
                )
            }
            Error::SchemaViolation { reason } => write!(f, "schema violation: {reason}"),
            Error::TableNotFound { name } => write!(f, "table not found: {name}"),
            Error::Io(source) => write!(f, "io error: {source}"),
            Error::CorruptSegment { detail } => write!(f, "corrupt segment: {detail}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Error::Io(source)
    }
}

/// Convenience alias so every public fallible signature reads uniformly.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn every_variant_constructs_and_classifies() {
        let cases = [
            (Error::Busy, ErrorClass::CallerFixable),
            (
                Error::DimensionMismatch {
                    expected: 512,
                    actual: 384,
                },
                ErrorClass::CallerFixable,
            ),
            (
                Error::SchemaViolation {
                    reason: "duplicate column".to_string(),
                },
                ErrorClass::CallerFixable,
            ),
            (
                Error::TableNotFound {
                    name: "embeddings".to_string(),
                },
                ErrorClass::CallerFixable,
            ),
            (
                Error::Io(io::Error::other("disk offline")),
                ErrorClass::Operational,
            ),
            (
                Error::CorruptSegment {
                    detail: "footer checksum mismatch".to_string(),
                },
                ErrorClass::Operational,
            ),
        ];
        for (error, want) in cases {
            assert_eq!(error.class(), want, "wrong class for {error}");
        }
    }

    #[test]
    fn display_is_actionable() {
        let e = Error::DimensionMismatch {
            expected: 512,
            actual: 384,
        };
        let s = e.to_string();
        assert!(s.contains("512") && s.contains("384"), "got: {s}");

        let e = Error::TableNotFound {
            name: "embeddings".to_string(),
        };
        assert!(e.to_string().contains("embeddings"));

        let e = Error::CorruptSegment {
            detail: "footer checksum mismatch".to_string(),
        };
        assert!(e.to_string().contains("footer checksum mismatch"));
    }

    #[test]
    fn only_io_exposes_a_source() {
        let io_error = io::Error::other("disk offline");
        let e: Error = io_error.into();
        assert!(e.source().is_some());

        let e = Error::Busy;
        assert!(e.source().is_none());
    }

    #[test]
    fn result_alias_defaults_to_taxonomy() {
        fn falls() -> Result<()> {
            Err(Error::Busy)
        }
        let r: Result<()> = falls();
        assert_eq!(r.unwrap_err().class(), ErrorClass::CallerFixable);
    }
}
