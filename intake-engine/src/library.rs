use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::types::{SourceInput, SourceType, StoredSource};

pub struct Library {
    base_dir: PathBuf,
}

impl Library {
    pub fn new(base_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(base_dir.join("texts"))?;
        std::fs::create_dir_all(base_dir.join("snapshots"))?;
        std::fs::create_dir_all(base_dir.join("files"))?;
        std::fs::create_dir_all(base_dir.join("audio"))?;
        std::fs::create_dir_all(base_dir.join("transcripts"))?;
        Ok(Self { base_dir })
    }

    pub async fn store(&self, id: &str, input: &SourceInput) -> anyhow::Result<StoredSource> {
        match input.source_type {
            SourceType::Text => self.store_text(id, input).await,
            SourceType::Url => self.store_url(id, input).await,
            SourceType::File => self.store_file(id, input).await,
            SourceType::Audio => self.store_audio(id, input).await,
        }
    }

    async fn store_text(&self, id: &str, input: &SourceInput) -> anyhow::Result<StoredSource> {
        let content = input
            .content
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("text source requires 'content' field"))?;

        let hash = sha256(content.as_bytes());
        let filename = format!("{id}.md");
        let rel_path = Path::new("texts").join(&filename);
        let abs_path = self.base_dir.join(&rel_path);
        std::fs::write(&abs_path, content)?;

        Ok(StoredSource {
            source_type: SourceType::Text,
            original_ref: format!("(inline text, {} bytes)", content.len()),
            stored_path: rel_path.to_string_lossy().to_string(),
            sha256: hash,
        })
    }

    async fn store_url(&self, id: &str, input: &SourceInput) -> anyhow::Result<StoredSource> {
        let url = input
            .path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("url source requires 'path' field"))?;

        let resp = reqwest::get(url).await?;
        let body = resp.text().await?;
        let hash = sha256(body.as_bytes());

        let filename = format!("{id}.html");
        let rel_path = Path::new("snapshots").join(&filename);
        let abs_path = self.base_dir.join(&rel_path);
        std::fs::write(&abs_path, &body)?;

        Ok(StoredSource {
            source_type: SourceType::Url,
            original_ref: url.clone(),
            stored_path: rel_path.to_string_lossy().to_string(),
            sha256: hash,
        })
    }

    async fn store_file(&self, id: &str, input: &SourceInput) -> anyhow::Result<StoredSource> {
        let path = input
            .path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("file source requires 'path' field"))?;

        let content = std::fs::read(path)?;
        let hash = sha256(&content);

        // Use the original filename for human readability, with ULID prefix for uniqueness
        let original_name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(id);
        let filename = if original_name.contains('.') {
            original_name.to_string()
        } else {
            let ext = Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("bin");
            format!("{original_name}.{ext}")
        };
        let rel_path = Path::new("files").join(&filename);
        let abs_path = self.base_dir.join(&rel_path);
        // If file already exists (same name, different content), add ID suffix
        let (rel_path, abs_path) =
            if abs_path.exists() && sha256(&std::fs::read(&abs_path)?) != hash {
                let stem = Path::new(&filename)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(id);
                let ext = Path::new(&filename)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("bin");
                let unique = format!("{stem}_{id}.{ext}");
                let rp = Path::new("files").join(&unique);
                let ap = self.base_dir.join(&rp);
                (rp, ap)
            } else {
                (rel_path, abs_path)
            };
        std::fs::write(&abs_path, &content)?;

        Ok(StoredSource {
            source_type: SourceType::File,
            original_ref: path.clone(),
            stored_path: rel_path.to_string_lossy().to_string(),
            sha256: hash,
        })
    }

    async fn store_audio(&self, id: &str, input: &SourceInput) -> anyhow::Result<StoredSource> {
        let path = input
            .path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("audio source requires 'path' field"))?;

        let content = std::fs::read(path)?;
        let hash = sha256(&content);

        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp3");
        let filename = format!("{id}.{ext}");
        let rel_path = Path::new("audio").join(&filename);
        let abs_path = self.base_dir.join(&rel_path);
        std::fs::write(&abs_path, &content)?;

        Ok(StoredSource {
            source_type: SourceType::Audio,
            original_ref: path.clone(),
            stored_path: rel_path.to_string_lossy().to_string(),
            sha256: hash,
        })
    }

    /// Read stored source content as text (for replay / forwarding to backend).
    pub fn read_source(&self, stored_path: &str) -> anyhow::Result<String> {
        let abs_path = self.base_dir.join(stored_path);
        Ok(std::fs::read_to_string(&abs_path)?)
    }
}

