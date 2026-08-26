//! The seven MCP tools and their strict mapping onto [`Engine`] primitives.
//!
//! No internal storage or database surface is exposed: every tool delegates
//! to exactly one retrieval primitive and serializes its protocol result.

use std::path::Path;

use lynx_core::{Engine, IndexStatus};
use lynx_protocol::{RelationKind, RetrievalMode};
use serde::Deserialize;
use serde_json::{json, Value};

/// Default result cap for `search` / `similar` when the client omits one.
pub(crate) const DEFAULT_LIMIT: usize = 10;
/// Default token budget for `context`.
const DEFAULT_TOKEN_BUDGET: usize = 2048;

/// Wire definitions for `tools/list`, in stable order.
pub(crate) fn definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "search",
                "description": "Hybrid code search: BM25 + embedding similarity fused by reciprocal rank fusion. Returns ranked Evidence items.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Natural-language or identifier query"},
                        "mode": {"type": "string", "enum": ["lexical", "semantic", "structural", "hybrid"], "description": "Retrieval channel (default: hybrid)"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Maximum results (default: 10)"}
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "resolve",
                "description": "Exact symbol lookup by fully-qualified name or trailing name. Returns the resolved Evidence with SymbolIdentity coordinates, or null.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Symbol fqdn (`auth::service::login`) or trailing name (`login`)"}
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "inspect",
                "description": "Full evidence for a symbol by content hash.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol_hash": {"type": "string", "description": "Symbol content hash"}
                    },
                    "required": ["symbol_hash"]
                }
            },
            {
                "name": "relations",
                "description": "Structural relation graph incident to a symbol (as source or target).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol_hash": {"type": "string", "description": "Symbol content hash"},
                        "kind": {"type": "string", "enum": ["Calls", "CalledBy", "Implements", "ImplementedBy", "Imports", "Contains"], "description": "Optional edge filter"}
                    },
                    "required": ["symbol_hash"]
                }
            },
            {
                "name": "similar",
                "description": "Embedding-similarity neighbors of a symbol, self excluded.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol_hash": {"type": "string", "description": "Symbol content hash"}
                    },
                    "required": ["symbol_hash"]
                }
            },
            {
                "name": "context",
                "description": "Compile a token-budgeted ContextPackage: ranked evidence plus structural relations for a query.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Query to assemble context for"},
                        "token_budget": {"type": "integer", "minimum": 1, "description": "Token ceiling (default: 2048)"}
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "index_status",
                "description": "Provenance and population counts of the index this server is serving.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Optional workspace path; must match the served workspace root"}
                    },
                    "required": []
                }
            }
        ]
    })
}

/// Tool-execution failure surfaced as an MCP `isError` result (or a JSON-RPC
/// error for legacy direct-method calls).
#[derive(Debug)]
pub(crate) struct ToolFailure {
    /// JSON-RPC error code (-32602 invalid params, -32603 internal).
    pub code: i64,
    /// Human-readable message.
    pub message: String,
}

impl ToolFailure {
    fn invalid_params(message: String) -> Self {
        Self {
            code: -32602,
            message,
        }
    }

    fn internal(message: String) -> Self {
        Self {
            code: -32603,
            message,
        }
    }
}

/// Dispatches one tool invocation against `engine`.
///
/// `workspace_root` backs the `index_status` path check. Returns the raw
/// protocol JSON for the primitive.
pub(crate) fn dispatch(
    engine: &Engine,
    workspace_root: &Path,
    tool: &str,
    params: &Value,
) -> Result<Value, ToolFailure> {
    match tool {
        "search" => search(engine, params),
        "resolve" => resolve(engine, params),
        "inspect" => inspect(engine, params),
        "relations" => relations(engine, params),
        "similar" => similar(engine, params),
        "context" => context(engine, params),
        "index_status" => index_status(engine, workspace_root, params),
        other => Err(ToolFailure::invalid_params(format!(
            "unknown tool: {other}"
        ))),
    }
}

/// Names of every exposed tool, for `tools/call` validation.
pub(crate) const TOOL_NAMES: &[&str] = &[
    "search",
    "resolve",
    "inspect",
    "relations",
    "similar",
    "context",
    "index_status",
];

fn search(engine: &Engine, params: &Value) -> Result<Value, ToolFailure> {
    #[derive(Deserialize)]
    struct Params {
        query: String,
        mode: Option<String>,
        limit: Option<u32>,
    }
    let p: Params = parse(params)?;
    let mode = parse_mode(p.mode.as_deref())?;
    let limit = clamp_limit(p.limit);
    let evidence = engine
        .search(&p.query, mode, limit)
        .map_err(|e| ToolFailure::internal(e.to_string()))?;
    Ok(serde_json::to_value(evidence).expect("Evidence serializes"))
}

fn resolve(engine: &Engine, params: &Value) -> Result<Value, ToolFailure> {
    #[derive(Deserialize)]
    struct Params {
        name: String,
    }
    let p: Params = parse(params)?;
    let evidence = engine
        .resolve(&p.name)
        .map_err(|e| ToolFailure::internal(e.to_string()))?;
    Ok(serde_json::to_value(evidence).expect("Option<Evidence> serializes"))
}

