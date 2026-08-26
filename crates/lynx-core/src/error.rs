//! Error contract for the core engine.

use thiserror::Error;

/// Every failure mode of the [`crate::Engine`] and its primitives.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Failure from the underlying storage substrate.
    #[error("storage failure: {0}")]
    Storage(#[from] lynx_storage::StorageError),
    /// Failure parsing a source file.
    #[error("parse failure: {0}")]
    Parse(#[from] lynx_parser::ParseError),
    /// Failure computing or comparing embeddings.
    #[error("embedding failure: {0}")]
    Embed(#[from] lynx_embed::EmbedError),
    /// Failure reading a file or walking the workspace.
    #[error("io failure: {0}")]
    Io(#[from] std::io::Error),
    /// Failure querying git for workspace provenance.
    #[error("git failure: {0}")]
    Git(String),
    /// The requested operation requires a populated index, but none exists.
    #[error("index not populated; run index_repository first")]
    NotIndexed,
    /// A provider returned a vector of an unexpected dimension.
    #[error("unexpected embedding dimension: expected {expected}, got {actual}")]
    DimensionMismatch {
        /// Expected dimension.
        expected: usize,
        /// Actual dimension returned.
        actual: usize,
    },
}
