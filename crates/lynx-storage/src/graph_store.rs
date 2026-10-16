//! SQLite (WAL) substrate for relational identity, structural graph, vectors,
//! file provenance, and workspace metadata.
//!
//! Everything a retrieval query needs *except* the lexical snippets is queryable
//! here, so a hydrated session never materializes repository-proportional state:
//! exact resolution, symbol metadata, embeddings, and counts are all answered by
//! bounded SQL.
//!
//! # Schema versioning
//!
//! The schema is versioned with `PRAGMA user_version`. A database written by an
//! older Lynx (or a partial write) is detected on open and reset to the current
//! schema: the substrate is a *derived* index, so the correct recovery is to
//! re-index, not to read stale bytes.

use std::path::Path;
use std::sync::Mutex;

use lynx_protocol::{
    Language, Relation, RelationKind, Snapshot, SourceRange, SymbolIdentity, SymbolKind,
};
use rusqlite::{params, Connection};

use crate::error::StorageError;

/// Current on-disk schema version. Bump whenever the layout changes.
pub const SCHEMA_VERSION: i64 = 3;

/// Maximum number of `?` bind parameters per statement. SQLite's default limit
/// is 999; chunk large `IN (...)` lookups below it.
const PARAM_CHUNK: usize = 500;

/// Metadata key holding the content hash of the most recently persisted
/// snapshot. `latest_snapshot` reads this rather than relying on timestamp
/// ordering, which is not monotonic at second resolution.
pub const LATEST_SNAPSHOT_META: &str = "latest_snapshot";

/// Relational store over `rusqlite` with WAL journaling.
///
/// The connection sits behind a `Mutex`, so a `GraphStore` is `Send + Sync`
/// and safe to share across threads.
pub struct GraphStore {
    conn: Mutex<Connection>,
    /// Whether opening the database observed a schema-version mismatch and
    /// reset the substrate. Surfaced as an `Incompatible` freshness signal.
    schema_reset: bool,
}

