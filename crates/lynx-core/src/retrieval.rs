//! Retrieval scoring helpers: reciprocal-rank fusion, cosine similarity,
//! bounded top-k selection, and definition boosting.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use lynx_protocol::{Evidence, RetrievalMode, SymbolKind};

/// Small constant added to ranks so the first-ranked item still scores below
/// the `k` term, matching standard RRF practice.
pub const RRF_K: f32 = 60.0;

/// Fuses two ranked lists (by symbol hash) using Reciprocal Rank Fusion.
///
/// Each list maps a symbol hash to its rank (0 = best). A symbol present in
/// both lists accumulates contributions from each, favouring items that rank
/// well across channels.
pub fn rrf_fuse(lexical: &[(String, usize)], semantic: &[(String, usize)]) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (hash, rank) in lexical {
        *scores.entry(hash.clone()).or_insert(0.0) += 1.0 / (RRF_K + *rank as f32);
    }
    for (hash, rank) in semantic {
        *scores.entry(hash.clone()).or_insert(0.0) += 1.0 / (RRF_K + *rank as f32);
    }
    let mut fused: Vec<(String, f32)> = scores.into_iter().collect();
    // Deterministic: score descending, then hash ascending to break ties.
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused
}

/// A score-then-hash key ordered so a `BinaryHeap` keeps the *worst* element on
/// top (via `Reverse` at the call site): lower score is worse; on equal score
/// the lexicographically greater hash is worse, giving a stable tie-break.
struct ScoreKey<'a> {
    score: f32,
    hash: &'a str,
}

impl PartialEq for ScoreKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.hash == other.hash
    }
}
impl Eq for ScoreKey<'_> {}
impl PartialOrd for ScoreKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScoreKey<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .partial_cmp(&other.score)
            .unwrap_or(Ordering::Equal)
            // Greater hash sorts as "greater" so `Reverse` treats it as worse.
            .then_with(|| other.hash.cmp(self.hash))
    }
}

/// Selects the top `k` scored items in `O(N log k)` time and `O(k)` memory,
/// returning `(symbol_hash, score)` sorted by descending score.
///
/// This is the bounded replacement for "collect all candidates, sort, truncate":
/// per-query memory no longer scales with the number of indexed symbols, only
/// with `k`. Only the `k` retained hashes are cloned. Ties break on hash for
/// determinism. Non-finite scores are dropped.
pub fn top_k_scored<'a, I>(items: I, k: usize) -> Vec<(String, f32)>
where
    I: Iterator<Item = (&'a str, f32)>,
{
    if k == 0 {
        return Vec::new();
    }
    let mut heap: BinaryHeap<std::cmp::Reverse<ScoreKey<'a>>> = BinaryHeap::with_capacity(k + 1);
    for (hash, score) in items {
        if !score.is_finite() {
            continue;
        }
        let key = ScoreKey { score, hash };
        if heap.len() < k {
            heap.push(std::cmp::Reverse(key));
        } else if let Some(std::cmp::Reverse(worst)) = heap.peek() {
            if key > *worst {
                heap.pop();
                heap.push(std::cmp::Reverse(key));
            }
        }
    }
    let mut kept: Vec<(f32, &str)> = heap
        .into_iter()
        .map(|std::cmp::Reverse(key)| (key.score, key.hash))
        .collect();
    kept.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.cmp(b.1))
    });
    kept.into_iter()
        .map(|(score, hash)| (hash.to_string(), score))
        .collect()
}

/// [`top_k_scored`] with ranks instead of scores (`0` = best).
pub fn top_k_ranked<'a, I>(items: I, k: usize) -> Vec<(String, usize)>
where
    I: Iterator<Item = (&'a str, f32)>,
{
    top_k_scored(items, k)
        .into_iter()
        .enumerate()
        .map(|(rank, (hash, _))| (hash, rank))
        .collect()
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

/// Score multiplier applied to symbols located in test/mock/fixture paths.
pub const NOISE_PENALTY_TEST: f32 = 0.5;

/// Score multiplier applied to symbols located in vendored/third-party paths.
pub const NOISE_PENALTY_VENDOR: f32 = 0.25;

/// Query classification: a bare symbol reference versus natural language.
///
/// Exact symbol queries are routed through deterministic exact resolution
/// (Rule: deterministic before probabilistic) instead of ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryClass {
    /// A single identifier/token (`Login`, `auth::service::Login`, `Server.Start`).
    Symbol,
    /// Anything else: natural language or multi-token intent.
    Natural,
}

/// Classifies a query. A query is a [`QueryClass::Symbol`] when it is a single
/// token composed only of symbol characters and containing a letter.
pub fn classify_query(query: &str) -> QueryClass {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return QueryClass::Natural;
    }
    let symbol_chars = trimmed
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '#' | '$'));
    if symbol_chars && trimmed.chars().any(char::is_alphabetic) {
        QueryClass::Symbol
    } else {
        QueryClass::Natural
    }
}

