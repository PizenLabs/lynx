//! Error contract for the storage substrate.

/// Every failure mode of the storage layer.
///
/// All fallible storage APIs return `Result<_, StorageError>`; no `anyhow`
/// escapes this crate.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// SQLite backend failure (schema violation, FK constraint, I/O).
    #[error("sqlite failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A stored value violated a protocol invariant on reconstruction.
    #[error("protocol violation: {0}")]
    Protocol(#[from] lynx_protocol::ProtocolError),
    /// Failure to open the Tantivy index directory.
    #[error("lexical index open failure: {0}")]
    TantivyOpen(#[from] tantivy::directory::error::OpenDirectoryError),
    /// Tantivy lexical-index failure.
    #[error("lexical index failure: {0}")]
    Tantivy(#[from] tantivy::TantivyError),
    /// Tantivy query parse failure.
    #[error("lexical query parse failure: {0}")]
    QueryParse(#[from] tantivy::query::QueryParserError),
    /// Filesystem failure while preparing index or database directories.
    #[error("io failure: {0}")]
    Io(#[from] std::io::Error),
    /// A column held a value outside the protocol enum's wire form.
    #[error("unknown enum value in column `{0}`")]
    UnknownEnumValue(String),
    /// An internal `Mutex` guarding the SQLite connection was poisoned by a
    /// panic in another thread.
    #[error("graph store lock poisoned: {0}")]
    Poisoned(String),
}
