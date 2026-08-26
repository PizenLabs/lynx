//! Storage layer for Lynx: a dual-layer persistence substrate.
//!
//! - [`GraphStore`] — SQLite (WAL) for relational identity
//!   ([`SymbolIdentity`]), the structural graph ([`Relation`]), and workspace
//!   provenance ([`Snapshot`]).
//! - [`TantivyStore`] — inverted lexical search over code chunks referencing
//!   symbol hashes stored in SQLite.
//!
//! [`DualStorage`] aggregates both under one root directory.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;
pub mod graph_store;
pub mod tantivy_store;

use std::path::Path;

use lynx_protocol::SymbolIdentity;

pub use error::StorageError;
pub use graph_store::GraphStore;
pub use tantivy_store::{LexicalDoc, LexicalHit, TantivyStore};

/// Aggregate handle over the graph and lexical substrates.
///
/// `open(root)` lays out:
/// - `root/graph.sqlite3` — the WAL-backed [`GraphStore`]
/// - `root/lexical/` — the [`TantivyStore`] index directory
pub struct DualStorage {
    graph: GraphStore,
    lexical: TantivyStore,
}

impl DualStorage {
    /// Opens (creating when absent) both substrates under `root`.
    pub fn open(root: &Path) -> Result<Self, StorageError> {
        std::fs::create_dir_all(root)?;
        Ok(Self {
            graph: GraphStore::open(&root.join("graph.sqlite3"))?,
            lexical: TantivyStore::open(&root.join("lexical"))?,
        })
    }

    /// Relational identity, structural graph, and snapshot store.
    pub fn graph(&self) -> &GraphStore {
        &self.graph
    }

    /// Lexical full-text index.
    pub fn lexical(&self) -> &TantivyStore {
        &self.lexical
    }

    /// Resolves a stored symbol identity by content hash through the graph.
    pub fn get_symbol_by_hash(&self, hash: &str) -> Result<Option<SymbolIdentity>, StorageError> {
        self.graph.get_symbol_by_hash(hash)
    }
}

// Compile-time guarantee: a DualStorage is safe to share across threads.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DualStorage>();
};