impl GraphStore {
    /// Opens (creating if absent) the database file at `path`.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::initialize(conn)
    }

    /// Opens an unnamed in-memory database; intended for tests.
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        Self::initialize(conn)
    }

    fn initialize(conn: Connection) -> Result<Self, StorageError> {
        apply_pragmas(&conn)?;
        // A version mismatch means an older or partial schema; reset to current.
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let schema_reset = if version != SCHEMA_VERSION {
            drop_all_tables(&conn)?;
            create_tables(&conn)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            version != 0
        } else {
            false
        };
        Ok(Self {
            conn: Mutex::new(conn),
            schema_reset,
        })
    }

    /// Whether opening this database discarded an incompatible schema.
    ///
    /// A `true` value means the previous index was written by a different
    /// Lynx schema version and has been reset to empty; the caller should
    /// re-index rather than trust the (now empty) substrate.
    pub fn schema_reset(&self) -> bool {
        self.schema_reset
    }

    /// Runs SQLite's `PRAGMA quick_check`, returning `true` when the database
    /// is structurally sound. A corrupt artifact fails this instead of
    /// silently serving partial or inconsistent state.
    pub fn quick_check(&self) -> Result<bool, StorageError> {
        let conn = self.lock()?;
        let result: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        Ok(result.eq_ignore_ascii_case("ok"))
    }

    /// Persists a workspace snapshot.
    ///
    /// Idempotent: re-persisting an already-known `content_hash` is a no-op, so
    /// a session may re-index a workspace without erroring on provenance.
    pub fn save_snapshot(&self, snapshot: &Snapshot) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO snapshots (content_hash, commit_hash, workspace_root, is_dirty)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                snapshot.content_hash,
                snapshot.commit_hash,
                snapshot.workspace_root.to_string_lossy(),
                i64::from(snapshot.is_dirty),
            ],
        )?;
        Ok(())
    }

    /// Records workspace metadata (latest snapshot hash, capability level).
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Reads workspace metadata written by [`Self::set_meta`].
    pub fn get_meta(&self, key: &str) -> Result<Option<String>, StorageError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare_cached("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = statement.query(params![key])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    /// Inserts symbols observed in the snapshot identified by `snapshot_hash`.
    ///
    /// All rows land in one transaction. Each entry pairs the identity with
    /// its source range so the row carries everything needed to rebuild an
    /// [`Evidence`] coordinate without retaining it in memory. The trailing
    /// name is denormalized for `O(log n)` exact resolution.
    pub fn insert_symbols(
        &self,
        entries: &[(SymbolIdentity, SourceRange)],
        snapshot_hash: &str,
    ) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute("BEGIN", [])?;
        let result = entries.iter().try_for_each(|(identity, range)| {
            conn.execute(
                "INSERT OR IGNORE INTO symbols (
                    content_hash, fqdn, trailing_name, kind, language, file_path,
                    start_byte, end_byte, start_line, end_line, snapshot_hash
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    identity.content_hash,
                    identity.fqdn,
                    identity.trailing_name(),
                    encode_symbol_kind(identity.kind),
                    encode_language(identity.language),
                    identity.file_path.to_string_lossy(),
                    range.start_byte as i64,
                    range.end_byte as i64,
                    range.start_line as i64,
                    range.end_line as i64,
                    snapshot_hash,
                ],
            )
            .map(|_| ())
        });
        finish(conn, result)
    }

    /// Records the indexed files, their per-file content hash, and the snapshot
    /// they belong to.
    ///
    /// This is the durable state an incremental pass diffs against: only paths
    /// whose stored `content_hash` differs from the current file are re-parsed
    /// and re-embedded.
    pub fn upsert_files(
        &self,
        files: &[(String, String)],
        snapshot_hash: &str,
    ) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute("BEGIN", [])?;
        let result = files.iter().try_for_each(|(path, content_hash)| {
            conn.execute(
                "INSERT INTO files (path, content_hash, snapshot_hash) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET
                    content_hash = excluded.content_hash,
                    snapshot_hash = excluded.snapshot_hash",
                params![path, content_hash, snapshot_hash],
            )
            .map(|_| ())
        });
        finish(conn, result)
    }

    /// Returns every indexed file as `(path, content_hash)`, ordered by path.
    pub fn file_states(&self) -> Result<Vec<(String, String)>, StorageError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare("SELECT path, content_hash FROM files ORDER BY path")?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        Ok(out)
    }

    /// Removes every row owned by the given workspace-relative file paths:
    /// their symbols, the relations incident to those symbols, their embedding
    /// vectors, and the file records themselves.
    ///
    /// Everything lands in one transaction, so an interrupted update can never
    /// leave a file half-removed. Idempotent for unknown paths.
    pub fn delete_files(&self, paths: &[String]) -> Result<(), StorageError> {
        if paths.is_empty() {
            return Ok(());
        }
        let conn = self.lock()?;
        conn.execute("BEGIN", [])?;
        let result = paths.iter().try_for_each(|path| {
            conn.execute(
                "DELETE FROM relations
                 WHERE source_id IN (SELECT content_hash FROM symbols WHERE file_path = ?1)
                    OR target_id IN (SELECT content_hash FROM symbols WHERE file_path = ?1)",
                params![path],
            )?;
            conn.execute(
                "DELETE FROM embeddings
                 WHERE content_hash IN (SELECT content_hash FROM symbols WHERE file_path = ?1)",
                params![path],
            )?;
            conn.execute("DELETE FROM symbols WHERE file_path = ?1", params![path])?;
            conn.execute("DELETE FROM files WHERE path = ?1", params![path])?;
            Ok(())
        });
        finish(conn, result)
    }

    /// Inserts relations observed in the snapshot identified by
    /// `snapshot_hash`.
    ///
    /// All rows land in one transaction. Foreign keys are enforced: both
    /// endpoint symbol hashes must already exist in `symbols`, otherwise the
    /// insert fails. Idempotent: a duplicate edge is ignored.
    pub fn insert_relations(
        &self,
        relations: &[Relation],
        snapshot_hash: &str,
    ) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute("BEGIN", [])?;
        let result = relations.iter().try_for_each(|relation| {
            conn.execute(
                "INSERT OR IGNORE INTO relations (source_id, target_id, kind, snapshot_hash)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    relation.source_id,
                    relation.target_id,
                    encode_relation_kind(relation.kind),
                    snapshot_hash,
                ],
            )
            .map(|_| ())
        });
        finish(conn, result)
    }

    /// Stores embedding vectors for `items`, keyed by symbol content hash.
    ///
    /// Vectors are persisted (little-endian `f32`), so a hydrated session can
    /// answer semantic queries without re-embedding the repository.
    pub fn put_embeddings(&self, items: &[(String, Vec<f32>)]) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute("BEGIN", [])?;
        let result = items.iter().try_for_each(|(hash, vector)| {
            conn.execute(
                "INSERT OR REPLACE INTO embeddings (content_hash, dim, vector) VALUES (?1, ?2, ?3)",
                params![hash, vector.len() as i64, encode_vector(vector)],
            )
            .map(|_| ())
        });
        finish(conn, result)
    }

    /// Loads one embedding vector by symbol content hash.
    pub fn embedding(&self, hash: &str) -> Result<Option<Vec<f32>>, StorageError> {
        let conn = self.lock()?;
        let mut statement =
            conn.prepare_cached("SELECT vector FROM embeddings WHERE content_hash = ?1")?;
        let mut rows = statement.query(params![hash])?;
        match rows.next()? {
            Some(row) => Ok(Some(decode_vector(&row.get::<_, Vec<u8>>(0)?))),
            None => Ok(None),
        }
    }

    /// Streams every stored embedding through `visit`, one vector at a time.
    ///
    /// Memory is `O(1)` in the number of embeddings (one decoded vector is
    /// reused), so a semantic scan never materializes the whole vector store.
    /// `visit` returns `false` to stop early (used for cancellation/deadline).
    pub fn for_each_embedding<F>(&self, mut visit: F) -> Result<(), StorageError>
    where
        F: FnMut(&str, &[f32]) -> bool,
    {
        let conn = self.lock()?;
        let mut statement =
            conn.prepare("SELECT content_hash, vector FROM embeddings ORDER BY content_hash")?;
        let mut rows = statement.query([])?;
        let mut buffer: Vec<f32> = Vec::new();
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            let bytes: Vec<u8> = row.get(1)?;
            decode_vector_into(&bytes, &mut buffer);
            if !visit(&hash, &buffer) {
                break;
            }
        }
        Ok(())
    }

    /// Loads a symbol identity and its retained range by content hash.
    pub fn get_symbol(
        &self,
        hash: &str,
    ) -> Result<Option<(SymbolIdentity, SourceRange)>, StorageError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare_cached(
            "SELECT fqdn, kind, language, file_path, start_byte, end_byte, start_line, end_line
             FROM symbols WHERE content_hash = ?1",
        )?;
        let mut rows = stmt.query(params![hash])?;
        match rows.next()? {
            Some(row) => Ok(Some(symbol_from_hash_row(row, hash)?)),
            None => Ok(None),
        }
    }

    /// Exact, deterministic resolution of `needle` (an fqdn or a trailing name)
    /// to a symbol identity and range.
    ///
    /// An exact fqdn match wins. A trailing-name match picks the
    /// lexicographically smallest `(fqdn, content_hash)` — deterministic across
    /// runs and independent of insertion order. Returns `None` when unknown.
    pub fn resolve(
        &self,
        needle: &str,
    ) -> Result<Option<(SymbolIdentity, SourceRange)>, StorageError> {
        Ok(self.resolve_all(needle)?.into_iter().next())
    }

    /// Every symbol matching `needle` (exact fqdn or trailing name), ordered
    /// deterministically by `(fqdn, content_hash)`.
    ///
    /// Callers that need ambiguity semantics receive the full set rather than a
    /// silently-chosen winner.
    pub fn resolve_all(
        &self,
        needle: &str,
    ) -> Result<Vec<(SymbolIdentity, SourceRange)>, StorageError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare_cached(
            "SELECT content_hash, fqdn, kind, language, file_path,
                    start_byte, end_byte, start_line, end_line
             FROM symbols
             WHERE fqdn = ?1 OR trailing_name = ?1
             ORDER BY fqdn ASC, content_hash ASC",
        )?;
        let mut rows = statement.query(params![needle])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(symbol_from_resolve_row(row)?);
        }
        Ok(out)
    }

    /// Batch-loads symbol identities and ranges for `hashes`.
    ///
    /// Chunked to stay within SQLite's bind-parameter limit; returns rows in
    /// unspecified order (callers key by hash).
    pub fn symbols_for(
        &self,
        hashes: &[String],
    ) -> Result<Vec<(String, SymbolIdentity, SourceRange)>, StorageError> {
        if hashes.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock()?;
        let mut out = Vec::with_capacity(hashes.len());
        for chunk in hashes.chunks(PARAM_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT content_hash, fqdn, kind, language, file_path,
                        start_byte, end_byte, start_line, end_line
                 FROM symbols WHERE content_hash IN ({placeholders})"
            );
            let mut statement = conn.prepare(&sql)?;
            let mut rows = statement.query(rusqlite::params_from_iter(chunk.iter()))?;
            while let Some(row) = rows.next()? {
                let (identity, range) = symbol_from_resolve_row(row)?;
                out.push((identity.content_hash.clone(), identity, range));
            }
        }
        Ok(out)
    }

    /// Whether any symbol is indexed.
    pub fn has_symbols(&self) -> Result<bool, StorageError> {
        let conn = self.lock()?;
        let count: i64 =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM symbols)", [], |row| row.get(0))?;
        Ok(count != 0)
    }

    /// `(files, symbols, relations)` population counts.
    pub fn counts(&self) -> Result<(usize, usize, usize), StorageError> {
        let conn = self.lock()?;
        let files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        let symbols: i64 = conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
        let relations: i64 =
            conn.query_row("SELECT COUNT(*) FROM relations", [], |row| row.get(0))?;
        Ok((files as usize, symbols as usize, relations as usize))
    }

    /// Returns the most recently persisted workspace snapshot, if any.
    ///
    /// The "latest" snapshot is the content hash recorded under
    /// [`LATEST_SNAPSHOT_META`] by the last indexing pass, falling back to
    /// timestamp ordering for indexes written before that key existed.
    pub fn latest_snapshot(&self) -> Result<Option<Snapshot>, StorageError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare_cached(
            "SELECT content_hash, commit_hash, workspace_root, is_dirty
             FROM snapshots
             WHERE content_hash = COALESCE(
                 (SELECT value FROM meta WHERE key = ?1),
                 (SELECT content_hash FROM snapshots
                  ORDER BY created_at DESC, content_hash ASC LIMIT 1))
             LIMIT 1",
        )?;
        let mut rows = statement.query(params![LATEST_SNAPSHOT_META])?;
        match rows.next()? {
            Some(row) => Ok(Some(Snapshot {
                content_hash: row.get(0)?,
                commit_hash: row.get(1)?,
                workspace_root: std::path::PathBuf::from(row.get::<_, String>(2)?),
                is_dirty: row.get::<_, i64>(3)? != 0,
            })),
            None => Ok(None),
        }
    }

    /// Drops all persisted state and recreates the empty current schema.
    ///
    /// Used before a full re-index so a rebuild cannot retain stale symbols,
    /// vectors, or edges.
    pub fn reset(&self) -> Result<(), StorageError> {
        let conn = self.lock()?;
        drop_all_tables(&conn)?;
        create_tables(&conn)?;
        Ok(())
    }

    /// Returns edges incident to `symbol_hash` (as source or as target),
    /// optionally filtered by `kind`.
    pub fn get_relations(
        &self,
        symbol_hash: &str,
        kind: Option<RelationKind>,
    ) -> Result<Vec<Relation>, StorageError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare_cached(
            "SELECT source_id, target_id, kind FROM relations
             WHERE (source_id = ?1 OR target_id = ?1) AND (?2 IS NULL OR kind = ?2)
             ORDER BY source_id, target_id",
        )?;
        let mut rows = statement.query(params![symbol_hash, kind.map(encode_relation_kind),])?;
        let mut relations = Vec::new();
        while let Some(row) = rows.next()? {
            relations.push(Relation {
                source_id: row.get(0)?,
                target_id: row.get(1)?,
                kind: decode_relation_kind(&row.get::<_, String>(2)?)?,
            });
        }
        Ok(relations)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StorageError> {
        self.conn
            .lock()
            .map_err(|poisoned| StorageError::Poisoned(poisoned.to_string()))
    }
}

