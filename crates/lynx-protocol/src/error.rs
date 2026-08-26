//! Error contracts raised when protocol primitives are constructed with
//! invalid data.
//!
//! Validation covers structural invariants only. This crate performs no
//! parsing, hashing, or I/O, so no other failure modes exist here.

use thiserror::Error;

/// Errors produced by fallible constructors of protocol primitives.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolError {
    /// A coordinate range regressed: an endpoint precedes its start.
    #[error(
        "invalid source range: lines {start_line}..={end_line}, bytes {start_byte}..={end_byte} must not regress"
    )]
    InvalidRange {
        /// First line of the offending range.
        start_line: usize,
        /// Last line of the offending range.
        end_line: usize,
        /// First byte offset of the offending range.
        start_byte: usize,
        /// End byte offset of the offending range.
        end_byte: usize,
    },
    /// An identity-carrying field that must contain content was empty.
    #[error("field `{field}` must not be empty")]
    EmptyField {
        /// Name of the offending field.
        field: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::ProtocolError;

    #[test]
    fn error_is_structurally_comparable() {
        let a = ProtocolError::EmptyField { field: "fqdn" };
        let b = ProtocolError::EmptyField { field: "fqdn" };
        assert_eq!(a, b);
    }
}
