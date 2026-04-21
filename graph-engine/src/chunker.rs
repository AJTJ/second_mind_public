use text_splitter::{ChunkConfig, MarkdownSplitter};

#[derive(Clone)]
pub struct ChunkerConfig {
    pub target_chunk_tokens: usize, // default 1500
    pub max_chunk_tokens: usize,    // default 2000
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            target_chunk_tokens: 1500,
            max_chunk_tokens: 2000,
        }
    }
}

/// Chunk a document into pieces suitable for embedding.
///
/// Uses markdown-aware splitting so headings and code blocks stay intact where
/// possible. Token counts are approximated as chars / 4 — a real tokenizer can
/// be swapped in later via the `tokenizers` feature of `text-splitter`.
pub fn chunk_document(text: &str, config: &ChunkerConfig) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    let target_chars = config.target_chunk_tokens * 4;
    let max_chars = config.max_chunk_tokens * 4;

    // If the document fits in a single chunk, return as-is.
    if text.len() <= target_chars {
        return vec![text.to_string()];
    }

    let splitter = MarkdownSplitter::new(ChunkConfig::new(target_chars..max_chars));
    splitter.chunks(text).map(|s: &str| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_returns_empty_vec() {
        let chunks = chunk_document("", &ChunkerConfig::default());
        assert!(chunks.is_empty());
    }

    #[test]
    fn short_text_returns_single_chunk() {
        let text = "Hello, this is a short document.";
        let chunks = chunk_document(text, &ChunkerConfig::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn long_text_gets_split() {
        // Build a document well over the default target (1500 * 4 = 6000 chars).
        let paragraph = "This is a sentence that adds some length to the document. ";
        let text = paragraph.repeat(200); // ~11600 chars
        let chunks = chunk_document(&text, &ChunkerConfig::default());
        assert!(chunks.len() > 1, "expected multiple chunks, got {}", chunks.len());
    }

    #[test]
    fn chunks_do_not_exceed_max_chars() {
        let config = ChunkerConfig::default();
        let max_chars = config.max_chunk_tokens * 4;

        let paragraph = "Word ".repeat(600); // ~3000 chars
        let text = format!("# Heading\n\n{paragraph}\n\n## Another section\n\n{paragraph}");
        let chunks = chunk_document(&text, &config);

        for (i, chunk) in chunks.iter().enumerate() {
            assert!(
                chunk.len() <= max_chars,
                "chunk {i} is {} chars, exceeds max {max_chars}",
                chunk.len()
            );
        }
    }
}
