//! Corruption / incompatibility recovery regression tests.
//!
//! Lynx must classify a broken or foreign index rather than silently serving
//! invalid repository intelligence, and must offer a rebuild path that always
//! produces a clean, queryable artifact.

use std::path::Path;

use lynx_embed::{EmbedError, VectorProvider};
use pizen_lynx_core::{Engine, Freshness};

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

fn write(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Writes a database with a deliberately old `user_version`, simulating a
/// substrate produced by an incompatible Lynx schema.
fn write_incompatible_db(storage: &Path) {
    std::fs::create_dir_all(storage).unwrap();
    let conn = rusqlite::Connection::open(storage.join("graph.sqlite3")).unwrap();
    conn.execute_batch("PRAGMA user_version = 1;").unwrap();
}

#[test]
fn incompatible_schema_is_classified_and_recoverable() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "a.rs", "pub fn alpha() -> i32 { 1 }\n");
    let storage = tempfile::tempdir().unwrap();
    write_incompatible_db(storage.path());

    let engine = Engine::new(storage.path(), Box::new(FakeProvider)).unwrap();
    // The foreign schema is reported, never served as if it were valid.
    assert_eq!(engine.freshness().unwrap(), Freshness::Incompatible);
    // And the documented recovery path produces a clean, queryable index.
    engine.rebuild_repository(ws.path(), false).unwrap();
    assert_eq!(engine.freshness().unwrap(), Freshness::Fresh);
    assert!(engine.resolve("alpha").unwrap().is_some());
}

#[test]
fn missing_index_is_classified_missing() {
    let storage = tempfile::tempdir().unwrap();
    let engine = Engine::new(storage.path(), Box::new(FakeProvider)).unwrap();
    assert_eq!(engine.freshness().unwrap(), Freshness::Missing);
}

#[test]
fn corrupt_database_is_detected() {
    let storage = tempfile::tempdir().unwrap();
    // A file that is not a valid SQLite database at all.
    std::fs::write(storage.path().join("graph.sqlite3"), b"not a database").unwrap();

    // Opening may fail loudly; if it opens, freshness must not claim "fresh".
    match Engine::new(storage.path(), Box::new(FakeProvider)) {
        Ok(engine) => {
            let state = engine.freshness().unwrap();
            assert_ne!(
                state,
                Freshness::Fresh,
                "corrupt store must not read as fresh"
            );
        }
        Err(_) => { /* propagated as an explicit error: acceptable */ }
    }
}
