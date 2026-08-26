//! Tree provenance primitive.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Git and working-tree provenance snapshot attached to evidence.
///
/// Every [`crate::Evidence`] item carries one so downstream agents can pin
/// findings to an exact tree state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Snapshot {
    /// Resolved git commit hash, when the workspace is a repository;
    /// `None` outside version control.
    pub commit_hash: Option<String>,
    /// Root of the indexed workspace.
    pub workspace_root: PathBuf,
    /// Whether the working tree has uncommitted modifications.
    pub is_dirty: bool,
    /// Hex-encoded SHA-256 over the working-tree state; opaque to consumers.
    pub content_hash: String,
}

#[cfg(test)]
mod tests {
    use super::Snapshot;
    use std::path::PathBuf;

    #[test]
    fn roundtrips_without_commit() -> Result<(), serde_json::Error> {
        let snapshot = Snapshot {
            commit_hash: None,
            workspace_root: PathBuf::from("/tmp/demo-ws"),
            is_dirty: true,
            content_hash: "abc123".to_string(),
        };
        let json = serde_json::to_string(&snapshot)?;
        assert_eq!(
            json,
            r#"{"commit_hash":null,"workspace_root":"/tmp/demo-ws","is_dirty":true,"content_hash":"abc123"}"#
        );
        let decoded: Snapshot = serde_json::from_str(&json)?;
        assert_eq!(decoded, snapshot);
        Ok(())
    }
}
