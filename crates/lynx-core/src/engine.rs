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
use crate::retrieval::{
    apply_definition_boost, cosine_similarity, finalize_fused, rrf_fuse,
};

/// Maximum number of source lines captured as a per-symbol snippet.
const MAX_SNIPPET_LINES: usize = 25;

/// Files and directories always excluded from indexing.
const EXCLUDED_SEGMENTS: &[&str] = &[
    ".git", "node_modules", "vendor", "target", "build", "dist",
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
            status: Mutex::new(status),
        })
    }

    /// Indexes every supported source file under `repo_path`.
    ///
    /// Pipeline per file: git provenance -> [`Parser::parse`] ->
    /// [`DualStorage::insert_symbols`] & [`DualStorage::insert_relations`] ->
    /// Tantivy indexing + vector embedding.
    pub fn index_repository(&self, repo_path: &Path, include_tests: bool) -> Result<IndexStatus, CoreError> {
        let snapshot = resolve_snapshot(repo_path)?;
        self.dual.graph().save_snapshot(&snapshot)?;

        let mut symbols: HashMap<String, StoredSymbol> = HashMap::new();
        let mut embeddings: HashMap<String, Vec<f32>> = HashMap::new();
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
            let (parsed_symbols, parsed_relations) =
                match self.parser.parse(relative, &source) {
                    Ok(pair) => pair,
                    Err(_) => continue, // unparseable file
                };
            file_count += 1;
            if capability_rank(adapter_capability(relative)) > capability_rank(max_capability) {
                max_capability = adapter_capability(relative);
            }

            let module = parsed_symbols.first().cloned();
            let mut file_identities: Vec<(SymbolIdentity, SourceRange, String)> = Vec::new();
            for identity in &parsed_symbols {
                let is_module = Some(identity) == module.as_ref()
                    && matches!(identity.kind, SymbolKind::Module);
                let (range, snippet) = symbol_range(&source, identity, is_module);
                file_identities.push((identity.clone(), range, snippet));
            }

            // Persist symbols + relations in one graph transaction.
            let entries: Vec<(SymbolIdentity, SourceRange)> = file_identities
                .iter()
                .map(|(identity, range, _)| (identity.clone(), range.clone()))
                .collect();
            self.dual.graph().insert_symbols(&entries, &snapshot.content_hash)?;
            self.dual.graph().insert_relations(&parsed_relations, &snapshot.content_hash)?;

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

            // Embed snippets.
            let texts: Vec<&str> = file_identities.iter().map(|(_, _, s)| s.as_str()).collect();
            let vectors = self.provider.embed_batch(&texts)?;

            for ((identity, range, snippet), vector) in
                file_identities.into_iter().zip(vectors)
            {
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

        *self.snapshot.lock().unwrap() = snapshot.clone();
        *self.symbols.lock().unwrap() = symbols;
        *self.embeddings.lock().unwrap() = embeddings;

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
    pub fn search(
        &self,
        query: &str,
        mode: RetrievalMode,
        limit: usize,
    ) -> Result<Vec<Evidence>, CoreError> {
        let symbols = self.symbols.lock().unwrap();
        let embeddings = self.embeddings.lock().unwrap();
        if symbols.is_empty() {
            return Err(CoreError::NotIndexed);
        }

        let mut lexical: Vec<(String, usize)> = Vec::new();
        if matches!(mode, RetrievalMode::Lexical | RetrievalMode::Hybrid) {
            let hits = self.dual.lexical().search_snippets(query, limit.max(50))?;
            lexical = hits
                .into_iter()
                .enumerate()
                .map(|(rank, hit)| (hit.symbol_hash, rank))
                .collect();
        }

        let mut semantic: Vec<(String, usize)> = Vec::new();
        if matches!(mode, RetrievalMode::Semantic | RetrievalMode::Hybrid) {
            let query_vec = self.provider.embed_query(query)?;
            check_dimension(self.provider.as_ref(), &query_vec)?;
            let mut scored: Vec<(String, f32)> = embeddings
                .iter()
                .filter_map(|(hash, vec)| {
                    cosine_similarity(&query_vec, vec).map(|sim| (hash.clone(), sim))
                })
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            semantic = scored
                .into_iter()
                .enumerate()
                .map(|(rank, (hash, _))| (hash, rank))
                .collect();
        }

        let snapshot = self.snapshot.lock().unwrap().clone();
        let fused = rrf_fuse(&lexical, &semantic);
        let mut evidence: Vec<Evidence> = fused
            .into_iter()
            .filter_map(|(hash, score)| {
                symbols.get(&hash).map(|stored| {
                    stored.to_evidence(&snapshot, score, mode)
                })
            })
            .collect();
        if evidence.is_empty() {
            // Fall back to the single best channel when one was empty.
            let list = if lexical.is_empty() { &semantic } else { &lexical };
            evidence = list
                .iter()
                .filter_map(|(hash, _)| symbols.get(hash).map(|stored| stored.to_evidence(&snapshot, 0.0, mode)))
                .collect();
        }
        apply_definition_boost(&mut evidence);
        let mut ranked = finalize_fused(evidence);
        ranked.truncate(limit);
        Ok(ranked)
    }

    /// `resolve`: exact lookup of a symbol by fqdn or trailing name.
    pub fn resolve(&self, fqdn_or_name: &str) -> Result<Option<Evidence>, CoreError> {
        let symbols = self.symbols.lock().unwrap();
        if symbols.is_empty() {
            return Err(CoreError::NotIndexed);
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let needle = fqdn_or_name.trim();
        let direct = symbols
            .values()
            .find(|stored| stored.identity.fqdn == needle);
        let by_name = symbols.values().find(|stored| {
            trailing_segment(&stored.identity.fqdn) == needle
        });
        Ok(direct
            .or(by_name)
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
        Ok(Some(stored.to_evidence(&snapshot, 1.0, RetrievalMode::Structural)))
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
    pub fn trace(
        &self,
        symbol_hash: &str,
        depth: usize,
    ) -> Result<Vec<Relation>, CoreError> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut edges: Vec<Relation> = Vec::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((symbol_hash.to_string(), 0));

        while let Some((current, level)) = queue.pop_front() {
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
            (a.kind as u8, &a.source_id, &a.target_id).cmp(&(b.kind as u8, &b.source_id, &b.target_id))
        });
        edges.dedup_by(|a, b| {
            a.kind == b.kind && a.source_id == b.source_id && a.target_id == b.target_id
        });
        Ok(edges)
    }

    /// `similar`: vector-only similarity lookup of a symbol against all others.
    pub fn similar(&self, symbol_hash: &str, limit: usize) -> Result<Vec<Evidence>, CoreError> {
        let symbols = self.symbols.lock().unwrap();
        let embeddings = self.embeddings.lock().unwrap();
        if symbols.is_empty() {
            return Err(CoreError::NotIndexed);
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let target = embeddings
            .get(symbol_hash)
            .ok_or(CoreError::NotIndexed)?;
        let mut scored: Vec<(String, f32)> = embeddings
            .iter()
            .filter(|(hash, _)| hash.as_str() != symbol_hash)
            .filter_map(|(hash, vec)| {
                cosine_similarity(target, vec).map(|sim| (hash.clone(), sim))
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(limit)
            .filter_map(|(hash, score)| {
                symbols.get(&hash).map(|stored| stored.to_evidence(&snapshot, score, RetrievalMode::Semantic))
            })
            .collect())
    }

    /// `context`: runs `search` plus `relations` and compiles a
    /// [`lynx_protocol::ContextPackage`] bounded to `token_budget` tokens.
    pub fn context(&self, query: &str, token_budget: usize) -> Result<lynx_protocol::ContextPackage, CoreError> {
        let search = self.search(query, RetrievalMode::Hybrid, 16)?;
        let mut relations: Vec<Relation> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for evidence in &search {
            let incident = self.dual.graph().get_relations(&evidence.identity.content_hash, None)?;
            for relation in incident {
                let key = format!("{}|{}|{}", relation.source_id, relation.target_id, relation.kind as u8);
                if seen.insert(key) {
                    relations.push(relation);
                }
            }
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let capability = self.status.lock().unwrap().capability_level;
        let compiler = ContextCompiler::new();
        Ok(compiler.compile(
            query.to_string(),
            snapshot,
            search,
            relations,
            capability,
            token_budget,
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
        .filter(|entry| !should_skip(entry.path().strip_prefix(repo_path).unwrap_or(entry.path()), false))
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
        part == "test" || part == "tests" || part == "mock" || part == "mocks" || part == "generated"
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

/// Computes the range and retained snippet for a single symbol.
///
/// Module pseudo-symbols capture the whole file; named symbols capture a
/// line-boundary window anchored at the first occurrence of their name.
fn symbol_range(
    source: &str,
    identity: &SymbolIdentity,
    is_module: bool,
) -> (SourceRange, String) {
    if is_module {
        let lines = source.lines().count().max(1);
        let len = source.len();
        return (
            SourceRange::new(1, lines, 0, len).unwrap_or(SourceRange::new(1, 1, 0, 0).unwrap()),
            source.to_string(),
        );
    }
    let name = trailing_segment(&identity.fqdn);
    let Some(offset) = source.find(name) else {
        let lines = source.lines().count().max(1);
        let len = source.len();
        return (
            SourceRange::new(1, lines, 0, len).unwrap_or(SourceRange::new(1, 1, 0, 0).unwrap()),
            source.to_string(),
        );
    };
    let start_line = byte_offset_line(source, offset).max(1);
    let end_line = (start_line + MAX_SNIPPET_LINES - 1).min(source.lines().count().max(1));
    let start_byte = line_start_byte(source, start_line);
    let end_byte = line_end_byte(source, end_line);
    let snippet = &source[start_byte..end_byte];
    (
        SourceRange::new(start_line, end_line, start_byte, end_byte)
            .unwrap_or(SourceRange::new(start_line, start_line, start_byte, start_byte).unwrap()),
        snippet.to_string(),
    )
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
        assert!(capability_rank(status.capability_level) >= capability_rank(CapabilityLevel::L3Structural));

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
        let results = engine.search("Validate", RetrievalMode::Hybrid, 10).unwrap();
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
}
