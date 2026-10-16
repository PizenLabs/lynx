//! Session bootstrap shared by every Lynx transport entry point.
//!
//! A [`Session`] is a hydrated handle over the *persistent* index at
//! `<workspace_root>/.lynx`. It is built by [`Session::hydrate`] /
//! [`Session::hydrate_with`], which read provenance and counts from storage and
//! perform **no** repository walk, file read, or embedding. Only
//! [`Session::build`] / [`Session::open_with`] (when the index is absent) run an
//! indexing pass.
//!
//! Consequences:
//! - a query path never re-indexes the repository;
//! - reopening a session is `O(1)` in repository size;
//! - `lx index` and `lx mcp` produce and consume the same `.lynx` artifact.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use lynx_core::{Engine, IndexUpdate};
use lynx_embed::{FastEmbedProvider, VectorProvider};

/// Directory (relative to the workspace root) holding the persistent index.
pub const STORAGE_DIR: &str = ".lynx";

/// A retrieval session over the persistent index at `<workspace_root>/.lynx`.
pub struct Session {
    engine: Engine,
    workspace_root: PathBuf,
    storage_root: PathBuf,
}

impl Session {
    /// Opens `workspace_root`, hydrating its index; builds one first when absent.
    ///
    /// This is the MCP startup policy: an agent's first request never pays a
    /// full indexing pass *per query*, but a workspace that has never been
    /// indexed is indexed exactly once, at startup.
    pub fn open(workspace_root: &Path, include_tests: bool) -> Result<Self> {
        let provider = FastEmbedProvider::with_cache_dir(model_cache_dir()?)
            .context("initializing embedding model")?;
        Self::open_with(workspace_root, include_tests, Box::new(provider))
    }

    /// [`Self::open`] with an injected provider; the seam hermetic tests use
    /// instead of loading the ONNX model.
    pub fn open_with(
        workspace_root: &Path,
        include_tests: bool,
        provider: Box<dyn VectorProvider>,
    ) -> Result<Self> {
        if storage_root(workspace_root).is_dir() && storage_is_populated(workspace_root)? {
            return Self::hydrate_with(workspace_root, provider);
        }
        Self::build_with(workspace_root, include_tests, provider)
    }

    /// Hydrates an existing persistent index using the default embedding model.
    pub fn hydrate(workspace_root: &Path) -> Result<Self> {
        let provider = FastEmbedProvider::with_cache_dir(model_cache_dir()?)
            .context("initializing embedding model")?;
        Self::hydrate_with(workspace_root, Box::new(provider))
    }

    /// Hydrates an existing persistent index with an injected provider.
    ///
    /// Fails with an actionable message when the workspace has not been indexed;
    /// it never indexes on this path.
    pub fn hydrate_with(workspace_root: &Path, provider: Box<dyn VectorProvider>) -> Result<Self> {
        let storage = storage_root(workspace_root);
        if !storage.is_dir() {
            bail!(
                "workspace {} is not indexed (no {}); run `lx index` first",
                workspace_root.display(),
                storage.display()
            );
        }
        let engine = Engine::new(&storage, provider)
            .with_context(|| format!("opening index at {}", storage.display()))?;
        let status = engine.index_status();
        if status.symbol_count == 0 {
            bail!("index at {} is empty; re-run `lx index`", storage.display());
        }
        warn_if_stale(&engine);
        Ok(Self {
            engine,
            workspace_root: workspace_root.to_path_buf(),
            storage_root: storage,
        })
    }

    /// Builds (or incrementally updates) the persistent index for
    /// `workspace_root`, returning the applied change set.
    pub fn update(workspace_root: &Path, include_tests: bool) -> Result<(Self, IndexUpdate)> {
        let provider = FastEmbedProvider::with_cache_dir(model_cache_dir()?)
            .context("initializing embedding model")?;
        Self::update_with(workspace_root, include_tests, Box::new(provider))
    }

    /// Builds (or rebuilds) the persistent index for `workspace_root`.
    pub fn build(workspace_root: &Path, include_tests: bool) -> Result<Self> {
        Ok(Self::update(workspace_root, include_tests)?.0)
    }

    /// [`Self::build`] with an injected provider.
    ///
    /// Indexes the workspace directly into `<workspace_root>/.lynx`. The engine
    /// excludes `.lynx` from the walk, so the index never ingests itself.
    pub fn build_with(
        workspace_root: &Path,
        include_tests: bool,
        provider: Box<dyn VectorProvider>,
    ) -> Result<Self> {
        Ok(Self::update_with(workspace_root, include_tests, provider)?.0)
    }

