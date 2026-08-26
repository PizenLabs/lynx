//! `lx`: command-line transport over the Lynx core primitives.
//!
//! Retrieval subcommands open an ephemeral session (fresh substrate
//! indexed once from the resolved workspace) and print protocol results;
//! `lx index` builds the persistent `.lynx` directory; `lx mcp` launches
//! the stdio MCP server from [`lynx_mcp`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use lynx_core::Engine;

use lynx_mcp::{workspace_root, ServerConfig};
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
    /// Index a repository into ./.lynx (persistent artifact)
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
    /// Hybrid search; prints ranked Evidence lines
    Search {
        query: String,
        /// Maximum number of results
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
        /// Retrieval channel
        #[arg(short, long, default_value = "hybrid")]
        mode: String,
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
    /// Launch the stdio MCP server
    Mcp {
        /// Include test, mock, and generated files in the index
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
        Commands::Search { query, limit, mode } => search(&query, limit, &mode),
        Commands::Resolve { name } => resolve(&name),
        Commands::Relations { symbol } => relations(&symbol),
        Commands::Context {
            query,
            token_budget,
        } => context(&query, token_budget),
        Commands::Mcp { include_tests } => lynx_mcp::run(ServerConfig {
            workspace_root: workspace_root()?,
            include_tests,
        }),
    }
}

/// Persistent storage directory for `lx index`.
const STORAGE_DIR: &str = ".lynx";

fn index(path: &std::path::Path, include_tests: bool, force: bool) -> Result<()> {
    let storage = PathBuf::from(STORAGE_DIR);
    if storage.exists() {
        if !force {
            bail!("{STORAGE_DIR} already exists; pass --force to rebuild it from scratch");
        }
        std::fs::remove_dir_all(&storage)
            .with_context(|| format!("removing existing {}", storage.display()))?;
    }

    // Stage the index outside the workspace and only materialize ./.lynx
    // afterwards: the engine's exclusion list does not skip `.lynx`, so a
    // storage directory inside the walked tree would index itself.
    let root = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    println!("Indexing {} into {}/", root.display(), STORAGE_DIR);
    let session = lynx_mcp::Session::open(&root, include_tests)?;
    let status = session.engine().index_status();
    println!(
        "Indexed {} files, {} symbols, {} relations (capability {}).",
        status.file_count,
        status.symbol_count,
        status.relation_count,
        capability_name(status.capability_level),
    );
    session.persist(&storage)
}

/// Opens an ephemeral retrieval session over the resolved workspace.
fn open_session() -> Result<lynx_mcp::Session> {
    let root = workspace_root()?;
    lynx_mcp::Session::open(&root, false)
}

fn search(query: &str, limit: usize, mode: &str) -> Result<()> {
    let parsed_mode = parse_mode(mode)?;
    let evidence = open_session()?.engine().search(query, parsed_mode, limit)?;
    if evidence.is_empty() {
        println!("No results found.");
        return Ok(());
    }
    for item in &evidence {
        println!("{}", evidence_line(item));
    }
    Ok(())
}

fn resolve(name: &str) -> Result<()> {
    let Some(evidence) = open_session()?.engine().resolve(name)? else {
        bail!("symbol not found: {name}");
    };
    let identity = &evidence.identity;
    println!("{}", identity.fqdn);
    println!("  kind      {:?}", identity.kind);
    println!("  language  {:?}", identity.language);
    println!("  file      {}", identity.file_path.display());
    println!(
        "  range     lines {}-{}, bytes {}-{}",
        evidence.range.start_line,
        evidence.range.end_line,
        evidence.range.start_byte,
        evidence.range.end_byte,
    );
    println!("  hash      {}", identity.content_hash);
    Ok(())
}

fn relations(symbol: &str) -> Result<()> {
    let session = open_session()?;
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

/// Resolves an endpoint hash's fqdn through [`Engine::inspect`]'s
/// persistent fallback; unknown hashes degrade to a short prefix.
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
