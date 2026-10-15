//! The core [`Engine`]: indexing pipeline and the eight retrieval primitives.

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
    apply_definition_boost, apply_noise_penalty, classify_query, cosine_similarity, finalize_fused,
    rrf_fuse, top_k_ranked, top_k_scored, QueryClass,
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

/// A symbol retained in memory so the engine can reconstruct [`Evidence`]
/// (the storage substrate stores identity + range but not the raw snippet).
#[derive(Debug, Clone)]
pub struct StoredSymbol {
    /// Frozen identity of the symbol.
    pub identity: SymbolIdentity,
    /// Byte/line range of the retained excerpt.
    pub range: SourceRange,
    /// Verbatim source excerpt.
    pub code_snippet: String,
}

impl StoredSymbol {
    /// Projects this stored symbol into an [`Evidence`] item.
    pub fn to_evidence(&self, snapshot: &Snapshot, score: f32, mode: RetrievalMode) -> Evidence {
        Evidence {
            identity: self.identity.clone(),
            snapshot: snapshot.clone(),
            range: self.range.clone(),
            code_snippet: self.code_snippet.clone(),
            score,
            retrieval_mode: mode,
        }
    }
}

/// Deterministic exact-resolution index: maps fqdn and trailing name to the
/// symbol's content hash in `O(1)`.
///
/// A trailing name may be shared by several fqdns; the winner is the
/// lexicographically smallest fqdn (then hash), so resolution is deterministic
/// across runs and independent of `HashMap` iteration order.
#[derive(Debug, Default, Clone)]
struct ResolveIndex {
    by_fqdn: HashMap<String, String>,
    by_name: HashMap<String, NameWinner>,
}

/// The deterministically selected winner for a shared trailing name.
#[derive(Debug, Clone)]
struct NameWinner {
    fqdn: String,
    content_hash: String,
}

impl ResolveIndex {
    /// Inserts one symbol, keeping the deterministic winner per trailing name.
    fn insert(&mut self, identity: &SymbolIdentity) {
        self.by_fqdn
            .entry(identity.fqdn.clone())
            .or_insert_with(|| identity.content_hash.clone());
        let name = trailing_segment(&identity.fqdn).to_string();
        match self.by_name.get(&name) {
            Some(existing)
                if (existing.fqdn.as_str(), existing.content_hash.as_str())
                    <= (identity.fqdn.as_str(), identity.content_hash.as_str()) => {}
            _ => {
                self.by_name.insert(
                    name,
                    NameWinner {
                        fqdn: identity.fqdn.clone(),
                        content_hash: identity.content_hash.clone(),
                    },
                );
            }
        }
    }

    /// Resolves an exact fqdn or a trailing name to a content hash.
    fn lookup(&self, needle: &str) -> Option<&str> {
        if let Some(hash) = self.by_fqdn.get(needle) {
            return Some(hash.as_str());
        }
        self.by_name
            .get(needle)
            .map(|winner| winner.content_hash.as_str())
    }
}

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

/// The core engine: combines a [`DualStorage`] substrate, a [`Parser`], a
/// [`VectorProvider`], and an active workspace [`Snapshot`].
pub struct Engine {
    dual: DualStorage,
    parser: Parser,
    provider: Box<dyn VectorProvider>,
    snapshot: Mutex<Snapshot>,
    symbols: Mutex<HashMap<String, StoredSymbol>>,
    embeddings: Mutex<HashMap<String, Vec<f32>>>,
    resolve_index: Mutex<ResolveIndex>,
    status: Mutex<IndexStatus>,
}

impl Engine {
    /// Opens an engine rooted at `storage_root`, using `provider` for vectors.
    pub fn new(storage_root: &Path, provider: Box<dyn VectorProvider>) -> Result<Self, CoreError> {
        let dual = DualStorage::open(storage_root)?;
        let empty_snapshot = Snapshot {
            commit_hash: None,
            workspace_root: PathBuf::from(""),
            is_dirty: false,
            content_hash: String::new(),
        };
        let status = IndexStatus {
            snapshot: empty_snapshot.clone(),
            file_count: 0,
            symbol_count: 0,
            relation_count: 0,
            capability_level: CapabilityLevel::L0Text,
        };
        Ok(Self {
            dual,
            parser: Parser::new(),
            provider,
            snapshot: Mutex::new(empty_snapshot),
            symbols: Mutex::new(HashMap::new()),
            embeddings: Mutex::new(HashMap::new()),
            resolve_index: Mutex::new(ResolveIndex::default()),
            status: Mutex::new(status),
        })
    }

