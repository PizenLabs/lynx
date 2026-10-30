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
use lynx_embed::{LazyFastEmbedProvider, VectorProvider};

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
    /// indexed is indexed exactly once, at startup. The embedding model is
    /// constructed lazily, so a server that only serves structural or lexical
    /// requests never loads it.
    pub fn open(workspace_root: &Path, include_tests: bool) -> Result<Self> {
        let provider = LazyFastEmbedProvider::with_cache_dir(model_cache_dir()?);
        Self::open_with(workspace_root, include_tests, Box::new(provider))
    }

    /// [`Self::open`] with an injected provider; the seam hermetic tests use
    /// instead of loading the ONNX model.
    pub fn open_with(
        workspace_root: &Path,
        include_tests: bool,
        provider: Box<dyn VectorProvider>,
    ) -> Result<Self> {
        let storage = storage_root(workspace_root);
        // Open the substrate once: the same handle both decides whether the
        // index is populated and serves the session. Probing population with a
        // separate engine doubled startup work (two SQLite connections, two
        // Tantivy readers) for no benefit.
        let engine = Engine::new(&storage, provider)
            .with_context(|| format!("opening index at {}", storage.display()))?;
        if engine.index_status().symbol_count == 0 {
            engine
                .update_repository(workspace_root, include_tests)
                .with_context(|| format!("indexing workspace {}", workspace_root.display()))?;
        } else {
            warn_if_stale(&engine);
        }
        Ok(Self {
            engine,
            workspace_root: workspace_root.to_path_buf(),
            storage_root: storage,
        })
    }

    /// Hydrates an existing persistent index using the default embedding model.
    pub fn hydrate(workspace_root: &Path) -> Result<Self> {
        let provider = LazyFastEmbedProvider::with_cache_dir(model_cache_dir()?);
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
        let provider = LazyFastEmbedProvider::with_cache_dir(model_cache_dir()?);
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
///
/// Reads `.git/HEAD` and the referenced ref directly for the common case; a
/// rare layout (custom `GIT_DIR`, unusual ref storage) falls back to the git
/// binary rather than losing the staleness warning.
fn git_head(dir: &Path) -> Option<String> {
    // No `.git` marker anywhere up the tree means this is not a work tree;
    // do not spawn git just to confirm that.
    git_toplevel(dir)?;
    read_git_head(dir).or_else(|| {
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
    })
}

/// Reads `HEAD` from a work tree without spawning `git`.
fn read_git_head(dir: &Path) -> Option<String> {
    let toplevel = git_toplevel(dir)?;
    let dot_git = toplevel.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let path = pointer.trim().strip_prefix("gitdir:")?.trim();
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            toplevel.join(path)
        }
    };

    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        return read_ref(&git_dir, reference);
    }
    is_git_object(head).then(|| head.to_string())
}

/// Reads a symbolic ref, checking loose refs and then `packed-refs`.
fn read_ref(git_dir: &Path, reference: &str) -> Option<String> {
    if let Some(hash) = read_ref_file(git_dir, reference) {
        return Some(hash);
    }
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|path| git_dir.join(path.trim()))
        .filter(|path| path != git_dir);
    if let Some(common) = common {
        if let Some(hash) = read_ref_file(&common, reference) {
            return Some(hash);
        }
        return read_packed_ref(&common.join("packed-refs"), reference);
    }
    read_packed_ref(&git_dir.join("packed-refs"), reference)
}

/// Reads a loose ref file, accepting only a valid object id.
fn read_ref_file(git_dir: &Path, reference: &str) -> Option<String> {
    let value = std::fs::read_to_string(git_dir.join(reference)).ok()?;
    let value = value.trim();
    is_git_object(value).then(|| value.to_string())
}

/// Finds a ref in `packed-refs`.
fn read_packed_ref(path: &Path, reference: &str) -> Option<String> {
    let packed = std::fs::read_to_string(path).ok()?;
    packed.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('^') {
            return None;
        }
        let (hash, name) = line.split_once(' ')?;
        (name.trim() == reference && is_git_object(hash)).then(|| hash.to_string())
    })
}

/// Whether `value` looks like a full git object id (SHA-1 or SHA-256).
fn is_git_object(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
///
/// Walks ancestors looking for the `.git` marker (a directory in a normal
/// checkout, a file in a linked worktree or submodule). This avoids spawning a
/// `git` process on the startup path of every command, which dominated the
/// cold latency of otherwise millisecond-scale operations.
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let mut current = Some(dir);
    while let Some(candidate) = current {
        if candidate.join(".git").exists() {
            return Some(candidate.to_path_buf());
        }
        current = candidate.parent();
    }
    None
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
    fn git_toplevel_finds_the_enclosing_work_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(git_toplevel(&nested).as_deref(), Some(root.as_path()));
    }

    #[test]
    fn git_head_reads_loose_and_packed_refs_without_git() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let git = root.join(".git");
        std::fs::create_dir_all(git.join("refs/heads")).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("refs/heads/main"), format!("{hash}\n")).unwrap();
        assert_eq!(read_git_head(&root).as_deref(), Some(hash));
        assert_eq!(git_head(&root).as_deref(), Some(hash));

        // Loose ref removed: the same ref is found in packed-refs.
        std::fs::remove_file(git.join("refs/heads/main")).unwrap();
        std::fs::write(
            git.join("packed-refs"),
            format!("# pack-refs with: peeled fully-peeled\n{hash} refs/heads/main\n"),
        )
        .unwrap();
        assert_eq!(read_git_head(&root).as_deref(), Some(hash));

        // Detached HEAD stores the object id directly.
        std::fs::write(git.join("HEAD"), format!("{hash}\n")).unwrap();
        assert_eq!(read_git_head(&root).as_deref(), Some(hash));
    }

    #[test]
    fn git_head_reads_common_refs_from_a_linked_worktree() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let git = main.join(".git");
        let worktree = tmp.path().join("feature");
        let worktree_git = git.join("worktrees/feature");
        std::fs::create_dir_all(git.join("refs/heads")).unwrap();
        std::fs::create_dir_all(&worktree_git).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("refs/heads/main"), format!("{hash}\n")).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .unwrap();
        std::fs::write(worktree_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(worktree_git.join("commondir"), "../..\n").unwrap();

        assert_eq!(git_toplevel(&worktree).as_deref(), Some(worktree.as_path()));
        assert_eq!(read_git_head(&worktree).as_deref(), Some(hash));
    }

    #[test]
    fn storage_root_is_dot_lynx() {
        assert_eq!(storage_root(Path::new("/ws")), PathBuf::from("/ws/.lynx"));
    }
}
