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
use lynx_mcp::{git_root, smoke_check, storage_root, workspace_root, Session};
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
    /// Build or incrementally update the persistent index at <path>/.lynx
    ///
    /// Only files added, modified, or deleted since the last pass are
    /// re-parsed and re-embedded.
    Index {
        /// Repository root to index (default: current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Include test, mock, and generated files
        #[arg(long)]
        include_tests: bool,
        /// Discard the existing index and rebuild from scratch
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
    /// Print provenance, population counts, and freshness of the index
    Status {
        /// Skip the workspace freshness re-scan (prints counts only)
        #[arg(long)]
        no_check: bool,
    },
    /// Print a diagnostic report of the installation, workspace, and index
    Doctor,
    /// Launch the stdio MCP server
    Mcp {
        /// Workspace root to serve (default: enclosing git toplevel of the CWD)
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
        /// Include test, mock, and generated files in a freshly built index
        #[arg(long)]
        include_tests: bool,
        /// Run a fast MCP protocol smoke test (no repository indexing) then exit
        #[arg(long)]
        check: bool,
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
        Commands::Status { no_check } => status(!no_check),
        Commands::Doctor => doctor(),
        Commands::Mcp {
            path,
            include_tests,
            check,
        } => {
            if check {
                mcp_check()
            } else {
                let workspace_root = match path {
                    Some(path) => path
                        .canonicalize()
                        .with_context(|| format!("resolving {}", path.display()))?,
                    None => workspace_root()?,
                };
                lynx_mcp::run(lynx_mcp::ServerConfig {
                    workspace_root,
                    include_tests,
                })
            }
        }
    }
}

