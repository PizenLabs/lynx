use crate::classifier::{classify_query, QueryType};
use crate::ranking::Ranker;
use crate::retrieval::Retriever;
use anyhow::Result;
use lynx_embed::EmbedderManager;
use lynx_protocol::DiscoveryResult;
use lynx_storage::Storage;

pub struct SearchPipeline<'a> {
    storage: &'a Storage,
    embedder: &'a EmbedderManager,
}

impl<'a> SearchPipeline<'a> {
    pub fn new(storage: &'a Storage, embedder: &'a EmbedderManager) -> Self {
        Self { storage, embedder }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<DiscoveryResult>> {
        let query_type = classify_query(query);

        if query_type == QueryType::Symbol {
            let symbol_results = self.storage.resolve_symbol_exact(query, 25)?;
            if !symbol_results.is_empty() {
                return Ok(symbol_results
                    .into_iter()
                    .map(|record| DiscoveryResult {
                        symbol_id: record.symbol_id,
                        score: 1.0,
                        file_path: record.file_path,
                        start_line: record.start_line,
                        end_line: record.end_line,
                        reasons: vec!["Exact symbol match".to_string()],
                    })
                    .collect());
            }
        }

        let retriever = Retriever::new(self.storage, self.embedder);
        let (lexical_results, semantic_results) = tokio::try_join!(
            retriever.retrieve_lexical(query, 50),
            retriever.retrieve_semantic(query, 50)
        )?;

        Ok(Ranker::rank(
            query,
            query_type,
            lexical_results,
            semantic_results,
            60.0,
        ))
    }
}
