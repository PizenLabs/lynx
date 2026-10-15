//! Lifecycle regression tests: repeated indexing/searching, malformed-input
//! containment, and bounded snippet materialization.
//!
//! These assert structural/lifecycle invariants (idempotence, stability,
//! boundedness, graceful degradation) rather than brittle RSS thresholds.

use std::path::Path;

use lynx_embed::{EmbedError, VectorProvider};
use lynx_protocol::RetrievalMode;
use pizen_lynx_core::retrieval::classify_path;
use pizen_lynx_core::{CoreError, Engine, QueryOptions};

/// Deterministic provider: no ONNX model, fixed dimension.
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

fn engine() -> (tempfile::TempDir, Engine) {
    let storage = tempfile::tempdir().unwrap();
    let engine = Engine::new(storage.path(), Box::new(FakeProvider)).unwrap();
    (storage, engine)
}

#[test]
fn repeated_indexing_is_idempotent() {
    let ws = tempfile::tempdir().unwrap();
    write(
        ws.path(),
        "auth/service.go",
        "package auth\n\nfunc Validate(u string) bool { return u != \"\" }\n",
    );
    let (_storage, engine) = engine();

    let first = engine.index_repository(ws.path(), false).unwrap();
    let second = engine.index_repository(ws.path(), false).unwrap();
    assert_eq!(first.symbol_count, second.symbol_count);
    assert_eq!(first.file_count, second.file_count);
    // State is still resolvable and searchable after a re-index.
    assert!(engine.resolve("auth::Validate").unwrap().is_some());
    assert!(!engine
        .search("Validate", RetrievalMode::Hybrid, 5)
        .unwrap()
        .is_empty());
}

#[test]
fn repeated_searches_are_stable_and_bounded() {
    let ws = tempfile::tempdir().unwrap();
    for i in 0..20 {
        write(
            ws.path(),
            &format!("pkg{i}/mod.rs"),
            &format!("pub fn handler_{i}(x: i64) -> i64 {{ x + {i} }}\n"),
        );
    }
    let (_storage, engine) = engine();
    engine.index_repository(ws.path(), false).unwrap();

    let baseline: Vec<String> = engine
        .search("handler", RetrievalMode::Hybrid, 10)
        .unwrap()
        .into_iter()
        .map(|e| e.identity.content_hash)
        .collect();

    // 100 sequential queries must not drift, error, or accumulate state.
    for _ in 0..100 {
        let again: Vec<String> = engine
            .search("handler", RetrievalMode::Hybrid, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.identity.content_hash)
            .collect();
        assert_eq!(baseline, again);
    }
    // Repeated session-style open/index/drop must also succeed.
    for _ in 0..5 {
        let storage = tempfile::tempdir().unwrap();
        let e = Engine::new(storage.path(), Box::new(FakeProvider)).unwrap();
        e.index_repository(ws.path(), false).unwrap();
        assert!(e.resolve("pkg0::mod::handler_0").unwrap().is_some());
    }
}

#[test]
fn empty_repository_yields_not_indexed() {
    let ws = tempfile::tempdir().unwrap();
    let (_storage, engine) = engine();
    let status = engine.index_repository(ws.path(), false).unwrap();
    assert_eq!(status.file_count, 0);
    assert!(matches!(
        engine
            .search("anything", RetrievalMode::Hybrid, 5)
            .unwrap_err(),
        CoreError::NotIndexed
    ));
    assert!(matches!(
        engine.resolve("anything").unwrap_err(),
        CoreError::NotIndexed
    ));
}

#[test]
fn malformed_and_binary_files_do_not_crash_indexing() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "good.rs", "pub fn good() -> i32 { 1 }\n");
    // Invalid UTF-8 bytes: read_to_string fails and the file is skipped.
    std::fs::write(ws.path().join("bad.rs"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
    // A very long single line.
    write(
        ws.path(),
        "long.rs",
        &format!("pub const BIG: &str = \"{}\";\n", "x".repeat(100_000)),
    );
    // Deeply nested directory with no parseable content.
    write(ws.path(), "empty_dir/README.md", "# nothing\n");

    let (_storage, engine) = engine();
    let status = engine.index_repository(ws.path(), false).unwrap();
    assert!(status.symbol_count >= 1);
    assert!(engine.resolve("good::good").unwrap().is_some());
}

#[test]
fn oversized_symbol_snippets_are_bounded() {
    let ws = tempfile::tempdir().unwrap();
    let mut body = String::from("pub fn huge() -> i32 {\n");
    for i in 0..10_000 {
        body.push_str(&format!("    let v{i} = {i};\n"));
    }
    body.push_str("    0\n}\n");
    write(ws.path(), "huge.rs", &body);

    let (_storage, engine) = engine();
    engine.index_repository(ws.path(), false).unwrap();

    let module = engine.resolve("huge").unwrap().unwrap();
    let module_lines = module.code_snippet.lines().count();
    assert!(module_lines <= 80, "module snippet {module_lines} lines");

    let symbol = engine.resolve("huge::huge").unwrap().unwrap();
    let symbol_lines = symbol.code_snippet.lines().count();
    assert!(symbol_lines <= 25, "symbol snippet {symbol_lines} lines");
}

#[test]
fn vendor_paths_are_excluded_and_test_paths_are_demoted() {
    let ws = tempfile::tempdir().unwrap();
    write(
        ws.path(),
        "auth/service.go",
        "package auth\n\nfunc Login() bool { return true }\n",
    );
    // Vendor is excluded from indexing entirely.
    write(
        ws.path(),
        "vendor/legacy/login.go",
        "package legacy\n\nfunc Login() bool { return true }\n",
    );
    // Test data is indexed (so it can be penalized at rank time, not hidden).
    write(
        ws.path(),
        "testdata/mock_auth.go",
        "package auth\n\nfunc TestLogin() bool { return true }\n",
    );

    let (_storage, engine) = engine();
    let status = engine.index_repository(ws.path(), false).unwrap();
    assert_eq!(status.file_count, 2, "vendor must not be indexed");

    let results = engine.search("Login", RetrievalMode::Hybrid, 10).unwrap();
    let fqdns: Vec<&str> = results.iter().map(|e| e.identity.fqdn.as_str()).collect();
    assert!(fqdns.contains(&"auth::Login"));
    // The exact hit is first, and the test double does not precede it.
    assert_eq!(fqdns[0], "auth::Login");
    assert!(
        !fqdns.contains(&"legacy::Login"),
        "vendor symbol leaked: {fqdns:?}"
    );

    assert!(matches!(
        classify_path(Path::new("a/vendor/b.rs")),
        pizen_lynx_core::PathNoise::Vendor
    ));
    assert!(matches!(
        classify_path(Path::new("a/test/b.rs")),
        pizen_lynx_core::PathNoise::Test
    ));
    assert!(matches!(
        classify_path(Path::new("a/src/b.rs")),
        pizen_lynx_core::PathNoise::None
    ));
}

#[test]
fn cancelled_query_releases_and_errors() {
    let ws = tempfile::tempdir().unwrap();
    write(ws.path(), "a.rs", "pub fn a() -> i32 { 1 }\n");
    let (_storage, engine) = engine();
    engine.index_repository(ws.path(), false).unwrap();

    let opts = QueryOptions::new().with_timeout_ms(0);
    assert!(matches!(
        engine
            .search_with("a", RetrievalMode::Hybrid, 5, &opts)
            .unwrap_err(),
        CoreError::TimedOut
    ));
    // The engine remains usable after an aborted query.
    assert!(!engine
        .search("a", RetrievalMode::Hybrid, 5)
        .unwrap()
        .is_empty());
}
