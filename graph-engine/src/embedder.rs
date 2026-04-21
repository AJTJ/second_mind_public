use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
}

pub struct OllamaEmbedder {
    client: Client,
    endpoint: String,
    model: String,
}

#[derive(Serialize)]
struct EmbedRequest {
    model: String,
    input: String,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl OllamaEmbedder {
    pub fn new(endpoint: String, model: String) -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            endpoint,
            model,
        }
    }
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let req = EmbedRequest {
            model: self.model.clone(),
            input: text.to_string(),
        };

        let resp = self
            .client
            .post(&self.endpoint)
            .json(&req)
            .send()
            .await?
            .error_for_status()?
            .json::<EmbedResponse>()
            .await?;

        resp.embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty embeddings response from Ollama"))
    }
}

/// Mock embedder for testing -- returns a zero vector.
pub struct MockEmbedder {
    pub dimensions: usize,
}

#[async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; self.dimensions])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction() {
        let embedder = OllamaEmbedder::new(
            "http://ollama:11434/api/embed".to_string(),
            "qwen3-embedding:4b".to_string(),
        );
        assert_eq!(embedder.endpoint, "http://ollama:11434/api/embed");
        assert_eq!(embedder.model, "qwen3-embedding:4b");
    }

    #[tokio::test]
    async fn mock_embedder_returns_zeros() {
        let embedder = MockEmbedder { dimensions: 10 };
        let result = embedder.embed("test").await.unwrap();
        assert_eq!(result.len(), 10);
        assert!(result.iter().all(|&v| v == 0.0));
    }
}
