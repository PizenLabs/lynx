//! Product validation harness: grounded discovery tasks across every primitive,
//! scored by Precision@k / Recall@k / MRR / Hit@k per category, plus
//! index/hydrate lifecycle timings.
//!
//! Usage:
//!   cargo run --release -p pizen-lynx-core --example validation_suite -- \
//!       [CORPUS_DIR] [TASKS_JSON] [TRUTH_JSON]
//!
//! Provider:
//!   default          deterministic hash-based proxy (hermetic, no model)
//!   LYNX_REAL_EMBED=1 use the real ONNX embedder (real semantic quality)
//!
//! The harness indexes the corpus into a temporary persistent index, drops the
//! engine, reopens it (hydrating), and runs every task against the hydrated
//! engine — so the measured query behaviour reflects the production path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use std::sync::Arc;

use lynx_embed::{EmbedError, VectorProvider};
use lynx_protocol::{RelationKind, RetrievalMode};
use pizen_lynx_core::{Engine, QueryOptions};
use serde::Deserialize;

/// A `VectorProvider` shareable across engines.
///
/// The product path constructs the embedding model **once per process** and uses
/// that single instance for both indexing and querying (the MCP server, the CLI,
/// and every session hold one provider). The harness must do the same: building
/// a second model instance in one process is not the production path and yields
/// different inference results from a fresh process.
struct SharedProvider(Arc<dyn VectorProvider>);

impl VectorProvider for SharedProvider {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        self.0.embed_query(text)
    }
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.0.embed_batch(texts)
    }
    fn dimension(&self) -> usize {
        self.0.dimension()
    }
}

/// Hermetic deterministic provider (fake 384-d vectors) — stability/regression
/// only; semantic quality requires `LYNX_REAL_EMBED=1`.
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
    let norm = out.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in out.iter_mut() {
            *x /= norm;
        }
    }
    out
}

#[derive(Deserialize)]
struct TaskFile {
    tasks: Vec<Task>,
}

#[derive(Deserialize)]
struct Task {
    id: String,
    category: String,
    primitive: String,
    query: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    expect: Option<String>,
}

#[derive(Deserialize)]
struct TruthFile {
    truth: BTreeMap<String, Truth>,
}

#[derive(Deserialize)]
struct Truth {
    primary: String,
    expected: Vec<String>,
}

/// Per-task outcome.
struct Outcome {
    category: String,
    id: String,
    p_at_1: f64,
    p_at_3: f64,
    recall_5: f64,
    recall_10: f64,
    hit_5: f64,
    mrr: f64,
    errored: bool,
    got: Vec<String>,
}

const K_LIST: usize = 10;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let corpus = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(default_corpus);
    let tasks_path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| default_bench("tasks/discovery.json"));
    let truth_path = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| default_bench("truth/discovery.json"));

    let tasks: TaskFile = read_json(&tasks_path);
    let truth: TruthFile = read_json(&truth_path);
    let real = std::env::var_os("LYNX_REAL_EMBED").is_some();

    let storage = tempfile::tempdir().expect("temp storage");

    // One embedding-model instance for the whole process, exactly as the
    // production transports hold it.
    let provider: Arc<dyn VectorProvider> = make_provider(real);

    // Lifecycle: index -> drop -> reopen (hydrate).
    let index_start = Instant::now();
    {
        let engine = Engine::new(storage.path(), Box::new(SharedProvider(provider.clone())))
            .expect("engine");
        let status = engine.index_repository(&corpus, false).expect("index");
        eprintln!(
            "indexed {} files, {} symbols, {} relations in {:?}",
            status.file_count,
            status.symbol_count,
            status.relation_count,
            index_start.elapsed()
        );
    }
    let hydrate_start = Instant::now();
    let engine =
        Engine::new(storage.path(), Box::new(SharedProvider(provider.clone()))).expect("engine");
    let hydrate = hydrate_start.elapsed();
    let status = engine.index_status();

    println!("corpus: {}", corpus.display());
    println!(
        "provider: {}",
        if real { "real ONNX" } else { "fake proxy" }
    );
    println!(
        "index: {} files, {} symbols, {} relations",
        status.file_count, status.symbol_count, status.relation_count
    );
    println!("index_time_ms: {}", index_start.elapsed().as_millis());
    println!("hydrate_time_ms: {}", hydrate.as_millis());
    println!();

    let mut outcomes = Vec::new();
    let mut total_query = std::time::Duration::ZERO;
    for task in &tasks.tasks {
        let t0 = Instant::now();
        let (got, errored) = run_task(&engine, task);
        total_query += t0.elapsed();
        let t = truth.truth.get(&task.id).expect("truth for task");
        outcomes.push(score(task, t, got, errored));
    }

    report(&outcomes);
    println!("\nquery_time_total_ms: {}", total_query.as_millis());
    let n = tasks.tasks.len().max(1);
    println!(
        "query_time_mean_ms: {:.2}",
        total_query.as_secs_f64() * 1000.0 / n as f64
    );
}

