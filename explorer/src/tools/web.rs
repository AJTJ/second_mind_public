use reqwest::Client;
use serde_json::json;

use crate::api;

/// Result from a web search including both content and token usage.
pub struct SearchResult {
    pub text: String,
    pub usage: api::Usage,
}

/// Web search via Claude Messages API with the web_search tool enabled.
/// Returns structured facts with sources, plus token usage for cost tracking.
pub async fn search(
    client: &Client,
    api_key: &str,
    model: &str,
    query: &str,
    retry_config: &api::RetryConfig,
) -> anyhow::Result<SearchResult> {
    let body = json!({
        "model": model,
        "max_tokens": 2048,
        "tools": [{"type": "web_search_20250305"}],
        "messages": [
            {
                "role": "user",
                "content": format!(
                    "Search the web for: {query}\n\n\
                     Return your findings as a structured list of facts. For each fact:\n\
                     - State the fact with specific numbers, dates, or names\n\
                     - Include the source URL\n\n\
                     Format:\n\
                     FACT: [specific factual statement]\n\
                     SOURCE: [url]\n\n\
                     List 5-10 key facts. No preamble or commentary."
                )
            }
        ]
    });

    let api_resp = api::call_with_retry(client, api_key, &body, retry_config).await?;

    let usage = api_resp.usage.clone().unwrap_or_default();

    let mut text_parts = Vec::new();
    for block in &api_resp.content {
        if let api::ContentBlock::Text { text } = block {
            text_parts.push(text.clone());
        }
    }

    Ok(SearchResult {
        text: text_parts.join("\n\n"),
        usage,
    })
}
