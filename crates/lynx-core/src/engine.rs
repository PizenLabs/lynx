//! The core [`Engine`]: the indexing pipeline and the retrieval primitives.
//!
//! # Persistence model
//!
//! `Engine` is a **thin query layer over the persisted substrate**, not a
//! container of repository state. [`Engine::index_repository`] streams the
//! repository into [`DualStorage`] (SQLite for identities/relations/vectors,
//! Tantivy for snippets) one file at a time, holding `O(file)` memory. Every
//! retrieval primitive then answers from storage:
//!
//! - exact resolution is a bounded SQL lookup (deterministic tie-break);
//! - lexical search is a Tantivy query over a bounded candidate set;
//! - semantic search streams vectors from SQLite through a bounded top-k
//!   collector (`O(k)` memory);
//! - `Evidence` is materialized only for the retained results, and snippets are
//!   fetched from Tantivy for exactly those results.
//!
//! Consequently `Engine::new` on an existing storage root **hydrates** it: no
//! repository state is rebuilt, no file is read, and no embedding is recomputed.
//! A query never re-indexes the repository.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use lynx_embed::VectorProvider;
use lynx_parser::Parser;
use lynx_protocol::{
    CapabilityLevel, Evidence, Language, Relation, RelationKind, RetrievalMode, Snapshot,
    SourceRange, SymbolIdentity, SymbolKind,
};
use lynx_storage::DualStorage;
use walkdir::WalkDir;

use crate::compiler::ContextCompiler;
use crate::error::CoreError;
use crate::query::{next_query_id, QueryOptions, QueryReport, QueryState};
use crate::retrieval::{
    apply_definition_boost, apply_noise_penalty, classify_path, classify_query, cosine_similarity,
    finalize_fused, rank_contributions, rrf_fuse, PathNoise, TopK,
};

/// Maximum number of source lines captured as a per-symbol snippet.
const MAX_SNIPPET_LINES: usize = 25;

/// Upper bound on the number of lines retained for a whole-file module snippet.
///
/// Module pseudo-symbols used to retain and embed the *entire* file, which made
/// retained memory and embedding cost scale with total repository bytes. A
/// bounded head keeps the file-level anchor cheap without losing its identity.
const MAX_MODULE_SNIPPET_LINES: usize = 80;

/// Maximum snippet characters fed to the embedding model for a code symbol.
///
/// `FastEmbedProvider` pads every batch to its longest member, so one long text
/// (a whole-file module snippet) inflated the cost of the entire per-file batch.
/// Bounding the descriptor keeps every batch member short and homogeneous.
const MAX_EMBED_CHARS: usize = 512;

/// Snippet characters fed to the model for a module pseudo-symbol.
const MAX_MODULE_EMBED_CHARS: usize = 128;

/// Per-channel candidate bound: `search` fuses at most this many lexical and
/// semantic candidates, so per-query memory is `O(candidate_limit)` rather than
/// `O(symbols)`. Scaled from the requested `limit` and clamped.
const CANDIDATE_FACTOR: usize = 20;
const CANDIDATE_FLOOR: usize = 100;
const CANDIDATE_CEILING: usize = 2000;

/// Absolute upper bound a caller may raise the candidate cap to. Guarantees no
/// caller can turn `search` back into an unbounded operation.
const CANDIDATE_HARD_MAX: usize = 100_000;

/// Metadata key holding the highest capability level of the indexed workspace.
const CAPABILITY_META: &str = "capability";

/// Metadata key recording whether the index was built with test/mock files
/// included, so freshness compares like with like.
const INCLUDE_TESTS_META: &str = "include_tests";

/// Largest source file (bytes) that will be read and indexed.
///
/// A resource guard, not a correctness rule: an adversarial or generated file
/// far larger than any real source unit cannot be allowed to spike indexing
/// memory. Files above the bound are skipped, leaving the process stable.
const MAX_INDEX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Freshness of a persisted index relative to its workspace.
///
/// Every state is a distinct, actionable outcome; no state means "probably
/// fine". [`Freshness::Fresh`] is only returned after the workspace has been
/// re-scanned and found identical to the persisted file states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// The persisted file states match the workspace exactly.
    Fresh,
    /// The workspace has added, modified, or deleted files since indexing.
    Stale,
    /// No populated index exists at the storage root.
    Missing,
    /// The index was written by an incompatible schema version and was reset.
    Incompatible,
    /// The substrate failed its integrity check and must be rebuilt.
    Corrupt,
    /// The recorded workspace root is absent, so freshness cannot be checked.
    Unknown,
}

impl Freshness {
    /// Stable lowercase wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Stale => "stale",
            Freshness::Missing => "missing",
            Freshness::Incompatible => "incompatible",
            Freshness::Corrupt => "corrupt",
            Freshness::Unknown => "unknown",
        }
    }
}

/// A workspace-relative file and the content hash of its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    /// Workspace-relative path, `/`-separated.
    pub path: String,
    /// Hex `blake3` digest of the file's bytes.
    pub content_hash: String,
}

/// The change set between a workspace and a persisted index.
#[derive(Debug, Clone, Default)]
pub struct UpdatePlan {
    /// Files present now and absent from the index.
    pub added: Vec<String>,
    /// Files whose content hash changed.
    pub modified: Vec<String>,
    /// Files present in the index and absent from the workspace.
    pub deleted: Vec<String>,
    /// Files whose content hash is unchanged.
    pub unchanged: usize,
}

impl UpdatePlan {
    /// Whether the plan would change nothing.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    /// Total files the plan would write or remove.
    pub fn changed(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }
}

/// The result of an incremental indexing pass.
#[derive(Debug, Clone)]
pub struct IndexUpdate {
    /// The applied change set.
    pub plan: UpdatePlan,
    /// The index state after the pass.
    pub status: IndexStatus,
}

/// Resolves the bounded per-channel candidate count for a requested `limit`.
fn candidate_limit(limit: usize) -> usize {
    limit
        .saturating_mul(CANDIDATE_FACTOR)
        .clamp(CANDIDATE_FLOOR, CANDIDATE_CEILING)
}

/// Returns the terminal state for `opts` if the query must stop now.
fn terminal_state(opts: &QueryOptions) -> Option<QueryState> {
    if opts.cancel.is_cancelled() {
        Some(QueryState::Cancelled)
    } else if opts
        .deadline
        .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    {
        Some(QueryState::TimedOut)
    } else {
        None
    }
}

/// Maps a terminal state to its error.
fn terminal_error(state: QueryState) -> CoreError {
    match state {
        QueryState::Cancelled => CoreError::Cancelled,
        QueryState::TimedOut => CoreError::TimedOut,
        QueryState::Completed => unreachable!("completed is not an error"),
    }
}

/// Evidence plus the structured diagnostics of the query that produced it.
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    /// Ranked evidence.
    pub evidence: Vec<Evidence>,
    /// Query diagnostics.
    pub report: QueryReport,
    /// Per-result ranking contributions, for explainability.
    pub contributions: Vec<crate::query::RankContribution>,
}

/// Files and directories always excluded from indexing.
const EXCLUDED_SEGMENTS: &[&str] = &[
    ".git",
    ".lynx",
    ".fastembed_cache",
    "node_modules",
    "vendor",
    "target",
    "build",
    "dist",
];

