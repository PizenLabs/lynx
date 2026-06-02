pub mod chunking;
pub mod indexing;
pub mod retrieval;
pub mod ranking;
pub mod classifier;
pub mod pipeline;
pub mod models;

use anyhow::Result;
use std::path::Path;
use lynx_parser::Parser;
use lynx_storage::Storage;
use lynx_embed::EmbedderManager;
use lynx_embed::fastembed::FastEmbedder;

pub struct Lynx {
    parser: Parser,
    storage: Storage,
    embedder: EmbedderManager,
}

impl Lynx {
    pub async fn new(storage_path: &Path) -> Result<Self> {
        let parser = Parser::new();
        let storage = Storage::new(storage_path)?;
        let embedder = EmbedderManager::new(Box::new(FastEmbedder::new()?));

        Ok(Self {
            parser,
            storage,
            embedder,
        })
    }

    pub async fn index_repository(&self, repo_path: &Path) -> Result<()> {
        let mut indexer = indexing::Indexer::new(&self.parser, &self.storage);
        indexer.index_repository(repo_path).await
    }

    pub async fn search(&self, query: &str) -> Result<Vec<lynx_protocol::DiscoveryResult>> {
        let pipeline = pipeline::SearchPipeline::new(&self.storage, &self.embedder);
        pipeline.search(query).await
    }
}
