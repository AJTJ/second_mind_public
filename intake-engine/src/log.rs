use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::types::{EntryStatus, LogEntry, LogQueryParams};

pub struct IntakeLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl IntakeLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn append(&self, entry: &LogEntry) -> anyhow::Result<()> {
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

    /// Read all entries, deduplicating by ID (last-write-wins).
    pub fn read_all(&self) -> anyhow::Result<Vec<LogEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::File::open(&self.path)?;
        let reader = std::io::BufReader::new(file);
        let mut entries: HashMap<String, LogEntry> = HashMap::new();
        let mut order: Vec<String> = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: LogEntry = serde_json::from_str(&line)?;
            if !entries.contains_key(&entry.id) {
                order.push(entry.id.clone());
            }
            entries.insert(entry.id.clone(), entry);
        }

        Ok(order
            .into_iter()
            .filter_map(|id| entries.remove(&id))
            .collect())
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<LogEntry>> {
        let all = self.read_all()?;
        Ok(all.into_iter().find(|e| e.id == id))
    }

    /// Mark all entries belonging to `dataset` as DatasetDeleted.
    /// Appends new versions of each affected entry (last-write-wins).
    pub fn mark_dataset_deleted(&self, dataset: &str) -> anyhow::Result<usize> {
        let entries = self.read_all()?;
        let mut count = 0;
        for mut entry in entries {
            if entry.datasets.iter().any(|d| d == dataset)
                && entry.status != EntryStatus::DatasetDeleted
            {
                entry.status = EntryStatus::DatasetDeleted;
                entry.timestamp = chrono::Utc::now();
                self.append(&entry)?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn compact(&self) -> anyhow::Result<usize> {
        let _guard = self.lock.lock().unwrap();
        // read raw line count
        let raw_count = if self.path.exists() {
            let content = std::fs::read_to_string(&self.path)?;
            content.lines().filter(|l| !l.trim().is_empty()).count()
        } else {
            return Ok(0);
        };

        // Read deduplicated (can't use self.read_all() because we already hold the lock)
        let file = std::fs::File::open(&self.path)?;
        let reader = std::io::BufReader::new(file);
        let mut entries: HashMap<String, LogEntry> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: LogEntry = serde_json::from_str(&line)?;
            if !entries.contains_key(&entry.id) {
                order.push(entry.id.clone());
            }
            entries.insert(entry.id.clone(), entry);
        }
        let deduped: Vec<_> = order
            .into_iter()
            .filter_map(|id| entries.remove(&id))
            .collect();
        let deduped_count = deduped.len();

        // Write atomically
        let tmp = self.path.with_extension("jsonl.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        for entry in &deduped {
            let line = serde_json::to_string(entry)?;
            writeln!(file, "{line}")?;
        }
        file.flush()?;
        std::fs::rename(&tmp, &self.path)?;

        Ok(raw_count - deduped_count)
    }

    pub fn query(&self, params: &LogQueryParams) -> anyhow::Result<Vec<LogEntry>> {
        let mut entries = self.read_all()?;

        if let Some(ref dataset) = params.dataset {
            entries.retain(|e| e.datasets.iter().any(|d| d == dataset));
        }
        if let Some(ref prompt) = params.prompt {
            entries.retain(|e| e.prompt.name == *prompt);
        }

        let limit = params.limit.unwrap_or(50);
        entries.reverse();
        entries.truncate(limit);
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use chrono::Utc;

    fn entry(id: &str, status: EntryStatus, dataset: &str) -> LogEntry {
        LogEntry {
            id: id.to_string(),
            timestamp: Utc::now(),
            sources: vec![],
            datasets: vec![dataset.to_string()],
            prompt: PromptRef {
                name: "research".to_string(),
                content: "...".to_string(),
                git_sha: None,
            },
            backend: "cognee".to_string(),
            status,
            error: None,
            replayed_from: None,
        }
    }

    #[test]
    fn mark_dataset_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let log = IntakeLog::new(dir.path().join("test.jsonl"));

        log.append(&entry("A", EntryStatus::Integrated, "research"))
            .unwrap();
        log.append(&entry("B", EntryStatus::Integrated, "personal"))
            .unwrap();
        log.append(&entry("C", EntryStatus::Integrated, "research"))
            .unwrap();

        let count = log.mark_dataset_deleted("research").unwrap();
        assert_eq!(count, 2);

        let all = log.read_all().unwrap();
        assert_eq!(all[0].status, EntryStatus::DatasetDeleted); // A
        assert_eq!(all[1].status, EntryStatus::Integrated); // B untouched
        assert_eq!(all[2].status, EntryStatus::DatasetDeleted); // C
    }

    #[test]
    fn append_read_and_last_write_wins() {
        let dir = tempfile::tempdir().unwrap();
        let log = IntakeLog::new(dir.path().join("test.jsonl"));

        log.append(&entry("A", EntryStatus::Pending, "research"))
            .unwrap();
        log.append(&entry("B", EntryStatus::Integrated, "personal"))
            .unwrap();
        // Update A's status — same ID, should overwrite on read
        log.append(&entry("A", EntryStatus::Integrated, "research"))
            .unwrap();

        let all = log.read_all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "A");
        assert_eq!(all[0].status, EntryStatus::Integrated); // last-write-wins
        assert_eq!(all[1].id, "B");
    }

    #[test]
    fn compact_removes_duplicate_entries() {
        let dir = tempfile::tempdir().unwrap();
        let log = IntakeLog::new(dir.path().join("test.jsonl"));

        // Write same ID multiple times (simulates status updates)
        log.append(&entry("A", EntryStatus::Pending, "research"))
            .unwrap();
        log.append(&entry("B", EntryStatus::Integrated, "personal"))
            .unwrap();
        log.append(&entry("A", EntryStatus::Added, "research"))
            .unwrap();
        log.append(&entry("A", EntryStatus::Integrated, "research"))
            .unwrap();

        // 4 raw lines, 2 unique entries
        let removed = log.compact().unwrap();
        assert_eq!(removed, 2);

        let all = log.read_all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "A");
        assert_eq!(all[0].status, EntryStatus::Integrated); // last write wins
        assert_eq!(all[1].id, "B");
    }

    #[test]
    fn compact_on_empty_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let log = IntakeLog::new(dir.path().join("test.jsonl"));
        assert_eq!(log.compact().unwrap(), 0);
    }

    #[test]
    fn compact_no_duplicates_removes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let log = IntakeLog::new(dir.path().join("test.jsonl"));

        log.append(&entry("A", EntryStatus::Integrated, "research"))
            .unwrap();
        log.append(&entry("B", EntryStatus::Integrated, "personal"))
            .unwrap();

        let removed = log.compact().unwrap();
        assert_eq!(removed, 0);
        assert_eq!(log.read_all().unwrap().len(), 2);
    }

    #[test]
    fn concurrent_appends_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        let log = std::sync::Arc::new(IntakeLog::new(dir.path().join("test.jsonl")));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let log = log.clone();
                std::thread::spawn(move || {
                    let e = entry(&format!("T{i}"), EntryStatus::Integrated, "research");
                    log.append(&e).unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let all = log.read_all().unwrap();
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn query_filters_by_dataset_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let log = IntakeLog::new(dir.path().join("test.jsonl"));

        log.append(&entry("A", EntryStatus::Integrated, "research"))
            .unwrap();
        log.append(&entry("B", EntryStatus::Integrated, "personal"))
            .unwrap();
        log.append(&entry("C", EntryStatus::Integrated, "research"))
            .unwrap();
        log.append(&entry("D", EntryStatus::Integrated, "research"))
            .unwrap();

        let params = LogQueryParams {
            dataset: Some("research".to_string()),
            prompt: None,
            limit: Some(2),
        };
        let results = log.query(&params).unwrap();
        assert_eq!(results.len(), 2);
        // query reverses and truncates, so we get the last 2 research entries
        assert_eq!(results[0].id, "D");
        assert_eq!(results[1].id, "C");
    }
}
