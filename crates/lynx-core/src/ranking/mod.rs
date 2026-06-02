use lynx_protocol::{CodeChunk, DiscoveryResult};
use std::collections::HashMap;
use crate::classifier::QueryType;

pub struct Ranker;

struct ScoredChunk {
    chunk: CodeChunk,
    score: f32,
}

impl Ranker {
    pub fn rank(
        query: &str,
        query_type: QueryType,
        lexical_results: Vec<(CodeChunk, f32)>,
        semantic_results: Vec<(CodeChunk, f32)>,
        k: f32,
    ) -> Vec<DiscoveryResult> {
        let (alpha, beta) = query_type.weights();
        let mut scores: HashMap<String, ScoredChunk> = HashMap::new();

        for (i, (chunk, _score)) in lexical_results.into_iter().enumerate() {
            let entry = scores.entry(chunk.id.clone()).or_insert(ScoredChunk {
                chunk: chunk.clone(),
                score: 0.0,
            });
            entry.score += alpha / (k + i as f32);
        }

        for (i, (chunk, _score)) in semantic_results.into_iter().enumerate() {
            let entry = scores.entry(chunk.id.clone()).or_insert(ScoredChunk {
                chunk: chunk.clone(),
                score: 0.0,
            });
            entry.score += beta / (k + i as f32);
        }

        let mut scored_chunks: Vec<ScoredChunk> = scores.into_values().collect();
        apply_definition_boost(&mut scored_chunks);
        apply_identifier_boost(&mut scored_chunks, query);
        apply_noise_suppression(&mut scored_chunks);
        apply_file_coherence_boost(&mut scored_chunks);

        scored_chunks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        scored_chunks
            .into_iter()
            .map(|scored| DiscoveryResult {
                symbol_id: scored
                    .chunk
                    .symbols_defined
                    .first()
                    .cloned()
                    .unwrap_or_else(|| format!("file:{}", scored.chunk.file_path)),
                score: scored.score,
                file_path: scored.chunk.file_path,
                start_line: scored.chunk.start_line,
                end_line: scored.chunk.end_line,
            })
            .collect()
    }
}

fn apply_definition_boost(scored_chunks: &mut [ScoredChunk]) {
    for scored in scored_chunks {
        if !scored.chunk.symbols_defined.is_empty() {
            scored.score *= 1.5;
        }
    }
}

fn apply_identifier_boost(scored_chunks: &mut [ScoredChunk], query: &str) {
    let query_tokens: Vec<String> = query
        .split_whitespace()
        .map(|token| token.to_lowercase())
        .collect();

    for scored in scored_chunks {
        if scored.chunk.symbols_defined.is_empty() {
            continue;
        }

        let mut boosted = false;
        for symbol_id in &scored.chunk.symbols_defined {
            let symbol_name = symbol_id.split(':').last().unwrap_or(symbol_id);
            let symbol_name_lower = symbol_name.to_lowercase();
            if query_tokens.iter().any(|token| symbol_name_lower == *token || symbol_name_lower.contains(token)) {
                boosted = true;
                break;
            }
        }

        if boosted {
            scored.score *= 1.2;
        }
    }
}

fn apply_noise_suppression(scored_chunks: &mut [ScoredChunk]) {
    let noise_markers = [
        "node_modules",
        "vendor",
        "dist",
        "build",
        "generated",
        "testdata",
        "fixtures",
        "examples",
        "target",
        ".pb.go",
    ];

    for scored in scored_chunks {
        let path_lower = scored.chunk.file_path.to_lowercase();
        if noise_markers.iter().any(|marker| path_lower.contains(marker)) {
            scored.score *= 0.05;
        }
    }
}

fn apply_file_coherence_boost(scored_chunks: &mut [ScoredChunk]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for scored in scored_chunks.iter() {
        *counts.entry(scored.chunk.file_path.clone()).or_insert(0) += 1;
    }

    for scored in scored_chunks.iter_mut() {
        if let Some(count) = counts.get(&scored.chunk.file_path) {
            if *count > 1 {
                scored.score *= 1.0 + ((*count as f32 - 1.0) * 0.05);
            }
        }
    }
}