    /// [`Self::update`] with an injected provider, returning the applied
    /// [`IndexUpdate`] so a caller can report what changed.
    ///
    /// This is the incremental path: on an existing index only added, modified,
    /// and deleted files are touched.
    pub fn update_with(
        workspace_root: &Path,
        include_tests: bool,
        provider: Box<dyn VectorProvider>,
    ) -> Result<(Self, IndexUpdate)> {
        let storage = storage_root(workspace_root);
        let engine = Engine::new(&storage, provider)
            .with_context(|| format!("opening index at {}", storage.display()))?;
        let update = engine
            .update_repository(workspace_root, include_tests)
            .with_context(|| format!("indexing workspace {}", workspace_root.display()))?;
        Ok((
            Self {
                engine,
                workspace_root: workspace_root.to_path_buf(),
                storage_root: storage,
            },
            update,
        ))
    }

    /// An ephemeral session over a fresh temporary substrate.
    ///
    /// Used by the MCP smoke check ([`crate::smoke_check`]) and tests: it opens
    /// an empty index in a temporary directory with an injected provider, so it
    /// neither loads the embedding model nor indexes any repository. The
    /// returned [`tempfile::TempDir`] guard must be held for the session's
    /// lifetime.
    pub(crate) fn ephemeral(
        provider: Box<dyn VectorProvider>,
    ) -> Result<(Self, tempfile::TempDir)> {
        let dir = tempfile::tempdir().context("creating temporary MCP workspace")?;
        let workspace_root = dir.path().to_path_buf();
        let storage = storage_root(&workspace_root);
        let engine = Engine::new(&storage, provider)
            .with_context(|| format!("opening index at {}", storage.display()))?;
        Ok((
            Self {
                engine,
                workspace_root,
                storage_root: storage,
            },
            dir,
        ))
    }

    /// Engine serving this session's queries.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Workspace root this session was indexed from.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Persistent index directory backing this session.
    pub fn storage_root(&self) -> &Path {
        &self.storage_root
    }
}

/// The persistent index directory for `workspace_root`.
pub fn storage_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(STORAGE_DIR)
}

/// Whether the index under `workspace_root` has at least one symbol, read
/// without constructing an engine.
fn storage_is_populated(workspace_root: &Path) -> Result<bool> {
    let storage = storage_root(workspace_root);
    let db = storage.join("graph.sqlite3");
    if !db.is_file() {
        return Ok(false);
    }
    let engine = Engine::new(&storage, Box::new(lynx_embed::UnavailableProvider))
        .with_context(|| format!("opening index at {}", storage.display()))?;
    Ok(engine.index_status().symbol_count > 0)
}

/// Emits a stderr warning when the persisted index was built from a different
/// git commit than the workspace's current HEAD.
///
/// This is a cheap git-only signal, safe for the query start-up path (no
/// repository scan). The authoritative check is [`lynx_core::Engine::freshness`],
/// exposed through `lx status` and the MCP `index_status` tool.
fn warn_if_stale(engine: &Engine) {
    let snapshot = engine.index_status().snapshot;
    let Some(indexed) = snapshot.commit_hash.as_deref() else {
        return;
    };
    let Some(head) = git_head(&snapshot.workspace_root) else {
        return;
    };
    if indexed != head {
        eprintln!(
            "lynx: warning: index is stale (indexed commit {indexed}, workspace HEAD {head}); run `lx index` to refresh"
        );
    }
}

/// Resolves the current `HEAD` commit for `dir`, when it is a git work tree.
fn git_head(dir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!head.is_empty()).then_some(head)
}

/// Resolves the workspace root for the current working directory: the
/// enclosing git toplevel when available, otherwise the directory itself.
pub fn workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("resolving current directory")?;
    Ok(git_toplevel(&cwd).unwrap_or(cwd))
}

/// The enclosing git toplevel of the current directory, when it is inside a
/// work tree.
///
/// `None` means the current directory is not inside a git repository, which is
/// a valid state: Lynx indexes any directory.
pub fn git_root() -> Result<Option<PathBuf>> {
    let cwd = std::env::current_dir().context("resolving current directory")?;
    Ok(git_toplevel(&cwd))
}

/// Stable per-user directory for embedding-model weights.
///
/// fastembed would otherwise write `.fastembed_cache` (including zero-byte
/// lock files) into the process working directory.
fn model_cache_dir() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return Ok(PathBuf::from(home)
            .join(".cache")
            .join("lynx")
            .join("fastembed"));
    }
    Ok(std::env::temp_dir().join("lynx-fastembed"))
}

/// Git toplevel containing `dir`, if `dir` is inside a work tree.
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_toplevel_is_none_outside_work_tree() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(git_toplevel(tmp.path()).is_none());
    }

    #[test]
    fn storage_root_is_dot_lynx() {
        assert_eq!(storage_root(Path::new("/ws")), PathBuf::from("/ws/.lynx"));
    }
}