/// Rebuilds an identity + range from a 9-column resolve row:
/// `(content_hash, fqdn, kind, language, file_path, start_byte, end_byte,
/// start_line, end_line)`.
fn symbol_from_resolve_row(
    row: &rusqlite::Row<'_>,
) -> Result<(SymbolIdentity, SourceRange), StorageError> {
    let content_hash: String = row.get(0)?;
    symbol_from_hash_row_at(row, &content_hash, 1)
}

/// Rebuilds an identity + range from an 8-column row beginning at `fqdn`.
fn symbol_from_hash_row(
    row: &rusqlite::Row<'_>,
    hash: &str,
) -> Result<(SymbolIdentity, SourceRange), StorageError> {
    symbol_from_hash_row_at(row, hash, 0)
}

/// Rebuilds an identity + range from `(fqdn, kind, language, file_path,
/// start_byte, end_byte, start_line, end_line)` beginning at column `base`.
fn symbol_from_hash_row_at(
    row: &rusqlite::Row<'_>,
    hash: &str,
    base: usize,
) -> Result<(SymbolIdentity, SourceRange), StorageError> {
    let fqdn: String = row.get(base)?;
    let kind = decode_symbol_kind(&row.get::<_, String>(base + 1)?)?;
    let language = decode_language(&row.get::<_, String>(base + 2)?)?;
    let file_path: String = row.get(base + 3)?;
    let range = SourceRange {
        start_byte: row.get::<_, i64>(base + 4)? as usize,
        end_byte: row.get::<_, i64>(base + 5)? as usize,
        start_line: row.get::<_, i64>(base + 6)? as usize,
        end_line: row.get::<_, i64>(base + 7)? as usize,
    };
    Ok((
        SymbolIdentity::new(language, fqdn, kind, file_path.into(), hash)?,
        range,
    ))
}

