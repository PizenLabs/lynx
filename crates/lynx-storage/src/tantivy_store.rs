//! Tantivy-backed lexical search over code snippets keyed by symbol hash.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TantivyDocument, Value, STORED, STRING, TEXT,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, Term};

use crate::error::StorageError;

/// Heap budget for the single shared indexing writer. One writer is reused for
/// an entire indexing pass; a fresh writer (with its own 15 MB arena and merge
/// threads) is *not* created per batch.
const WRITER_HEAP_BYTES: usize = 15_000_000;

/// A code snippet indexed for lexical retrieval.
///
/// `language` uses the protocol enum's wire form ("Rust", "Go", ...).
#[derive(Debug, Clone)]
pub struct LexicalDoc {
    /// Hash of the symbol this snippet belongs to; joins to the graph store.
    pub symbol_hash: String,
    /// Source text of the snippet.
    pub code_snippet: String,
    /// Language wire form of the snippet.
    pub language: String,
    /// Workspace-relative path of the declaring file.
    pub file_path: String,
}

/// A lexical hit: the matched document's symbol hash and its BM25 score.
#[derive(Debug, Clone)]
pub struct LexicalHit {
    /// Hash of the matched symbol.
    pub symbol_hash: String,
    /// Relevance score; strictly positive for a genuine match.
    pub score: f32,
}

/// Inverted index over [`LexicalDoc`]s stored in a caller-provided directory.
///
/// A single [`IndexWriter`] is created lazily on the first [`Self::add_documents`]
/// and reused until [`Self::commit`]. This is the fix for the per-file writer
/// churn that dominated indexing cost: creating and committing a writer for
/// every file spawned merge threads and rewrote the segment metadata on each
/// call, making indexing `O(files)` heavy and mmap'ing many transient segments.
///
/// Snippets are `STORED`, so a hydrated session can fetch the exact source
/// excerpt for a bounded set of symbol hashes without retaining any of them in
/// memory ([`Self::snippets_for`]).
pub struct TantivyStore {
    dir: PathBuf,
    index: Index,
    reader: IndexReader,
    writer: Mutex<Option<IndexWriter>>,
    symbol_hash: Field,
    code_snippet: Field,
    language: Field,
    file_path: Field,
}

impl TantivyStore {
    /// Opens or creates the index under `dir` (created when absent).
    pub fn open(dir: &Path) -> Result<Self, StorageError> {
        std::fs::create_dir_all(dir)?;

        let mut builder = Schema::builder();
        let symbol_hash = builder.add_text_field("symbol_hash", STRING | STORED);
        let code_snippet = builder.add_text_field("code_snippet", TEXT | STORED);
        let language = builder.add_text_field("language", STRING);
        let file_path = builder.add_text_field("file_path", STRING | STORED);
        let schema = builder.build();

        let index = Index::open_or_create(
            tantivy::directory::MmapDirectory::open(dir)?,
            schema.clone(),
        )?;
        // Manual reload: the reader is refreshed explicitly after the single
        // per-pass commit, so queries never pay for a reader rebuild.
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        Ok(Self {
            dir: dir.to_path_buf(),
            index,
            reader,
            writer: Mutex::new(None),
            symbol_hash,
            code_snippet,
            language,
            file_path,
        })
    }

    /// Appends documents to the shared indexing writer.
    ///
    /// Documents are not searchable until [`Self::commit`] is called. The writer
    /// is created on first use and reused across calls, so callers may append
    /// incrementally (e.g. one batch per file) without paying writer
    /// construction and commit cost per call.
    pub fn add_documents(&self, docs: &[LexicalDoc]) -> Result<(), StorageError> {
        let mut guard = self.lock_writer()?;
        let writer = match guard.as_mut() {
            Some(writer) => writer,
            None => guard.insert(self.index.writer(WRITER_HEAP_BYTES)?),
        };
        for doc in docs {
            writer.add_document(doc!(
                self.symbol_hash => doc.symbol_hash.as_str(),
                self.code_snippet => doc.code_snippet.as_str(),
                self.language => doc.language.as_str(),
                self.file_path => doc.file_path.as_str(),
            ))?;
        }
        Ok(())
    }