    /// Indexes every supported source file under `repo_path`.
    ///
    /// Pipeline per file: git provenance -> [`Parser::parse`] ->
    /// [`DualStorage::insert_symbols`] & [`DualStorage::insert_relations`] ->
    /// Tantivy indexing + vector embedding.
    pub fn index_repository(
        &self,
        repo_path: &Path,
        include_tests: bool,
    ) -> Result<IndexStatus, CoreError> {
        let snapshot = resolve_snapshot(repo_path)?;
        self.dual.graph().save_snapshot(&snapshot)?;

        let mut symbols: HashMap<String, StoredSymbol> = HashMap::new();
        let mut embeddings: HashMap<String, Vec<f32>> = HashMap::new();
        let mut resolve_index = ResolveIndex::default();
        let mut relations: Vec<Relation> = Vec::new();
        let mut file_count = 0usize;
        let mut max_capability = CapabilityLevel::L0Text;

        for entry in WalkDir::new(repo_path).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let relative = path.strip_prefix(repo_path).unwrap_or(path);
            if should_skip(relative, include_tests) {
                continue;
            }
            let source = match std::fs::read_to_string(path) {
                Ok(source) => source,
                Err(_) => continue, // binary or unreadable
            };
            let (parsed_symbols, parsed_relations) = match self.parser.parse(relative, &source) {
                Ok(pair) => pair,
                Err(_) => continue, // unparseable file
            };
            file_count += 1;
            if capability_rank(adapter_capability(relative)) > capability_rank(max_capability) {
                max_capability = adapter_capability(relative);
            }

            let snippets = compute_file_snippets(&source, &parsed_symbols);
            let file_identities: Vec<(SymbolIdentity, SourceRange, String)> = parsed_symbols
                .iter()
                .cloned()
                .zip(snippets)
                .map(|(identity, (range, snippet))| (identity, range, snippet))
                .collect();
            // Persist symbols + relations in one graph transaction.
            let entries: Vec<(SymbolIdentity, SourceRange)> = file_identities
                .iter()
                .map(|(identity, range, _)| (identity.clone(), range.clone()))
                .collect();
            self.dual
                .graph()
                .insert_symbols(&entries, &snapshot.content_hash)?;
            self.dual
                .graph()
                .insert_relations(&parsed_relations, &snapshot.content_hash)?;

            // Index snippets into Tantivy.
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

            for ((identity, range, snippet), vector) in file_identities.into_iter().zip(vectors) {
                resolve_index.insert(&identity);
                embeddings.insert(identity.content_hash.clone(), vector);
                symbols.insert(
                    identity.content_hash.clone(),
                    StoredSymbol {
                        identity,
                        range,
                        code_snippet: snippet,
                    },
                );
            }
            relations.extend(parsed_relations);
        }

        // Flush the shared lexical writer once, at the end of the pass, so the
        // on-disk index is quiescent before the session is persisted or served.
        self.dual.lexical().commit()?;

        *self.snapshot.lock().unwrap() = snapshot.clone();
        *self.symbols.lock().unwrap() = symbols;
        *self.embeddings.lock().unwrap() = embeddings;
        *self.resolve_index.lock().unwrap() = resolve_index;

