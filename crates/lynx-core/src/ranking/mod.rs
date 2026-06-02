use lynx_protocol::{CodeChunk, DiscoveryResult};
use std::collections::HashMap;

pub struct Ranker;

impl Ranker {
    pub fn rrf(
        lexical_results: Vec<(CodeChunk, f32)>,
        semantic_results: Vec<(CodeChunk, f32)>,
        k: f32,
    ) -> Vec<DiscoveryResult> {
        let mut scores: HashMap<String, (f32, CodeChunk)> = HashMap::new();

        for (i, (chunk, _score)) in lexical_results.into_iter().enumerate() {
            let entry = scores.entry(chunk.id.clone()).or_insert((0.0, chunk.clone()));
            entry.0 += 1.0 / (k + i as f32);
        }

        for (i, (chunk, _score)) in semantic_results.into_iter().enumerate() {
            let entry = scores.entry(chunk.id.clone()).or_insert((0.0, chunk.clone()));
            entry.0 += 1.0 / (k + i as f32);
        }

        let mut final_results: Vec<_> = scores.into_iter().collect();
        final_results.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap());

        final_results
            .into_iter()
            .map(|(_id, (score, chunk))| DiscoveryResult {
                symbol_id: chunk.symbols_defined.first().cloned().unwrap_or_else(|| format!("file:{}", chunk.file_path)),
                score,
                file_path: chunk.file_path,
                start_line: chunk.start_line,
                end_line: chunk.end_line,
            })
            .collect()
    }
}
