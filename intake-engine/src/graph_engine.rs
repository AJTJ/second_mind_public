//! Graph Engine HTTP adapter — direct JSON API, no auth.
//!
//! Replaces the CogneeAdapter for talking to the second-mind graph engine.
//!
//! ## Endpoints used
//!
//! | Method | Path                          | Purpose                        |
//! |--------|-------------------------------|--------------------------------|
//! | POST   | /api/v1/add                   | Add document (JSON body)       |
//! | POST   | /api/v1/integrate             | Entity extraction + embedding  |
//! | POST   | /api/v1/search                | Query knowledge graph          |
//! | GET    | /api/v1/datasets              | List channels                  |
//! | DELETE | /api/v1/datasets/{name}       | Delete channel by name         |
//! | DELETE | /api/v1/datasets              | Delete all channels            |

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::backend::Backend;

pub struct GraphEngineAdapter {
    client: Client,
    base_url: String,
}

impl GraphEngineAdapter {
    pub fn new(base_url: String) -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { client, base_url }
    }

    /// Build a client with a longer timeout for integrate (LLM processing).
    fn integrate_client(&self) -> Client {
        Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(600))
            .build()
            .unwrap_or_else(|_| self.client.clone())
    }
}

#[async_trait]
impl Backend for GraphEngineAdapter {
    async fn add(&self, data: &str, dataset_name: &str, filename: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/add", self.base_url);

        let body = json!({
            "dataset_name": dataset_name,
            "content": data,
            "source_ref": filename,
        });

        let resp = self.client.post(&url).json(&body).send().await?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Graph engine add failed ({}): {}", status, body)
        }
    }

    async fn integrate(&self, datasets: &[String], custom_prompt: &str) -> anyhow::Result<()> {
        let integrate_client = self.integrate_client();
        let url = format!("{}/api/v1/integrate", self.base_url);

        let body = json!({
            "datasets": datasets,
            "custom_prompt": custom_prompt,
        });

        let resp = integrate_client.post(&url).json(&body).send().await?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Graph engine integrate failed ({}): {}", status, body)
        }
    }

    async fn search(
        &self,
        query: &str,
        datasets: Option<&[String]>,
        search_type: Option<&str>,
        top_k: Option<i32>,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}/api/v1/search", self.base_url);

        let mut body = json!({
            "query": query,
        });

        if let Some(ds) = datasets {
            body["datasets"] = json!(ds);
        }
        if let Some(st) = search_type {
            body["search_type"] = json!(st);
        }
        if let Some(k) = top_k {
            body["top_k"] = json!(k);
        }

        let resp = self.client.post(&url).json(&body).send().await?;

        let status = resp.status();
        if status.is_success() {
            let result: serde_json::Value = resp.json().await?;
            Ok(result)
        } else {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Graph engine search failed ({}): {}", status, body)
        }
    }

    async fn delete_dataset(&self, dataset_name: &str) -> anyhow::Result<()> {
        // URL-encode dataset name to handle spaces and special characters
        let base = reqwest::Url::parse(&format!("{}/api/v1/datasets/", self.base_url))?;
        let url = base.join(dataset_name)?.to_string();

        let resp = self.client.delete(&url).send().await?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Graph engine delete failed ({}): {}", status, body)
        }
    }

    async fn delete_all_datasets(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/datasets", self.base_url);

        let resp = self.client.delete(&url).send().await?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Graph engine delete-all failed ({}): {}", status, body)
        }
    }
}
