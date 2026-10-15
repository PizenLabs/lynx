//! Stage profiler for the indexing pipeline.
//!
//! Mirrors `Engine::index_repository` stage-by-stage so the dominant cost can be
//! attributed causally (parse vs graph insert vs lexical writer vs embed).
//!
//! Usage:
//!   cargo run --release -p pizen-lynx-core --example profile_index -- <repo> <files> <symbols>
//! Env:
//!   LYNX_REAL_EMBED=1  use the real ONNX provider instead of the fake one.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lynx_embed::{EmbedError, VectorProvider};
use lynx_parser::Parser;
use lynx_storage::{DualStorage, LexicalDoc};
use walkdir::WalkDir;

/// Instant, deterministic fake provider: isolates embedding *plumbing* cost
/// (vector allocation) from ONNX inference cost.
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
    for (i, ch) in text.bytes().enumerate() {
        out[i % 384] += ch as f32;
    }
    out
}

fn gen_repo(dir: &Path, files: usize, symbols_per_file: usize) {
    for i in 0..files {
        let d = dir.join(format!("pkg{}", i % 64));
        std::fs::create_dir_all(&d).unwrap();
        let mut src = String::new();
        for s in 0..symbols_per_file {
            src.push_str(&format!(
                "pub fn func_{i}_{s}(x: i64) -> i64 {{\n    let v = x + {s};\n    v * 2\n}}\n"
            ));
        }
        std::fs::write(d.join(format!("mod_{i}.rs")), src).unwrap();
    }
}

fn profile<P: VectorProvider>(repo: &Path, files_vec: &[PathBuf], provider: &P) {
    let storage = tempfile::tempdir().unwrap();
    let dual = DualStorage::open(storage.path()).unwrap();
    let parser = Parser::new();

    let mut t_read = 0.0f64;
    let mut t_parse = 0.0f64;
    let mut t_graph = 0.0f64;
    let mut t_lex = 0.0f64;
    let mut t_embed = 0.0f64;
    let mut total_symbols = 0usize;

    for path in files_vec {
        let s = Instant::now();
        let source = std::fs::read_to_string(path).unwrap();
        t_read += s.elapsed().as_secs_f64();

        let rel = path.strip_prefix(repo).unwrap();
        let s = Instant::now();
        let (symbols_parsed, relations) = parser.parse(rel, &source).unwrap();
        t_parse += s.elapsed().as_secs_f64();
        total_symbols += symbols_parsed.len();

        let s = Instant::now();
        let entries: Vec<_> = symbols_parsed
            .iter()
            .map(|i| {
                (
                    i.clone(),
                    lynx_protocol::SourceRange::new(1, 1, 0, 0).unwrap(),
                )
            })
            .collect();
        dual.graph()
            .save_snapshot(&lynx_protocol::Snapshot {
                commit_hash: None,
                workspace_root: repo.to_path_buf(),
                is_dirty: false,
                content_hash: "h".into(),
            })
            .ok();
        dual.graph().insert_symbols(&entries, "h").ok();
        dual.graph().insert_relations(&relations, "h").ok();
        t_graph += s.elapsed().as_secs_f64();

        let s = Instant::now();
        let docs: Vec<LexicalDoc> = symbols_parsed
            .iter()
            .map(|i| LexicalDoc {
                symbol_hash: i.content_hash.clone(),
                code_snippet: source.clone(),
                language: "Rust".into(),
                file_path: i.file_path.to_string_lossy().to_string(),
            })
            .collect();
        dual.lexical().add_documents(&docs).unwrap();
        t_lex += s.elapsed().as_secs_f64();

        let s = Instant::now();
        let texts: Vec<&str> = symbols_parsed.iter().map(|_| source.as_str()).collect();
        let _ = provider.embed_batch(&texts).unwrap();
        t_embed += s.elapsed().as_secs_f64();
    }

    let s = Instant::now();
    dual.lexical().commit().unwrap();
    let t_commit = s.elapsed().as_secs_f64();

    let n = files_vec.len() as f64;
    println!(
        "repo={} files={} symbols={total_symbols}",
        repo.display(),
        files_vec.len()
    );
    println!("  read  total={t_read:8.3}s  per-file={:8.4}s", t_read / n);
    println!(
        "  parse total={t_parse:8.3}s  per-file={:8.4}s",
        t_parse / n
    );
    println!(
        "  graph total={t_graph:8.3}s  per-file={:8.4}s",
        t_graph / n
    );
    println!("  lex   total={t_lex:8.3}s  per-file={:8.4}s", t_lex / n);
    println!(
        "  embed total={t_embed:8.3}s  per-file={:8.4}s",
        t_embed / n
    );
    println!("  commit(total, once) ={t_commit:8.3}s");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tmp;
    let (repo, files): (PathBuf, usize) = if args.len() >= 3 {
        (PathBuf::from(&args[1]), args[2].parse().unwrap())
    } else {
        tmp = tempfile::tempdir().unwrap();
        gen_repo(tmp.path(), 300, 10);
        (tmp.path().to_path_buf(), 300)
    };

    let files_vec: Vec<PathBuf> = WalkDir::new(&repo)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    let _ = files;

    if std::env::var_os("LYNX_REAL_EMBED").is_some() {
        let provider = lynx_embed::FastEmbedProvider::new().unwrap();
        profile(&repo, &files_vec, &provider);
    } else {
        profile(&repo, &files_vec, &FakeProvider);
    }
}
