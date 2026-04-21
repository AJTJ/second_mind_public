//! Cognee API adapter — the sole integration point with Cognee's REST API.
//!
//! Validated against Cognee 0.5.6-local (image pinned in docker-compose.yml).
//!
//! ## Endpoints used
//!
//! | Method | Path                          | Purpose                        |
//! |--------|-------------------------------|--------------------------------|
//! | POST   | /api/v1/auth/login            | Get bearer token (form-urlenc) |
//! | POST   | /api/v1/add                   | Upload doc to dataset (mpart)  |
//! | POST   | /api/v1/integrate              | Entity extraction + embedding  |
//! | POST   | /api/v1/search                | Query knowledge graph/vectors  |
//! | GET    | /api/v1/datasets              | List datasets (returns UUIDs)  |
//! | DELETE | /api/v1/datasets/{id}         | Delete dataset by UUID         |
//!
//! ## Conventions
//! - Cognee uses camelCase in JSON (`datasetName`, `customPrompt`, `chunksPerBatch`).
//! - Dataset delete requires UUID, not name — use `find_dataset_id()` first.
//! - Auth is form-urlencoded, not JSON.
//! - Integration can take 5+ minutes on this VPS (600s timeout).

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::sync::RwLock;

use crate::backend::Backend;

pub struct CogneeAdapter {
    client: Client,
    base_url: String,
    username: String,
    password: String,
    token: RwLock<Option<String>>,
}

#[derive(Deserialize)]
struct LoginResponse {
    access_token: String,
}

/// Typed representation of a Cognee dataset from the /api/v1/datasets endpoint.
#[derive(Deserialize)]
struct CogneeDataset {
    id: String,
    name: String,
}

impl CogneeAdapter {
    pub fn new(base_url: String, username: String, password: String) -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            base_url,
            username,
            password,
            token: RwLock::new(None),
        }
    }

    /// Build a client with a longer timeout for integrate (LLM processing).
    fn integrate_client(&self) -> Client {
        Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(600))
            .build()
            .unwrap_or_else(|_| self.client.clone())
    }

    async fn ensure_token(&self) -> anyhow::Result<String> {
        // Check if we have a cached token
        {
            let guard = self.token.read().unwrap();
            if let Some(ref t) = *guard {
                return Ok(t.clone());
            }
        }

        // Login to get a fresh token
        let resp = self
            .client
            .post(format!("{}/api/v1/auth/login", self.base_url))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "username={}&password={}",
                self.username, self.password
            ))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Cognee login failed ({}): {}", status, err);
        }

        let login: LoginResponse = resp.json().await?;
        let token = login.access_token.clone();

        {
            let mut guard = self.token.write().unwrap();
            *guard = Some(login.access_token);
        }

        Ok(token)
    }

    async fn auth_header(&self) -> anyhow::Result<String> {
        let token = self.ensure_token().await?;
        Ok(format!("Bearer {token}"))
    }

    /// Clear cached token so next request re-authenticates
    fn clear_token(&self) {
        let mut guard = self.token.write().unwrap();
        *guard = None;
    }

    /// Look up a dataset's UUID by name via the Cognee datasets API.
    async fn find_dataset_id(&self, dataset_name: &str) -> anyhow::Result<String> {
        let auth = self.auth_header().await?;
        let url = format!("{}/api/v1/datasets", self.base_url);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", &auth)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to list datasets ({}): {}", status, body);
        }

        let datasets: Vec<CogneeDataset> = resp.json().await?;
        for ds in &datasets {
            if ds.name == dataset_name {
                return Ok(ds.id.clone());
            }
        }

        anyhow::bail!("Dataset '{}' not found in Cognee", dataset_name)
    }

    /// Check if an error is transient and worth retrying.
    fn is_transient(status: reqwest::StatusCode) -> bool {
        matches!(status.as_u16(), 401 | 409 | 502 | 503 | 504)
    }
}