/// Summary of the current index state.
#[derive(Debug, Clone)]
pub struct IndexStatus {
    /// Provenance of the indexed workspace.
    pub snapshot: Snapshot,
    /// Number of files indexed.
    pub file_count: usize,
    /// Number of symbols indexed.
    pub symbol_count: usize,
    /// Number of relations indexed.
    pub relation_count: usize,
    /// Maximum capability held by the indexed workspace.
    pub capability_level: CapabilityLevel,
}

impl IndexStatus {
    fn empty() -> Self {
        Self {
            snapshot: empty_snapshot(),
            file_count: 0,
            symbol_count: 0,
            relation_count: 0,
            capability_level: CapabilityLevel::L0Text,
        }
    }
}

/// The core engine: a query layer over [`DualStorage`], a [`Parser`], and a
/// [`VectorProvider`].
pub struct Engine {
    dual: DualStorage,
    parser: Parser,
    provider: Box<dyn VectorProvider>,
    snapshot: Mutex<Snapshot>,
    status: Mutex<IndexStatus>,
}

impl Engine {
    /// Opens the substrate at `storage_root`, **hydrating** it when populated.
    ///
    /// Hydration reads only provenance and counts; it never reads a source file
    /// or recomputes an embedding, so opening an existing index is `O(1)`.
    pub fn new(storage_root: &Path, provider: Box<dyn VectorProvider>) -> Result<Self, CoreError> {
        let dual = DualStorage::open(storage_root)?;
        let engine = Self {
            dual,
            parser: Parser::new(),
            provider,
            snapshot: Mutex::new(empty_snapshot()),
            status: Mutex::new(IndexStatus::empty()),
        };
        engine.hydrate()?;
        Ok(engine)
    }

    /// Reads persisted provenance, counts, and capability into the status.
    fn hydrate(&self) -> Result<(), CoreError> {
        let (files, symbols, relations) = self.dual.graph().counts()?;
        let snapshot = self
            .dual
            .graph()
            .latest_snapshot()?
            .unwrap_or_else(empty_snapshot);
        let capability = self
            .dual
            .graph()
            .get_meta(CAPABILITY_META)?
            .and_then(|value| parse_capability(&value))
            .unwrap_or(CapabilityLevel::L0Text);
        *self.snapshot.lock().unwrap() = snapshot.clone();
        *self.status.lock().unwrap() = IndexStatus {
            snapshot,
            file_count: files,
            symbol_count: symbols,
            relation_count: relations,
            capability_level: capability,
        };
        Ok(())
    }

    /// Whether the substrate holds at least one symbol.
    fn populated(&self) -> Result<bool, CoreError> {
        Ok(self.dual.graph().has_symbols()?)
    }

    /// Indexes `repo_path`, updating only the files that changed since the last
    /// pass.
    ///
    /// The first call on an empty substrate behaves as a full build; every
    /// subsequent call diffs the workspace's per-file content hashes against
    /// the persisted [`crate::FileState`]s and re-parses/re-embeds only added
    /// and modified files, removing the rows and documents of deleted or
    /// replaced files first. State is therefore never left stale, and a small
    /// edit costs a small pass.
    ///
    /// Memory remains `O(largest file + embedding batch)`: the diff holds only
    /// path/hash pairs, never file bodies.
    pub fn index_repository(
        &self,
        repo_path: &Path,
        include_tests: bool,
    ) -> Result<IndexStatus, CoreError> {
        Ok(self.update_repository(repo_path, include_tests)?.status)
    }

    /// Discards the entire index and rebuilds it from `repo_path`.
    ///
    /// This is the explicit recovery path: use it after a corruption or an
    /// incompatible-schema reset, or when a consumer wants a guaranteed-clean
    /// artifact regardless of what was indexed before.
    pub fn rebuild_repository(
        &self,
        repo_path: &Path,
        include_tests: bool,
    ) -> Result<IndexStatus, CoreError> {
        self.dual.reset()?;
        self.index_repository(repo_path, include_tests)
    }

    /// [`Engine::index_repository`] returning the applied [`IndexUpdate`]: the
    /// diff that was computed and the resulting status.
    pub fn update_repository(
        &self,
        repo_path: &Path,
        include_tests: bool,
    ) -> Result<IndexUpdate, CoreError> {
        let snapshot = resolve_snapshot(repo_path)?;
        self.dual.graph().save_snapshot(&snapshot)?;

        let current = scan_workspace(repo_path, include_tests)?;
        let plan = self.diff_plan(&current)?;

        // Delete first (in opstamp order), so a re-added file replaces its old
        // documents rather than duplicating them.
        let mut remove: Vec<String> = plan.deleted.clone();
        remove.extend(plan.modified.iter().cloned());
        if !remove.is_empty() {
            self.dual.graph().delete_files(&remove)?;
            self.dual.lexical().delete_by_file_paths(&remove)?;
        }

        let mut changed: Vec<&(String, String)> = current
            .iter()
            .filter(|(path, _)| plan.added.contains(path) || plan.modified.contains(path))
            .collect();
        changed.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, _) in changed {
            let source = match std::fs::read_to_string(repo_path.join(path)) {
                Ok(source) => source,
                Err(_) => continue, // vanished or became unreadable mid-pass
            };
            self.index_file(Path::new(path), &source, &snapshot)?;
        }

        let max_capability = current
            .iter()
            .map(|(path, _)| adapter_capability(Path::new(path)))
            .max_by_key(|level| capability_rank(*level))
            .unwrap_or(CapabilityLevel::L0Text);

        self.dual
            .graph()
            .upsert_files(&current, &snapshot.content_hash)?;
        // Flush the shared lexical writer once, at the end of the pass, so the
        // on-disk index is quiescent before the session is persisted or served.
        self.dual.lexical().commit()?;
        self.dual.graph().set_meta(
            lynx_storage::graph_store::LATEST_SNAPSHOT_META,
            &snapshot.content_hash,
        )?;
        self.dual
            .graph()
            .set_meta(CAPABILITY_META, capability_name(max_capability))?;
        self.dual.graph().set_meta(
            INCLUDE_TESTS_META,
            if include_tests { "true" } else { "false" },
        )?;

