//! Error contract for the parser crate.

use thiserror::Error;

/// Errors raised while parsing sources into protocol primitives.
#[derive(Debug, Error)]
pub enum ParseError {
    /// A tree-sitter query failed to compile against its grammar.
    ///
    /// `tree_sitter::QueryError` is neither `Clone` nor owned-cache friendly
    /// behind a lazily initialized [`std::sync::OnceLock`], so the compile
    /// diagnostics are carried as a message instead of `#[from]`.
    #[error("invalid tree-sitter query: {0}")]
    TreeSitter(String),
    /// The grammar rejected by the parser does not accept the source.
    #[error("incompatible tree-sitter grammar: {0}")]
    Grammar(String),
    /// A grammar-backed parse produced no syntax tree.
    #[error("source does not parse as {0}")]
    InvalidSource(&'static str),
    /// No [`crate::LanguageAdapter`] claims the given path.
    #[error("unsupported language for path `{0}`")]
    UnsupportedLanguage(String),
}