#[async_trait]
impl Backend for CogneeAdapter {
    async fn add(&self, data: &str, dataset_name: &str, filename: &str) -> anyhow::Result<()> {
        let mut last_err = None;

        for attempt in 0..2 {
            let auth = self.auth_header().await?;
            let url = format!("{}/api/v1/add", self.base_url);

            // Cognee's add endpoint expects multipart with field name "data".
            // Filename must be unique per document — Cognee deduplicates by filename.
            // Strip any path prefix — Cognee stores files flat and cognify expects them at root.
            let flat_name = std::path::Path::new(filename)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(filename);
            let part = reqwest::multipart::Part::text(data.to_string())
                .file_name(flat_name.to_string())
                .mime_str("text/plain")?;

            let form = reqwest::multipart::Form::new()
                .text("datasetName", dataset_name.to_string())
                .part("data", part);

            let resp = match self
                .client
                .post(&url)
                .header("Authorization", &auth)
                .multipart(form)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) if attempt == 0 && (e.is_connect() || e.is_timeout()) => {
                    tracing::warn!("Cognee add transient error, retrying: {e}");
                    last_err = Some(anyhow::anyhow!("{e}"));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status();
            if status.as_u16() == 401 && attempt == 0 {
                tracing::warn!("Cognee auth expired on add, refreshing token and retrying");
                self.clear_token();
                last_err = Some(anyhow::anyhow!("Cognee auth expired"));
                continue;
            }
            if Self::is_transient(status) && attempt == 0 {
                tracing::warn!("Cognee add returned {status}, retrying");
                last_err = Some(anyhow::anyhow!("Cognee add returned {status}"));
                continue;
            }

            if status.is_success() {
                return Ok(());
            } else {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!("Cognee add failed ({}): {}", status, body));
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Cognee add failed after retries")))
    }

    async fn integrate(&self, datasets: &[String], custom_prompt: &str) -> anyhow::Result<()> {
        let integrate_client = self.integrate_client();
        let mut last_err = None;

        // 3 attempts for integrate — it's the most timeout-prone operation
        // (graph-engine's internal 60s timeout on Ollama embeddings under load)
        for attempt in 0..3 {
            if attempt > 0 {
                // Wait before retry — give Ollama time to drain its queue
                let delay = 30 * attempt as u64;
                tracing::warn!(
                    "Integrate retry: waiting {delay}s before attempt {}",
                    attempt + 1
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }

            let auth = self.auth_header().await?;
            let url = format!("{}/api/v1/integrate", self.base_url);

            // chunksPerBatch limits concurrent embedding requests to avoid
            // overwhelming Ollama on a constrained VPS (16GB, qwen3-embedding:4b).
            let chunks_per_batch: u32 = std::env::var("COGNEE_CHUNKS_PER_BATCH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5);

            let body = json!({
                "datasets": datasets,
                "custom_prompt": custom_prompt,
                "chunksPerBatch": chunks_per_batch,
            });

            let resp = match integrate_client
                .post(&url)
                .header("Authorization", &auth)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) if attempt < 2 && (e.is_connect() || e.is_timeout()) => {
                    tracing::warn!("Integrate transient error, retrying: {e}");
                    last_err = Some(anyhow::anyhow!("{e}"));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status();
            if status.as_u16() == 401 && attempt < 2 {
                tracing::warn!("Auth expired on integrate, refreshing token and retrying");
                self.clear_token();
                last_err = Some(anyhow::anyhow!("Auth expired"));
                continue;
            }
            if Self::is_transient(status) && attempt < 2 {
                let body_text = resp.text().await.unwrap_or_default();
                tracing::warn!(
                    "Integrate returned {status}, retrying. Body: {}",
                    &body_text[..body_text.len().min(200)]
                );
                last_err = Some(anyhow::anyhow!(
                    "Integrate returned {status}: {}",
                    &body_text[..body_text.len().min(200)]
                ));
                continue;
            }

            if status.is_success() {
                return Ok(());
            } else {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!("Integrate failed ({}): {}", status, body));
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Integrate failed after retries")))
    }

    async fn search(
        &self,
        query: &str,
        datasets: Option<&[String]>,
        search_type: Option<&str>,
        top_k: Option<i32>,
    ) -> anyhow::Result<serde_json::Value> {
        let mut last_err = None;

        for attempt in 0..2 {
            let auth = self.auth_header().await?;
            let url = format!("{}/api/v1/search", self.base_url);

            let mut body = json!({
                "query": query,
                "search_type": search_type.unwrap_or("CHUNKS"),
            });

            if let Some(ds) = datasets {
                body["datasets"] = json!(ds);
            }
            if let Some(k) = top_k {
                body["top_k"] = json!(k);
            }

            let resp = match self
                .client
                .post(&url)
                .header("Authorization", &auth)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) if attempt == 0 && (e.is_connect() || e.is_timeout()) => {
                    tracing::warn!("Cognee search transient error, retrying: {e}");
                    last_err = Some(anyhow::anyhow!("{e}"));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status();
            if status.as_u16() == 401 && attempt == 0 {
                tracing::warn!("Cognee auth expired on search, refreshing token and retrying");
                self.clear_token();
                last_err = Some(anyhow::anyhow!("Cognee auth expired"));
                continue;
            }
            if Self::is_transient(status) && attempt == 0 {
                tracing::warn!("Cognee search returned {status}, retrying");
                last_err = Some(anyhow::anyhow!("Cognee search returned {status}"));
                continue;
            }

            let body: serde_json::Value =
                resp.json().await.unwrap_or(json!({"error": "no response"}));

            if status.is_success() {
                return Ok(body);
            } else {
                return Err(anyhow::anyhow!(
                    "Cognee search failed ({}): {}",
                    status,
                    body
                ));
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Cognee search failed after retries")))
    }

    async fn delete_dataset(&self, dataset_name: &str) -> anyhow::Result<()> {
        // Cognee's delete endpoint requires the dataset UUID, not the name.
        // First, list datasets to find the UUID for the given name.
        let dataset_id = self.find_dataset_id(dataset_name).await?;

        let mut last_err = None;

        for attempt in 0..2 {
            let auth = self.auth_header().await?;
            let url = format!("{}/api/v1/datasets/{}", self.base_url, dataset_id);

            let resp = match self
                .client
                .delete(&url)
                .header("Authorization", &auth)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) if attempt == 0 && (e.is_connect() || e.is_timeout()) => {
                    tracing::warn!("Cognee delete transient error, retrying: {e}");
                    last_err = Some(anyhow::anyhow!("{e}"));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status();
            if status.as_u16() == 401 && attempt == 0 {
                tracing::warn!("Cognee auth expired on delete, refreshing token and retrying");
                self.clear_token();
                last_err = Some(anyhow::anyhow!("Cognee auth expired"));
                continue;
            }
            if Self::is_transient(status) && attempt == 0 {
                tracing::warn!("Cognee delete returned {status}, retrying");
                last_err = Some(anyhow::anyhow!("Cognee delete returned {status}"));
                continue;
            }

            if status.is_success() {
                return Ok(());
            } else {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!(
                    "Cognee delete failed ({}): {}",
                    status,
                    body
                ));
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Cognee delete failed after retries")))
    }

    async fn delete_all_datasets(&self) -> anyhow::Result<()> {
        let auth = self.auth_header().await?;
        let url = format!("{}/api/v1/datasets", self.base_url);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", &auth)
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 204 {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(anyhow::anyhow!(
                "Cognee delete-all failed ({}): {}",
                status,
                body
            ))
        }
    }
}
