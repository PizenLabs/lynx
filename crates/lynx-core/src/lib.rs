//! Core engine for the Lynx Repository Evidence Substrate.
//!
//! [`Engine`] ties together the frozen [`DualStorage`] substrate, the
//! [`Parser`] adapter pipeline, and a [`VectorProvider`] to expose eight
//! primitive retrieval operations:
//!
//! - [`Engine::search`] — BM25 + vector similarity fused by Reciprocal Rank
//!   Fusion with definition boosting.
//! - [`Engine::resolve`] — exact symbol lookup by fqdn or name.
//! - [`Engine::inspect`] — full evidence for a symbol by content hash.
//! - [`Engine::relations`] — structural graph query.
//! - [`Engine::trace`] — breadth-first relation traversal.
//! - [`Engine::similar`] — vector-only similarity.
//! - [`Engine::context`] — compile a budget-bounded [`ContextPackage`].
//! - [`Engine::index_status`] — current index state.
//!
//! The [`ContextCompiler`] (Evidence Compiler) assembles [`ContextPackage`]
//! payloads with strict token estimation (4 chars = 1 token) and bounded,
//! line-preserving evidence slicing.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod compiler;
pub mod engine;
pub mod error;
pub mod retrieval;

pub use compiler::ContextCompiler;
pub use engine::{Engine, IndexStatus, StoredSymbol};
pub use error::CoreError;
