use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// Compute a cache key from model name and message content.
pub fn cache_key(model: &str, messages: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", model, messages).as_bytes());
    hex::encode(hasher.finalize())
}

/// Check the cache for a previous response.
pub async fn get(pool: &PgPool, key: &str) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT response FROM llm_cache WHERE hash = $1")
            .bind(key)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(r,)| r))
}

/// Store a response in the cache.
pub async fn set(pool: &PgPool, key: &str, model: &str, response: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO llm_cache (hash, model, response) VALUES ($1, $2, $3) \
         ON CONFLICT (hash) DO NOTHING",
    )
    .bind(key)
    .bind(model)
    .bind(response)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_consistent() {
        let k1 = cache_key("claude-sonnet", "hello world");
        let k2 = cache_key("claude-sonnet", "hello world");
        assert_eq!(k1, k2);
    }

    #[test]
    fn cache_key_differs_for_different_input() {
        let k1 = cache_key("claude-sonnet", "hello world");
        let k2 = cache_key("claude-sonnet", "goodbye world");
        let k3 = cache_key("gpt-4", "hello world");
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn cache_key_is_64_hex_chars() {
        let k = cache_key("model", "messages");
        assert_eq!(k.len(), 64);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
