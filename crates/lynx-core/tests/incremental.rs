//! Incremental-indexing and staleness regression tests.
//!
//! These pin the product-critical property that a re-index updates **only** the
//! files that changed, removes every stale symbol/vector/document, and reports
//! an honest freshness state — rather than silently serving a mix of old and
//! new repository intelligence.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use lynx_embed::{EmbedError, VectorProvider};
use lynx_protocol::RetrievalMode;
use pizen_lynx_core::{Engine, Freshness};

/// Deterministic provider that counts every text it embeds, so a test can prove
/// an unchanged file is never re-embedded.
struct CountingProvider(Arc<AtomicUsize>);

impl VectorProvider for CountingProvider {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(fake_vec(text))
    }
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.0.fetch_add(texts.len(), Ordering::SeqCst);
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

fn write(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn engine(storage: &Path, counter: &Arc<AtomicUsize>) -> Engine {
    Engine::new(storage, Box::new(CountingProvider(counter.clone()))).unwrap()
}

#[test]
fn unchanged_workspace_reindexes_nothing() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "a.rs", "pub fn alpha() -> i32 { 1 }\n");
    write(ws.path(), "b.rs", "pub fn beta() -> i32 { 2 }\n");
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);

    engine.index_repository(ws.path(), false).unwrap();
    let after_first = counter.load(Ordering::SeqCst);
    assert!(after_first > 0, "first pass must embed");

    let update = engine.update_repository(ws.path(), false).unwrap();
    assert!(update.plan.is_empty(), "no change -> empty plan");
    assert_eq!(update.plan.unchanged, 2);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        after_first,
        "an unchanged workspace must not re-embed anything"
    );
}

#[test]
fn modifying_one_file_reembeds_only_that_file() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "a.rs", "pub fn alpha() -> i32 { 1 }\n");
    write(ws.path(), "b.rs", "pub fn beta() -> i32 { 2 }\n");
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);
    engine.index_repository(ws.path(), false).unwrap();

    let before = counter.load(Ordering::SeqCst);
    write(ws.path(), "a.rs", "pub fn alpha2() -> i32 { 10 }\n");
    let update = engine.update_repository(ws.path(), false).unwrap();

    assert_eq!(update.plan.modified, vec!["a.rs".to_string()]);
    assert!(update.plan.added.is_empty());
    assert!(update.plan.deleted.is_empty());
    let embedded = counter.load(Ordering::SeqCst) - before;
    // a.rs contributes a module symbol + one function = 2 texts; b.rs is untouched.
    assert_eq!(embedded, 2, "only the changed file may be embedded");

    // The replaced symbol is gone from every surface (no stale entity remains).
    assert!(engine.resolve("alpha").unwrap().is_none());
    assert!(engine.resolve("alpha2").unwrap().is_some());
    assert!(!engine
        .search("alpha", RetrievalMode::Lexical, 10)
        .unwrap()
        .iter()
        .any(|e| e.identity.fqdn.ends_with("::alpha")));
    // And its vector is gone: semantic search must not surface it.
    assert!(!engine
        .search("alpha", RetrievalMode::Semantic, 10)
        .unwrap()
        .iter()
        .any(|e| e.identity.fqdn.ends_with("::alpha")));
}

#[test]
fn deleted_and_renamed_files_leave_no_stale_state() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "keep.rs", "pub fn keep() -> i32 { 1 }\n");
    write(ws.path(), "gone.rs", "pub fn gone() -> i32 { 2 }\n");
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);
    engine.index_repository(ws.path(), false).unwrap();

    // Delete one file, rename another.
    std::fs::remove_file(ws.path().join("gone.rs")).unwrap();
    std::fs::rename(ws.path().join("keep.rs"), ws.path().join("renamed.rs")).unwrap();

    let update = engine.update_repository(ws.path(), false).unwrap();
    // A rename is a delete of the old path plus an add of the new one.
    assert_eq!(
        update.plan.deleted,
        vec!["gone.rs".to_string(), "keep.rs".to_string()]
    );
    assert_eq!(update.plan.added, vec!["renamed.rs".to_string()]);

    assert!(engine.resolve("gone").unwrap().is_none());
    // `keep` survives under its new module path; the old `keep::keep` is gone.
    assert!(engine.resolve("keep::keep").unwrap().is_none());
    assert_eq!(
        engine.resolve("keep").unwrap().unwrap().identity.fqdn,
        "renamed::keep"
    );
    // Lexical and semantic views agree: the old file paths are fully gone.
    for evidence in engine
        .search("keep gone", RetrievalMode::Hybrid, 20)
        .unwrap()
    {
        let path = evidence.identity.file_path.to_string_lossy().to_string();
        assert!(
            !path.contains("gone") && path != "keep.rs",
            "stale file path surfaced: {path:?}"
        );
    }
}

