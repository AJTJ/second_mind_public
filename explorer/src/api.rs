use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

/// Build a reqwest client with proper timeouts for external API calls.
pub fn build_client() -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| Client::new())
}

#[derive(Debug, Deserialize)]
pub struct ApiResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Configurable retry settings for API calls.
pub struct RetryConfig {
    /// Backoff delays in seconds for each retry attempt.
    pub delays: Vec<u64>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            delays: vec![30, 60, 120],
        }
    }
}

impl RetryConfig {
    pub fn from_env() -> Self {
        if let Ok(val) = std::env::var("EXPLORER_RETRY_DELAYS") {
            let delays: Vec<u64> = val
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if !delays.is_empty() {
                return Self { delays };
            }
        }
        Self::default()
    }
}

/// Call the Claude API with retry on 429 rate limit errors.
/// Returns both the response and usage for cost tracking.
pub async fn call_with_retry(
    client: &Client,
    api_key: &str,
    body: &Value,
    retry_config: &RetryConfig,
) -> anyhow::Result<ApiResponse> {
    for (attempt, delay_secs) in std::iter::once(&0)
        .chain(retry_config.delays.iter())
        .enumerate()
    {
        if *delay_secs > 0 {
            tracing::warn!(
                "Rate limited, waiting {}s before retry (attempt {}/{})",
                delay_secs,
                attempt + 1,
                retry_config.delays.len() + 1
            );
            tokio::time::sleep(Duration::from_secs(*delay_secs)).await;
        }

        let resp = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await?;

        let status = resp.status();
        if status.as_u16() == 429 && attempt < retry_config.delays.len() {
            let err = resp.text().await.unwrap_or_default();
            tracing::warn!(
                "Claude API rate limited (429): {}",
                &err[..err.len().min(200)]
            );
            continue;
        }

        if !status.is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Claude API error ({status}): {err}");
        }

        return Ok(resp.json().await?);
    }

    anyhow::bail!(
        "Claude API rate limited after {} retries",
        retry_config.delays.len()
    )
}
