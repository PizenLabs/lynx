use anyhow::Result;
use lynx_protocol::{CodeChunk, SymbolRecord};
use lynx_storage::Storage;
use lynx_embed::EmbedderManager;

pub struct Retriever<'a> {
    storage: &'a Storage,
    embedder: &'a EmbedderManager,
}

impl<'a> Retriever<'a> {
    pub fn new(storage: &'a Storage, embedder: &'a EmbedderManager) -> Self {
        Self { storage, embedder }
    }

    pub async fn retrieve_lexical(&self, query: &str, limit: usize) -> Result<Vec<(CodeChunk, f32)>> {
        // Tantivy returns docs, but doesn't easily return scores here with the current Storage API
        // I'll update Storage API to return scores
        Ok(self.storage.search_chunks_with_scores(query, limit)?)
    }

    pub async fn retrieve_semantic(&self, _query: &str, _limit: usize) -> Result<Vec<(CodeChunk, f32)>> {
        // For now, return empty or implement simple linear scan if we had the embeddings
        // Since we don't have a vector store yet, we'll skip this or do a mock
        Ok(vec![])
    }
}
