//! Embedding cost model probe: is cost per-text constant or per-byte?
//!
//! Usage: cargo run --release -p pizen-lynx-core --example profile_embed_len

use std::time::Instant;

use lynx_embed::{FastEmbedProvider, VectorProvider};

fn main() {
    let provider = FastEmbedProvider::new().unwrap();
    let _ = provider.embed_query("warmup").unwrap();

    for (label, text) in [
        ("tiny", "a".to_string()),
        ("short", "pub fn f(x: i64) -> i64 { x + 1 }".to_string()),
        (
            "medium",
            "pub fn f(x: i64) -> i64 {\n    let v = x + 1;\n    v * 2\n}\n".repeat(8),
        ),
        (
            "long",
            "pub fn f(x: i64) -> i64 {\n    let v = x + 1;\n    v * 2\n}\n".repeat(120),
        ),
    ] {
        let batch: Vec<&str> = std::iter::repeat_n(text.as_str(), 200).collect();
        let s = Instant::now();
        let _ = provider.embed_batch(&batch).unwrap();
        let elapsed = s.elapsed().as_secs_f64();
        println!(
            "{label:7} chars={:6} bytes={:6} 200-texts={elapsed:7.3}s  per-text={:8.4}s",
            text.chars().count(),
            text.len(),
            elapsed / 200.0
        );
    }
}