fn make_provider(real: bool) -> Arc<dyn VectorProvider> {
    if real {
        if let Some(home) = std::env::var_os("HOME") {
            let cache = PathBuf::from(home).join(".cache/lynx/fastembed");
            Arc::new(lynx_embed::FastEmbedProvider::with_cache_dir(cache).expect("onnx model"))
        } else {
            Arc::new(lynx_embed::FastEmbedProvider::new().expect("onnx model"))
        }
    } else {
        Arc::new(FakeProvider)
    }
}

/// Runs one task, returning the ordered fqdn list and whether it errored.
fn run_task(engine: &Engine, task: &Task) -> (Vec<String>, bool) {
    match task.primitive.as_str() {
        "resolve" => match engine.resolve(&task.query) {
            Ok(Some(e)) => (vec![e.identity.fqdn], false),
            Ok(None) => (vec![], false),
            Err(_) => (vec![], true),
        },
        "resolve_all" => match engine.resolve_all(&task.query) {
            Ok(list) => (list.into_iter().map(|e| e.identity.fqdn).collect(), false),
            Err(_) => (vec![], true),
        },
        "search" => {
            let opts = QueryOptions::new();
            match engine.search_with(&task.query, RetrievalMode::Hybrid, K_LIST, &opts) {
                Ok(outcome) => (
                    outcome
                        .evidence
                        .into_iter()
                        .map(|e| e.identity.fqdn)
                        .collect(),
                    false,
                ),
                Err(_) => (vec![], true),
            }
        }
        "relations" => {
            let kind = match task.kind.as_deref() {
                Some("Calls") => Some(RelationKind::Calls),
                Some("Contains") => Some(RelationKind::Contains),
                _ => None,
            };
            let Ok(Some(anchor)) = engine.resolve(&task.query) else {
                return (vec![], true);
            };
            let self_hash = anchor.identity.content_hash;
            let Ok(edges) = engine.relations(&self_hash, kind) else {
                return (vec![], true);
            };
            let mut fqdns = Vec::new();
            for edge in edges {
                let other = if edge.source_id == self_hash {
                    &edge.target_id
                } else {
                    &edge.source_id
                };
                if let Ok(Some(e)) = engine.inspect(other) {
                    if e.identity.content_hash != self_hash {
                        fqdns.push(e.identity.fqdn);
                    }
                }
            }
            fqdns.sort();
            fqdns.dedup();
            (fqdns, false)
        }
        "context" => match engine.context(&task.query, 2048) {
            Ok(package) => (
                package
                    .primary_evidence
                    .into_iter()
                    .map(|e| e.identity.fqdn)
                    .collect(),
                false,
            ),
            Err(_) => (vec![], true),
        },
        other => panic!("unknown primitive {other}"),
    }
}