fn sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SourceInput, SourceType};
    use tempfile::TempDir;

    fn make_library() -> (Library, TempDir) {
        let tmp = TempDir::new().unwrap();
        let lib = Library::new(tmp.path().to_path_buf()).unwrap();
        (lib, tmp)
    }

    fn text_input(content: &str) -> SourceInput {
        SourceInput {
            source_type: SourceType::Text,
            content: Some(content.to_string()),
            path: None,
        }
    }

    #[test]
    fn sha256_deterministic() {
        let a = sha256(b"hello world");
        let b = sha256(b"hello world");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // hex-encoded SHA-256 is 64 chars
    }

    #[test]
    fn sha256_different_inputs() {
        let a = sha256(b"hello");
        let b = sha256(b"world");
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn store_text_creates_file() {
        let (lib, tmp) = make_library();
        let input = text_input("some research content");
        let stored = lib.store("entry-001", &input).await.unwrap();

        assert!(matches!(stored.source_type, SourceType::Text));
        assert_eq!(stored.stored_path, "texts/entry-001.md");
        assert_eq!(stored.sha256, sha256(b"some research content"));
        assert!(stored.original_ref.contains("21 bytes"));

        // File actually exists and has correct content
        let abs = tmp.path().join("texts/entry-001.md");
        assert_eq!(
            std::fs::read_to_string(abs).unwrap(),
            "some research content"
        );
    }

    #[tokio::test]
    async fn store_text_missing_content_errors() {
        let (lib, _tmp) = make_library();
        let input = SourceInput {
            source_type: SourceType::Text,
            content: None,
            path: None,
        };
        let result = lib.store("entry-002", &input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("content"));
    }

    #[tokio::test]
    async fn read_source_returns_stored_content() {
        let (lib, _tmp) = make_library();
        let input = text_input("readable content here");
        let stored = lib.store("entry-003", &input).await.unwrap();

        let content = lib.read_source(&stored.stored_path).unwrap();
        assert_eq!(content, "readable content here");
    }

    #[tokio::test]
    async fn store_file_basic() {
        let (lib, tmp) = make_library();

        // Create a source file to store
        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("report.pdf");
        std::fs::write(&src_file, b"fake pdf content").unwrap();

        let input = SourceInput {
            source_type: SourceType::File,
            content: None,
            path: Some(src_file.to_string_lossy().to_string()),
        };
        let stored = lib.store("entry-004", &input).await.unwrap();

        assert!(matches!(stored.source_type, SourceType::File));
        assert_eq!(stored.stored_path, "files/report.pdf");
        assert_eq!(stored.sha256, sha256(b"fake pdf content"));

        let abs = tmp.path().join("files/report.pdf");
        assert_eq!(std::fs::read(abs).unwrap(), b"fake pdf content");
    }

    #[tokio::test]
    async fn store_file_dedup_same_content_overwrites() {
        let (lib, _tmp) = make_library();

        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("data.csv");
        std::fs::write(&src_file, b"same content").unwrap();

        let input = SourceInput {
            source_type: SourceType::File,
            content: None,
            path: Some(src_file.to_string_lossy().to_string()),
        };

        let stored1 = lib.store("entry-005", &input).await.unwrap();
        let stored2 = lib.store("entry-006", &input).await.unwrap();

        // Same filename, same content — should use same path
        assert_eq!(stored1.stored_path, stored2.stored_path);
        assert_eq!(stored1.sha256, stored2.sha256);
    }

    #[tokio::test]
    async fn store_file_dedup_different_content_gets_unique_name() {
        let (lib, tmp) = make_library();

        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("data.csv");

        // First version
        std::fs::write(&src_file, b"version 1").unwrap();
        let input1 = SourceInput {
            source_type: SourceType::File,
            content: None,
            path: Some(src_file.to_string_lossy().to_string()),
        };
        let stored1 = lib.store("entry-007", &input1).await.unwrap();

        // Overwrite source with different content
        std::fs::write(&src_file, b"version 2").unwrap();
        let input2 = SourceInput {
            source_type: SourceType::File,
            content: None,
            path: Some(src_file.to_string_lossy().to_string()),
        };
        let stored2 = lib.store("entry-008", &input2).await.unwrap();

        // Different content, same filename — should get unique path
        assert_eq!(stored1.stored_path, "files/data.csv");
        assert_eq!(stored2.stored_path, "files/data_entry-008.csv");
        assert_ne!(stored1.sha256, stored2.sha256);

        // Both files exist with correct content
        assert_eq!(
            std::fs::read(tmp.path().join("files/data.csv")).unwrap(),
            b"version 1"
        );
        assert_eq!(
            std::fs::read(tmp.path().join("files/data_entry-008.csv")).unwrap(),
            b"version 2"
        );
    }

    #[tokio::test]
    async fn store_audio_basic() {
        let (lib, tmp) = make_library();

        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("interview.wav");
        std::fs::write(&src_file, b"fake audio bytes").unwrap();

        let input = SourceInput {
            source_type: SourceType::Audio,
            content: None,
            path: Some(src_file.to_string_lossy().to_string()),
        };
        let stored = lib.store("entry-009", &input).await.unwrap();

        assert!(matches!(stored.source_type, SourceType::Audio));
        assert_eq!(stored.stored_path, "audio/entry-009.wav");
        assert_eq!(stored.sha256, sha256(b"fake audio bytes"));

        let abs = tmp.path().join("audio/entry-009.wav");
        assert_eq!(std::fs::read(abs).unwrap(), b"fake audio bytes");
    }

    #[tokio::test]
    async fn store_audio_missing_path_errors() {
        let (lib, _tmp) = make_library();
        let input = SourceInput {
            source_type: SourceType::Audio,
            content: None,
            path: None,
        };
        let result = lib.store("entry-010", &input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path"));
    }

    #[test]
    fn library_creates_subdirectories() {
        let tmp = TempDir::new().unwrap();
        let _lib = Library::new(tmp.path().to_path_buf()).unwrap();

        assert!(tmp.path().join("texts").is_dir());
        assert!(tmp.path().join("snapshots").is_dir());
        assert!(tmp.path().join("files").is_dir());
        assert!(tmp.path().join("audio").is_dir());
        assert!(tmp.path().join("transcripts").is_dir());
    }
}