    /// Marks every document belonging to `paths` for deletion in the shared
    /// writer.
    ///
    /// Deletions are queued as term deletes and applied by the next
    /// [`Self::commit`] in opstamp order. Call this **before**
    /// [`Self::add_documents`] for a replacement version of the same file, so
    /// the newly added documents survive the delete.
    pub fn delete_by_file_paths(&self, paths: &[String]) -> Result<(), StorageError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut guard = self.lock_writer()?;
        let writer = match guard.as_mut() {
            Some(writer) => writer,
            None => guard.insert(self.index.writer(WRITER_HEAP_BYTES)?),
        };
        for path in paths {
            writer.delete_term(Term::from_field_text(self.file_path, path));
        }
        Ok(())
    }

    /// Commits all pending documents, releases the writer, and reloads the
    /// reader so subsequent queries observe the committed state.
    ///
    /// Idempotent: a commit with no pending writer still reloads (cheap) and is
    /// otherwise a no-op. Dropping the writer after commit guarantees the
    /// on-disk index is quiescent, so a caller may move/copy the index
    /// directory.
    pub fn commit(&self) -> Result<(), StorageError> {
        let mut guard = self.lock_writer()?;
        if let Some(mut writer) = guard.take() {
            writer.commit()?;
            self.reader.reload()?;
        }
        Ok(())
    }

    /// Removes every indexed document, leaving an empty index.
    pub fn clear(&self) -> Result<(), StorageError> {
        // Drop any pending writer first so its arena cannot re-add documents.
        {
            let mut guard = self.lock_writer()?;
            *guard = None;
        }
        let mut writer = self.index.writer::<TantivyDocument>(WRITER_HEAP_BYTES)?;
        writer.delete_all_documents()?;
        writer.commit()?;
        drop(writer);
        self.reader.reload()?;
        Ok(())
    }

    /// Directory backing this index.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock_writer(&self) -> Result<std::sync::MutexGuard<'_, Option<IndexWriter>>, StorageError> {
        self.writer
            .lock()
            .map_err(|poisoned| StorageError::Poisoned(poisoned.to_string()))
    }

    /// Full-text search over snippet content, language, and file path;
    /// returns at most `limit` hits ordered by descending relevance.
    pub fn search_snippets(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<LexicalHit>, StorageError> {
        self.commit()?;
        // Malformed or empty queries are contained, not fatal: the lexical
        // channel simply yields no candidates and retrieval degrades to the
        // other channels. A caller's query syntax must never fail a query.
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let parser = QueryParser::for_index(
            &self.index,
            vec![self.code_snippet, self.language, self.file_path],
        );
        let query = match parser.parse_query(query) {
            Ok(query) => query,
            Err(_) => return Ok(Vec::new()),
        };
        let searcher = self.reader.searcher();
        let top_docs =
            searcher.search(&query, &TopDocs::with_limit(limit.max(1)).order_by_score())?;

        let hits = top_docs
            .into_iter()
            .map(|(score, address)| {
                let doc: TantivyDocument = searcher.doc(address)?;
                let value = doc
                    .get_first(self.symbol_hash)
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        StorageError::UnknownEnumValue(format!(
                            "missing symbol_hash at {address:?}"
                        ))
                    })?;
                Ok(LexicalHit {
                    symbol_hash: value.to_string(),
                    score,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(hits)
    }

    /// Fetches the stored snippet text for one symbol hash, if present.
    pub fn snippet_for(&self, hash: &str) -> Result<Option<String>, StorageError> {
        Ok(self
            .snippets_for(std::slice::from_ref(&hash.to_string()))?
            .remove(hash))
    }

    /// Fetches stored snippets for a bounded set of symbol hashes.
    ///
    /// Runs one `BooleanQuery` over the hashes and returns only the retained
    /// snippets, keyed by hash. Cost is `O(hashes)` — callers pass only the
    /// results they will return, never the whole index.
    pub fn snippets_for(&self, hashes: &[String]) -> Result<HashMap<String, String>, StorageError> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }
        self.commit()?;
        let clauses: Vec<(Occur, Box<dyn Query>)> = hashes
            .iter()
            .map(|hash| {
                let term = Term::from_field_text(self.symbol_hash, hash);
                let query = TermQuery::new(term, IndexRecordOption::Basic);
                (Occur::Should, Box::new(query) as Box<dyn Query>)
            })
            .collect();
        let query = BooleanQuery::new(clauses);
        let searcher = self.reader.searcher();
        let docs = searcher.search(&query, &TopDocs::with_limit(hashes.len()).order_by_score())?;

        let mut out = HashMap::with_capacity(hashes.len());
        for (_score, address) in docs {
            let doc: TantivyDocument = searcher.doc(address)?;
            let Some(hash) = doc
                .get_first(self.symbol_hash)
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let snippet = doc
                .get_first(self.code_snippet)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            out.insert(hash.to_string(), snippet);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(symbol_hash: &str, snippet: &str) -> LexicalDoc {
        LexicalDoc {
            symbol_hash: symbol_hash.to_string(),
            code_snippet: snippet.to_string(),
            language: "Rust".to_string(),
            file_path: "src/lib.rs".to_string(),
        }
    }

    #[test]
    fn indexes_and_searches_snippets() {
        let dir = tempfile::tempdir().unwrap();
        let store = TantivyStore::open(dir.path()).unwrap();
        store
            .add_documents(&[
                doc("h1", "fn build_client() {}"),
                doc("h2", "fn parse_request() {}"),
            ])
            .unwrap();

        let hits = store.search_snippets("build_client", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].symbol_hash, "h1");
    }

    #[test]
    fn no_match_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = TantivyStore::open(dir.path()).unwrap();
        store
            .add_documents(&[doc("h1", "fn build_client() {}")])
            .unwrap();
        let hits = store.search_snippets("totally_unrelated", 10).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn snippets_are_retrievable_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = TantivyStore::open(dir.path()).unwrap();
        store
            .add_documents(&[doc("h1", "fn alpha() {}"), doc("h2", "fn beta() {}")])
            .unwrap();
        assert_eq!(
            store.snippet_for("h1").unwrap().as_deref(),
            Some("fn alpha() {}")
        );
        let many = store
            .snippets_for(&["h1".to_string(), "h2".to_string(), "absent".to_string()])
            .unwrap();
        assert_eq!(many.len(), 2);
        assert_eq!(many["h2"], "fn beta() {}");
    }

    #[test]
    fn clear_removes_all_documents() {
        let dir = tempfile::tempdir().unwrap();
        let store = TantivyStore::open(dir.path()).unwrap();
        store.add_documents(&[doc("h1", "fn alpha() {}")]).unwrap();
        assert_eq!(store.search_snippets("alpha", 5).unwrap().len(), 1);
        store.clear().unwrap();
        assert!(store.search_snippets("alpha", 5).unwrap().is_empty());
        assert!(store.snippet_for("h1").unwrap().is_none());
    }
}
