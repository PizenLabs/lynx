//! `lx`: command-line transport over the Lynx core primitives.
//!
//! `lx index` builds the persistent `.lynx` index for a repository. Retrieval
//! subcommands (`search`, `resolve`, `relations`, `context`) **hydrate** that
//! index — they never re-index the repository. `lx mcp` launches the stdio MCP
//! server from [`lynx_mcp`], which hydrates (or builds once at startup).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use lynx_core::Engine;

use lynx_core::QueryOptions;
use lynx_embed::UnavailableProvider;
use lynx_mcp::{storage_root, workspace_root, Session};
use lynx_protocol::{CapabilityLevel, Evidence, RetrievalMode};

#[derive(Parser)]
#[command(
    name = "lx",
    about = "Lynx: Discovery Engine for AI-Native Software Engineering",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index a repository into <path>/.lynx (persistent artifact)
    Index {
        /// Repository root to index (default: current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Include test, mock, and generated files
        #[arg(long)]
        include_tests: bool,
        /// Rebuild, replacing an existing .lynx
        #[arg(long)]
        force: bool,
    },
    /// Hybrid search over the persistent index; prints ranked Evidence lines
    Search {
        query: String,
        /// Maximum number of results
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
        /// Retrieval channel
        #[arg(short, long, default_value = "hybrid")]
        mode: String,
        /// Print per-result ranking contributions and query diagnostics
        #[arg(long)]
        explain: bool,
        /// Wall-clock timeout in milliseconds (aborts the query when exceeded)
        #[arg(long)]
        timeout_ms: Option<u64>,
        /// Override the per-channel candidate cap (bounded internally)
        #[arg(long)]
        candidate_limit: Option<usize>,
    },
    /// Print exact SymbolIdentity coordinates for a symbol
    Resolve { name: String },
    /// Print the structural relation graph incident to a symbol
    Relations { symbol: String },
    /// Print a compiled ContextPackage as JSON
    Context {
        query: String,
        /// Token ceiling for the compiled package
        #[arg(long, default_value_t = 2048)]
        token_budget: usize,
    },
    /// Print provenance and population counts of the persistent index
    Status,
    /// Launch the stdio MCP server
    Mcp {
        /// Include test, mock, and generated files in a freshly built index
        #[arg(long)]
        include_tests: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Index {
            path,
            include_tests,
            force,
        } => index(&path, include_tests, force),
        Commands::Search {
            query,
            limit,
            mode,
            explain,
            timeout_ms,
            candidate_limit,
        } => search(&query, limit, &mode, explain, timeout_ms, candidate_limit),
        Commands::Resolve { name } => resolve(&name),
        Commands::Relations { symbol } => relations(&symbol),
        Commands::Context {
            query,
            token_budget,
        } => context(&query, token_budget),
        Commands::Status => status(),
        Commands::Mcp { include_tests } => lynx_mcp::run(lynx_mcp::ServerConfig {
            workspace_root: workspace_root()?,
            include_tests,
        }),
    }
}

fn index(path: &Path, include_tests: bool, force: bool) -> Result<()> {
    let root = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    let storage = storage_root(&root);
    if storage.exists() {
        if !force {
            bail!(
                "{} already exists; pass --force to rebuild it from scratch",
                storage.display()
            );
        }
        std::fs::remove_dir_all(&storage)
            .with_context(|| format!("removing existing {}", storage.display()))?;
    }
    println!("Indexing {} into {}/", root.display(), storage.display());
    let session = Session::build(&root, include_tests)?;
    let status = session.engine().index_status();
    println!(
        "Indexed {} files, {} symbols, {} relations (capability {}).",
        status.file_count,
        status.symbol_count,
        status.relation_count,
        capability_name(status.capability_level),
    );
    Ok(())
}

/// Hydrates the persistent index for the current workspace with the real
/// embedding model (semantic-capable).
fn open_session() -> Result<Session> {
    let root = workspace_root()?;
    Session::hydrate(&root)
}

/// Hydrates the persistent index without loading the embedding model.
///
/// Used by structural primitives (`resolve`, `relations`) that never need
/// vectors, so their cold start does not pay the ~250 MB ONNX model load.
fn open_structural_session() -> Result<Session> {
    let root = workspace_root()?;
    Session::hydrate_with(&root, Box::new(UnavailableProvider))
}

fn search(
    query: &str,
    limit: usize,
    mode: &str,
    explain: bool,
    timeout_ms: Option<u64>,
    candidate_limit: Option<usize>,
) -> Result<()> {
    let parsed_mode = parse_mode(mode)?;
    let mut opts = QueryOptions::new();
    if let Some(ms) = timeout_ms {
        opts = opts.with_timeout_ms(ms);
    }
    opts.candidate_limit = candidate_limit;
    let session = open_session()?;
    let outcome = session
        .engine()
        .search_with(query, parsed_mode, limit, &opts)?;
    if outcome.evidence.is_empty() {
        println!("No results found.");
    } else {
        for item in &outcome.evidence {
            println!("{}", evidence_line(item));
        }
    }
    if explain {
        print_explain(query, &outcome);
    }
    Ok(())
}

