//! Deferred-initialization wrapper around [`FastEmbedProvider`].
//!
//! Loading the ONNX model costs hundreds of MB of resident memory (≈600 MB
//! measured on the macOS arm64 reference environment) and a noticeable fraction
//! of a second. Most operations (`resolve`, `relations`, `status`, lexical-only
//! search) never touch a vector, so the process must not pay that cost merely
//! because it *could* serve a semantic request.
//!
//! [`LazyFastEmbedProvider`] satisfies the [`crate::VectorProvider`] contract
//! immediately and constructs the underlying model on the first embedding call.
//! The first semantic request therefore pays the initialization cost, and every
//! later request reuses the loaded model. This is a deliberate cold-start
//! trade-off: model-free commands stay model-free, while a semantic command has
//! to load the model exactly once per process, as before.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::EmbedError;
use crate::fastembed::FastEmbedProvider;
use crate::VectorProvider;

/// Where the FastEmbed model weights are cached.
#[derive(Debug, Clone)]
enum ModelCache {
    /// FastEmbed's default cache location.
    Default,
    /// An explicit cache directory pinned by the transport.
    Directory(PathBuf),
}

/// A [`VectorProvider`] that constructs its ONNX model on first use.
///
/// Construction is infallible and performs no I/O, so callers may place this in
/// a session unconditionally. Initialization failures surface from the first
/// embedding call (and are retried on the next call rather than poisoning the
/// provider forever).
pub struct LazyFastEmbedProvider {
    cache: ModelCache,
    model: Mutex<Option<FastEmbedProvider>>,
}

impl LazyFastEmbedProvider {
    /// Creates a provider that will use FastEmbed's default model cache.
    pub fn new() -> Self {
        Self {
            cache: ModelCache::Default,
            model: Mutex::new(None),
        }
    }

    /// Creates a provider that will cache weights under `cache_dir`.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        Self {
            cache: ModelCache::Directory(cache_dir),
            model: Mutex::new(None),
        }
    }

    /// Whether the underlying model has been constructed.
    ///
    /// Exposed so transports and tests can assert that a model-free path did
    /// not initialize the model.
    pub fn is_initialized(&self) -> bool {
        self.model
            .lock()
            .map(|model| model.is_some())
            .unwrap_or(false)
    }

    /// Returns the loaded model, constructing it once when absent.
    fn loaded(&self) -> Result<std::sync::MutexGuard<'_, Option<FastEmbedProvider>>, EmbedError> {
        let mut guard = self
            .model
            .lock()
            .map_err(|poisoned| EmbedError::Poisoned(poisoned.to_string()))?;
        if guard.is_none() {
            let provider = match &self.cache {
                ModelCache::Default => FastEmbedProvider::new()?,
                ModelCache::Directory(dir) => FastEmbedProvider::with_cache_dir(dir.clone())?,
            };
            *guard = Some(provider);
        }
        Ok(guard)
    }
}

impl Default for LazyFastEmbedProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl VectorProvider for LazyFastEmbedProvider {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let guard = self.loaded()?;
        let provider = guard
            .as_ref()
            .ok_or_else(|| EmbedError::Init("model was not constructed".to_string()))?;
        provider.embed_query(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let guard = self.loaded()?;
        let provider = guard
            .as_ref()
            .ok_or_else(|| EmbedError::Init("model was not constructed".to_string()))?;
        provider.embed_batch(texts)
    }

    fn dimension(&self) -> usize {
        // Known without constructing the model; `bge-small-en-v1.5` is fixed.
        crate::fastembed::BGE_SMALL_EN_V1_5_DIM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_does_not_initialize_the_model() {
        let provider = LazyFastEmbedProvider::with_cache_dir(PathBuf::from("/nonexistent"));
        assert!(!provider.is_initialized());
        assert_eq!(provider.dimension(), 384);
        // Dimension queries must not trigger initialization either.
        assert!(!provider.is_initialized());
    }
}
