//! Parser throughput check: the production single-pass path versus the
//! historical two-pass composition (`parse_symbols` then a second tree for
//! `extract_relations`).
//!
//! The two variants produce identical symbols and relations; the only
//! difference is how many times a file is parsed and analyzed. Keeping both in
//! one binary makes the comparison reproducible on any machine.
//!
//! Usage:
//!   cargo run --release -p pizen-lynx-parser --example parser_bench [ITERATIONS]

use std::path::Path;
use std::time::Instant;

use pizen_lynx_parser::{adapter_for_path, Parser};

/// A representative source unit: many small functions, methods, calls, and a
/// trait implementation, so relations are actually extracted.
fn source(functions: usize) -> String {
    let mut text = String::from("struct Widget;\n\ntrait Render {\n    fn render(&self) -> String;\n}\n\nimpl Render for Widget {\n    fn render(&self) -> String {\n        helper(1)\n    }\n}\n\nfn helper(x: i64) -> i64 {\n    x + 1\n}\n");
    for index in 0..functions {
        text.push_str(&format!(
            "fn operation_{index}(value: i64) -> i64 {{\n    let adjusted = helper(value);\n    adjusted * {index}\n}}\n\n"
        ));
    }
    text
}

fn main() {
    let iterations: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(100);
    let path = Path::new("src/generated.rs");
    let source = source(200);
    let parser = Parser::new();

    // Warm caches and tree-sitter internals.
    for _ in 0..3 {
        let _ = parser.parse(path, &source).unwrap();
    }

    let start = Instant::now();
    let mut symbols = 0usize;
    let mut relations = 0usize;
    for _ in 0..iterations {
        let (parsed_symbols, parsed_relations) = parser.parse(path, &source).unwrap();
        symbols = parsed_symbols.len();
        relations = parsed_relations.len();
    }
    let single = start.elapsed();

    let adapter = adapter_for_path(path);
    let start = Instant::now();
    for _ in 0..iterations {
        let parsed_symbols = adapter.parse_symbols(&source, path).unwrap();
        let tree = adapter.new_tree(&source).unwrap().unwrap();
        let parsed_relations = adapter
            .extract_relations(&source, &tree, &parsed_symbols)
            .unwrap();
        assert_eq!(parsed_symbols.len(), symbols);
        assert_eq!(parsed_relations.len(), relations);
    }
    let two_pass = start.elapsed();

    let per_file_single = single.as_secs_f64() * 1000.0 / iterations as f64;
    let per_file_two = two_pass.as_secs_f64() * 1000.0 / iterations as f64;
    println!(
        "source_bytes={} symbols={symbols} relations={relations} iterations={iterations}",
        source.len()
    );
    println!("single_pass_total_ms={:.2}", single.as_secs_f64() * 1000.0);
    println!("two_pass_total_ms={:.2}", two_pass.as_secs_f64() * 1000.0);
    println!("single_pass_per_file_ms={per_file_single:.3}");
    println!("two_pass_per_file_ms={per_file_two:.3}");
    println!(
        "speedup={:.2}x",
        two_pass.as_secs_f64() / single.as_secs_f64().max(f64::EPSILON)
    );
    println!(
        "METRICS single_pass_ms={per_file_single:.3} two_pass_ms={per_file_two:.3} speedup={:.2}",
        two_pass.as_secs_f64() / single.as_secs_f64().max(f64::EPSILON)
    );
}
