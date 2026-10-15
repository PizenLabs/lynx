//! Query lifecycle: cancellation, deadlines, bounds, and structured diagnostics.
//!
//! Every retrieval primitive is given a [`QueryOptions`] describing its bounded
//! lifecycle. Memory is bounded unconditionally by the engine's candidate caps;
//! a caller additionally bounds *time* via [`QueryOptions::deadline`] and may
//! stop a running query from another thread via [`QueryOptions::cancel`].
//!
//! A completed (or aborted) query yields a [`QueryReport`] describing what it
//! did: candidate counts, result count, per-stage timings, and terminal state.
//! This is the observability surface consumed by `lx search --explain` and by
//! embedding runtimes that need to diagnose a slow or heavy query.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Cooperative cancellation flag, cheaply cloneable and shareable across
/// threads. A caller retains a handle and calls [`CancelToken::cancel`]; the
/// engine observes it at bounded intervals and aborts the query.
#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// Creates a fresh, uncancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Safe to call from any thread, at any time.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// Explicit, per-query lifecycle bounds.
#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// Caller-supplied correlation id; generated when absent.
    pub query_id: Option<String>,
    /// Hard wall-clock deadline for the query.
    pub deadline: Option<Instant>,
    /// Overrides the engine's default per-channel candidate cap.
    pub candidate_limit: Option<usize>,
    /// Cooperative cancellation handle.
    pub cancel: CancelToken,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            query_id: None,
            deadline: None,
            candidate_limit: None,
            cancel: CancelToken::new(),
        }
    }
}

impl QueryOptions {
    /// A default, bounded, uncancelled query.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a wall-clock timeout of `ms` milliseconds from now.
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.deadline = Some(Instant::now() + Duration::from_millis(ms));
        self
    }

    /// Requests cancellation from this handle.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether the query should stop: explicitly cancelled or past its deadline.
    pub fn is_expired(&self) -> bool {
        self.cancel.is_cancelled() || self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}

/// Monotonic query-id source, so logs correlate without a clock dependency.
static QUERY_SEQ: AtomicU64 = AtomicU64::new(1);

/// Allocates a process-monotonic query id.
pub fn next_query_id() -> String {
    format!("q{}", QUERY_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// One named pipeline stage and its elapsed microseconds.
#[derive(Debug, Clone)]
pub struct StageTiming {
    /// Stage name (`lexical`, `semantic`, `fuse`, `rank`).
    pub stage: &'static str,
    /// Elapsed microseconds.
    pub micros: u64,
}

/// Why one result ranked where it did: the per-channel contributions that fed
/// the reciprocal-rank fusion and the heuristic layer.
#[derive(Debug, Clone)]
pub struct RankContribution {
    /// Symbol content hash.
    pub content_hash: String,
    /// Fully-qualified name.
    pub fqdn: String,
    /// Rank in the lexical channel (`None` = absent).
    pub lexical_rank: Option<usize>,
    /// Rank in the semantic channel (`None` = absent).
    pub semantic_rank: Option<usize>,
    /// Raw RRF score before boosting/normalization.
    pub rrf_score: f32,
    /// Whether the definition multiplier applied.
    pub definition_boost: bool,
    /// Final score as returned to the caller.
    pub final_score: f32,
}

/// Terminal state of a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    /// Ran to completion.
    Completed,
    /// Aborted via [`CancelToken`].
    Cancelled,
    /// Aborted because the deadline elapsed.
    TimedOut,
}

/// Structured diagnostics for one query.
#[derive(Debug, Clone)]
pub struct QueryReport {
    /// Correlation id.
    pub query_id: String,
    /// Query state at return.
    pub state: QueryState,
    /// Lexical candidates considered.
    pub lexical_candidates: usize,
    /// Semantic candidates considered.
    pub semantic_candidates: usize,
    /// Fused candidates materialized.
    pub fused_candidates: usize,
    /// Results returned.
    pub results: usize,
    /// Per-stage timings.
    pub stages: Vec<StageTiming>,
    /// Total wall-clock microseconds.
    pub total_micros: u64,
}

impl QueryReport {
    /// Builds a report skeleton for `query_id`.
    pub fn start(query_id: String) -> ReportBuilder {
        ReportBuilder {
            query_id,
            start: Instant::now(),
            lexical_candidates: 0,
            semantic_candidates: 0,
            fused_candidates: 0,
            results: 0,
            stages: Vec::new(),
        }
    }
}

/// Accumulates a [`QueryReport`] as stages run.
#[derive(Debug)]
pub struct ReportBuilder {
    query_id: String,
    start: Instant,
    lexical_candidates: usize,
    semantic_candidates: usize,
    fused_candidates: usize,
    results: usize,
    stages: Vec<StageTiming>,
}

impl ReportBuilder {
    /// Records a stage's elapsed time.
    pub fn stage(&mut self, stage: &'static str, elapsed: Duration) {
        self.stages.push(StageTiming {
            stage,
            micros: elapsed.as_micros().min(u64::MAX as u128) as u64,
        });
    }

    /// Records candidate counts.
    pub fn candidates(&mut self, lexical: usize, semantic: usize) {
        self.lexical_candidates = lexical;
        self.semantic_candidates = semantic;
    }

    /// Records the fused candidate count.
    pub fn fused(&mut self, fused: usize) {
        self.fused_candidates = fused;
    }

    /// Finalizes the report.
    pub fn finish(mut self, state: QueryState, results: usize) -> QueryReport {
        self.results = results;
        QueryReport {
            query_id: self.query_id,
            state,
            lexical_candidates: self.lexical_candidates,
            semantic_candidates: self.semantic_candidates,
            fused_candidates: self.fused_candidates,
            results: self.results,
            stages: std::mem::take(&mut self.stages),
            total_micros: self.start.elapsed().as_micros().min(u64::MAX as u128) as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn cancel_token_propagates_across_threads() {
        let token = CancelToken::new();
        let remote = token.clone();
        let handle = thread::spawn(move || {
            while !remote.is_cancelled() {
                thread::yield_now();
            }
            remote.is_cancelled()
        });
        token.cancel();
        assert!(handle.join().unwrap());
    }

    #[test]
    fn timeout_expires() {
        let opts = QueryOptions::new().with_timeout_ms(0);
        assert!(opts.is_expired());
    }

    #[test]
    fn default_is_not_expired_and_query_ids_are_unique() {
        let opts = QueryOptions::new();
        assert!(!opts.is_expired());
        assert_ne!(next_query_id(), next_query_id());
    }

    #[test]
    fn report_records_state_and_results() {
        let mut builder = QueryReport::start("q-test".into());
        builder.stage("lexical", Duration::from_micros(10));
        builder.candidates(3, 7);
        builder.fused(9);
        let report = builder.finish(QueryState::Completed, 5);
        assert_eq!(report.query_id, "q-test");
        assert_eq!(report.state, QueryState::Completed);
        assert_eq!(report.lexical_candidates, 3);
        assert_eq!(report.semantic_candidates, 7);
        assert_eq!(report.fused_candidates, 9);
        assert_eq!(report.results, 5);
        assert_eq!(report.stages.len(), 1);
    }
}
