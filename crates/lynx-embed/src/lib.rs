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
