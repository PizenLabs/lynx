//! Tantivy-backed lexical search over code chunks keyed by symbol hash.

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, TantivyDocument, Value, STORED, STRING, TEXT};
use tantivy::{doc, Index, IndexWriter};

use crate::error::StorageError;

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
pub struct TantivyStore {
    index: Index,
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

        Ok(Self {
            index,
            symbol_hash,
            code_snippet,
            language,
            file_path,
        })
    }

    /// Indexes documents in one committed batch.
    pub fn add_documents(&self, docs: &[LexicalDoc]) -> Result<(), StorageError> {
        let mut writer: IndexWriter = self.index.writer(15_000_000)?;
        for doc in docs {
            writer.add_document(doc!(
                self.symbol_hash => doc.symbol_hash.as_str(),
                self.code_snippet => doc.code_snippet.as_str(),
                self.language => doc.language.as_str(),
                self.file_path => doc.file_path.as_str(),
            ))?;
        }
        writer.commit()?;
        Ok(())
    }

    /// Full-text search over snippet content, language, and file path;
    /// returns at most `limit` hits ordered by descending relevance.
    pub fn search_snippets(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<LexicalHit>, StorageError> {
        let parser = QueryParser::for_index(
            &self.index,
            vec![self.code_snippet, self.language, self.file_path],
        );
        let query = parser.parse_query(query)?;
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;

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
}
