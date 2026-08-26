//! Error contract for the vector embedding provider.

use thiserror::Error;

/// Every failure mode of a [`crate::VectorProvider`].
#[derive(Debug, Error)]
pub enum EmbedError {
    /// The underlying FastEmbed model failed to initialize.
    #[error("embedding model init failure: {0}")]
    Init(String),
    /// Embedding inference failed.
    #[error("embedding inference failure: {0}")]
    Inference(String),
    /// An internal mutex guarding the shared model was poisoned.
    #[error("embedding model lock poisoned: {0}")]
    Poisoned(String),
}
