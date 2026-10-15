//! Embedding micro-benchmark: separates per-call overhead from throughput.
//!
//! Usage:
//!   cargo run --release -p pizen-lynx-core --example profile_embed -- <repo> <files>
//!
//! Compares embedding the same corpus as (a) one large batch and (b) many
//! small per-file batches, to quantify fastembed call overhead.

use std::path::PathBuf;
use std::time::Instant;

use lynx_embed::{FastEmbedProvider, VectorProvider};
use walkdir::WalkDir;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let repo = PathBuf::from(&args[1]);

    let mut file_texts: Vec<Vec<String>> = Vec::new();
    for entry in WalkDir::new(&repo)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let Ok(source) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        // Mirror the engine: module snippet is the whole file.
        file_texts.push(vec![source]);
    }
    let total: usize = file_texts.iter().map(Vec::len).sum();

    let provider = FastEmbedProvider::new().unwrap();
    // Warm up (first inference allocates kernels / threadpool).
    let _ = provider.embed_query("warmup").unwrap();

    // (a) one large batch
    let flat: Vec<&str> = file_texts.iter().flatten().map(String::as_str).collect();
    let s = Instant::now();
    let _ = provider.embed_batch(&flat).unwrap();
    let one_big = s.elapsed().as_secs_f64();

    // (b) per-file batches (the current engine behavior)
    let s = Instant::now();
    for group in &file_texts {
        let refs: Vec<&str> = group.iter().map(String::as_str).collect();
        let _ = provider.embed_batch(&refs).unwrap();
    }
    let per_file = s.elapsed().as_secs_f64();

    println!("texts={total} file_groups={}", file_texts.len());
    println!("  one large batch: {one_big:8.3}s");
    println!("  per-file batches: {per_file:8.3}s");
    println!("  ratio: {:.1}x", per_file / one_big.max(1e-9));
}
