//! Pure vector embedding provider for Lynx.
//!
//! This crate owns no DTOs and performs no retrieval; it only turns text into
//! dense vectors. [`VectorProvider`] is the injection seam through which the
//! core engine obtains embeddings, and [`FastEmbedProvider`] is the concrete
//! implementation backed by FastEmbed's `bge-small-en-v1.5` model (384
//! dimensions).
//!
//! All methods are synchronous and thread-safe: the underlying ONNX model is
//! guarded by interior mutability so a single provider can be shared across
//! indexing and query threads.
//!
//! [`UnavailableProvider`] is the opposite seam: it satisfies the
//! [`VectorProvider`] contract while performing no inference, so a
//! structural/lexical-only session (exact resolution, relations, inspection)
//! can run without loading the ~250 MB ONNX model.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;
pub mod fastembed;

pub use error::EmbedError;
pub use fastembed::FastEmbedProvider;

/// A provider of dense text embeddings.
///
/// Implementations must be `Send + Sync` so they can be shared across the
/// engine's indexing and retrieval paths.
pub trait VectorProvider: Send + Sync {
    /// Embeds a single query string into a fixed-length dense vector.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError>;

    /// Embeds a batch of strings, preserving order.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;

    /// Length of every vector this provider returns.
    fn dimension(&self) -> usize;
}

/// A [`VectorProvider`] that performs no embedding.
///
/// Used for engines that only ever serve structural and lexical primitives
/// (`resolve`, `inspect`, `relations`, lexical `search`). Any semantic call
/// fails with [`EmbedError::Unavailable`] instead of silently returning
/// meaningless vectors, so a misconfigured request is a clear error.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableProvider;

impl VectorProvider for UnavailableProvider {
    fn embed_query(&self, _text: &str) -> Result<Vec<f32>, EmbedError> {
        Err(EmbedError::Unavailable(
            "this engine was opened without an embedding provider".to_string(),
        ))
    }

    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Err(EmbedError::Unavailable(
            "this engine was opened without an embedding provider".to_string(),
        ))
    }

    fn dimension(&self) -> usize {
        0
    }
}
