use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchEntry {
    pub timestamp: DateTime<Utc>,
    pub query: String,
    pub datasets: Option<Vec<String>>,
    pub search_type: Option<String>,
    pub top_k: Option<i32>,
    /// Truncated result for logging — full result goes to caller
    pub result_preview: String,
    pub result_chars: usize,
}

pub struct SearchLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl SearchLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn append(&self, entry: &SearchEntry) -> anyhow::Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}")?;
        file.flush()?;
        Ok(())
    }

    pub fn read_all(&self) -> anyhow::Result<Vec<SearchEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.path)?;
        let mut entries = Vec::new();
        for line in content.lines() {
            if let Ok(entry) = serde_json::from_str::<SearchEntry>(line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_entry(query: &str) -> SearchEntry {
        SearchEntry {
            timestamp: Utc::now(),
            query: query.to_string(),
            datasets: Some(vec!["research".to_string()]),
            search_type: Some("CHUNKS".to_string()),
            top_k: Some(5),
            result_preview: "some results...".to_string(),
            result_chars: 150,
        }
    }

    #[test]
    fn read_all_returns_empty_when_no_file() {
        let tmp = TempDir::new().unwrap();
        let log = SearchLog::new(tmp.path().join("searches.jsonl"));
        let entries = log.read_all().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn append_and_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let log = SearchLog::new(tmp.path().join("searches.jsonl"));

        log.append(&make_entry("what is cognee")).unwrap();
        log.append(&make_entry("embedding models")).unwrap();

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].query, "what is cognee");
        assert_eq!(entries[1].query, "embedding models");
    }

    #[test]
    fn append_preserves_all_fields() {
        let tmp = TempDir::new().unwrap();
        let log = SearchLog::new(tmp.path().join("searches.jsonl"));

        let entry = SearchEntry {
            timestamp: Utc::now(),
            query: "test query".to_string(),
            datasets: Some(vec!["personal".to_string(), "research".to_string()]),
            search_type: Some("GRAPH_COMPLETION".to_string()),
            top_k: Some(10),
            result_preview: "preview text".to_string(),
            result_chars: 42,
        };
        log.append(&entry).unwrap();

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.query, "test query");
        assert_eq!(
            e.datasets.as_ref().unwrap(),
            &vec!["personal".to_string(), "research".to_string()]
        );
        assert_eq!(e.search_type.as_deref(), Some("GRAPH_COMPLETION"));
        assert_eq!(e.top_k, Some(10));
        assert_eq!(e.result_preview, "preview text");
        assert_eq!(e.result_chars, 42);
    }

    #[test]
    fn append_with_none_optionals() {
        let tmp = TempDir::new().unwrap();
        let log = SearchLog::new(tmp.path().join("searches.jsonl"));

        let entry = SearchEntry {
            timestamp: Utc::now(),
            query: "broad search".to_string(),
            datasets: None,
            search_type: None,
            top_k: None,
            result_preview: "".to_string(),
            result_chars: 0,
        };
        log.append(&entry).unwrap();

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].datasets.is_none());
        assert!(entries[0].search_type.is_none());
        assert!(entries[0].top_k.is_none());
    }

    #[test]
    fn read_all_skips_corrupted_lines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("searches.jsonl");
        let log = SearchLog::new(path.clone());

        log.append(&make_entry("good entry")).unwrap();

        // Manually append a corrupted line
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{{not valid json for SearchEntry}}").unwrap();

        log.append(&make_entry("another good entry")).unwrap();

        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].query, "good entry");
        assert_eq!(entries[1].query, "another good entry");
    }
}
