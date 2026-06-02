pub mod schema;
pub mod tantivy;
pub mod cache;

use anyhow::Result;
use lynx_protocol::{CodeChunk, SymbolRecord};
use std::path::Path;

pub struct Storage {
    inner: tantivy::TantivyStorage,
}

impl Storage {
    pub fn new(path: &Path) -> Result<Self> {
        Ok(Self {
            inner: tantivy::TantivyStorage::new(path)?,
        })
    }

    pub fn index_chunks(&self, chunks: &[CodeChunk]) -> Result<()> {
        self.inner.index_chunks(chunks)
    }

    pub fn index_symbols(&self, symbols: &[SymbolRecord]) -> Result<()> {
        self.inner.index_symbols(symbols)
    }

    pub fn search_chunks(&self, query: &str, limit: usize) -> Result<Vec<CodeChunk>> {
        self.inner.search_chunks(query, limit)
    }

    pub fn search_chunks_with_scores(&self, query: &str, limit: usize) -> Result<Vec<(CodeChunk, f32)>> {
        self.inner.search_chunks_with_scores(query, limit)
    }

    pub fn search_symbols(&self, query: &str, limit: usize) -> Result<Vec<SymbolRecord>> {
        self.inner.search_symbols(query, limit)
    }
}