        let (files, symbols, relations) = self.dual.graph().counts()?;
        *self.snapshot.lock().unwrap() = snapshot.clone();
        let status = IndexStatus {
            snapshot,
            file_count: files,
            symbol_count: symbols,
            relation_count: relations,
            capability_level: max_capability,
        };
        *self.status.lock().unwrap() = status.clone();
        Ok(IndexUpdate { plan, status })
    }

    /// Parses, stores, and indexes one file's symbols, relations, snippets, and
    /// vectors. The file's previous rows must already have been removed.
    fn index_file(
        &self,
        relative: &Path,
        source: &str,
        snapshot: &Snapshot,
    ) -> Result<(), CoreError> {
        let (parsed_symbols, parsed_relations) = match self.parser.parse(relative, source) {
            Ok(pair) => pair,
            Err(_) => return Ok(()), // unparseable file: skip, never fatal
        };

        let snippets = compute_file_snippets(source, &parsed_symbols);
        let file_identities: Vec<(SymbolIdentity, SourceRange, String)> = parsed_symbols
            .iter()
            .cloned()
            .zip(snippets)
            .map(|(identity, (range, snippet))| (identity, range, snippet))
            .collect();
        let entries: Vec<(SymbolIdentity, SourceRange)> = file_identities
            .iter()
            .map(|(identity, range, _)| (identity.clone(), range.clone()))
            .collect();
        // Symbols must land before their intra-file relations (FKs enforced);
        // all three writes are per-file transactions, so indexing is a
        // streaming pass rather than an accumulating one.
        self.dual
            .graph()
            .insert_symbols(&entries, &snapshot.content_hash)?;
        self.dual
            .graph()
            .insert_relations(&parsed_relations, &snapshot.content_hash)?;

        let docs: Vec<lynx_storage::LexicalDoc> = file_identities
            .iter()
            .map(|(identity, _, snippet)| lynx_storage::LexicalDoc {
                symbol_hash: identity.content_hash.clone(),
                code_snippet: snippet.clone(),
                language: wire_language(identity.language).to_string(),
                file_path: identity.file_path.to_string_lossy().to_string(),
            })
            .collect();
        self.dual.lexical().add_documents(&docs)?;

        // Embed a compact, bounded descriptor per symbol rather than the raw
        // excerpt: it keeps the whole batch short (fastembed pads to the
        // longest member) and adds high-signal identity tokens (fqdn, kind).
        let texts: Vec<String> = file_identities
            .iter()
            .map(|(identity, _, snippet)| embedding_text(identity, snippet))
            .collect();
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let vectors = self.provider.embed_batch(&text_refs)?;
        let embedding_items: Vec<(String, Vec<f32>)> = file_identities
            .iter()
            .map(|(identity, _, _)| identity.content_hash.clone())
            .zip(vectors)
            .collect();
        self.dual.graph().put_embeddings(&embedding_items)?;
        Ok(())
    }

    /// Diffs a freshly scanned workspace against the persisted file states.
    fn diff_plan(&self, current: &[(String, String)]) -> Result<UpdatePlan, CoreError> {
        let stored: HashMap<String, String> =
            self.dual.graph().file_states()?.into_iter().collect();
        let mut plan = UpdatePlan::default();
        let mut present: HashSet<&str> = HashSet::with_capacity(current.len());
        for (path, hash) in current {
            present.insert(path.as_str());
            match stored.get(path) {
                None => plan.added.push(path.clone()),
                Some(previous) if previous != hash => plan.modified.push(path.clone()),
                Some(_) => plan.unchanged += 1,
            }
        }
        for path in stored.keys() {
            if !present.contains(path.as_str()) {
                plan.deleted.push(path.clone());
            }
        }
        plan.added.sort();
        plan.modified.sort();
        plan.deleted.sort();
        Ok(plan)
    }

    /// Computes the change set between the persisted index and `repo_path`
    /// without applying it. The basis of [`Engine::freshness`].
    pub fn plan_update(
        &self,
        repo_path: &Path,
        include_tests: bool,
    ) -> Result<UpdatePlan, CoreError> {
        let current = scan_workspace(repo_path, include_tests)?;
        self.diff_plan(&current)
    }

    /// Classifies the persisted index against its workspace.
    ///
    /// Unlike [`Engine::index_status`], this reads the repository (hashing
    /// every file) and runs a SQLite integrity check, so it is an explicit,
    /// on-demand operation rather than something paid on the query path.
    ///
    /// The persisted index records whether it was built with tests included;
    /// freshness compares against the same policy so a mismatch in that policy
    /// surfaces as [`Freshness::Stale`].
    pub fn freshness(&self) -> Result<Freshness, CoreError> {
        if !self.populated()? {
            // An empty substrate that was reset on open means the caller must
            // re-index; an empty substrate that was never indexed is simply
            // missing. A populated substrate is valid regardless of a past
            // reset, so a re-index clears the incompatibility.
            return Ok(if self.dual.graph().schema_reset() {
                Freshness::Incompatible
            } else {
                Freshness::Missing
            });
        }
        if !self.dual.graph().quick_check()? {
            return Ok(Freshness::Corrupt);
        }
        let Some(snapshot) = self.dual.graph().latest_snapshot()? else {
            return Ok(Freshness::Missing);
        };
        let root = snapshot.workspace_root.clone();
        if root.as_os_str().is_empty() || !root.is_dir() {
            return Ok(Freshness::Unknown);
        }
        let include_tests = self
            .dual
            .graph()
            .get_meta(INCLUDE_TESTS_META)?
            .map(|value| value == "true")
            .unwrap_or(false);
        let plan = self.plan_update(&root, include_tests)?;
        Ok(if plan.is_empty() {
            Freshness::Fresh
        } else {
            Freshness::Stale
        })
    }

    /// Reports the current index state.
    pub fn index_status(&self) -> IndexStatus {
        self.status.lock().unwrap().clone()
    }

    /// `search`: fuses BM25 (Tantivy) with vector similarity via Reciprocal
    /// Rank Fusion, applies definition boosting, and returns ranked evidence.
    ///
    /// Uses default query options (no deadline, no cancellation); see
    /// [`Engine::search_with`].
    pub fn search(
        &self,
        query: &str,
        mode: RetrievalMode,
        limit: usize,
    ) -> Result<Vec<Evidence>, CoreError> {
        Ok(self
            .search_with(query, mode, limit, &QueryOptions::default())?
            .evidence)
    }

    /// [`Engine::search`] with an explicit [`QueryOptions`] lifecycle, returning
    /// evidence plus a [`QueryReport`].
    ///
    /// Invariants:
    /// - per-query memory is `O(candidate limit)`, never `O(symbols)`;
    /// - the semantic scan streams vectors from storage and observes
    ///   cancellation/deadline at every candidate;
    /// - snippets are materialized only for the returned results;
    /// - on cancellation or timeout the query returns `Err` and releases all
    ///   query-local buffers (there are no background tasks or channels).
    pub fn search_with(
        &self,
        query: &str,
        mode: RetrievalMode,
        limit: usize,
        opts: &QueryOptions,
    ) -> Result<SearchOutcome, CoreError> {
        let query_id = opts.query_id.clone().unwrap_or_else(next_query_id);
        let mut report = QueryReport::start(query_id);

        if let Some(state) = terminal_state(opts) {
            return Err(terminal_error(state));
        }

        // Deterministic exact resolution for symbol/qualified queries: this
        // must not be a ranking problem. A miss degrades gracefully to hybrid
        // retrieval, so an unresolvable identifier still gets results.
        let class = classify_query(query);
        report.class(class);
        let exact = if class.is_deterministic() {
            self.resolve(query).ok().flatten()
        } else {
            None
        };

        if !self.populated()? {
            return Err(CoreError::NotIndexed);
        }

        let candidates = opts
            .candidate_limit
            .map(|c| c.clamp(1, CANDIDATE_HARD_MAX))
            .unwrap_or_else(|| candidate_limit(limit));

        let mut lexical: Vec<(String, usize)> = Vec::new();
        if matches!(mode, RetrievalMode::Lexical | RetrievalMode::Hybrid) {
            let stage = std::time::Instant::now();
            let hits = self.dual.lexical().search_snippets(query, candidates)?;
            report.stage("lexical", stage.elapsed());
            lexical = hits
                .into_iter()
                .enumerate()
                .map(|(rank, hit)| (hit.symbol_hash, rank))
                .collect();
        }

        let mut semantic: Vec<(String, usize)> = Vec::new();
        if matches!(mode, RetrievalMode::Semantic | RetrievalMode::Hybrid) {
            if let Some(state) = terminal_state(opts) {
                return Err(terminal_error(state));
            }
            let stage = std::time::Instant::now();
            let query_vec = self.provider.embed_query(query)?;
            check_dimension(self.provider.as_ref(), &query_vec)?;
            // Bounded, cancellable, streaming top-k: vectors are read from
            // storage one at a time (O(k) memory retained), and the scan stops
            // as soon as the deadline/cancellation flag trips.
            let mut collector = TopK::new(candidates);
            self.dual.graph().for_each_embedding(|hash, vector| {
                if opts.is_expired() {
                    return false;
                }
                if let Some(similarity) = cosine_similarity(&query_vec, vector) {
                    collector.push(hash, similarity);
                }
                true
            })?;
            semantic = collector
                .into_sorted()
                .into_iter()
                .enumerate()
                .map(|(rank, (hash, _))| (hash, rank))
                .collect();
            report.stage("semantic", stage.elapsed());
            if let Some(state) = terminal_state(opts) {
                return Err(terminal_error(state));
            }
        }

        report.candidates(lexical.len(), semantic.len());

        let snapshot = self.snapshot.lock().unwrap().clone();
        let stage = std::time::Instant::now();
        let fused = rrf_fuse(&lexical, &semantic);
        report.fused(fused.len());
        let ranked = self.assemble(&fused, &snapshot, mode, exact, limit)?;
        report.stage("rank", stage.elapsed());

        let results = ranked.len();
        let contributions = rank_contributions(&ranked, &lexical, &semantic);
        Ok(SearchOutcome {
            evidence: ranked,
            report: report.finish(QueryState::Completed, results),
            contributions,
        })
    }

    /// Materializes, boosts, ranks, and truncates fused candidates.
    ///
    /// Only identity metadata (kind, path) is loaded for the candidate set;
    /// snippets are fetched **after** truncation, for the retained results only.
    fn assemble(
        &self,
        fused: &[(String, f32)],
        snapshot: &Snapshot,
        mode: RetrievalMode,
        exact: Option<Evidence>,
        limit: usize,
    ) -> Result<Vec<Evidence>, CoreError> {
        if fused.is_empty() && exact.is_none() {
            return Ok(Vec::new());
        }
        let hashes: Vec<String> = fused.iter().map(|(hash, _)| hash.clone()).collect();
        let metas: HashMap<String, (SymbolIdentity, SourceRange)> = self
            .dual
            .graph()
            .symbols_for(&hashes)?
            .into_iter()
            .map(|(hash, identity, range)| (hash, (identity, range)))
            .collect();

        let mut evidence: Vec<Evidence> = fused
            .iter()
            .filter_map(|(hash, score)| {
                metas.get(hash).map(|(identity, range)| Evidence {
                    identity: identity.clone(),
                    snapshot: snapshot.clone(),
                    range: range.clone(),
                    code_snippet: String::new(),
                    score: *score,
                    retrieval_mode: mode,
                })
            })
            .collect();

        apply_definition_boost(&mut evidence);
        apply_noise_penalty(&mut evidence);
        let mut ranked = finalize_fused(evidence);

        if let Some(exact) = exact {
            let exact_hash = exact.identity.content_hash.clone();
            ranked.retain(|item| item.identity.content_hash != exact_hash);
            let mut with_exact = Vec::with_capacity(ranked.len() + 1);
            with_exact.push(exact);
            with_exact.extend(ranked);
            ranked = with_exact;
        }
        ranked.truncate(limit);

        // Fetch snippets only for the retained results.
        let retained: Vec<String> = ranked
            .iter()
            .filter(|item| item.code_snippet.is_empty())
            .map(|item| item.identity.content_hash.clone())
            .collect();
        if !retained.is_empty() {
            let snippets = self.dual.lexical().snippets_for(&retained)?;
            for item in ranked.iter_mut() {
                if item.code_snippet.is_empty() {
                    if let Some(text) = snippets.get(&item.identity.content_hash) {
                        item.code_snippet = text.clone();
                    }
                }
            }
        }
        Ok(ranked)
    }

    /// `resolve`: exact lookup of a symbol by fqdn or trailing name.
    ///
    /// Deterministic: an exact fqdn match wins; otherwise, among trailing-name
    /// matches, production code outranks test/vendor code and ties break on the
    /// lexicographically smallest `(fqdn, content_hash)`. Resolved by a bounded
    /// SQL lookup rather than a scan or a ranking pass, and it materializes the
    /// snippet for exactly one symbol regardless of how many share the name.
    ///
    /// When several distinct symbols share the name, use [`Engine::resolve_all`]
    /// to see every candidate rather than the single deterministic winner.
    pub fn resolve(&self, fqdn_or_name: &str) -> Result<Option<Evidence>, CoreError> {
        let mut matches = self.ranked_matches(fqdn_or_name)?;
        let Some((identity, range)) = matches.drain(..).next() else {
            return Ok(None);
        };
        let code_snippet = self
            .dual
            .lexical()
            .snippet_for(&identity.content_hash)?
            .unwrap_or_default();
        let snapshot = self.snapshot.lock().unwrap().clone();
        Ok(Some(Evidence {
            identity,
            snapshot,
            range,
            code_snippet,
            score: 1.0,
            retrieval_mode: RetrievalMode::Structural,
        }))
    }

    /// Every symbol matching `fqdn_or_name` (exact fqdn or trailing name),
    /// ordered deterministically: production paths before test, then vendor;
    /// exact fqdn matches first; ties on lexicographic `(fqdn, hash)`.
    ///
    /// This is the ambiguity surface: a consumer that needs to disambiguate
    /// `Login` receives every `Login` rather than a silently-chosen winner.
    pub fn resolve_all(&self, fqdn_or_name: &str) -> Result<Vec<Evidence>, CoreError> {
        let matches = self.ranked_matches(fqdn_or_name)?;
        if matches.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let hashes: Vec<String> = matches
            .iter()
            .map(|(identity, _)| identity.content_hash.clone())
            .collect();
        let snippets = self.dual.lexical().snippets_for(&hashes)?;
        Ok(matches
            .into_iter()
            .map(|(identity, range)| {
                let code_snippet = snippets
                    .get(&identity.content_hash)
                    .cloned()
                    .unwrap_or_default();
                Evidence {
                    identity,
                    snapshot: snapshot.clone(),
                    range,
                    code_snippet,
                    score: 1.0,
                    retrieval_mode: RetrievalMode::Structural,
                }
            })
            .collect())
    }

    /// All symbols matching `needle`, ordered deterministically (exact fqdn
    /// first, production before test/vendor, then lexicographic). Identity
    /// metadata only — no snippets. Errors `NotIndexed` when the substrate is
    /// empty.
    fn ranked_matches(
        &self,
        fqdn_or_name: &str,
    ) -> Result<Vec<(SymbolIdentity, SourceRange)>, CoreError> {
        let needle = fqdn_or_name.trim();
        let mut matches = self.dual.graph().resolve_all(needle)?;
        if matches.is_empty() {
            if !self.populated()? {
                return Err(CoreError::NotIndexed);
            }
            return Ok(Vec::new());
        }
        matches.sort_by(|a, b| {
            let key = |(identity, _): &(SymbolIdentity, SourceRange)| {
                (
                    usize::from(identity.fqdn != needle),
                    noise_rank(&identity.file_path),
                    identity.fqdn.clone(),
                    identity.content_hash.clone(),
                )
            };
            key(a).cmp(&key(b))
        });
        Ok(matches)
    }

    /// `inspect`: full evidence details for a symbol by content hash.
    pub fn inspect(&self, symbol_hash: &str) -> Result<Option<Evidence>, CoreError> {
        let resolved = self.dual.graph().get_symbol(symbol_hash)?;
        let Some((identity, range)) = resolved else {
            if !self.populated()? {
                return Err(CoreError::NotIndexed);
            }
            return Ok(None);
        };
        let snippet = self
            .dual
            .lexical()
            .snippet_for(&identity.content_hash)?
            .unwrap_or_default();
        let snapshot = self.snapshot.lock().unwrap().clone();
        Ok(Some(Evidence {
            identity,
            snapshot,
            range,
            code_snippet: snippet,
            score: 1.0,
            retrieval_mode: RetrievalMode::Structural,
        }))
    }

    /// `relations`: structural graph query incident to `symbol_hash`.
    pub fn relations(
        &self,
        symbol_hash: &str,
        kind: Option<RelationKind>,
    ) -> Result<Vec<Relation>, CoreError> {
        Ok(self.dual.graph().get_relations(symbol_hash, kind)?)
    }

    /// `trace`: breadth-first traversal of the relation graph from
    /// `symbol_hash` up to `depth`, returning all discovered edges (deduped).
    pub fn trace(&self, symbol_hash: &str, depth: usize) -> Result<Vec<Relation>, CoreError> {
        self.trace_with(symbol_hash, depth, &QueryOptions::default())
    }

    /// [`Engine::trace`] with an explicit query lifecycle. `depth` bounds the
    /// traversal breadth-first so the frontier cannot grow without limit.
    pub fn trace_with(
        &self,
        symbol_hash: &str,
        depth: usize,
        opts: &QueryOptions,
    ) -> Result<Vec<Relation>, CoreError> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut edges: Vec<Relation> = Vec::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((symbol_hash.to_string(), 0));

        while let Some((current, level)) = queue.pop_front() {
            if opts.is_expired() {
                return Err(terminal_error(
                    terminal_state(opts).unwrap_or(QueryState::Cancelled),
                ));
            }
            if level >= depth || !visited.insert(current.clone()) {
                continue;
            }
            let incident = self.dual.graph().get_relations(&current, None)?;
            for relation in incident {
                edges.push(relation.clone());
                let next = if relation.source_id == current {
                    relation.target_id.clone()
                } else {
                    relation.source_id.clone()
                };
                if !visited.contains(&next) {
                    queue.push_back((next, level + 1));
                }
            }
        }
        edges.sort_by(|a, b| {
            (a.kind as u8, &a.source_id, &a.target_id).cmp(&(
                b.kind as u8,
                &b.source_id,
                &b.target_id,
            ))
        });
        edges.dedup_by(|a, b| {
            a.kind == b.kind && a.source_id == b.source_id && a.target_id == b.target_id
        });
        Ok(edges)
    }

    /// `similar`: vector-only similarity lookup of a symbol against all others.
    ///
    /// Bounded (`O(N log k)` time, `O(k)` memory) and cancellable; vectors are
    /// streamed from storage rather than held in memory.
    pub fn similar(&self, symbol_hash: &str, limit: usize) -> Result<Vec<Evidence>, CoreError> {
        self.similar_with(symbol_hash, limit, &QueryOptions::default())
    }

    /// [`Engine::similar`] with an explicit query lifecycle.
    pub fn similar_with(
        &self,
        symbol_hash: &str,
        limit: usize,
        opts: &QueryOptions,
    ) -> Result<Vec<Evidence>, CoreError> {
        if let Some(state) = terminal_state(opts) {
            return Err(terminal_error(state));
        }
        if !self.populated()? {
            return Err(CoreError::NotIndexed);
        }
        let target = self
            .dual
            .graph()
            .embedding(symbol_hash)?
            .ok_or(CoreError::NotIndexed)?;
        let mut collector = TopK::new(limit);
        self.dual.graph().for_each_embedding(|hash, vector| {
            if opts.is_expired() {
                return false;
            }
            if hash == symbol_hash {
                return true;
            }
            if let Some(similarity) = cosine_similarity(&target, vector) {
                collector.push(hash, similarity);
            }
            true
        })?;
        if let Some(state) = terminal_state(opts) {
            return Err(terminal_error(state));
        }
        let ranked = collector.into_sorted();
        let snapshot = self.snapshot.lock().unwrap().clone();
        self.materialize_ranked(ranked, &snapshot, RetrievalMode::Semantic)
    }

    /// Loads identity metadata and snippets for already-ranked `(hash, score)`
    /// pairs, preserving order.
    fn materialize_ranked(
        &self,
        ranked: Vec<(String, f32)>,
        snapshot: &Snapshot,
        mode: RetrievalMode,
    ) -> Result<Vec<Evidence>, CoreError> {
        let hashes: Vec<String> = ranked.iter().map(|(hash, _)| hash.clone()).collect();
        let metas: HashMap<String, (SymbolIdentity, SourceRange)> = self
            .dual
            .graph()
            .symbols_for(&hashes)?
            .into_iter()
            .map(|(hash, identity, range)| (hash, (identity, range)))
            .collect();
        let snippets = self.dual.lexical().snippets_for(&hashes)?;
        Ok(ranked
            .into_iter()
            .filter_map(|(hash, score)| {
                metas.get(&hash).map(|(identity, range)| Evidence {
                    identity: identity.clone(),
                    snapshot: snapshot.clone(),
                    range: range.clone(),
                    code_snippet: snippets.get(&hash).cloned().unwrap_or_default(),
                    score,
                    retrieval_mode: mode,
                })
            })
            .collect())
    }

    /// `context`: runs `search` plus `relations` and compiles a
    /// [`lynx_protocol::ContextPackage`] bounded to `token_budget` tokens.
    pub fn context(
        &self,
        query: &str,
        token_budget: usize,
    ) -> Result<lynx_protocol::ContextPackage, CoreError> {
        Ok(self
            .context_with(query, token_budget, &QueryOptions::default())?
            .0)
    }

    /// [`Engine::context`] with an explicit query lifecycle. Returns the package
    /// together with the diagnostics of the underlying search.
    pub fn context_with(
        &self,
        query: &str,
        token_budget: usize,
        opts: &QueryOptions,
    ) -> Result<(lynx_protocol::ContextPackage, QueryReport), CoreError> {
        let outcome = self.search_with(query, RetrievalMode::Hybrid, 16, opts)?;
        let search = outcome.evidence;
        let mut relations: Vec<Relation> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for evidence in &search {
            if let Some(state) = terminal_state(opts) {
                return Err(terminal_error(state));
            }
            let incident = self
                .dual
                .graph()
                .get_relations(&evidence.identity.content_hash, None)?;
            for relation in incident {
                let key = format!(
                    "{}|{}|{}",
                    relation.source_id, relation.target_id, relation.kind as u8
                );
                if seen.insert(key) {
                    relations.push(relation);
                }
            }
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let capability = self.status.lock().unwrap().capability_level;
        let compiler = ContextCompiler::new();
        Ok((
            compiler.compile(
                query.to_string(),
                snapshot,
                search,
                relations,
                capability,
                token_budget,
            ),
            outcome.report,
        ))
    }
}