/// Prints the query report and per-result ranking contributions.
fn print_explain(query: &str, outcome: &lynx_core::SearchOutcome) {
    let report = &outcome.report;
    println!("\n--- explain ---");
    println!("query_id            {}", report.query_id);
    println!("query               {query}");
    println!("state               {:?}", report.state);
    println!("lexical_candidates  {}", report.lexical_candidates);
    println!("semantic_candidates {}", report.semantic_candidates);
    println!("fused_candidates    {}", report.fused_candidates);
    println!("results             {}", report.results);
    println!("total_micros        {}", report.total_micros);
    print!("stages              ");
    for stage in &report.stages {
        print!("{}={}us ", stage.stage, stage.micros);
    }
    println!();
    println!("\nrank  lex   sem   rrf      def?  final    symbol");
    for (rank, contribution) in outcome.contributions.iter().enumerate() {
        println!(
            "{:4}  {:>4}  {:>4}  {:.5}  {:>5}  {:.5}  {}",
            rank + 1,
            contribution
                .lexical_rank
                .map(|r| r.to_string())
                .unwrap_or_else(|| "-".into()),
            contribution
                .semantic_rank
                .map(|r| r.to_string())
                .unwrap_or_else(|| "-".into()),
            contribution.rrf_score,
            if contribution.definition_boost {
                "yes"
            } else {
                "no"
            },
            contribution.final_score,
            contribution.fqdn,
        );
    }
}

fn resolve(name: &str) -> Result<()> {
    let engine_session = open_structural_session()?;
    let matches = engine_session.engine().resolve_all(name)?;
    let Some(primary) = matches.first() else {
        bail!("symbol not found: {name}");
    };
    let identity = &primary.identity;
    println!("{}", identity.fqdn);
    println!("  kind      {:?}", identity.kind);
    println!("  language  {:?}", identity.language);
    println!("  file      {}", identity.file_path.display());
    println!(
        "  range     lines {}-{}, bytes {}-{}",
        primary.range.start_line,
        primary.range.end_line,
        primary.range.start_byte,
        primary.range.end_byte,
    );
    println!("  hash      {}", identity.content_hash);
    if matches.len() > 1 {
        println!("\n  {} other symbol(s) share this name:", matches.len() - 1);
        for alternative in &matches[1..] {
            println!(
                "    {}  {:?}  {}:{}",
                alternative.identity.fqdn,
                alternative.identity.kind,
                alternative.identity.file_path.display(),
                alternative.range.start_line,
            );
        }
    }
    Ok(())
}

fn relations(symbol: &str) -> Result<()> {
    let session = open_structural_session()?;
    let engine = session.engine();
    let Some(evidence) = engine.resolve(symbol)? else {
        bail!("symbol not found: {symbol}");
    };
    let hash = &evidence.identity.content_hash;
    let edges = engine.relations(hash, None)?;

    // Enrich endpoint hashes with fqdns where resolvable.
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for edge in &edges {
        for endpoint in [&edge.source_id, &edge.target_id] {
            names.entry(endpoint.clone()).or_insert_with(|| {
                endpoint_fqdn(engine, endpoint)
                    .unwrap_or_else(|| format!("{}…", &endpoint[..endpoint.len().min(12)]))
            });
        }
    }

    if edges.is_empty() {
        println!("No relations recorded for {symbol}.");
        return Ok(());
    }
    println!("{} ({} edges)", evidence.identity.fqdn, edges.len());
    for edge in &edges {
        let (arrow, other) = if edge.source_id == *hash {
            ("->", edge.target_id.as_str())
        } else {
            ("<-", edge.source_id.as_str())
        };
        println!("  {:?} {} {}", edge.kind, arrow, names[other]);
    }
    Ok(())
}

fn context(query: &str, token_budget: usize) -> Result<()> {
    let package = open_session()?.engine().context(query, token_budget)?;
    println!("{}", serde_json::to_string_pretty(&package)?);
    Ok(())
}

fn status() -> Result<()> {
    let session = open_structural_session()?;
    let status = session.engine().index_status();
    println!("workspace   {}", session.workspace_root().display());
    println!("index       {}", session.storage_root().display());
    println!("files       {}", status.file_count);
    println!("symbols     {}", status.symbol_count);
    println!("relations   {}", status.relation_count);
    println!("capability  {}", capability_name(status.capability_level));
    if let Some(commit) = &status.snapshot.commit_hash {
        println!("commit      {commit}");
    }
    println!("dirty       {}", status.snapshot.is_dirty);
    Ok(())
}

/// Resolves an endpoint hash's fqdn through [`Engine::inspect`]; unknown hashes
/// degrade to a short prefix.
fn endpoint_fqdn(engine: &Engine, hash: &str) -> Option<String> {
    engine
        .inspect(hash)
        .ok()
        .flatten()
        .map(|evidence| evidence.identity.fqdn)
}

/// Human line for one Evidence item.
fn evidence_line(evidence: &Evidence) -> String {
    format!(
        "[{:8.4}] {} ({:?}, {:?}) {}:{}-{} [{:?}]",
        evidence.score,
        evidence.identity.fqdn,
        evidence.identity.kind,
        evidence.identity.language,
        evidence.identity.file_path.display(),
        evidence.range.start_line,
        evidence.range.end_line,
        evidence.retrieval_mode,
    )
}

fn parse_mode(mode: &str) -> Result<RetrievalMode> {
    match mode.to_ascii_lowercase().as_str() {
        "lexical" => Ok(RetrievalMode::Lexical),
        "semantic" => Ok(RetrievalMode::Semantic),
        "structural" => Ok(RetrievalMode::Structural),
        "hybrid" => Ok(RetrievalMode::Hybrid),
        other => bail!("invalid mode `{other}`; expected lexical, semantic, structural, or hybrid"),
    }
}

fn capability_name(level: CapabilityLevel) -> &'static str {
    match level {
        CapabilityLevel::L0Text => "L0Text",
        CapabilityLevel::L1Parsed => "L1Parsed",
        CapabilityLevel::L2Symbolized => "L2Symbolized",
        CapabilityLevel::L3Structural => "L3Structural",
    }
}
