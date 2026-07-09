use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::sync::{OnceLock, Mutex};

use crate::error::SearchError;

/// Singleton instance of the embedding model to avoid reloading it.
static MODEL: OnceLock<Mutex<TextEmbedding>> = OnceLock::new();

pub struct SemanticSearcher;

impl SemanticSearcher {
    /// Initialize or retrieve the global embedding model.
    /// This may block to download the model (~133MB) on the first run.
    fn get_model() -> Result<&'static Mutex<TextEmbedding>, SearchError> {
        if let Some(model) = MODEL.get() {
            return Ok(model);
        }
        
        let mut options = InitOptions::new(EmbeddingModel::BGESmallENV15);
        options.show_download_progress = true;
        
        let model = TextEmbedding::try_new(options)
            .map_err(|e| SearchError::Internal(e.to_string()))?;
        
        // Try to set it, if another thread already set it, just use the existing one.
        MODEL.get_or_init(|| Mutex::new(model));
        Ok(MODEL.get().unwrap())
    }

    /// Generate embeddings for a list of strings.
    pub fn generate_embeddings(texts: Vec<String>) -> Result<Vec<Vec<f32>>, SearchError> {
        let model_mutex = Self::get_model()?;
        let mut model = model_mutex.lock().map_err(|e| SearchError::Internal(e.to_string()))?;
        let embeddings = model.embed(texts, None)
            .map_err(|e| SearchError::Internal(e.to_string()))?;
        Ok(embeddings)
    }
}