/// An empty workspace snapshot.
fn empty_snapshot() -> Snapshot {
    Snapshot {
        commit_hash: None,
        workspace_root: PathBuf::from(""),
        is_dirty: false,
        content_hash: String::new(),
    }
}

/// Parses a persisted capability-level wire value.
fn parse_capability(value: &str) -> Option<CapabilityLevel> {
    match value {
        "L0Text" => Some(CapabilityLevel::L0Text),
        "L1Parsed" => Some(CapabilityLevel::L1Parsed),
        "L2Symbolized" => Some(CapabilityLevel::L2Symbolized),
        "L3Structural" => Some(CapabilityLevel::L3Structural),
        _ => None,
    }
}

/// Resolves workspace provenance from git.
fn resolve_snapshot(repo_path: &Path) -> Result<Snapshot, CoreError> {
    let commit_hash = run_git(repo_path, &["rev-parse", "HEAD"]);
    let is_dirty = run_git(repo_path, &["status", "--porcelain"])
        .map(|output| !output.trim().is_empty())
        .unwrap_or(false);
    let content_hash = tree_hash(repo_path);
    Ok(Snapshot {
        commit_hash,
        workspace_root: repo_path.to_path_buf(),
        is_dirty,
        content_hash,
    })
}

/// Runs a git subcommand, returning trimmed stdout on success.
fn run_git(repo_path: &Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Deterministic content hash over every regular file under `repo_path`,
/// ignoring excluded segments.
fn tree_hash(repo_path: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut paths: Vec<PathBuf> = WalkDir::new(repo_path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            !should_skip(
                entry.path().strip_prefix(repo_path).unwrap_or(entry.path()),
                false,
            )
        })
        .map(|entry| entry.into_path())
        .collect();
    paths.sort();
    for path in paths {
        let relative = path.strip_prefix(repo_path).unwrap_or(&path);
        if let Ok(bytes) = std::fs::read(&path) {
            hasher.update(relative.to_string_lossy().as_bytes());
            hasher.update(&[0]);
            hasher.update(&bytes);
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// Walks `repo_path`, returning `(relative_path, content_hash)` for every
/// indexable file, sorted by path.
///
/// "Indexable" mirrors the indexing predicate: not excluded by
/// [`should_skip`], valid UTF-8 (a binary file cannot be parsed to text), and
/// within [`MAX_INDEX_FILE_BYTES`]. Only path/hash pairs are retained, so the
/// scan is `O(files)` in metadata, never repository bytes.
fn scan_workspace(
    repo_path: &Path,
    include_tests: bool,
) -> Result<Vec<(String, String)>, CoreError> {
    let mut out: Vec<(String, String)> = Vec::new();
    for entry in WalkDir::new(repo_path).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(repo_path).unwrap_or(path);
        if should_skip(relative, include_tests) {
            continue;
        }
        if entry.metadata().map(|meta| meta.len()).unwrap_or(0) > MAX_INDEX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if std::str::from_utf8(&bytes).is_err() {
            continue; // binary: not a source file
        }
        out.push((relative.to_string_lossy().to_string(), hash_bytes(&bytes)));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Hex `blake3` digest of raw bytes: the per-file content identity used by the
/// incremental diff.
fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Whether a workspace-relative path should be excluded from indexing.
fn should_skip(relative: &Path, include_tests: bool) -> bool {
    let text = relative.to_string_lossy();
    if EXCLUDED_SEGMENTS
        .iter()
        .any(|segment| text.split('/').any(|part| part == *segment))
    {
        return true;
    }
    if include_tests {
        return false;
    }
    text.split('/').any(|part| {
        part == "test"
            || part == "tests"
            || part == "mock"
            || part == "mocks"
            || part == "generated"
    })
}

/// Numeric rank of a path's noise tier (production < test < vendor), used to
/// order ambiguous exact matches so production code wins deterministically.
fn noise_rank(path: &Path) -> u8 {
    match classify_path(path) {
        PathNoise::None => 0,
        PathNoise::Test => 1,
        PathNoise::Vendor => 2,
    }
}

/// Numeric rank of a capability level, used to track the strongest guarantee
/// seen while indexing (since `CapabilityLevel` does not implement `Ord`).
fn capability_rank(level: CapabilityLevel) -> u8 {
    match level {
        CapabilityLevel::L0Text => 0,
        CapabilityLevel::L1Parsed => 1,
        CapabilityLevel::L2Symbolized => 2,
        CapabilityLevel::L3Structural => 3,
    }
}

/// Wire name of a capability level, for persistence.
fn capability_name(level: CapabilityLevel) -> &'static str {
    match level {
        CapabilityLevel::L0Text => "L0Text",
        CapabilityLevel::L1Parsed => "L1Parsed",
        CapabilityLevel::L2Symbolized => "L2Symbolized",
        CapabilityLevel::L3Structural => "L3Structural",
    }
}

/// Highest capability reachable for a path's language.
fn adapter_capability(path: &Path) -> CapabilityLevel {
    match lynx_parser::language_for_path(path) {
        Language::Rust
        | Language::Go
        | Language::TypeScript
        | Language::JavaScript
        | Language::Python => CapabilityLevel::L3Structural,
        Language::Markdown
        | Language::Yaml
        | Language::Json
        | Language::Toml
        | Language::Generic => CapabilityLevel::L1Parsed,
    }
}

/// Builds the bounded text fed to the embedding model for one symbol.
///
/// Format: `"{fqdn} {Kind}\n{snippet-prefix}"`. The fqdn and kind carry strong
/// lexical signal (e.g. `validate_token`, `Trait`) that raw excerpts dilute;
/// the cap keeps every member of a batch short so padding cannot inflate cost.
fn embedding_text(identity: &SymbolIdentity, snippet: &str) -> String {
    let is_module = matches!(identity.kind, SymbolKind::Module);
    let budget = if is_module {
        MAX_MODULE_EMBED_CHARS
    } else {
        MAX_EMBED_CHARS
    };
    let mut text = String::with_capacity(budget + 64);
    text.push_str(&identity.fqdn);
    text.push(' ');
    text.push_str(&format!("{:?}", identity.kind));
    text.push('\n');
    let remaining = budget.saturating_sub(text.len());
    if remaining > 0 {
        text.extend(snippet.chars().take(remaining));
    }
    text
}

/// Computes one non-overlapping range+snippet per symbol for a whole file.
///
/// Anchors each symbol at the first whole-word occurrence of its trailing name,
/// orders symbols by that anchor, and ends each excerpt at the next symbol's
/// anchor (bounded by [`MAX_SNIPPET_LINES`]). Module pseudo-symbols retain a
/// bounded head of the file ([`MAX_MODULE_SNIPPET_LINES`]).
fn compute_file_snippets(
    source: &str,
    identities: &[SymbolIdentity],
) -> Vec<(SourceRange, String)> {
    let total_lines = source.lines().count().max(1);

    let mut anchors: Vec<(usize, usize)> = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| {
            let line = if matches!(identity.kind, SymbolKind::Module) {
                1
            } else {
                find_whole_word(source, identity.trailing_name())
                    .map(|offset| byte_offset_line(source, offset).max(1))
                    .unwrap_or(1)
            };
            (line, index)
        })
        .collect();

    anchors.sort_by_key(|(line, index)| (*line, *index));

    let mut snippets: Vec<Option<(SourceRange, String)>> = vec![None; identities.len()];
    for position in 0..anchors.len() {
        let (start_line, index) = anchors[position];
        let identity = &identities[index];
        let (end_line, snippet) = if matches!(identity.kind, SymbolKind::Module) {
            let end = total_lines.min(MAX_MODULE_SNIPPET_LINES);
            (end, slice_lines(source, 1, end))
        } else {
            let next_line = anchors
                .get(position + 1)
                .map(|(line, _)| *line)
                .unwrap_or(total_lines + 1);
            let cap = start_line.saturating_add(MAX_SNIPPET_LINES - 1);
            let end = cap.min(next_line.saturating_sub(1)).max(start_line);
            let end = end.min(total_lines);
            (end, slice_lines(source, start_line, end))
        };
        let start_byte = line_start_byte(source, start_line);
        let end_byte = line_end_byte(source, end_line);
        let range = SourceRange::new(start_line, end_line, start_byte, end_byte)
            .unwrap_or(SourceRange::new(1, 1, 0, 0).unwrap());
        snippets[index] = Some((range, snippet));
    }

    snippets
        .into_iter()
        .map(|entry| {
            entry.unwrap_or_else(|| (SourceRange::new(1, 1, 0, 0).unwrap(), String::new()))
        })
        .collect()
}

/// Returns the source text on inclusive 1-based lines `[start, end]`.
fn slice_lines(source: &str, start: usize, end: usize) -> String {
    let from = line_start_byte(source, start);
    let to = line_end_byte(source, end).min(source.len());
    if from >= to {
        return String::new();
    }
    source[from..to].to_string()
}

/// First byte offset of `needle` in `source` bounded by identifier characters,
/// so `find_whole_word("find", "find")` does not match inside `find_all`.
fn find_whole_word(source: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let bytes = source.as_bytes();
    let len = needle.len();
    let mut from = 0usize;
    while let Some(relative) = source[from..].find(needle) {
        let start = from + relative;
        let end = start + len;
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return Some(start);
        }
        from = start + 1;
    }
    None
}

/// Whether `byte` continues an identifier on either side of a match.
fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// 1-based line number containing byte `offset`.
fn byte_offset_line(source: &str, offset: usize) -> usize {
    source[..offset].bytes().filter(|b| *b == b'\n').count() + 1
}

/// Byte offset of the first byte of 1-based `line`.
fn line_start_byte(source: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    let mut count = 0usize;
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            count += 1;
            if count == line - 1 {
                return index + 1;
            }
        }
    }
    source.len()
}

