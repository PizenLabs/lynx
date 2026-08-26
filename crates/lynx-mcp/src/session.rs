//! Session bootstrap shared by every Lynx transport entry point.
//!
//! The frozen storage substrate is write-only: it persists identities,
//! relations, and lexical docs, but the retrieval primitives of
//! [`Engine`] operate over in-memory symbols and embeddings that only
//! [`Engine::index_repository`] populates. A transport therefore cannot
//! hydrate from a pre-existing `.lynx` directory; instead each session
//! builds a fresh *ephemeral* substrate in a temporary directory, indexes
//! the resolved workspace once, and serves every query from that state.
//!
//! Consequences of this design:
//!
//! - Transport sessions never create, touch, or delete a user-visible
//!   `.lynx` directory; only `lx index` does that.
//! - Retrieval reflects the workspace contents at session start.
//! - Every session pays one indexing pass (parse + embed); the embedding
//!   model is loaded once per process.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lynx_core::Engine;
use lynx_embed::{FastEmbedProvider, VectorProvider};

/// A live retrieval session over an ephemeral substrate.
///
/// The owned temp directory keeps the substrate alive for the session's
/// lifetime and removes it on drop.
pub struct Session {
    engine: Engine,
    workspace_root: PathBuf,
    _substrate: tempfile::TempDir,
}

impl Session {
    /// Opens a session: fresh ephemeral substrate, indexed from
    pub fn open(workspace_root: &Path, include_tests: bool) -> Result<Self> {
        let provider = FastEmbedProvider::with_cache_dir(model_cache_dir()?)
            .context("initializing embedding model")?;
        Self::open_with(workspace_root, include_tests, Box::new(provider))
    }

    /// [`Self::open`] with an injected vector provider; the seam hermetic
    /// tests use instead of loading the ONNX model.
    pub fn open_with(
        workspace_root: &Path,
        include_tests: bool,
        provider: Box<dyn VectorProvider>,
    ) -> Result<Self> {
        let substrate = tempfile::tempdir().context("creating ephemeral index substrate")?;
        let engine =
            Engine::new(substrate.path(), provider).context("opening ephemeral substrate")?;
        engine
            .index_repository(workspace_root, include_tests)
            .with_context(|| format!("indexing workspace {}", workspace_root.display()))?;
        Ok(Self {
            engine,
            workspace_root: workspace_root.to_path_buf(),
            _substrate: substrate,
        })
    }

    /// Engine serving this session's queries.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Consumes the session, publishing its substrate directory to
    /// `dest` (which must not exist).
    ///
    /// `lx index` uses this to stage the index outside the indexed
    /// workspace and only materialize `.lynx` after the walk completes:
    /// the frozen engine excludes no `.lynx` segment, so indexing a
    /// workspace that already contains its own storage would ingest the
    /// storage files themselves.
    pub fn persist(self, dest: &Path) -> Result<()> {
        let Self {
            engine: _,
            workspace_root: _,
            _substrate,
        } = self;
        let source = _substrate.keep();
        Self::publish_dir(&source, dest)
            .with_context(|| format!("publishing {} -> {}", source.display(), dest.display()))
    }

    /// Moves or copies a directory tree into place.
    ///
    /// Prefers an atomic rename; falls back to a recursive copy when the
    /// source and destination live on different filesystems.
    fn publish_dir(source: &Path, dest: &Path) -> std::io::Result<()> {
        if std::fs::rename(source, dest).is_ok() {
            return Ok(());
        }
        // Cross-device or otherwise immovable: copy into place instead.
        copy_tree(source, dest)?;
        std::fs::remove_dir_all(source)
    }

    /// Workspace root this session was indexed from.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
}

/// Recursively copies the directory tree at `source` under `dest`.
fn copy_tree(source: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Resolves the workspace root for the current working directory: the
/// enclosing git toplevel when available, otherwise the directory itself.
pub fn workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("resolving current directory")?;
    Ok(git_toplevel(&cwd).unwrap_or(cwd))
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
        // A fresh temp dir is not inside any git work tree.
        let tmp = tempfile::tempdir().unwrap();
        assert!(git_toplevel(tmp.path()).is_none());
    }
}