fn inspect(engine: &Engine, params: &Value) -> Result<Value, ToolFailure> {
    #[derive(Deserialize)]
    struct Params {
        symbol_hash: String,
    }
    let p: Params = parse(params)?;
    let evidence = engine
        .inspect(&p.symbol_hash)
        .map_err(|e| ToolFailure::internal(e.to_string()))?;
    Ok(serde_json::to_value(evidence).expect("Option<Evidence> serializes"))
}

fn relations(engine: &Engine, params: &Value) -> Result<Value, ToolFailure> {
    #[derive(Deserialize)]
    struct Params {
        symbol_hash: String,
        kind: Option<String>,
    }
    let p: Params = parse(params)?;
    let kind = match p.kind.as_deref() {
        None => None,
        Some(kind) => Some(parse_relation_kind(kind)?),
    };
    let relations = engine
        .relations(&p.symbol_hash, kind)
        .map_err(|e| ToolFailure::internal(e.to_string()))?;
    Ok(serde_json::to_value(relations).expect("Vec<Relation> serializes"))
}

fn similar(engine: &Engine, params: &Value) -> Result<Value, ToolFailure> {
    #[derive(Deserialize)]
    struct Params {
        symbol_hash: String,
    }
    let p: Params = parse(params)?;
    let evidence = engine
        .similar(&p.symbol_hash, DEFAULT_LIMIT)
        .map_err(|e| ToolFailure::internal(e.to_string()))?;
    Ok(serde_json::to_value(evidence).expect("Vec<Evidence> serializes"))
}

fn context(engine: &Engine, params: &Value) -> Result<Value, ToolFailure> {
    #[derive(Deserialize)]
    struct Params {
        query: String,
        token_budget: Option<u32>,
    }
    let p: Params = parse(params)?;
    let budget = p.token_budget.map_or(DEFAULT_TOKEN_BUDGET, |b| b as usize);
    let package = engine
        .context(&p.query, budget)
        .map_err(|e| ToolFailure::internal(e.to_string()))?;
    Ok(serde_json::to_value(package).expect("ContextPackage serializes"))
}

fn index_status(
    engine: &Engine,
    workspace_root: &Path,
    params: &Value,
) -> Result<Value, ToolFailure> {
    #[derive(Deserialize)]
    struct Params {
        path: Option<String>,
    }
    let p: Params = parse(params)?;
    if let Some(requested) = p.path.as_deref() {
        if !same_path(requested, workspace_root) {
            return Err(ToolFailure::invalid_params(format!(
                "workspace not indexed at {requested}; this server serves {}",
                workspace_root.display()
            )));
        }
    }
    Ok(status_json(&engine.index_status()))
}

fn status_json(status: &IndexStatus) -> Value {
    json!({
        "snapshot": status.snapshot,
        "file_count": status.file_count,
        "symbol_count": status.symbol_count,
        "relation_count": status.relation_count,
        "capability_level": status.capability_level,
    })
}

fn parse<P: serde::de::DeserializeOwned>(params: &Value) -> Result<P, ToolFailure> {
    serde_json::from_value(params.clone())
        .map_err(|e| ToolFailure::invalid_params(format!("invalid params: {e}")))
}

/// Lenient case-insensitive `RetrievalMode` wire parsing; absent -> hybrid.
fn parse_mode(mode: Option<&str>) -> Result<RetrievalMode, ToolFailure> {
    let Some(mode) = mode else {
        return Ok(RetrievalMode::Hybrid);
    };
    match mode.to_ascii_lowercase().as_str() {
        "lexical" => Ok(RetrievalMode::Lexical),
        "semantic" => Ok(RetrievalMode::Semantic),
        "structural" => Ok(RetrievalMode::Structural),
        "hybrid" => Ok(RetrievalMode::Hybrid),
        other => Err(ToolFailure::invalid_params(format!(
            "invalid mode `{other}`; expected lexical, semantic, structural, or hybrid"
        ))),
    }
}

/// Lenient case-insensitive `RelationKind` wire parsing.
fn parse_relation_kind(kind: &str) -> Result<RelationKind, ToolFailure> {
    let lowered = kind.to_ascii_lowercase();
    match lowered.as_str() {
        "calls" => Ok(RelationKind::Calls),
        "calledby" => Ok(RelationKind::CalledBy),
        "implements" => Ok(RelationKind::Implements),
        "implementedby" => Ok(RelationKind::ImplementedBy),
        "imports" => Ok(RelationKind::Imports),
        "contains" => Ok(RelationKind::Contains),
        other => Err(ToolFailure::invalid_params(format!(
            "invalid relation kind `{other}`"
        ))),
    }
}

fn clamp_limit(limit: Option<u32>) -> usize {
    limit.map_or(DEFAULT_LIMIT, |l| l.clamp(1, 100) as usize)
}

/// Path equality tolerant of trailing separators and unresolved symlinks.
fn same_path(requested: &str, served: &Path) -> bool {
    let requested = Path::new(requested);
    let strip = |p: &Path| p.to_string_lossy().trim_end_matches('/').to_string();
    strip(requested) == strip(served)
}