/// Byte offset just past the last byte of 1-based `line` (excluding newline).
fn line_end_byte(source: &str, line: usize) -> usize {
    let mut current_line = 1usize;
    for (index, byte) in source.bytes().enumerate() {
        if current_line == line && byte == b'\n' {
            return index;
        }
        if byte == b'\n' {
            current_line += 1;
        }
    }
    source.len()
}

/// Protocol wire-form of a language, for the Tantivy `language` field.
fn wire_language(language: Language) -> &'static str {
    match language {
        Language::Rust => "Rust",
        Language::Go => "Go",
        Language::TypeScript => "TypeScript",
        Language::JavaScript => "JavaScript",
        Language::Python => "Python",
        Language::Markdown => "Markdown",
        Language::Yaml => "Yaml",
        Language::Json => "Json",
        Language::Toml => "Toml",
        Language::Generic => "Generic",
    }
}

/// Rejects embeddings whose length mismatches the provider's declared dimension.
fn check_dimension(provider: &dyn VectorProvider, vec: &[f32]) -> Result<(), CoreError> {
    if vec.len() == provider.dimension() {
        Ok(())
    } else {
        Err(CoreError::DimensionMismatch {
            expected: provider.dimension(),
            actual: vec.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lynx_embed::EmbedError;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    /// Deterministic fake provider: maps a text to a fixed vector seeded by a
    /// hash of its characters, so tests never need the ONNX model.
    struct FakeProvider;

    impl VectorProvider for FakeProvider {
        fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
            Ok(fake_vec(text))
        }
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
            Ok(texts.iter().map(|t| fake_vec(t)).collect())
        }
        fn dimension(&self) -> usize {
            8
        }
    }

    /// Counts embedding calls (via a shared counter) so a test can prove a
    /// hydrated engine does not re-embed — i.e. does not re-index.
    struct CountingProvider(std::sync::Arc<AtomicUsize>);

    impl VectorProvider for CountingProvider {
        fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
            Ok(fake_vec(text))
        }
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
            self.0.fetch_add(texts.len(), AtomicOrdering::SeqCst);
            Ok(texts.iter().map(|t| fake_vec(t)).collect())
        }
        fn dimension(&self) -> usize {
            8
        }
    }

    fn fake_vec(text: &str) -> Vec<f32> {
        let mut out = [0.0_f32; 8];
        for (i, ch) in text.chars().enumerate() {
            out[i % 8] += ch as u32 as f32;
        }
        let norm = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in out.iter_mut() {
                *x /= norm;
            }
        }
        out.to_vec()
    }

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_file(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn indexed_engine() -> (tempfile::TempDir, tempfile::TempDir, Engine) {
        let ws = temp_root();
        write_file(
            ws.path(),
            "auth/service.go",
            "package auth\n\ntype User struct {\n    Name string\n}\n\nfunc (u *User) Name() string { return u.Name }\n\nfunc Validate(u *User) bool { return u.Name() != \"\" }\n",
        );
        let storage = temp_root();
        let engine = Engine::new(storage.path(), Box::new(FakeProvider)).unwrap();
        engine.index_repository(ws.path(), false).unwrap();
        (ws, storage, engine)
    }

    #[test]
    fn index_populates_status_and_resolve() {
        let (_ws, _storage, engine) = indexed_engine();
        let status = engine.index_status();
        assert!(status.symbol_count > 0);
        assert_eq!(status.file_count, 1);
        assert!(
            capability_rank(status.capability_level)
                >= capability_rank(CapabilityLevel::L3Structural)
        );

        let resolved = engine.resolve("auth::Validate").unwrap().unwrap();
        assert_eq!(resolved.identity.fqdn, "auth::Validate");
        assert_eq!(resolved.retrieval_mode, RetrievalMode::Structural);
        assert!(!resolved.code_snippet.is_empty());
    }

    #[test]
    fn resolve_by_trailing_name() {
        let (_ws, _storage, engine) = indexed_engine();
        let resolved = engine.resolve("Validate").unwrap().unwrap();
        assert_eq!(resolved.identity.fqdn, "auth::Validate");
    }

    #[test]
    fn resolve_missing_returns_none() {
        let (_ws, _storage, engine) = indexed_engine();
        assert!(engine.resolve("nope::Missing").unwrap().is_none());
    }

    #[test]
    fn inspect_by_hash() {
        let (_ws, _storage, engine) = indexed_engine();
        let resolved = engine.resolve("auth::Validate").unwrap().unwrap();
        let hash = resolved.identity.content_hash.clone();
        let inspected = engine.inspect(&hash).unwrap().unwrap();
        assert_eq!(inspected.identity.fqdn, "auth::Validate");
    }

    #[test]
    fn relations_and_trace() {
        let (_ws, _storage, engine) = indexed_engine();
        let resolved = engine.resolve("auth::Validate").unwrap().unwrap();
        let hash = resolved.identity.content_hash;
        let rels = engine.relations(&hash, Some(RelationKind::Calls)).unwrap();
        assert!(!rels.is_empty());

        let trace = engine.trace(&hash, 2).unwrap();
        assert!(trace.len() >= rels.len());
    }

    #[test]
    fn search_returns_ranked_evidence() {
        let (_ws, _storage, engine) = indexed_engine();
        let results = engine
            .search("Validate", RetrievalMode::Hybrid, 10)
            .unwrap();
        assert!(!results.is_empty());
        assert!(results[0].score >= results.last().map(|l| l.score).unwrap_or(0.0));
        // Returned evidence carries its snippet (fetched post-truncation).
        assert!(results.iter().any(|e| !e.code_snippet.is_empty()));
    }

    #[test]
    fn similar_excludes_self() {
        let (_ws, _storage, engine) = indexed_engine();
        let resolved = engine.resolve("auth::Validate").unwrap().unwrap();
        let hash = resolved.identity.content_hash;
        let similar = engine.similar(&hash, 10).unwrap();
        assert!(!similar.iter().any(|e| e.identity.content_hash == hash));
    }

    #[test]
    fn context_stays_within_budget() {
        let (_ws, _storage, engine) = indexed_engine();
        let package = engine.context("Validate", 30).unwrap();
        assert!(package.estimated_tokens <= 30);
        assert!(!package.primary_evidence.is_empty());
        assert_eq!(package.target_query, "Validate");
    }

    #[test]
    fn search_is_deterministic() {
        let (_ws, _storage, engine) = indexed_engine();
        let a = engine
            .search("Validate", RetrievalMode::Hybrid, 10)
            .unwrap();
        let b = engine
            .search("Validate", RetrievalMode::Hybrid, 10)
            .unwrap();
        let hashes = |v: &[Evidence]| {
            v.iter()
                .map(|e| e.identity.content_hash.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(hashes(&a), hashes(&b));
    }

    #[test]
    fn resolve_is_deterministic_under_duplicate_names() {
        let ws = temp_root();
        write_file(
            ws.path(),
            "a/one.go",
            "package a\n\nfunc Dup() int { return 1 }\n",
        );
        write_file(
            ws.path(),
            "b/two.go",
            "package b\n\nfunc Dup() int { return 2 }\n",
        );
        let storage = temp_root();
        let engine = Engine::new(storage.path(), Box::new(FakeProvider)).unwrap();
        engine.index_repository(ws.path(), false).unwrap();

        let first = engine.resolve("Dup").unwrap().unwrap();
        for _ in 0..20 {
            let again = engine.resolve("Dup").unwrap().unwrap();
            assert_eq!(first.identity.fqdn, again.identity.fqdn);
            assert_eq!(first.identity.content_hash, again.identity.content_hash);
        }
        assert_eq!(first.identity.fqdn, "a::Dup");
    }

    /// The headline persistence invariant: a second `Engine` over the same
    /// storage root hydrates and answers queries **without re-indexing**.
    #[test]
    fn reopening_hydrates_without_reindexing() {
        let ws = temp_root();
        write_file(
            ws.path(),
            "auth/service.go",
            "package auth\n\nfunc Validate(u string) bool { return u != \"\" }\n",
        );
        let storage = temp_root();
        let first_counter = std::sync::Arc::new(AtomicUsize::new(0));
        let first = Engine::new(
            storage.path(),
            Box::new(CountingProvider(first_counter.clone())),
        )
        .unwrap();
        let status = first.index_repository(ws.path(), false).unwrap();
        assert!(status.symbol_count > 0);
        assert!(first_counter.load(AtomicOrdering::SeqCst) > 0);
        drop(first);

        // Reopen: hydration must not embed anything.
        let reopened_counter = std::sync::Arc::new(AtomicUsize::new(0));
        let second = Engine::new(
            storage.path(),
            Box::new(CountingProvider(reopened_counter.clone())),
        )
        .unwrap();
        let reopened = second.index_status();
        assert_eq!(reopened.symbol_count, status.symbol_count);
        assert_eq!(reopened.file_count, status.file_count);
        assert_eq!(reopened.capability_level, CapabilityLevel::L3Structural);
        assert_eq!(reopened_counter.load(AtomicOrdering::SeqCst), 0);
        // And resolution + lexical search work straight from storage.
        assert!(second.resolve("auth::Validate").unwrap().is_some());
        assert!(!second
            .search("Validate", RetrievalMode::Lexical, 5)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reindexing_removes_stale_symbols() {
        let ws = temp_root();
        write_file(
            ws.path(),
            "a.rs",
            "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { 2 }\n",
        );
        let storage = temp_root();
        let engine = Engine::new(storage.path(), Box::new(FakeProvider)).unwrap();
        engine.index_repository(ws.path(), false).unwrap();
        assert!(engine.resolve("beta").unwrap().is_some());

        std::fs::write(ws.path().join("a.rs"), "pub fn alpha() -> i32 { 1 }\n").unwrap();
        engine.index_repository(ws.path(), false).unwrap();
        assert!(engine.resolve("alpha").unwrap().is_some());
        assert!(engine.resolve("beta").unwrap().is_none());
    }

    #[test]
    fn search_with_precancelled_token_returns_cancelled() {
        let (_ws, _storage, engine) = indexed_engine();
        let opts = QueryOptions::new();
        opts.cancel();
        let err = engine
            .search_with("Validate", RetrievalMode::Hybrid, 10, &opts)
            .unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
    }

    #[test]
    fn search_with_expired_deadline_returns_timed_out() {
        let (_ws, _storage, engine) = indexed_engine();
        let mut opts = QueryOptions::new();
        opts.deadline = Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
        let err = engine
            .search_with("Validate", RetrievalMode::Hybrid, 10, &opts)
            .unwrap_err();
        assert!(matches!(err, CoreError::TimedOut));
    }

    #[test]
    fn search_candidates_are_bounded_and_reported() {
        let (_ws, _storage, engine) = indexed_engine();
        let opts = QueryOptions::new();
        let outcome = engine
            .search_with("Validate", RetrievalMode::Hybrid, 3, &opts)
            .unwrap();
        assert_eq!(outcome.report.state, QueryState::Completed);
        assert!(outcome.report.fused_candidates <= 2 * CANDIDATE_FLOOR);
        assert_eq!(outcome.report.results, outcome.evidence.len());
        assert!(!outcome.report.stages.is_empty());
    }

    #[test]
    fn similar_is_cancellable() {
        let (_ws, _storage, engine) = indexed_engine();
        let hash = engine
            .resolve("auth::Validate")
            .unwrap()
            .unwrap()
            .identity
            .content_hash;
        let opts = QueryOptions::new();
        opts.cancel();
        assert!(matches!(
            engine.similar_with(&hash, 5, &opts).unwrap_err(),
            CoreError::Cancelled
        ));
    }

    #[test]
    fn find_whole_word_respects_identifier_boundaries() {
        assert_eq!(find_whole_word("let find_all = 1;", "find"), None);
        assert_eq!(find_whole_word("call find(x)", "find"), Some(5));
        assert_eq!(find_whole_word("xfind", "find"), None);
    }

    #[test]
    fn symbol_snippets_are_non_overlapping_and_anchored() {
        let ws = temp_root();
        write_file(
            ws.path(),
            "m.rs",
            "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { alpha() }\npub fn gamma() -> i32 { 3 }\n",
        );
        let storage = temp_root();
        let engine = Engine::new(storage.path(), Box::new(FakeProvider)).unwrap();
        engine.index_repository(ws.path(), false).unwrap();

        let mut ranges: Vec<(usize, usize)> = ["m::alpha", "m::beta", "m::gamma"]
            .iter()
            .map(|name| {
                let evidence = engine.resolve(name).unwrap().unwrap();
                (evidence.range.start_line, evidence.range.end_line)
            })
            .collect();
        ranges.sort();
        assert_eq!(ranges, vec![(1, 1), (2, 2), (3, 3)]);
        for window in ranges.windows(2) {
            assert!(window[0].1 < window[1].0);
        }
    }
}