fn index(path: &Path, include_tests: bool, force: bool) -> Result<()> {
    let root = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    let storage = storage_root(&root);
    if force && storage.exists() {
        // A forced rebuild discards the artifact outright, guaranteeing no
        // stale segment or row can survive.
        std::fs::remove_dir_all(&storage)
            .with_context(|| format!("removing existing {}", storage.display()))?;
    }
    let (session, update) = Session::update(&root, include_tests)?;
    let status = update.status.clone();
    let plan = update.plan;
    if plan.is_empty() && !force {
        println!(
            "Up to date: {} files, {} symbols, {} relations unchanged.",
            status.file_count, status.symbol_count, status.relation_count,
        );
    } else {
        println!(
            "Indexed {}: +{} added, ~{} modified, -{} deleted, {} unchanged.",
            storage.display(),
            plan.added.len(),
            plan.modified.len(),
            plan.deleted.len(),
            plan.unchanged,
        );
        println!(
            "Now {} files, {} symbols, {} relations (capability {}).",
            status.file_count,
            status.symbol_count,
            status.relation_count,
            capability_name(status.capability_level),
        );
    }
    let _ = session;
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
    println!("class               {}", report.class.as_str());
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

fn status(check: bool) -> Result<()> {
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
    if check {
        // The authoritative freshness check re-scans the workspace and runs a
        // SQLite integrity check; it is explicit, never paid on the query path.
        let freshness = session.engine().freshness()?;
        println!("freshness   {}", freshness.as_str());
        if matches!(
            freshness,
            lynx_core::Freshness::Stale | lynx_core::Freshness::Missing
        ) {
            println!("            run `lx index` to refresh");
        }
    }
    Ok(())
}

/// `lx doctor`: a fast, model-free diagnostic of the installation.
///
/// Verifies the binary, platform, installation path, workspace detection, state
/// writability, index presence/compatibility/freshness, and the MCP protocol
/// contract. It never loads the embedding model and never indexes the
/// repository, so it is safe to run anywhere and at any time. The `Status` line
/// carries the verdict (`READY` / `NOT READY`); doctor exits non-zero only when
/// the diagnosis itself cannot run.
fn doctor() -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let platform = format!("{} {}", os_display(), arch_display());
    let install = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());

    let repository = git_root()?;
    let workspace = match &repository {
        Some(root) => root.clone(),
        None => std::env::current_dir().context("resolving current directory")?,
    };
    // Lynx indexes any directory; a git work tree is the common case, but a
    // populated directory with no git metadata is still a valid workspace.
    let repository_detected = repository.is_some() || directory_has_entries(&workspace);

    let storage = storage_root(&workspace);
    let state_ok = writable(&storage) || writable(&workspace);

    // Read the index without loading the model: presence, counts, compatibility,
    // and freshness. `freshness()` re-scans only file hashes (never embeds).
    let db = storage.join("graph.sqlite3");
    let index_present = db.is_file();
    let (symbols, freshness) = if index_present {
        match lynx_core::Engine::new(&storage, Box::new(UnavailableProvider)) {
            Ok(engine) => {
                let count = engine.index_status().symbol_count;
                let fresh = engine
                    .freshness()
                    .map(|f| f.as_str().to_string())
                    .unwrap_or_else(|error| format!("error: {error}"));
                (count, fresh)
            }
            Err(error) => (0, format!("error: {error}")),
        }
    } else {
        (0, "missing".to_string())
    };

    let mcp_ok = smoke_check().is_ok();
    let ready = state_ok
        && repository_detected
        && index_present
        && symbols > 0
        && freshness == "fresh"
        && mcp_ok;

    println!("Lynx Doctor");
    println!();
    println!("Binary       OK");
    println!("Version      {version}");
    println!("Platform     {platform}");
    println!("Install      {install}");
    println!("Workspace    {}", workspace.display());
    println!("State        {}", verdict(state_ok));
    println!(
        "Repository   {}",
        if repository.is_some() {
            "detected (git)"
        } else if repository_detected {
            "detected"
        } else {
            "not detected"
        }
    );
    println!("Index        {freshness} ({symbols} symbols)");
    println!("MCP          {}", verdict(mcp_ok));
    println!();
    println!("Status       {}", if ready { "READY" } else { "NOT READY" });

    if !ready {
        println!();
        println!("Next steps");
        if !repository_detected {
            println!("  - run `lx doctor` inside a git work tree, or pass a path to `lx index`");
        }
        if !index_present {
            println!("  - run `lx index` to build the index for this workspace");
        } else if freshness != "fresh" {
            println!("  - run `lx index` to refresh the {freshness} index");
        }
        if !state_ok {
            println!(
                "  - the state directory is not writable: {}",
                storage.display()
            );
        }
        if !mcp_ok {
            println!("  - the MCP protocol check failed; run `lx mcp --check` for details");
        }
    }
    Ok(())
}

/// `lx mcp --check`: a fast, in-process MCP protocol smoke test.
///
/// Validates start, `initialize`, tool discovery, a basic request, and clean
/// termination without loading the embedding model or indexing a repository.
fn mcp_check() -> Result<()> {
    let check = smoke_check().context("running the MCP protocol smoke test")?;
    println!("Lynx MCP check");
    println!();
    println!("Protocol     {}", check.protocol_version);
    println!(
        "Server       {} {}",
        check.server_name, check.server_version
    );
    println!("Tools        {}", check.tools.join(", "));
    println!("Initialize   OK");
    println!("Tools list   OK");
    println!("Request      OK");
    println!("Shutdown     OK");
    println!();
    println!("Status       OK");
    Ok(())
}

/// `OK` / `FAIL` verdict label.
fn verdict(ok: bool) -> &'static str {
    if ok {
        "OK"
    } else {
        "FAIL"
    }
}

/// Whether `path` is a directory containing at least one entry.
fn directory_has_entries(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// Whether `path` exists and is not marked read-only.
///
/// A permissions heuristic (no probe file is written), sufficient for a
/// diagnostic; a probe would mutate the state directory doctor is inspecting.
fn writable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| !meta.permissions().readonly())
        .unwrap_or(false)
}

/// Human platform name for `std::env::consts::OS` values.
fn os_display() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    }
}

/// Release-matrix architecture name for `std::env::consts::ARCH` values.
///
/// Matches the artifact scheme (`arm64`, `x86_64`) rather than Rust's target
/// triple spelling (`aarch64`).
fn arch_display() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
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
