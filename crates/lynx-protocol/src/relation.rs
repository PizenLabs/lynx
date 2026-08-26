//! Observed inter-symbol relations.

use serde::{Deserialize, Serialize};

/// Category of a directed edge between two symbols.
///
/// Wire form: each variant serializes as its identifier verbatim.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RelationKind {
    /// `source` invokes `target`.
    Calls,
    /// Inverse of [`RelationKind::Calls`]: `source` is invoked by `target`.
    CalledBy,
    /// `source` realizes the contract declared by `target`
    /// (trait/interface conformance).
    Implements,
    /// Inverse of [`RelationKind::Implements`].
    ImplementedBy,
    /// `source` depends on declarations exposed by `target`.
    Imports,
    /// `source` lexically or structurally owns `target`.
    Contains,
}

/// Observed directed relation between two symbols.
///
/// Endpoints are loose identifiers rather than full [`crate::SymbolIdentity`]s:
/// relations may reference symbols that are observed but not yet resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Relation {
    /// Identifier of the edge origin.
    pub source_id: String,
    /// Identifier of the edge destination.
    pub target_id: String,
    /// Category of the observed edge.
    pub kind: RelationKind,
}

#[cfg(test)]
mod tests {
    use super::{Relation, RelationKind};

    #[test]
    fn roundtrips_relation() -> Result<(), serde_json::Error> {
        let relation = Relation {
            source_id: "auth::service::AuthService::validate".to_string(),
            target_id: "auth::token::verify".to_string(),
            kind: RelationKind::Calls,
        };
        let json = serde_json::to_string(&relation)?;
        assert_eq!(
            json,
            r#"{"source_id":"auth::service::AuthService::validate","target_id":"auth::token::verify","kind":"Calls"}"#
        );
        let decoded: Relation = serde_json::from_str(&json)?;
        assert_eq!(decoded, relation);
        Ok(())
    }
}