/// Encodes an `f32` vector as little-endian bytes.
pub(crate) fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Decodes a little-endian `f32` vector from bytes.
fn decode_vector(bytes: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(bytes.len() / 4);
    decode_vector_into(bytes, &mut out);
    out
}

/// Decodes `bytes` into `out`, reusing `out`'s allocation.
fn decode_vector_into(bytes: &[u8], out: &mut Vec<f32>) {
    out.clear();
    let (chunks, _remainder) = bytes.as_chunks::<4>();
    out.reserve(chunks.len());
    for chunk in chunks {
        out.push(f32::from_le_bytes(*chunk));
    }
}

/// Commits when `result` succeeded, rolls back otherwise, then propagates it.
fn finish(
    conn: std::sync::MutexGuard<'_, Connection>,
    result: Result<(), rusqlite::Error>,
) -> Result<(), StorageError> {
    match result {
        Ok(()) => {
            conn.execute("COMMIT", [])?;
            Ok(())
        }
        Err(error) => {
            // Roll back best-effort; surface the original failure either way.
            let _ignored = conn.execute("ROLLBACK", []);
            Err(error.into())
        }
    }
}

fn apply_pragmas(conn: &Connection) -> Result<(), StorageError> {
    // PRAGMA assignments may return no rows (e.g. `journal_mode=WAL` on an
    // in-memory database, which reports "memory"), so drive them through
    // `execute_batch` rather than `query_row`.
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}

