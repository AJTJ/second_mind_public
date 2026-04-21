use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- Source types ---

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Text,
    Url,
    File,
    Audio,
}

// --- MCP tool parameter types ---

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SourceInput {
    #[schemars(description = "Source type: text, url, file, or audio")]
    pub source_type: SourceType,
    #[schemars(description = "Inline text content (for type=text)")]
    pub content: Option<String>,
    #[schemars(description = "File path or URL (for type=url/file/audio)")]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IngestParams {
    #[schemars(description = "Array of sources to ingest")]
    pub sources: Vec<SourceInput>,
    #[schemars(description = "Target dataset names, e.g. [\"research\"] or [\"personal\"]")]
    pub datasets: Vec<String>,
    #[schemars(
        description = "Prompt name from channels/ directory, e.g. \"research\" or \"personal\""
    )]
    pub prompt_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Search query string")]
    pub query: String,
    #[schemars(description = "Datasets to search")]
    pub datasets: Option<Vec<String>>,
    #[schemars(description = "Search type: GRAPH_COMPLETION, SIMILARITY, CHUNKS, SUMMARIES")]
    pub search_type: Option<String>,
    #[schemars(description = "Number of results to return")]
    pub top_k: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplayParams {
    #[schemars(description = "Specific entry ID to replay, or omit for all")]
    pub entry_id: Option<String>,
    #[schemars(description = "Override prompt name for replay")]
    pub prompt_override: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LogQueryParams {
    #[schemars(description = "Filter by dataset name")]
    pub dataset: Option<String>,
    #[schemars(description = "Filter by prompt name")]
    pub prompt: Option<String>,
    #[schemars(description = "Max entries to return (default 50)")]
    pub limit: Option<usize>,
}

// --- Log entry types (stored in JSONL) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub sources: Vec<StoredSource>,
    pub datasets: Vec<String>,
    pub prompt: PromptRef,
    pub backend: String,
    pub status: EntryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replayed_from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSource {
    pub source_type: SourceType,
    pub original_ref: String,
    pub stored_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRef {
    pub name: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    Pending,
    Added,
    #[serde(alias = "cognified")]
    Integrated,
    Failed,
    /// Data was added but integration failed. Recoverable via replay.
    #[serde(alias = "added_not_cognified")]
    AddedNotIntegrated,
    /// Dataset was deleted from Cognee.
    DatasetDeleted,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, status: EntryStatus) -> LogEntry {
        LogEntry {
            id: id.to_string(),
            timestamp: Utc::now(),
            sources: vec![StoredSource {
                source_type: SourceType::Text,
                original_ref: "(inline)".to_string(),
                stored_path: "texts/test.md".to_string(),
                sha256: "abc123".to_string(),
            }],
            datasets: vec!["research".to_string()],
            prompt: PromptRef {
                name: "research".to_string(),
                content: "Extract claims...".to_string(),
                git_sha: Some("cafe123".to_string()),
            },
            backend: "cognee".to_string(),
            status,
            error: None,
            replayed_from: None,
        }
    }

    #[test]
    fn log_entry_roundtrip() {
        let entry = make_entry("test-001", EntryStatus::Integrated);
        let json = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "test-001");
        assert_eq!(back.status, EntryStatus::Integrated);
        assert_eq!(back.datasets, vec!["research"]);
        assert_eq!(back.sources.len(), 1);
        assert_eq!(back.prompt.name, "research");
        assert!(back.error.is_none());
        assert!(back.replayed_from.is_none());
    }

    #[test]
    fn log_entry_with_optionals() {
        let mut entry = make_entry("test-002", EntryStatus::Failed);
        entry.error = Some("connection refused".to_string());
        entry.replayed_from = Some("test-001".to_string());

        let json = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.error.as_deref(), Some("connection refused"));
        assert_eq!(back.replayed_from.as_deref(), Some("test-001"));
    }
}