        let symbol_count = self.symbols.lock().unwrap().len();
        let relation_count = relations.len();
        let status = IndexStatus {
            snapshot,
            file_count,
            symbol_count,
            relation_count,
            capability_level: max_capability,
        };
        *self.status.lock().unwrap() = status.clone();
        Ok(status)
    }

    /// Reports the current index state.
    pub fn index_status(&self) -> IndexStatus {
        self.status.lock().unwrap().clone()
    }

    /// `search`: fuses BM25 (Tantivy) with vector similarity via Reciprocal
    /// Rank Fusion, applies definition boosting, and returns ranked evidence.
    ///
    /// Bounded and cancellable: see [`Engine::search_with`]. Uses default query
    /// options (no deadline, no cancellation).
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
    /// - the semantic scan is `O(N log k)` and observes cancellation/deadline at
    ///   every candidate, so a caller can stop a pathological query without
    ///   killing the process;
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

        // Deterministic exact resolution for bare symbol queries: this must not
        // be a ranking problem. A miss degrades gracefully to hybrid retrieval.
        let exact = if classify_query(query) == QueryClass::Symbol {
            self.resolve(query).ok().flatten()
        } else {
            None
        };

        let symbols = self.symbols.lock().unwrap();
        let embeddings = self.embeddings.lock().unwrap();
        if symbols.is_empty() {
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
            let stop = |opts: &QueryOptions| opts.is_expired();
            // Bounded, cancellable top-k: O(N log k) time, O(k) memory, and only
            // the retained hashes are allocated. `take_while` observes the
            // cancellation/deadline flag per candidate.
            semantic = top_k_ranked(
                embeddings
                    .iter()
                    .take_while(|_| !stop(opts))
                    .filter_map(|(hash, vec)| {
                        cosine_similarity(&query_vec, vec).map(|sim| (hash.as_str(), sim))
                    }),
                candidates,
            );
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
        let mut evidence: Vec<Evidence> = fused
            .into_iter()
            .filter_map(|(hash, score)| {
                symbols
                    .get(&hash)
                    .map(|stored| stored.to_evidence(&snapshot, score, mode))
            })
            .collect();
        if evidence.is_empty() {
            // Fall back to the single best channel when one was empty.
            let list = if lexical.is_empty() {
                &semantic
            } else {
                &lexical
            };
            evidence = list
                .iter()
                .filter_map(|(hash, _)| {
                    symbols
                        .get(hash)
                        .map(|stored| stored.to_evidence(&snapshot, 0.0, mode))
                })
                .collect();
        }
        apply_definition_boost(&mut evidence);
        apply_noise_penalty(&mut evidence);
        let mut ranked = finalize_fused(evidence);

        // Prepend the deterministic exact hit (if any), removing it from the
        // ranked list so it appears once, in first position.
        if let Some(exact) = exact {
            let exact_hash = exact.identity.content_hash.clone();
            ranked.retain(|item| item.identity.content_hash != exact_hash);
            let mut with_exact = Vec::with_capacity(ranked.len() + 1);
            with_exact.push(exact);
            with_exact.extend(ranked);
            ranked = with_exact;
        }
        ranked.truncate(limit);
        report.stage("rank", stage.elapsed());

        let results = ranked.len();
        let contributions = crate::retrieval::rank_contributions(&ranked, &lexical, &semantic);
        Ok(SearchOutcome {
            evidence: ranked,
            report: report.finish(QueryState::Completed, results),
            contributions,
        })
    }

    /// `resolve`: exact lookup of a symbol by fqdn or trailing name.
    ///
    /// Deterministic and `O(1)`: the identity is resolved through the
    /// precomputed index, never a linear scan, and the resolved result is
    /// independent of hash-map iteration order.
    pub fn resolve(&self, fqdn_or_name: &str) -> Result<Option<Evidence>, CoreError> {
        let resolve_index = self.resolve_index.lock().unwrap();
        if resolve_index.by_fqdn.is_empty() {
            return Err(CoreError::NotIndexed);
        }
        let needle = fqdn_or_name.trim();
        let Some(hash) = resolve_index.lookup(needle) else {
            return Ok(None);
        };
        let hash = hash.to_string();
        drop(resolve_index);

        let symbols = self.symbols.lock().unwrap();
        let snapshot = self.snapshot.lock().unwrap().clone();
        Ok(symbols
            .get(&hash)
            .map(|stored| stored.to_evidence(&snapshot, 1.0, RetrievalMode::Structural)))
    }

    /// `inspect`: full evidence details for a symbol by content hash.
    pub fn inspect(&self, symbol_hash: &str) -> Result<Option<Evidence>, CoreError> {
        let symbols = self.symbols.lock().unwrap();
        if symbols.is_empty() {
            return Err(CoreError::NotIndexed);
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let stored = match symbols.get(symbol_hash) {
            Some(stored) => stored,
            None => {
                // Confirm via storage whether the hash is merely uncached.
                let confirmed = self.dual.get_symbol_by_hash(symbol_hash)?;
                return Ok(confirmed.map(|identity| Evidence {
                    identity,
                    snapshot: snapshot.clone(),
                    range: SourceRange::new(1, 1, 0, 0).unwrap(),
                    code_snippet: String::new(),
                    score: 0.0,
                    retrieval_mode: RetrievalMode::Structural,
                }));
            }
        };
        Ok(Some(stored.to_evidence(
            &snapshot,
            1.0,
            RetrievalMode::Structural,
        )))
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
    /// Bounded (`O(N log k)`) and cancellable via [`QueryOptions`].
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
        let symbols = self.symbols.lock().unwrap();
        let embeddings = self.embeddings.lock().unwrap();
        if symbols.is_empty() {
            return Err(CoreError::NotIndexed);
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let target = embeddings.get(symbol_hash).ok_or(CoreError::NotIndexed)?;
        // Bounded top-k: O(N log k) time, O(k) memory, preserving cosine scores.
        let ranked = top_k_scored(
            embeddings
                .iter()
                .take_while(|_| !opts.is_expired())
                .filter(|(hash, _)| hash.as_str() != symbol_hash)
                .filter_map(|(hash, vec)| {
                    cosine_similarity(target, vec).map(|sim| (hash.as_str(), sim))
                }),
            limit,
        );
        if let Some(state) = terminal_state(opts) {
            return Err(terminal_error(state));
        }
        Ok(ranked
            .into_iter()
            .filter_map(|(hash, score)| {
                symbols
                    .get(&hash)
                    .map(|stored| stored.to_evidence(&snapshot, score, RetrievalMode::Semantic))
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
///
/// This replaces the previous per-symbol `source.find(name)` 25-line window,
/// which anchored on the first *substring* match (often a call site or comment)
/// and produced heavily overlapping excerpts. Overlap multiplied embedding cost
/// (the same file text embedded once per symbol) and made reported ranges
/// ambiguous. The result is aligned one-to-one with `identities`.
fn compute_file_snippets(
    source: &str,
    identities: &[SymbolIdentity],
) -> Vec<(SourceRange, String)> {
    let total_lines = source.lines().count().max(1);

    // Anchor each symbol: modules at the file head, others at the first
    // whole-word occurrence of the trailing name.
    let mut anchors: Vec<(usize, usize)> = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| {
            let line = if matches!(identity.kind, SymbolKind::Module) {
                1
            } else {
                let name = trailing_segment(&identity.fqdn);
                find_whole_word(source, name)
                    .map(|offset| byte_offset_line(source, offset).max(1))
                    .unwrap_or(1)
            };
            (line, index)
        })
        .collect();

    // Order by anchor line for non-overlapping window boundaries; module (line
    // 1) sorts first, which is fine because its window is computed separately.
    anchors.sort_by_key(|(line, index)| (*line, *index));

    let mut snippets: Vec<Option<(SourceRange, String)>> = vec![None; identities.len()];
    for position in 0..anchors.len() {
        let (start_line, index) = anchors[position];
        let identity = &identities[index];
        let (end_line, snippet) = if matches!(identity.kind, SymbolKind::Module) {
            let end = total_lines.min(MAX_MODULE_SNIPPET_LINES);
            (end, slice_lines(source, 1, end))
        } else {
            // End just before the next symbol's anchor (or at the snippet cap).
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

/// Trailing identifier segment of an fqdn (`a::b.C` -> `C`).
fn trailing_segment(fqdn: &str) -> &str {
    match fqdn.rfind([':', '.']) {
        Some(index) => &fqdn[index + 1..],
        None => fqdn,
    }
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

        // Same query, same index, same config -> same logical result, every time.
        let first = engine.resolve("Dup").unwrap().unwrap();
        for _ in 0..20 {
            let again = engine.resolve("Dup").unwrap().unwrap();
            assert_eq!(first.identity.fqdn, again.identity.fqdn);
            assert_eq!(first.identity.content_hash, again.identity.content_hash);
        }
        // The winner is the lexicographically smallest fqdn (deterministic).
        assert_eq!(first.identity.fqdn, "a::Dup");
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
        // Fused candidates never exceed the sum of the two bounded channels.
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
        // Non-overlapping: each start is strictly after the previous end.
        for window in ranges.windows(2) {
            assert!(window[0].1 < window[1].0);
        }
    }
}