fn drop_all_tables(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS relations;
         DROP TABLE IF EXISTS symbols;
         DROP TABLE IF EXISTS files;
         DROP TABLE IF EXISTS embeddings;
         DROP TABLE IF EXISTS snapshots;
         DROP TABLE IF EXISTS meta;",
    )?;
    Ok(())
}

fn create_tables(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshots (
            content_hash TEXT PRIMARY KEY,
            commit_hash TEXT,
            workspace_root TEXT NOT NULL,
            is_dirty INTEGER NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS symbols (
            content_hash TEXT PRIMARY KEY,
            fqdn TEXT NOT NULL,
            trailing_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            language TEXT NOT NULL,
            file_path TEXT NOT NULL,
            start_byte INTEGER NOT NULL,
            end_byte INTEGER NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            snapshot_hash TEXT NOT NULL,
            FOREIGN KEY(snapshot_hash) REFERENCES snapshots(content_hash)
        );
        CREATE TABLE IF NOT EXISTS files (
            path TEXT PRIMARY KEY,
            content_hash TEXT NOT NULL,
            snapshot_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS embeddings (
            content_hash TEXT PRIMARY KEY,
            dim INTEGER NOT NULL,
            vector BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS relations (
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            snapshot_hash TEXT NOT NULL,
            PRIMARY KEY (source_id, target_id, kind, snapshot_hash),
            FOREIGN KEY(source_id) REFERENCES symbols(content_hash),
            FOREIGN KEY(target_id) REFERENCES symbols(content_hash)
        );
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_symbols_fqdn ON symbols(fqdn);
        CREATE INDEX IF NOT EXISTS idx_symbols_trailing ON symbols(trailing_name);
        CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_id);
        CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_id);",
    )?;
    Ok(())
}