/// Classification of a path into a noise tier, used to demote non-production
/// code that lexically resembles real symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathNoise {
    /// Normal production path.
    None,
    /// Test, mock, fixture, or generated code.
    Test,
    /// Vendored or third-party code.
    Vendor,
}

/// Classifies a workspace-relative path into a noise tier.
pub fn classify_path(path: &std::path::Path) -> PathNoise {
    let text = path.to_string_lossy().to_ascii_lowercase();
    let segments: Vec<&str> = text.split(['/', '\\']).collect();
    if segments.iter().any(|s| {
        matches!(
            *s,
            "vendor" | "third_party" | "third-party" | "node_modules"
        )
    }) {
        return PathNoise::Vendor;
    }
    if segments.iter().any(|s| {
        matches!(
            *s,
            "test"
                | "tests"
                | "testdata"
                | "mock"
                | "mocks"
                | "generated"
                | "fixtures"
                | "__tests__"
        )
    }) || text.ends_with("_test.go")
        || text.contains("_test.")
        || text.contains(".test.")
        || text.contains(".spec.")
    {
        return PathNoise::Test;
    }
    PathNoise::None
}

/// Applies the noise penalty for a path in place, returning the multiplier used.
pub fn apply_noise_penalty(evidence: &mut [Evidence]) {
    for item in evidence.iter_mut() {
        let factor = match classify_path(&item.identity.file_path) {
            PathNoise::None => continue,
            PathNoise::Test => NOISE_PENALTY_TEST,
            PathNoise::Vendor => NOISE_PENALTY_VENDOR,
        };
        item.score *= factor;
    }
}

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
    evidence.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.identity.content_hash.cmp(&b.identity.content_hash))
    });
    for item in evidence.iter_mut() {
        item.retrieval_mode = RetrievalMode::Hybrid;
    }
    evidence
}

/// Builds per-result [`RankContribution`]s from the two channel rankings and
/// the final evidence, so a developer can see *why* each result ranked.
pub fn rank_contributions(
    evidence: &[Evidence],
    lexical: &[(String, usize)],
    semantic: &[(String, usize)],
) -> Vec<crate::query::RankContribution> {
    let rank_of = |list: &[(String, usize)], hash: &str| {
        list.iter().find(|(h, _)| h == hash).map(|(_, rank)| *rank)
    };
    evidence
        .iter()
        .map(|item| {
            let hash = item.identity.content_hash.as_str();
            let lexical_rank = rank_of(lexical, hash);
            let semantic_rank = rank_of(semantic, hash);
            let mut rrf = 0.0_f32;
            if let Some(rank) = lexical_rank {
                rrf += 1.0 / (RRF_K + rank as f32);
            }
            if let Some(rank) = semantic_rank {
                rrf += 1.0 / (RRF_K + rank as f32);
            }
            crate::query::RankContribution {
                content_hash: item.identity.content_hash.clone(),
                fqdn: item.identity.fqdn.clone(),
                lexical_rank,
                semantic_rank,
                rrf_score: rrf,
                definition_boost: is_definition_kind(item.identity.kind),
                final_score: item.score,
            }
        })
        .collect()
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
    fn top_k_keeps_highest_scores_in_order() {
        let items = vec![("a", 0.1f32), ("b", 0.9), ("c", 0.5), ("d", 0.3)];
        let ranked = top_k_ranked(items.into_iter(), 2);
        assert_eq!(ranked, vec![("b".to_string(), 0), ("c".to_string(), 1)]);
    }

    #[test]
    fn top_k_bounds_memory_and_handles_k_zero() {
        assert!(top_k_ranked([("a", 1.0f32)].into_iter(), 0).is_empty());
        let ranked = top_k_ranked([("a", 1.0f32), ("b", 2.0)].into_iter(), 10);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, "b");
    }

    #[test]
    fn top_k_is_deterministic_on_ties() {
        let ranked_a = top_k_ranked([("z", 1.0f32), ("a", 1.0), ("m", 1.0)].into_iter(), 2);
        let ranked_b = top_k_ranked([("m", 1.0f32), ("a", 1.0), ("z", 1.0)].into_iter(), 2);
        assert_eq!(ranked_a, ranked_b);
        assert_eq!(ranked_a[0].0, "a");
    }

    #[test]
    fn top_k_drops_non_finite_scores() {
        let ranked = top_k_ranked([("a", f32::NAN), ("b", 1.0)].into_iter(), 5);
        assert_eq!(ranked, vec![("b".to_string(), 0)]);
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
