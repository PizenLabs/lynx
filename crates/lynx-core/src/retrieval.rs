//! Retrieval scoring helpers: reciprocal-rank fusion, cosine similarity, and
//! definition boosting.

use std::collections::HashMap;

use lynx_protocol::{Evidence, RetrievalMode, SymbolKind};

/// Small constant added to ranks so the first-ranked item still scores below
/// the `k` term, matching standard RRF practice.
pub const RRF_K: f32 = 60.0;

/// Fuses two ranked lists (by symbol hash) using Reciprocal Rank Fusion.
///
/// Each list maps a symbol hash to its rank (0 = best). A symbol present in
/// both lists accumulates contributions from each, favouring items that rank
/// well across channels.
pub fn rrf_fuse(
    lexical: &[(String, usize)],
    semantic: &[(String, usize)],
) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (hash, rank) in lexical {
        *scores.entry(hash.clone()).or_insert(0.0) += 1.0 / (RRF_K + *rank as f32);
    }
    for (hash, rank) in semantic {
        *scores.entry(hash.clone()).or_insert(0.0) += 1.0 / (RRF_K + *rank as f32);
    }
    let mut fused: Vec<(String, f32)> = scores.into_iter().collect();
    fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

/// Cosine similarity between two equal-length vectors, in `[0.0, 1.0]`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = (na.sqrt() * nb.sqrt()).max(f32::EPSILON);
    Some((dot / denom).clamp(0.0, 1.0))
}

/// Kinds treated as declarations and therefore eligible for a definition
/// boost over plain references/modules.
pub fn is_definition_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Struct
            | SymbolKind::Class
            | SymbolKind::Trait
            | SymbolKind::Interface
            | SymbolKind::Enum
    )
}

/// Definition boosting multiplier applied to declaration symbols.
pub const DEFINITION_BOOST: f32 = 1.5;

/// Applies a `1.5x` score boost to definition-kind evidence in place.
pub fn apply_definition_boost(evidence: &mut [Evidence]) {
    for item in evidence.iter_mut() {
        if is_definition_kind(item.identity.kind) {
            item.score *= DEFINITION_BOOST;
        }
    }
}

/// Re-scores fused evidence: normalizes and stamps the [`RetrievalMode::Hybrid`].
pub fn finalize_fused(mut evidence: Vec<Evidence>) -> Vec<Evidence> {
    let max = evidence
        .iter()
        .map(|item| item.score)
        .fold(0.0_f32, f32::max);
    if max > 0.0 {
        for item in evidence.iter_mut() {
            item.score /= max;
        }
    }
    evidence.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    for item in evidence.iter_mut() {
        item.retrieval_mode = RetrievalMode::Hybrid;
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_ranks_present_in_both_higher() {
        let lexical = vec![("a".to_string(), 0usize), ("b".to_string(), 1)];
        let semantic = vec![("a".to_string(), 0usize), ("c".to_string(), 1)];
        let fused = rrf_fuse(&lexical, &semantic);
        assert_eq!(fused[0].0, "a");
        assert!((fused[0].1 - fused[1].1) > 0.0);
    }

    #[test]
    fn cosine_identical_is_one() {
        let v = vec![1.0, 0.0, 1.0];
        assert!((cosine_similarity(&v, &v).unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_similarity(&a, &b).unwrap() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_mismatched_length_is_none() {
        assert!(cosine_similarity(&[1.0], &[1.0, 2.0]).is_none());
    }
}