#[test]
fn moving_a_directory_reindexes_moved_files() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "old/pkg/mod.rs", "pub fn thing() -> i32 { 1 }\n");
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);
    engine.index_repository(ws.path(), false).unwrap();
    assert!(engine.resolve("old::pkg::mod::thing").unwrap().is_some());

    std::fs::create_dir_all(ws.path().join("new/moved")).unwrap();
    std::fs::rename(
        ws.path().join("old/pkg/mod.rs"),
        ws.path().join("new/moved/mod.rs"),
    )
    .unwrap();

    engine.index_repository(ws.path(), false).unwrap();
    assert!(engine.resolve("old::pkg::mod::thing").unwrap().is_none());
    assert!(engine.resolve("new::moved::mod::thing").unwrap().is_some());
}

#[test]
fn multiple_simultaneous_changes_are_consistent() {
    // Approximates a branch switch: several files change at once.
    let ws = tempfile::tempdir().unwrap();
    for i in 0..5 {
        write(
            ws.path(),
            &format!("pkg{i}/mod.rs"),
            &format!("pub fn old_{i}() -> i32 {{ {i} }}\n"),
        );
    }
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);
    engine.index_repository(ws.path(), false).unwrap();

    for i in 0..5 {
        write(
            ws.path(),
            &format!("pkg{i}/mod.rs"),
            &format!("pub fn new_{i}() -> i32 {{ {i} }}\n"),
        );
    }
    write(ws.path(), "extra/mod.rs", "pub fn extra() -> i32 { 9 }\n");

    let update = engine.update_repository(ws.path(), false).unwrap();
    assert_eq!(update.plan.modified.len(), 5);
    assert_eq!(update.plan.added, vec!["extra/mod.rs".to_string()]);
    for i in 0..5 {
        assert!(engine.resolve(&format!("old_{i}")).unwrap().is_none());
        assert!(engine.resolve(&format!("new_{i}")).unwrap().is_some());
    }
    assert!(engine.resolve("extra").unwrap().is_some());
}

#[test]
fn identical_bodies_in_distinct_files_both_survive() {
    // Regression: identical source text must not collide into one content hash
    // and silently erase a distinct symbol.
    let ws = tempfile::tempdir().unwrap();
    write(
        ws.path(),
        "internal/auth/login.go",
        "package auth\n\nfunc Login() bool { return true }\n",
    );
    write(
        ws.path(),
        "admin/auth/login.go",
        "package auth\n\nfunc Login() bool { return true }\n",
    );
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);
    engine.index_repository(ws.path(), false).unwrap();

    let matches = engine.resolve_all("Login").unwrap();
    let fqdns: Vec<&str> = matches.iter().map(|e| e.identity.fqdn.as_str()).collect();
    // Both functions (in different packages) and both module symbols survive.
    assert_eq!(
        matches
            .iter()
            .filter(|e| e.identity.fqdn.ends_with("::Login"))
            .count(),
        2,
        "distinct identical-body symbols must both persist: {fqdns:?}"
    );
}

#[test]
fn freshness_reports_fresh_stale_and_missing() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "a.rs", "pub fn alpha() -> i32 { 1 }\n");
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);

    // Missing before any index.
    assert_eq!(engine.freshness().unwrap(), Freshness::Missing);

    engine.index_repository(ws.path(), false).unwrap();
    assert_eq!(engine.freshness().unwrap(), Freshness::Fresh);

    write(ws.path(), "a.rs", "pub fn alpha() -> i32 { 2 }\n");
    assert_eq!(engine.freshness().unwrap(), Freshness::Stale);

    engine.index_repository(ws.path(), false).unwrap();
    assert_eq!(engine.freshness().unwrap(), Freshness::Fresh);

    std::fs::remove_file(ws.path().join("a.rs")).unwrap();
    assert_eq!(engine.freshness().unwrap(), Freshness::Stale);
}

#[test]
fn freshness_is_unknown_when_workspace_root_vanishes() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "a.rs", "pub fn alpha() -> i32 { 1 }\n");
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);
    engine.index_repository(ws.path(), false).unwrap();
    assert_eq!(engine.freshness().unwrap(), Freshness::Fresh);

    // The recorded workspace root is gone: freshness cannot be evaluated.
    std::fs::remove_dir_all(ws.path()).unwrap();
    assert_eq!(engine.freshness().unwrap(), Freshness::Unknown);
}

#[test]
fn oversize_files_are_skipped_without_destabilizing_indexing() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "small.rs", "pub fn small() -> i32 { 1 }\n");
    // One file larger than the indexing bound.
    let huge = "pub const BLOB: &str = \"".to_string() + &"x".repeat(9 * 1024 * 1024) + "\";\n";
    std::fs::write(ws.path().join("huge.rs"), huge).unwrap();

    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = engine(storage.path(), &counter);
    let status = engine.index_repository(ws.path(), false).unwrap();

    assert_eq!(status.file_count, 1, "oversize file must not be indexed");
    assert!(engine.resolve("small").unwrap().is_some());
}
