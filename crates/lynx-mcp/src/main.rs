//! Standalone Lynx MCP server: newline-delimited JSON-RPC 2.0 over stdio.
//!
//! Usage: `pizen-lynx-mcp [WORKSPACE_ROOT]`
//!
//! The workspace root defaults to the enclosing git toplevel of the current
//! directory (or the directory itself outside any work tree). Set
//! `LYNX_INCLUDE_TESTS=1` to index test, mock, and generated files.

use std::path::PathBuf;

use anyhow::{Context, Result};
use pizen_lynx_mcp::{workspace_root, ServerConfig};

fn main() -> Result<()> {
    let workspace_root = match std::env::args().nth(1) {
        Some(explicit) => PathBuf::from(explicit),
        None => workspace_root().context("resolving workspace root")?,
    };
    let config = ServerConfig {
        workspace_root,
        include_tests: std::env::var("LYNX_INCLUDE_TESTS").is_ok_and(|v| v == "1"),
    };
    pizen_lynx_mcp::run(config)
}
