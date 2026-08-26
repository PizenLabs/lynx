//! Symbol identification primitives.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::ProtocolError;
use crate::language::Language;

/// Structural role of an identified code entity.
///
/// Wire form: each variant serializes as its identifier verbatim.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    /// Free function or procedure.
    Function,
    /// Function bound to a receiver or enclosing type.
    Method,
    /// Product type or record definition.
    Struct,
    /// Object-oriented class definition.
    Class,
    /// Behavioral contract declared separately from implementations (e.g. Rust `trait`).
    Trait,
    /// Interface declaration in interface-explicit languages (e.g. TypeScript `interface`).
    Interface,
    /// Sum type or enumerated definition.
    Enum,
    /// Namespace container: file, directory, or declared module.
    Module,
    /// Mutable named binding.
    Variable,
    /// Immutable named binding.
    Constant,
}

/// Stable identifier for a code entity across indexing runs.
///
/// Two identities compare equal only when every field matches; this makes the
/// type safe as a map key for cross-run deduplication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SymbolIdentity {
    /// Implementation language of the entity.
    pub language: Language,
    /// Fully qualified name, `::`-separated
    /// (e.g. `auth::service::AuthService::validate`).
    pub fqdn: String,
    /// Structural role of the entity.
    pub kind: SymbolKind,
    /// Workspace-relative path of the file declaring the entity.
    pub file_path: PathBuf,
    /// Hash of the entity's AST-node content, hex-encoded; digest algorithm is
    /// a producer decision and opaque to consumers.
    pub content_hash: String,
}

impl SymbolIdentity {
    /// Builds an identity, rejecting an empty [`Self::fqdn`].
    ///
    /// Direct struct literal construction bypasses validation and remains
    /// available to producers reading trusted data back from storage.
    pub fn new(
        language: Language,
        fqdn: impl Into<String>,
        kind: SymbolKind,
        file_path: PathBuf,
        content_hash: impl Into<String>,
    ) -> Result<Self, ProtocolError> {
        let fqdn = fqdn.into();
        if fqdn.is_empty() {
            return Err(ProtocolError::EmptyField { field: "fqdn" });
        }
        Ok(Self {
            language,
            fqdn,
            kind,
            file_path,
            content_hash: content_hash.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{SymbolIdentity, SymbolKind};
    use crate::error::ProtocolError;
    use crate::language::Language;
    use std::path::PathBuf;

    fn identity(fqdn: &str) -> Result<SymbolIdentity, ProtocolError> {
        SymbolIdentity::new(
            Language::Go,
            fqdn,
            SymbolKind::Method,
            PathBuf::from("internal/auth/service.go"),
            "deadbeef",
        )
    }

    #[test]
    fn equal_identities_deduplicate_in_a_set() -> Result<(), ProtocolError> {
        let mut set = std::collections::HashSet::new();
        assert!(set.insert(identity("auth::service::AuthService::validate")?));
        assert!(!set.insert(identity("auth::service::AuthService::validate")?));
        assert_eq!(set.len(), 1);
        Ok(())
    }

    #[test]
    fn rejects_empty_fqdn() {
        let err = match SymbolIdentity::new(
            Language::Rust,
            "",
            SymbolKind::Function,
            PathBuf::from("src/lib.rs"),
            "aa",
        ) {
            Err(err) => err,
            Ok(_) => panic!("empty fqdn accepted"),
        };
        assert!(
            matches!(err, ProtocolError::EmptyField { field: "fqdn" }),
            "unexpected error: {err:?}"
        );
    }
}
