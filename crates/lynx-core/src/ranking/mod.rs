use crate::classifier::QueryType;
use lynx_protocol::{CodeChunk, DiscoveryResult};
use std::collections::HashMap;

pub struct Ranker;

struct ScoredChunk {
    chunk: CodeChunk,
    score: f32,
    reasons: Vec<String>,
}

impl Ranker {
    pub fn rank(
        query: &str,
        query_type: QueryType,
        lexical_results: Vec<(CodeChunk, f32)>,
        semantic_results: Vec<(CodeChunk, f32)>,
        k: f32,
        include_tests: bool,
    ) -> Vec<DiscoveryResult> {
        let (alpha, beta) = query_type.weights();
        let mut scores: HashMap<String, ScoredChunk> = HashMap::new();

        for (i, (chunk, _score)) in lexical_results.into_iter().enumerate() {
            let entry = scores.entry(chunk.id.clone()).or_insert(ScoredChunk {
                chunk: chunk.clone(),
                score: 0.0,
                reasons: Vec::new(),
            });
            entry.score += alpha / (k + i as f32);
            if !entry.reasons.contains(&"Lexical match".to_string()) {
                entry.reasons.push("Lexical match".to_string());
            }
        }

        for (i, (chunk, _score)) in semantic_results.into_iter().enumerate() {
            let entry = scores.entry(chunk.id.clone()).or_insert(ScoredChunk {
                chunk: chunk.clone(),
                score: 0.0,
                reasons: Vec::new(),
            });
            entry.score += beta / (k + i as f32);
            if !entry.reasons.contains(&"Semantic match".to_string()) {
                entry.reasons.push("Semantic match".to_string());
            }
        }

        let mut scored_chunks: Vec<ScoredChunk> = scores.into_values().collect();

        // Noise suppression and filtering
        if !include_tests {
            scored_chunks.retain(|scored| {
                let path = scored.chunk.file_path.to_lowercase();
                !(path.contains("vendor")
                    || path.contains("node_modules")
                    || path.contains("mock")
                    || path.contains("test")
                    || path.contains("generated")
                    || path.contains(".pb.go")
                    || path.contains("fixtures")
                    || path.contains("examples"))
            });
        }
        
        apply_noise_suppression(&mut scored_chunks, include_tests);

        apply_definition_boost(&mut scored_chunks, query);
        apply_identifier_boost(&mut scored_chunks, query);

        // Apply concept-specific boost for natural language queries
        if let QueryType::NaturalLanguage = query_type {
            apply_concept_boost(&mut scored_chunks, query);
        }
        apply_file_coherence_boost(&mut scored_chunks);

        scored_chunks.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

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
                reasons: scored.reasons,
            })
            .collect()
    }
}

fn is_definition_of_query(chunk: &CodeChunk, query: &str) -> bool {
    let query_lower = query.to_lowercase();
    let query_tokens: Vec<&str> = query_lower.split_whitespace().collect();

    for symbol_id in &chunk.symbols_defined {
        let symbol_name = symbol_id
            .split(':')
            .next_back()
            .unwrap_or(symbol_id)
            .to_lowercase();

        if symbol_name == query_lower {
            return true;
        }

        // Handle Method.Name
        let name_parts: Vec<&str> = symbol_name.split('.').collect();
        for part in name_parts {
            if query_tokens.contains(&part) {
                return true;
            }
        }
    }
    false
}

fn apply_definition_boost(scored_chunks: &mut [ScoredChunk], query: &str) {
    for scored in scored_chunks {
        if is_definition_of_query(&scored.chunk, query) {
            scored.score *= 2.0; // Increased boost from 1.5 to 2.0
            scored.reasons.push("Definition boost".to_string());
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
            let symbol_name = symbol_id.split(':').next_back().unwrap_or(symbol_id);
            let symbol_name_lower = symbol_name.to_lowercase();
            if query_tokens
                .iter()
                .any(|token| symbol_name_lower == *token || symbol_name_lower.contains(token))
            {
                boosted = true;
                break;
            }
        }

        if boosted {
            scored.score *= 1.3; // Increased boost from 1.2 to 1.3
            scored.reasons.push("Identifier match boost".to_string());
        }
    }
}

fn apply_noise_suppression(scored_chunks: &mut [ScoredChunk], include_tests: bool) {
    if !include_tests {
        return;
    }
    for scored in scored_chunks {
        let path_lower = scored.chunk.file_path.to_lowercase();

        if path_lower.contains("vendor") || path_lower.contains("node_modules") {
            scored.score *= 0.01;
            scored
                .reasons
                .push("Vendor/node_modules penalty".to_string());
        } else if path_lower.contains("generated") || path_lower.contains(".pb.go") {
            scored.score *= 0.05;
            scored.reasons.push("Generated code penalty".to_string());
        } else if path_lower.ends_with("_test.go")
            || path_lower.ends_with(".test.ts")
            || path_lower.ends_with("_test.rs")
            || path_lower.ends_with("_test.py")
            || path_lower.contains("testdata")
        {
            scored.score *= if include_tests { 0.10 } else { 0.01 };
            scored.reasons.push("Test/testdata penalty".to_string());
        } else if path_lower.contains("mock") || path_lower.contains("test") {
            scored.score *= if include_tests { 0.20 } else { 0.05 };
            scored
                .reasons
                .push("Mock/Test directory penalty".to_string());
        } else if path_lower.contains("fixtures")
            || path_lower.contains("examples")
            || path_lower.contains("target")
            || path_lower.contains("build")
            || path_lower.contains("dist")
        {
            scored.score *= 0.10;
            scored
                .reasons
                .push("Fixtures/Examples/Build penalty".to_string());
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
                scored.reasons.push("File coherence boost".to_string());
            }
        }
    }
}

fn apply_concept_boost(scored_chunks: &mut [ScoredChunk], query: &str) {
    // Boost chunks for conceptual matches when query is natural language.
    let query_tokens: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    for scored in scored_chunks.iter_mut() {
        // Only consider chunks that have defined symbols.
        if scored.chunk.symbols_defined.is_empty() {
            continue;
        }
        // If any query token appears as a substring of a symbol name, boost strongly.
        let mut boosted = false;
        for symbol_id in &scored.chunk.symbols_defined {
            let symbol_name = symbol_id.split(':').next_back().unwrap_or(symbol_id);
            let symbol_name_lower = symbol_name.to_lowercase();
            if query_tokens
                .iter()
                .any(|tok| symbol_name_lower.contains(tok))
            {
                boosted = true;
                break;
            }
        }
        if boosted {
            scored.score *= 1.5; // Increased from 1.15 to 1.5 for stronger intent signal
            scored.reasons.push("Concept match boost".to_string());
        }
    }
}
