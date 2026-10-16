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
///
/// RRF is symmetric, so `(lex=i, sem=j)` and `(lex=j, sem=i)` score **exactly**
/// equal. Such ties are resolved deterministically by the lexical rank first
/// (then semantic rank, then hash): when fusion cannot separate two candidates,
/// the one matched better by the high-precision exact-term channel is preferred.
/// This is a total, stable order independent of the arbitrary hash values.
pub fn rrf_fuse(lexical: &[(String, usize)], semantic: &[(String, usize)]) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (hash, rank) in lexical {
        *scores.entry(hash.clone()).or_insert(0.0) += 1.0 / (RRF_K + *rank as f32);
    }
    for (hash, rank) in semantic {
        *scores.entry(hash.clone()).or_insert(0.0) += 1.0 / (RRF_K + *rank as f32);
    }
    let lex_of: HashMap<&str, usize> = lexical.iter().map(|(h, r)| (h.as_str(), *r)).collect();
    let sem_of: HashMap<&str, usize> = semantic.iter().map(|(h, r)| (h.as_str(), *r)).collect();
    let missing = usize::MAX;

    let mut fused: Vec<(String, f32)> = scores.into_iter().collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                lex_of
                    .get(a.0.as_str())
                    .copied()
                    .unwrap_or(missing)
                    .cmp(&lex_of.get(b.0.as_str()).copied().unwrap_or(missing))
            })
            .then_with(|| {
                sem_of
                    .get(a.0.as_str())
                    .copied()
                    .unwrap_or(missing)
                    .cmp(&sem_of.get(b.0.as_str()).copied().unwrap_or(missing))
            })
            .then_with(|| a.0.cmp(&b.0))
    });
    fused
}

/// An owned score-then-hash key ordered so a `BinaryHeap` keeps the *worst*
/// element on top (via `Reverse` at the call site): lower score is worse; on
/// equal score the lexicographically greater hash is worse, giving a stable
/// tie-break. Owned because streaming sources cannot lend a stable `&str`.
#[derive(Debug, Clone)]
struct OwnedScoreKey {
    score: f32,
    hash: String,
}

impl PartialEq for OwnedScoreKey {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.hash == other.hash
    }
}
impl Eq for OwnedScoreKey {}
impl PartialOrd for OwnedScoreKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OwnedScoreKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .partial_cmp(&other.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| other.hash.cmp(&self.hash))
    }
}

/// A bounded top-`k` collector over a stream of `(hash, score)` items.
///
/// Memory is `O(k)`: only the retained hashes are owned, and a candidate whose
/// score cannot displace the current worst is rejected **before** its hash is
/// cloned. This is the collector a streaming semantic scan feeds one candidate
/// at a time, so a query never materializes the whole candidate set.
///
/// The retained set is a pure function of the *set* of pushed items (the total
/// order is `score` then `hash`), so results are independent of stream order.
pub struct TopK {
    k: usize,
    heap: BinaryHeap<std::cmp::Reverse<OwnedScoreKey>>,
}

impl TopK {
    /// Creates a collector retaining the `k` highest-scoring items.
    pub fn new(k: usize) -> Self {
        Self {
            k,
            heap: BinaryHeap::with_capacity(k.saturating_add(1)),
        }
    }

    /// Offers one candidate. Non-finite scores are ignored.
    pub fn push(&mut self, hash: &str, score: f32) {
        if self.k == 0 || !score.is_finite() {
            return;
        }
        let candidate = OwnedScoreKey {
            score,
            hash: hash.to_string(),
        };
        if self.heap.len() < self.k {
            self.heap.push(std::cmp::Reverse(candidate));
            return;
        }
        if let Some(std::cmp::Reverse(worst)) = self.heap.peek() {
            if candidate > *worst {
                self.heap.pop();
                self.heap.push(std::cmp::Reverse(candidate));
            }
        }
    }

    /// Number of retained candidates.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Whether no candidate is retained.
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Consumes the collector, returning retained items sorted by descending
    /// score (ties by ascending hash).
    pub fn into_sorted(self) -> Vec<(String, f32)> {
        let mut kept: Vec<(String, f32)> = self
            .heap
            .into_iter()
            .map(|std::cmp::Reverse(key)| (key.hash, key.score))
            .collect();
        kept.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        kept
    }
}

/// Selects the top `k` scored items in `O(N log k)` time and `O(k)` memory,
/// returning `(symbol_hash, score)` sorted by descending score.
///
/// This is the bounded replacement for "collect all candidates, sort, truncate":
/// per-query memory no longer scales with the number of indexed symbols, only
/// with `k`. Ties break on hash for determinism. Non-finite scores are dropped.
pub fn top_k_scored<'a, I>(items: I, k: usize) -> Vec<(String, f32)>
where
    I: Iterator<Item = (&'a str, f32)>,
{
    let mut collector = TopK::new(k);
    for (hash, score) in items {
        collector.push(hash, score);
    }
    collector.into_sorted()
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

/// Query classification: which retrieval route an intent takes.
///
/// Symbol and qualified-symbol queries are routed through deterministic exact
/// resolution first (deterministic before probabilistic); only conceptual
/// queries depend primarily on ranked retrieval. This keeps `Login`,
/// `auth.Login`, and `ValidateToken` from becoming embedding searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryClass {
    /// A single bare identifier (`Login`, `ValidateToken`).
    Symbol,
    /// A qualified path (`auth::service::Login`, `Server.Start`).
    Qualified,
    /// Anything else: natural language or multi-token intent.
    Natural,
}

impl QueryClass {
    /// Stable lowercase wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            QueryClass::Symbol => "symbol",
            QueryClass::Qualified => "qualified",
            QueryClass::Natural => "natural",
        }
    }

    /// Whether the class has a deterministic exact-resolution route, so its
    /// primary answer must not depend on ranking.
    pub fn is_deterministic(self) -> bool {
        matches!(self, QueryClass::Symbol | QueryClass::Qualified)
    }
}

/// Classifies a query into its retrieval route.
pub fn classify_query(query: &str) -> QueryClass {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return QueryClass::Natural;
    }
    let symbol_chars = trimmed
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '#' | '$'));
    if !symbol_chars || !trimmed.chars().any(char::is_alphabetic) {
        return QueryClass::Natural;
    }
    if trimmed.contains("::")
        || trimmed.contains('.')
        || trimmed.contains('#')
        || trimmed.contains('$')
    {
        QueryClass::Qualified
    } else {
        QueryClass::Symbol
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
    // Stable sort by score only: equal scores keep the deterministic fused
    // order established by [`rrf_fuse`] (lexical-rank tie-break), so ranking
    // never depends on opaque content hashes.
    evidence.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
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
