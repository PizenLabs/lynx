//! FastEmbed-backed [`crate::VectorProvider`] implementation.

use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptionsWithLength, TextEmbedding};

use crate::error::EmbedError;
use crate::VectorProvider;

/// Number of dimensions produced by the `bge-small-en-v1.5` model.
const BGE_SMALL_EN_V1_5_DIM: usize = 384;

/// A [`crate::VectorProvider`] backed by FastEmbed's `bge-small-en-v1.5`.
///
/// The ONNX model is created on first construction (downloading the weights on
/// demand) and guarded behind a `Mutex` so the provider can be shared across
/// threads. Embedding is performed synchronously.
pub struct FastEmbedProvider {
    model: Mutex<TextEmbedding>,
}

impl FastEmbedProvider {
    /// Builds a provider using the `bge-small-en-v1.5` embedding model.
    pub fn new() -> Result<Self, EmbedError> {
        Self::with_model(EmbeddingModel::BGESmallENV15)
    }

    /// Builds a provider around an arbitrary FastEmbed [`EmbeddingModel`].
    pub fn with_model(model_name: EmbeddingModel) -> Result<Self, EmbedError> {
        let mut options = InitOptionsWithLength::new(model_name);
        options = options.with_show_download_progress(true);
        let model = TextEmbedding::try_new(options)
            .map_err(|error| EmbedError::Init(error.to_string()))?;
        Ok(Self {
            model: Mutex::new(model),
        })
    }
}

impl Default for FastEmbedProvider {
    fn default() -> Self {
        Self::new().expect("bge-small-en-v1.5 model must initialize")
    }
}

impl VectorProvider for FastEmbedProvider {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let mut model = self
            .model
            .lock()
            .map_err(|poisoned| EmbedError::Poisoned(poisoned.to_string()))?;
        let mut embeddings = model
            .embed(vec![text], None)
            .map_err(|error| EmbedError::Inference(error.to_string()))?;
        embeddings
            .pop()
            .ok_or_else(|| EmbedError::Inference("model returned no embedding".to_string()))
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut model = self
            .model
            .lock()
            .map_err(|poisoned| EmbedError::Poisoned(poisoned.to_string()))?;
        model
            .embed(texts, None)
            .map_err(|error| EmbedError::Inference(error.to_string()))
    }

    fn dimension(&self) -> usize {
        BGE_SMALL_EN_V1_5_DIM
    }
}
