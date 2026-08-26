use crate::classifier::QueryIntent;
use lynx_protocol::{CodeChunk, DiscoveryResult};
use std::collections::HashMap;

pub const MIN_CONFIDENCE_THRESHOLD: f32 = 0.15;

pub struct Ranker;

struct ScoredChunk {
    chunk: CodeChunk,
    score: f32,
    reasons: Vec<String>,
}

impl Ranker {
    pub fn rank(
        query: &str,
        query_intent: QueryIntent,
        lexical_results: Vec<(CodeChunk, f32)>,
        semantic_results: Vec<(CodeChunk, f32)>,
        k: f32,
        include_tests: bool,
    ) -> Vec<DiscoveryResult> {
        let (alpha, beta) = query_intent.weights();
        let mut scores: HashMap<String, ScoredChunk> = HashMap::new();

        // Process lexical candidate hits into the unified scoring tracker
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

        // Process semantic candidate hits into the unified scoring tracker
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

        // Noise suppression and active workspace path filtering
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

        // Apply baseline system infrastructure penalties
        apply_noise_suppression(&mut scored_chunks);

        // Apply targeted syntax structure match boosts
        apply_definition_boost(&mut scored_chunks, query);
        apply_identifier_boost(&mut scored_chunks, query);

        // Apply intent-specific heuristic transformations
        match query_intent {
            QueryIntent::Semantic | QueryIntent::Flow | QueryIntent::Architecture => {
                apply_concept_boost(&mut scored_chunks, query);
                apply_intent_boost(&mut scored_chunks);

                // CRITICAL FIX: Explicitly invoke generic boilerplate mitigation
                // to aggressively suppress infrastructure symbols (main, new, init)
                apply_generic_symbol_penalty(&mut scored_chunks);
            }
            _ => {}
        }

        // Execute structural package/file grouping reinforcement
        apply_file_coherence_boost(&mut scored_chunks);

        // Normalize computed RRF scores and modifiers to a stable [0.0, 1.0] confidence scale
        let max_rrf = 1.0 / k;
        for scored in scored_chunks.iter_mut() {
            // Calibrate score distribution against theoretical max boundaries and cap at 1.0
            scored.score = (scored.score / (max_rrf * 2.0)).min(1.0);
        }

        // In-place filtration to discard low-confidence noise rows
        scored_chunks.retain(|scored| scored.score >= MIN_CONFIDENCE_THRESHOLD);

        // Sort descending based on final normalized confidence
        scored_chunks.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Map internal structural trackers back to external protocol data transfer objects (DTOs)
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

fn apply_intent_boost(scored_chunks: &mut [ScoredChunk]) {
    for scored in scored_chunks.iter_mut() {
        for symbol_id in &scored.chunk.symbols_defined {
            let symbol_name = symbol_id
                .split(':')
                .next_back()
                .unwrap_or(symbol_id)
                .to_lowercase();

            // Heuristic boost for potential service entry points, handlers, and public interfaces
            if symbol_name.contains("service")
                || symbol_name.contains("handler")
                || symbol_name.contains("controller")
                || symbol_name.contains("router")
                || symbol_name.contains("interface")
                || symbol_name.contains("api")
            {
                scored.score *= 2.5;
                scored
                    .reasons
                    .push("Intent-based service/handler boost".to_string());
            }
        }
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

        // Parse token components separated by object/module access tokens
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
            scored.score *= 2.0;
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
            scored.score *= 1.3;
            scored.reasons.push("Identifier match boost".to_string());
        }
    }
}

fn apply_noise_suppression(scored_chunks: &mut [ScoredChunk]) {
    for scored in scored_chunks {
        let path_lower = scored.chunk.file_path.to_lowercase();
        let mut penalty = 1.0;
        let mut reason = "";

        if path_lower.contains("vendor") || path_lower.contains("node_modules") {
            penalty = 0.01;
            reason = "Vendor/node_modules penalty";
        } else if path_lower.contains("generated") || path_lower.contains(".pb.go") {
            penalty = 0.05;
            reason = "Generated code penalty";
        } else if path_lower.ends_with("_test.go")
            || path_lower.ends_with(".test.ts")
            || path_lower.ends_with("_test.rs")
            || path_lower.ends_with("_test.py")
            || path_lower.contains("testdata")
        {
            penalty = 0.10;
            reason = "Test/testdata penalty";
        } else if path_lower.contains("mock") || path_lower.contains("test") {
            penalty = 0.20;
            reason = "Mock/Test directory penalty";
        } else if path_lower.contains("fixtures")
            || path_lower.contains("examples")
            || path_lower.contains("target")
            || path_lower.contains("build")
            || path_lower.contains("dist")
        {
            penalty = 0.10;
            reason = "Fixtures/Examples/Build penalty";
        }

        if penalty < 1.0 {
            scored.score *= penalty;
            scored.reasons.push(reason.to_string());
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
    let query_tokens: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    for scored in scored_chunks.iter_mut() {
        if scored.chunk.symbols_defined.is_empty() {
            continue;
        }
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
            scored.score *= 1.5;
            scored.reasons.push("Concept match boost".to_string());
        }
    }
}

fn apply_generic_symbol_penalty(scored_chunks: &mut [ScoredChunk]) {
    let generic_symbols = ["main", "new", "init", "test", "helper", "util", "config"];
    for scored in scored_chunks.iter_mut() {
        for symbol_id in &scored.chunk.symbols_defined {
            let symbol_name = symbol_id
                .split(':')
                .next_back()
                .unwrap_or(symbol_id)
                .to_lowercase();

            if generic_symbols.contains(&symbol_name.as_str()) {
                scored.score *= 0.2; // Severely suppress boilerplate identifiers
                scored.reasons.push("Generic symbol penalty".to_string());
                break; // Penalty applied once per unified code chunk
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::QueryIntent;

    const K: f32 = 60.0;
    const EPS: f32 = 1e-3;

    fn chunk(id: &str, path: &str, symbols: &[&str]) -> CodeChunk {
        CodeChunk {
            id: id.to_string(),
            file_path: path.to_string(),
            start_line: 1,
            end_line: 10,
            raw_content: "fn stub() {}".to_string(),
            symbols_defined: symbols.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_rank_fuses_and_deduplicates_across_channels() {
        let c = chunk("c1", "src/auth.rs", &[]);
        let results = Ranker::rank(
            "totally unrelated wording",
            QueryIntent::Semantic,
            vec![(c.clone(), 0.9)],
            vec![(c, 0.8)],
            K,
            true,
        );

        assert_eq!(results.len(), 1);
        let r = &results[0];
        // First place in both channels: (alpha + beta) / k normalized against
        // the theoretical max of 2/k is exactly 0.5.
        assert!((r.score - 0.5).abs() < EPS);
        assert!(r.reasons.contains(&"Lexical match".to_string()));
        assert!(r.reasons.contains(&"Semantic match".to_string()));
        // No symbols defined -> file-path fallback id
        assert_eq!(r.symbol_id, "file:src/auth.rs");
    }

    #[test]
    fn test_include_tests_false_filters_test_paths() {
        let keep = chunk("c1", "src/auth.rs", &[]);
        let drop = chunk("c2", "tests/auth_test.rs", &[]);
        let results = Ranker::rank(
            "unrelated wording",
            QueryIntent::Symbol,
            vec![(keep.clone(), 1.0), (drop, 1.0)],
            vec![(keep, 1.0)],
            K,
            false,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "src/auth.rs");
    }

    #[test]
    fn test_mock_path_penalty_lowers_score_when_included() {
        // include_tests=true keeps the candidate but mock paths take a x0.20
        // penalty; boosts recover it above threshold yet below the cap.
        let c = chunk(
            "c1",
            "mocks/user_service.rs",
            &["func:userservice:UserService"],
        );
        let results = Ranker::rank(
            "userservice",
            QueryIntent::Symbol,
            vec![(c.clone(), 1.0)],
            vec![(c, 1.0)],
            K,
            true,
        );

        assert_eq!(results.len(), 1);
        let r = &results[0];
        // Without the penalty the same row would hit the 1.0 cap.
        assert!((r.score - 0.26).abs() < EPS);
        assert!(r
            .reasons
            .contains(&"Mock/Test directory penalty".to_string()));
    }

    #[test]
    fn test_vendor_penalty_drops_below_confidence_even_when_included() {
        // Noise suppression runs regardless of include_tests.
        let c = chunk("c1", "vendor/lib/sqlite.rs", &["func:sqlite:sqlite3_open"]);
        let results = Ranker::rank(
            "sqlite3_open",
            QueryIntent::Symbol,
            vec![(c, 1.0)],
            vec![],
            K,
            true,
        );

        // x0.01 vendor penalty pushes the row below MIN_CONFIDENCE_THRESHOLD
        assert!(results.is_empty());
    }

    #[test]
    fn test_definition_boost_ranks_exact_match_first() {
        let def = chunk("c1", "src/auth/service.rs", &["type:auth:AuthService"]);
        let other = chunk("c2", "src/db/pool.rs", &["type:db:DatabasePool"]);
        let results = Ranker::rank(
            "authservice",
            QueryIntent::Symbol,
            vec![(other.clone(), 1.0), (def.clone(), 1.0)],
            vec![(other, 1.0), (def, 1.0)],
            K,
            true,
        );

        assert_eq!(results.len(), 2);
        // Exact symbol definition doubles the score and wins the top slot;
        // the boosted row saturates at the 1.0 normalization cap.
        assert_eq!(results[0].symbol_id, "type:auth:AuthService");
        assert!((results[0].score - 1.0).abs() < EPS);
        assert!((results[1].score - 0.5).abs() < EPS);
        assert_eq!(results[1].symbol_id, "type:db:DatabasePool");
    }

    #[test]
    fn test_generic_symbol_penalty_suppresses_boilerplate() {
        let main = chunk("c1", "src/main.rs", &["func:main:main"]);
        let real = chunk("c2", "src/server.rs", &["func:server:ServerLoop"]);

        let results = Ranker::rank(
            "startup sequence",
            QueryIntent::Semantic,
            vec![(main.clone(), 1.0), (real.clone(), 1.0)],
            vec![(main, 1.0), (real, 1.0)],
            K,
            true,
        );

        // `main` takes the x0.2 generic penalty and falls below the
        // confidence floor; only the substantive symbol survives.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol_id, "func:server:ServerLoop");
    }
}