fn score(task: &Task, truth: &Truth, got: Vec<String>, errored: bool) -> Outcome {
    let expected: std::collections::HashSet<&str> =
        truth.expected.iter().map(String::as_str).collect();

    // Adversarial/containment tasks score on "did not error".
    let binary = task.expect.is_some();
    if binary {
        let pass = !errored;
        return Outcome {
            category: task.category.clone(),
            id: task.id.clone(),
            p_at_1: pass as u8 as f64,
            p_at_3: pass as u8 as f64,
            recall_5: pass as u8 as f64,
            recall_10: pass as u8 as f64,
            hit_5: pass as u8 as f64,
            mrr: pass as u8 as f64,
            errored,
            got,
        };
    }

    let hits_at = |k: usize| -> usize {
        got.iter()
            .take(k)
            .filter(|f| expected.contains(f.as_str()))
            .count()
    };
    let p_at_1 = got
        .first()
        .map(|f| (*f == truth.primary) as u8 as f64)
        .unwrap_or(0.0);
    let p_at_3 = got.iter().take(3).any(|f| *f == truth.primary) as u8 as f64;
    let denom = expected.len().max(1) as f64;
    let recall_5 = hits_at(5) as f64 / denom;
    let recall_10 = hits_at(K_LIST) as f64 / denom;
    let hit_5 = (hits_at(5) > 0) as u8 as f64;
    let mrr = got
        .iter()
        .position(|f| expected.contains(f.as_str()))
        .map(|rank| 1.0 / (rank + 1) as f64)
        .unwrap_or(0.0);

    Outcome {
        category: task.category.clone(),
        id: task.id.clone(),
        p_at_1,
        p_at_3,
        recall_5,
        recall_10,
        hit_5,
        mrr,
        errored,
        got,
    }
}

fn report(outcomes: &[Outcome]) {
    println!(
        "{:<12} {:>4} {:>6} {:>6} {:>6} {:>7} {:>6} {:>6}",
        "category", "n", "P@1", "P@3", "R@5", "R@10", "Hit@5", "MRR"
    );
    let mut by_category: BTreeMap<String, Vec<&Outcome>> = BTreeMap::new();
    for o in outcomes {
        by_category.entry(o.category.clone()).or_default().push(o);
    }
    let print_row = |name: &str, items: &[&Outcome]| {
        let n = items.len().max(1) as f64;
        let mean = |f: fn(&Outcome) -> f64| items.iter().map(|o| f(o)).sum::<f64>() / n;
        println!(
            "{:<12} {:>4} {:>6.3} {:>6.3} {:>6.3} {:>7.3} {:>6.3} {:>6.3}",
            name,
            items.len(),
            mean(|o| o.p_at_1),
            mean(|o| o.p_at_3),
            mean(|o| o.recall_5),
            mean(|o| o.recall_10),
            mean(|o| o.hit_5),
            mean(|o| o.mrr),
        );
    };
    for (category, items) in &by_category {
        print_row(category, items);
    }
    let all: Vec<&Outcome> = outcomes.iter().collect();
    print_row("ALL", &all);

    let errored = outcomes.iter().filter(|o| o.errored).count();
    println!("\nerrored_tasks: {errored}");

    // Failures worth inspecting (P@1 miss on a scored task).
    println!("\n-- representative misses (first 20) --");
    let mut shown = 0;
    for o in outcomes {
        if o.p_at_1 < 1.0 && shown < 20 {
            println!(
                "{:<14} {:<12} got={:?}",
                o.id,
                o.category,
                o.got.iter().take(5).collect::<Vec<_>>()
            );
            shown += 1;
        }
    }

    // machine-readable aggregates
    println!();
    for (category, items) in &by_category {
        let n = items.len().max(1) as f64;
        let mean = |f: fn(&Outcome) -> f64| items.iter().map(|o| f(o)).sum::<f64>() / n;
        println!(
            "METRICS category={category} n={} precision@1={:.3} precision@3={:.3} recall@5={:.3} recall@10={:.3} hit@5={:.3} mrr={:.3}",
            items.len(),
            mean(|o| o.p_at_1),
            mean(|o| o.p_at_3),
            mean(|o| o.recall_5),
            mean(|o| o.recall_10),
            mean(|o| o.hit_5),
            mean(|o| o.mrr),
        );
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn default_corpus() -> PathBuf {
    default_bench("corpus/auth-service")
}

fn default_bench(rel: &str) -> PathBuf {
    let from_root = PathBuf::from("benchmarks").join(rel);
    if from_root.exists() {
        return from_root;
    }
    manifest_dir().join("benchmarks").join(rel)
}

fn manifest_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
