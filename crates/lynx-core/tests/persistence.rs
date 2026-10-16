//! Persistence and incremental-index regression tests.
//!
//! These prove the product-critical lifecycle:
//!
//! ```text
//! index -> persist -> drop -> reopen -> hydrate -> query
//! ```
//!
//! without the query path reading the workspace or re-indexing it. They assert
//! structural invariants (no re-read, no stale rows, deterministic results)
//! rather than timing thresholds.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use lynx_embed::{EmbedError, VectorProvider};
use lynx_protocol::RetrievalMode;
use pizen_lynx_core::Engine;

/// Deterministic fake provider that counts how many texts it embeds, so a test
/// can prove hydration performed no indexing work.
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

fn open(storage: &Path, counter: &Arc<AtomicUsize>) -> Engine {
    Engine::new(storage, Box::new(CountingProvider(counter.clone()))).unwrap()
}

fn hashes(evidence: &[lynx_protocol::Evidence]) -> Vec<String> {
    evidence
        .iter()
        .map(|e| e.identity.content_hash.clone())
        .collect()
}

#[test]
fn reopened_index_answers_without_reading_the_workspace() {
    let ws = tempfile::tempdir().unwrap();
    write(
        ws.path(),
        "auth/service.go",
        "package auth\n\nfunc Validate(u string) bool { return u != \"\" }\n\nfunc Login(u string) bool { return u != \"\" }\n",
    );
    let storage = tempfile::tempdir().unwrap();

    let index_counter = Arc::new(AtomicUsize::new(0));
    let engine = open(storage.path(), &index_counter);
    engine.index_repository(ws.path(), false).unwrap();
    assert!(index_counter.load(Ordering::SeqCst) > 0);
    let before = engine.search("Validate", RetrievalMode::Hybrid, 5).unwrap();
    drop(engine);

    // Delete the workspace entirely: a hydrated index must still answer, which
    // is only possible if hydration does not re-read or re-index the workspace.
    std::fs::remove_dir_all(ws.path()).unwrap();

    let reopen_counter = Arc::new(AtomicUsize::new(0));
    let reopened = open(storage.path(), &reopen_counter);
    assert_eq!(reopen_counter.load(Ordering::SeqCst), 0);
    assert!(reopened.resolve("auth::Validate").unwrap().is_some());
    let after = reopened
        .search("Validate", RetrievalMode::Hybrid, 5)
        .unwrap();
    assert_eq!(hashes(&before), hashes(&after));
}

#[test]
fn hydration_is_deterministic_across_reopens() {
    let ws = tempfile::tempdir().unwrap();
    for i in 0..10 {
        write(
            ws.path(),
            &format!("pkg{i}/mod.rs"),
            &format!("pub fn handler_{i}(x: i64) -> i64 {{ x + {i} }}\n"),
        );
    }
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = open(storage.path(), &counter);
    engine.index_repository(ws.path(), false).unwrap();
    let baseline = hashes(&engine.search("handler", RetrievalMode::Hybrid, 5).unwrap());
    drop(engine);

    for _ in 0..3 {
        let reopened = open(storage.path(), &counter);
        let again = hashes(
            &reopened
                .search("handler", RetrievalMode::Hybrid, 5)
                .unwrap(),
        );
        assert_eq!(baseline, again);
    }
}

#[test]
fn reindex_reflects_added_deleted_and_renamed_files() {
    let ws = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = open(storage.path(), &counter);

    write(ws.path(), "a.rs", "pub fn alpha() -> i32 { 1 }\n");
    write(ws.path(), "b.rs", "pub fn beta() -> i32 { 2 }\n");
    engine.index_repository(ws.path(), false).unwrap();
    assert!(engine.resolve("alpha").unwrap().is_some());
    assert!(engine.resolve("beta").unwrap().is_some());

    // Delete beta, rename alpha -> gamma, add delta.
    std::fs::remove_file(ws.path().join("b.rs")).unwrap();
    std::fs::rename(ws.path().join("a.rs"), ws.path().join("c.rs")).unwrap();
    write(ws.path(), "c.rs", "pub fn gamma() -> i32 { 1 }\n");
    write(ws.path(), "d.rs", "pub fn delta() -> i32 { 4 }\n");
    engine.index_repository(ws.path(), false).unwrap();

    assert!(engine.resolve("gamma").unwrap().is_some(), "new symbol");
    assert!(engine.resolve("delta").unwrap().is_some(), "added file");
    assert!(engine.resolve("beta").unwrap().is_none(), "deleted symbol");
    assert!(engine.resolve("alpha").unwrap().is_none(), "renamed symbol");

    let status = engine.index_status();
    assert_eq!(status.file_count, 2);
    let found = engine
        .search("gamma", RetrievalMode::Hybrid, 5)
        .unwrap()
        .iter()
        .any(|e| e.identity.fqdn.ends_with("gamma"));
    assert!(found, "rebuilt lexical index must see the new symbol");
}

#[test]
fn ambiguous_name_is_surfaced_and_noise_demoted() {
    let ws = tempfile::tempdir().unwrap();
    write(
        ws.path(),
        "auth/service.go",
        "package auth\n\nfunc Login() bool { return true }\n",
    );
    write(
        ws.path(),
        "admin/service.go",
        "package admin\n\nfunc Login() bool { return true }\n",
    );
    // A test double that also declares `Login` in a testdata path.
    write(
        ws.path(),
        "testdata/mock_auth.go",
        "package mockauth\n\nfunc Login() bool { return true }\n",
    );
    let storage = tempfile::tempdir().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let engine = open(storage.path(), &counter);
    engine.index_repository(ws.path(), false).unwrap();

    // `Login` is genuinely ambiguous: three symbols share the trailing name.
    let matches = engine.resolve_all("Login").unwrap();
    assert_eq!(matches.len(), 3, "expected every ambiguous candidate");
    // Noise is demoted: the testdata double ranks last.
    assert!(
        matches
            .last()
            .unwrap()
            .identity
            .file_path
            .starts_with("testdata"),
        "test double must not outrank production code: {:?}",
        matches
            .iter()
            .map(|e| e.identity.fqdn.clone())
            .collect::<Vec<_>>()
    );
    // The deterministic winner is a production symbol.
    assert!(!matches[0].identity.file_path.starts_with("testdata"));
    // `resolve` returns exactly that winner.
    let winner = engine.resolve("Login").unwrap().unwrap();
    assert_eq!(winner.identity.fqdn, matches[0].identity.fqdn);
}
