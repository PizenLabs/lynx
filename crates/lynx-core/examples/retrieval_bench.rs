//! Retrieval benchmark harness: Precision@k, Recall@k, MRR over a corpus with
//! ground-truth queries.
//!
//! Usage:
//!   cargo run --release -p pizen-lynx-core --example retrieval_bench [CORPUS_DIR]
//!
//! Provider:
//!   default          deterministic lexical/semantic proxy (hermetic, no model)
//!   LYNX_REAL_EMBED=1 use the real ONNX embedder (measures real semantic quality)
//!
//! Emits a `METRICS ...` line for before/after diffing. Corpus defaults to
//! `benchmarks/retrieval-corpus`.

use std::path::{Path, PathBuf};

use lynx_embed::{EmbedError, VectorProvider};
use lynx_protocol::RetrievalMode;
use pizen_lynx_core::{Engine, QueryOptions};
use serde::Deserialize;

/// Deterministic lexical proxy: identical to the engine test provider. Useful
/// for regression-testing the fusion/ranking path without the ONNX model.
struct FakeProvider;

impl VectorProvider for FakeProvider {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(fake_vec(text))
    }
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| fake_vec(t)).collect())
    }
    fn dimension(&self) -> usize {
        384
    }
}

fn fake_vec(text: &str) -> Vec<f32> {
    let mut out = vec![0.0_f32; 384];
    for (i, byte) in text.bytes().enumerate() {
        out[i % 384] += byte as f32;
    }
    normalize(&mut out);
    out
}

fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[derive(Deserialize)]
struct Corpus {
    queries: Vec<Query>,
}

#[derive(Deserialize)]
struct Query {
    id: String,
    class: String,
    query: String,
    primary: String,
    expected: Vec<String>,
}

const K: usize = 5;

fn main() {
    let corpus_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_corpus);
    // Ground truth lives beside the corpus so it is never itself indexed.
    let truth_path = corpus_dir
        .parent()
        .map(|p| p.join("retrieval-truth.json"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("benchmarks/retrieval-truth.json"));
    let truth: Corpus = serde_json::from_str(
        &std::fs::read_to_string(&truth_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", truth_path.display())),
    )
    .expect("parse queries.json");

    let storage = tempfile::tempdir().unwrap();
    let engine = if std::env::var_os("LYNX_REAL_EMBED").is_some() {
        let provider = lynx_embed::FastEmbedProvider::new().unwrap();
        Engine::new(storage.path(), Box::new(provider)).unwrap()
    } else {
        Engine::new(storage.path(), Box::new(FakeProvider)).unwrap()
    };
    let status = engine.index_repository(&corpus_dir, false).unwrap();
    println!(
        "indexed {} files, {} symbols, {} relations -> {}",
        status.file_count,
        status.symbol_count,
        status.relation_count,
        if std::env::var_os("LYNX_REAL_EMBED").is_some() {
            "real embeddings"
        } else {
            "fake embeddings"
        }
    );

    let mut sum_prec = 0.0;
    let mut sum_recall = 0.0;
    let mut sum_hit = 0.0;
    let mut sum_mrr = 0.0;
    let n = truth.queries.len() as f64;

    println!(
        "\n{:<26} {:<11} {:>7} {:>7} {:>7} {:>7}",
        "query", "class", "P@1", "R@5", "hit@5", "MRR"
    );
    for q in &truth.queries {
        let outcome = engine
            .search_with(&q.query, RetrievalMode::Hybrid, K, &QueryOptions::new())
            .unwrap();
        let got: Vec<&str> = outcome
            .evidence
            .iter()
            .map(|e| e.identity.fqdn.as_str())
            .collect();

        let hits_in_top = got
            .iter()
            .filter(|fqdn| q.expected.iter().any(|e| e == *fqdn))
            .count();
        let precision_at_1 = got.first().map(|fqdn| *fqdn == q.primary).unwrap_or(false);
        let recall = hits_in_top as f64 / q.expected.len().max(1) as f64;
        let mrr = got
            .iter()
            .position(|fqdn| q.expected.iter().any(|e| e == *fqdn))
            .map(|rank| 1.0 / (rank + 1) as f64)
            .unwrap_or(0.0);

        sum_prec += precision_at_1 as u8 as f64;
        sum_recall += recall;
        sum_hit += (hits_in_top > 0) as u8 as f64;
        sum_mrr += mrr;

        println!(
            "{:<26} {:<11} {:>7.2} {:>7.2} {:>7.2} {:>7.3}  got={:?}",
            q.id,
            q.class,
            precision_at_1 as u8 as f64,
            recall,
            (hits_in_top > 0) as u8 as f64,
            mrr,
            got,
        );
    }

    println!("\n--- aggregate (n={}) ---", truth.queries.len());
    println!("Precision@1 {:.3}", sum_prec / n);
    println!("Recall@5    {:.3}", sum_recall / n);
    println!("Hit@5       {:.3}", sum_hit / n);
    println!("MRR         {:.3}", sum_mrr / n);
    println!(
        "METRICS precision@1={:.3} recall@5={:.3} hit@5={:.3} mrr={:.3}",
        sum_prec / n,
        sum_recall / n,
        sum_hit / n,
        sum_mrr / n
    );
}

fn default_corpus() -> PathBuf {
    let from_root = PathBuf::from("benchmarks/retrieval-corpus");
    if from_root.exists() {
        return from_root;
    }
    manifest_dir().join("benchmarks/retrieval-corpus")
}

fn manifest_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
