//! SQLite (WAL) substrate for relational identity, structural graph, and
//! workspace provenance.

use std::path::Path;
use std::sync::Mutex;

use lynx_protocol::{Language, Relation, RelationKind, Snapshot, SourceRange, SymbolIdentity, SymbolKind};
use rusqlite::{params, Connection};

use crate::error::StorageError;

/// Relational store over `rusqlite` with WAL journaling.
///
/// The connection sits behind a `Mutex`, so a `GraphStore` is `Send + Sync`
/// and safe to share across indexing threads.
pub struct GraphStore {
    conn: Mutex<Connection>,
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
        init_tables(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Persists a workspace snapshot.
    ///
    /// Inserting an already-known `content_hash` fails on the primary key.
    pub fn save_snapshot(&self, snapshot: &Snapshot) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO snapshots (content_hash, commit_hash, workspace_root, is_dirty)
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

    /// Inserts symbols observed in the snapshot identified by `snapshot_hash`.
    ///
    /// All rows land in one transaction. Each entry pairs the identity with
    /// its source range because the schema requires NOT NULL range columns
    /// that [`SymbolIdentity`] alone does not carry. A duplicate
    /// `content_hash` fails the transaction (strict INSERT).
    pub fn insert_symbols(
        &self,
        entries: &[(SymbolIdentity, SourceRange)],
        snapshot_hash: &str,
    ) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute("BEGIN", [])?;
        let result = entries.iter().try_for_each(|(identity, range)| {
            conn.execute(
                "INSERT INTO symbols (
                    content_hash, fqdn, kind, language, file_path,
                    start_byte, end_byte, start_line, end_line, snapshot_hash
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    identity.content_hash,
                    identity.fqdn,
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

    /// Inserts relations observed in the snapshot identified by
    /// `snapshot_hash`.
    ///
    /// All rows land in one transaction. Foreign keys are enforced: both
    /// endpoint symbol hashes must already exist in `symbols`, otherwise the
    /// insert fails.
    pub fn insert_relations(
        &self,
        relations: &[Relation],
        snapshot_hash: &str,
    ) -> Result<(), StorageError> {
        let conn = self.lock()?;
        conn.execute("BEGIN", [])?;
        let result = relations.iter().try_for_each(|relation| {
            conn.execute(
                "INSERT INTO relations (source_id, target_id, kind, snapshot_hash)
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

    /// Loads a symbol identity by content hash.
    pub fn get_symbol_by_hash(&self, hash: &str) -> Result<Option<SymbolIdentity>, StorageError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare_cached(
            "SELECT fqdn, kind, language, file_path FROM symbols WHERE content_hash = ?1",
        )?;
        let mut rows = statement.query(params![hash])?;
        match rows.next()? {
            Some(row) => {
                let fqdn: String = row.get(0)?;
                let kind = decode_symbol_kind(&row.get::<_, String>(1)?)?;
                let language = decode_language(&row.get::<_, String>(2)?)?;
                let file_path: String = row.get(3)?;
                Ok(Some(SymbolIdentity::new(
                    language,
                    fqdn,
                    kind,
                    file_path.into(),
                    hash,
                )?))
            }
            None => Ok(None),
        }
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
        let mut rows = statement.query(params![
            symbol_hash,
            kind.map(encode_relation_kind),
        ])?;
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

fn init_tables(conn: &Connection) -> Result<(), StorageError> {
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
        CREATE TABLE IF NOT EXISTS relations (
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            snapshot_hash TEXT NOT NULL,
            PRIMARY KEY (source_id, target_id, kind, snapshot_hash),
            FOREIGN KEY(source_id) REFERENCES symbols(content_hash),
            FOREIGN KEY(target_id) REFERENCES symbols(content_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_symbols_fqdn ON symbols(fqdn);
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

    #[test]
    fn roundtrips_snapshot_and_symbol() {
        let store = GraphStore::open_in_memory().unwrap();
        store.save_snapshot(&snapshot("snap1")).unwrap();

        let (id, range) = identity("crate::greet", SymbolKind::Function);
        store.insert_symbols(&[(id.clone(), range)], "snap1").unwrap();

        let loaded = store.get_symbol_by_hash("crate::greet").unwrap().unwrap();
        assert_eq!(loaded, id);
        assert_eq!(loaded.fqdn, "crate::greet");
    }

    #[test]
    fn relation_insert_and_filter_by_kind() {
        let store = GraphStore::open_in_memory().unwrap();
        store.save_snapshot(&snapshot("snap1")).unwrap();

        let (caller, _) = identity("crate::caller", SymbolKind::Function);
        let (callee, _) = identity("crate::callee", SymbolKind::Function);
        store
            .insert_symbols(&[(caller.clone(), SourceRange::new(1, 1, 0, 10).unwrap())], "snap1")
            .unwrap();
        store
            .insert_symbols(&[(callee.clone(), SourceRange::new(2, 2, 0, 10).unwrap())], "snap1")
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
        assert!(store.get_symbol_by_hash("nope").unwrap().is_none());
    }
}