// Column encodings mirror the protocol enums' serde wire form verbatim
// ("Method", "Go", "Calls"); decoding rejects anything else.

fn encode_language(language: Language) -> &'static str {
    match language {
        Language::Rust => "Rust",
        Language::Go => "Go",
        Language::TypeScript => "TypeScript",
        Language::JavaScript => "JavaScript",
        Language::Python => "Python",
        Language::Markdown => "Markdown",
        Language::Yaml => "Yaml",
        Language::Json => "Json",
        Language::Toml => "Toml",
        Language::Generic => "Generic",
    }
}

fn decode_language(value: &str) -> Result<Language, StorageError> {
    match value {
        "Rust" => Ok(Language::Rust),
        "Go" => Ok(Language::Go),
        "TypeScript" => Ok(Language::TypeScript),
        "JavaScript" => Ok(Language::JavaScript),
        "Python" => Ok(Language::Python),
        "Markdown" => Ok(Language::Markdown),
        "Yaml" => Ok(Language::Yaml),
        "Json" => Ok(Language::Json),
        "Toml" => Ok(Language::Toml),
        "Generic" => Ok(Language::Generic),
        other => Err(StorageError::UnknownEnumValue(other.to_string())),
    }
}

fn encode_symbol_kind(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "Function",
        SymbolKind::Method => "Method",
        SymbolKind::Struct => "Struct",
        SymbolKind::Class => "Class",
        SymbolKind::Trait => "Trait",
        SymbolKind::Interface => "Interface",
        SymbolKind::Enum => "Enum",
        SymbolKind::Module => "Module",
        SymbolKind::Variable => "Variable",
        SymbolKind::Constant => "Constant",
    }
}

