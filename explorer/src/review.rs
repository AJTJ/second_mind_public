use std::io::{self, Write};
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
pub struct ReviewEntry {
    pub id: String,
    pub timestamp: String,
    pub directive: String,
    pub status: ReviewStatus,
    pub findings_json: Option<Value>,
    pub raw_output_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
}

/// Convert a directive into a filesystem-safe slug.
/// "AI infrastructure demand in 2025-2026: What physical..." → "ai-infrastructure-demand-in-2025-2026"
fn slugify(text: &str) -> String {
    let slug: String = text
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Collapse repeated dashes, trim, limit length
    let mut result = String::new();
    let mut last_dash = false;
    for c in slug.chars().take(60) {
        if c == '-' {
            if !last_dash && !result.is_empty() {
                result.push('-');
            }
            last_dash = true;
        } else {
            result.push(c);
            last_dash = false;
        }
    }

    result.trim_end_matches('-').to_string()
}

pub struct ReviewStore {
    dir: PathBuf,
}

impl ReviewStore {
    pub fn new(dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn save(
        &self,
        directive: &str,
        findings_json: Option<Value>,
        raw_output: &str,
    ) -> anyhow::Result<String> {
        let id = ulid::Ulid::new().to_string();
        let slug = slugify(directive);

        let raw_path = self.dir.join(format!("{slug}_raw.md"));
        std::fs::write(&raw_path, raw_output)?;

        let entry = ReviewEntry {
            id: id.clone(),
            timestamp: Utc::now().to_rfc3339(),
            directive: directive.to_string(),
            status: ReviewStatus::Pending,
            findings_json,
            raw_output_path: raw_path.to_string_lossy().to_string(),
        };

        let entry_path = self.dir.join(format!("{slug}.json"));
        let json = serde_json::to_string_pretty(&entry)?;
        std::fs::write(&entry_path, json)?;

        Ok(id)
    }

    pub fn list_pending(&self) -> anyhow::Result<Vec<ReviewEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let content = std::fs::read_to_string(&path)?;
                let entry: ReviewEntry = serde_json::from_str(&content)?;
                if entry.status == ReviewStatus::Pending {
                    entries.push(entry);
                }
            }
        }
        entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        Ok(entries)
    }

    /// Find the file path for a review by ID (supports both ULID and human-readable filenames).
    fn find_path(&self, id: &str) -> anyhow::Result<PathBuf> {
        // Try direct ULID match first
        let direct = self.dir.join(format!("{id}.json"));
        if direct.exists() {
            return Ok(direct);
        }

        // Scan all JSONs for matching id field
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let content = std::fs::read_to_string(&path)?;
                if let Ok(review) = serde_json::from_str::<ReviewEntry>(&content)
                    && (review.id == id || review.id.starts_with(id))
                {
                    return Ok(path);
                }
            }
        }

        anyhow::bail!("Review entry '{id}' not found")
    }

    pub fn update_status(&self, id: &str, status: ReviewStatus) -> anyhow::Result<()> {
        let path = self.find_path(id)?;
        let content = std::fs::read_to_string(&path)?;
        let mut entry: ReviewEntry = serde_json::from_str(&content)?;
        entry.status = status;
        let json = serde_json::to_string_pretty(&entry)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> anyhow::Result<ReviewEntry> {
        let path = self.find_path(id)?;
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn slugify_basic() {
        assert_eq!(
            slugify("AI infrastructure demand in 2025"),
            "ai-infrastructure-demand-in-2025"
        );
    }

    #[test]
    fn slugify_collapses_dashes() {
        assert_eq!(slugify("hello---world"), "hello-world");
        assert_eq!(slugify("a   b   c"), "a-b-c");
    }

    #[test]
    fn slugify_strips_leading_and_trailing_dashes() {
        // Leading special chars become dashes which are skipped when result is empty
        assert_eq!(slugify("---hello---"), "hello");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn slugify_truncates_at_60_chars() {
        let long = "a".repeat(100);
        let result = slugify(&long);
        assert!(result.len() <= 60);
    }

    #[test]
    fn slugify_special_characters() {
        assert_eq!(
            slugify("What's the cost? (2026 estimate)"),
            "what-s-the-cost-2026-estimate"
        );
    }

    #[test]
    fn review_store_save_and_list() {
        let tmp = TempDir::new().unwrap();
        let store = ReviewStore::new(tmp.path().to_path_buf()).unwrap();

        let id = store
            .save("test directive", None, "raw output here")
            .unwrap();
        assert!(!id.is_empty());

        let pending = store.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].directive, "test directive");
        assert_eq!(pending[0].status, ReviewStatus::Pending);
    }

    #[test]
    fn review_store_update_status() {
        let tmp = TempDir::new().unwrap();
        let store = ReviewStore::new(tmp.path().to_path_buf()).unwrap();

        let id = store.save("approve me", None, "raw").unwrap();
        store.update_status(&id, ReviewStatus::Approved).unwrap();

        let entry = store.get(&id).unwrap();
        assert_eq!(entry.status, ReviewStatus::Approved);

        // Should no longer appear in pending
        let pending = store.list_pending().unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn review_store_get_by_prefix() {
        let tmp = TempDir::new().unwrap();
        let store = ReviewStore::new(tmp.path().to_path_buf()).unwrap();

        let id = store.save("prefix test", None, "raw").unwrap();
        // ULID prefix match (first 6 chars)
        let prefix = &id[..6];
        let entry = store.get(prefix).unwrap();
        assert_eq!(entry.id, id);
    }

    #[test]
    fn review_store_not_found() {
        let tmp = TempDir::new().unwrap();
        let store = ReviewStore::new(tmp.path().to_path_buf()).unwrap();

        let result = store.get("nonexistent-id");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn review_store_saves_raw_output_file() {
        let tmp = TempDir::new().unwrap();
        let store = ReviewStore::new(tmp.path().to_path_buf()).unwrap();

        store.save("check raw", None, "the raw markdown").unwrap();

        let raw_path = tmp.path().join("check-raw_raw.md");
        assert!(raw_path.exists());
        assert_eq!(
            std::fs::read_to_string(raw_path).unwrap(),
            "the raw markdown"
        );
    }

    #[test]
    fn review_entry_roundtrip() {
        let entry = ReviewEntry {
            id: "test-123".to_string(),
            timestamp: "2026-04-13T00:00:00Z".to_string(),
            directive: "test".to_string(),
            status: ReviewStatus::Pending,
            findings_json: Some(serde_json::json!({"findings": []})),
            raw_output_path: "/tmp/raw.md".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ReviewEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "test-123");
        assert_eq!(back.status, ReviewStatus::Pending);
    }
}

/// Interactive review — prompts the user at the terminal.
pub fn interactive_review(entry: &ReviewEntry) -> anyhow::Result<ReviewStatus> {
    println!("\n--- Research Findings ---");
    println!("Directive: {}", entry.directive);
    println!("Time: {}", entry.timestamp);

    if let Some(ref findings) = entry.findings_json {
        if let Some(claims) = findings["findings"].as_array() {
            println!("\n{} claims found:\n", claims.len());
            for (i, claim) in claims.iter().enumerate() {
                let text = claim["claim"].as_str().unwrap_or("?");
                let confidence = claim["confidence"].as_str().unwrap_or("?");
                println!("  {}. [{}] {}", i + 1, confidence, text);
            }
        }
        if let Some(gaps) = findings["gaps"].as_array()
            && !gaps.is_empty()
        {
            println!("\nGaps identified:");
            for gap in gaps {
                println!("  - {}", gap.as_str().unwrap_or("?"));
            }
        }
    } else {
        println!("\n(No structured findings — check raw output)");
    }

    println!("\n[a]pprove  [r]eject  [v]iew raw output");
    print!("> ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    match input.trim() {
        "a" | "approve" => Ok(ReviewStatus::Approved),
        "r" | "reject" => Ok(ReviewStatus::Rejected),
        "v" | "view" => {
            let raw = std::fs::read_to_string(&entry.raw_output_path)?;
            println!("\n--- Raw Output ---\n{raw}\n--- End ---\n");
            // Ask again after viewing
            interactive_review(entry)
        }
        _ => {
            println!("Invalid choice. Try again.");
            interactive_review(entry)
        }
    }
}
