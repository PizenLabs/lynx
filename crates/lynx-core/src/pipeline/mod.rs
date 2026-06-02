use anyhow::Result;
use lynx_protocol::DiscoveryResult;
use lynx_storage::Storage;
use lynx_embed::EmbedderManager;
use crate::retrieval::Retriever;
use crate::ranking::Ranker;

pub struct SearchPipeline<'a> {
    storage: &'a Storage,
    embedder: &'a EmbedderManager,
}

impl<'a> SearchPipeline<'a> {
    pub fn new(storage: &'a Storage, embedder: &'a EmbedderManager) -> Self {
        Self { storage, embedder }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<DiscoveryResult>> {
        let retriever = Retriever::new(self.storage, self.embedder);
        
        let lexical_results = retriever.retrieve_lexical(query, 50).await?;
        let semantic_results = retriever.retrieve_semantic(query, 50).await?;

        let fused_results = Ranker::rrf(lexical_results, semantic_results, 60.0);

        Ok(fused_results)
    }
}