fn decode_symbol_kind(value: &str) -> Result<SymbolKind, StorageError> {
    match value {
        "Function" => Ok(SymbolKind::Function),
        "Method" => Ok(SymbolKind::Method),
        "Struct" => Ok(SymbolKind::Struct),
        "Class" => Ok(SymbolKind::Class),
        "Trait" => Ok(SymbolKind::Trait),
        "Interface" => Ok(SymbolKind::Interface),
        "Enum" => Ok(SymbolKind::Enum),
        "Module" => Ok(SymbolKind::Module),
        "Variable" => Ok(SymbolKind::Variable),
        "Constant" => Ok(SymbolKind::Constant),
        other => Err(StorageError::UnknownEnumValue(other.to_string())),
    }
}

fn encode_relation_kind(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Calls => "Calls",
        RelationKind::CalledBy => "CalledBy",
        RelationKind::Implements => "Implements",
        RelationKind::ImplementedBy => "ImplementedBy",
        RelationKind::Imports => "Imports",
        RelationKind::Contains => "Contains",
    }
}

fn decode_relation_kind(value: &str) -> Result<RelationKind, StorageError> {
    match value {
        "Calls" => Ok(RelationKind::Calls),
        "CalledBy" => Ok(RelationKind::CalledBy),
        "Implements" => Ok(RelationKind::Implements),
        "ImplementedBy" => Ok(RelationKind::ImplementedBy),
        "Imports" => Ok(RelationKind::Imports),
        "Contains" => Ok(RelationKind::Contains),
        other => Err(StorageError::UnknownEnumValue(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lynx_protocol::{Language, SourceRange, SymbolIdentity, SymbolKind};
    use std::path::PathBuf;

    fn snapshot(hash: &str) -> Snapshot {
        Snapshot {
            commit_hash: Some("deadbeef".to_string()),
            workspace_root: PathBuf::from("/tmp/ws"),
            is_dirty: true,
            content_hash: hash.to_string(),
        }
    }

    fn identity(fqdn: &str, kind: SymbolKind) -> (SymbolIdentity, SourceRange) {
        (
            SymbolIdentity::new(
                Language::Rust,
                fqdn,
                kind,
                PathBuf::from("src/lib.rs"),
                fqdn,
            )
            .unwrap(),
            SourceRange::new(1, 10, 0, 100).unwrap(),
        )
    }

    fn seeded() -> GraphStore {
        let store = GraphStore::open_in_memory().unwrap();
        store.save_snapshot(&snapshot("snap1")).unwrap();
        let (greet, range) = identity("crate::greet", SymbolKind::Function);
        store.insert_symbols(&[(greet, range)], "snap1").unwrap();
        store
    }

    #[test]
    fn roundtrips_snapshot_and_symbol() {
        let store = seeded();
        let (loaded, range) = store.get_symbol("crate::greet").unwrap().unwrap();
        assert_eq!(loaded.fqdn, "crate::greet");
        assert_eq!(range, SourceRange::new(1, 10, 0, 100).unwrap());
    }

    #[test]
    fn resolves_by_fqdn_and_trailing_name() {
        let store = seeded();
        let by_fqdn = store.resolve("crate::greet").unwrap().unwrap();
        assert_eq!(by_fqdn.0.fqdn, "crate::greet");
        let by_name = store.resolve("greet").unwrap().unwrap();
        assert_eq!(by_name.0.fqdn, "crate::greet");
        assert!(store.resolve("missing").unwrap().is_none());
    }

    #[test]
    fn duplicate_trailing_names_resolve_deterministically_to_smallest_fqdn() {
        let store = GraphStore::open_in_memory().unwrap();
        store.save_snapshot(&snapshot("s")).unwrap();
        for fqdn in ["b::Dup", "a::Dup"] {
            let (id, range) = identity(fqdn, SymbolKind::Function);
            store.insert_symbols(&[(id, range)], "s").unwrap();
        }
        let winner = store.resolve("Dup").unwrap().unwrap();
        assert_eq!(winner.0.fqdn, "a::Dup");
    }

    #[test]
    fn embeddings_roundtrip_and_stream() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .put_embeddings(&[
                ("h1".to_string(), vec![1.0, 2.0, 3.0]),
                ("h2".to_string(), vec![4.0, 5.0, 6.0]),
            ])
            .unwrap();
        assert_eq!(store.embedding("h1").unwrap().unwrap(), vec![1.0, 2.0, 3.0]);
        let mut seen = Vec::new();
        store
            .for_each_embedding(|hash, vector| {
                seen.push((hash.to_string(), vector.to_vec()));
                true
            })
            .unwrap();
        assert_eq!(seen.len(), 2);
        // Ordered by content_hash for deterministic scans.
        assert_eq!(seen[0].0, "h1");
    }

    #[test]
    fn for_each_embedding_stops_on_false() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .put_embeddings(&[("h1".to_string(), vec![1.0]), ("h2".to_string(), vec![2.0])])
            .unwrap();
        let mut count = 0;
        store
            .for_each_embedding(|_, _| {
                count += 1;
                false
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn reset_clears_all_state() {
        let store = seeded();
        assert!(store.has_symbols().unwrap());
        store.reset().unwrap();
        assert!(!store.has_symbols().unwrap());
        assert_eq!(store.counts().unwrap(), (0, 0, 0));
    }

    #[test]
    fn symbols_for_batches_across_chunks() {
        let store = GraphStore::open_in_memory().unwrap();
        store.save_snapshot(&snapshot("s")).unwrap();
        let mut entries = Vec::new();
        for i in 0..(PARAM_CHUNK + 5) {
            entries.push(identity(&format!("p::s{i}"), SymbolKind::Function));
        }
        store.insert_symbols(&entries, "s").unwrap();
        let hashes: Vec<String> = entries
            .iter()
            .map(|(id, _)| id.content_hash.clone())
            .collect();
        let loaded = store.symbols_for(&hashes).unwrap();
        assert_eq!(loaded.len(), hashes.len());
    }

    #[test]
    fn relation_insert_and_filter_by_kind() {
        let store = GraphStore::open_in_memory().unwrap();
        store.save_snapshot(&snapshot("snap1")).unwrap();

        let (caller, _) = identity("crate::caller", SymbolKind::Function);
        let (callee, _) = identity("crate::callee", SymbolKind::Function);
        store
            .insert_symbols(
                &[(caller.clone(), SourceRange::new(1, 1, 0, 10).unwrap())],
                "snap1",
            )
            .unwrap();
        store
            .insert_symbols(
                &[(callee.clone(), SourceRange::new(2, 2, 0, 10).unwrap())],
                "snap1",
            )
            .unwrap();

        let relation = Relation {
            source_id: caller.content_hash.clone(),
            target_id: callee.content_hash.clone(),
            kind: RelationKind::Calls,
        };
        store.insert_relations(&[relation], "snap1").unwrap();

        let all = store.get_relations(&caller.content_hash, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].kind, RelationKind::Calls);
        assert_eq!(all[0].target_id, callee.content_hash);

        let calls = store
            .get_relations(&caller.content_hash, Some(RelationKind::Calls))
            .unwrap();
        assert_eq!(calls.len(), 1);

        let contains = store
            .get_relations(&caller.content_hash, Some(RelationKind::Contains))
            .unwrap();
        assert!(contains.is_empty());
    }

    #[test]
    fn missing_symbol_returns_none() {
        let store = GraphStore::open_in_memory().unwrap();
        assert!(store.get_symbol("nope").unwrap().is_none());
    }
}
