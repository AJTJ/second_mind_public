use async_trait::async_trait;

use crate::types::{ExtractedEntity, ExtractedRelationship, ExtractionResult};

#[async_trait]
pub trait Extractor: Send + Sync {
    async fn extract(&self, text: &str, prompt: &str) -> anyhow::Result<ExtractionResult>;
}

/// Mock extractor for testing. Returns deterministic entities based on text
/// content — looks for capitalized words and creates `related_to` edges between
/// consecutive entities.
pub struct MockExtractor;

#[async_trait]
impl Extractor for MockExtractor {
    async fn extract(&self, text: &str, _prompt: &str) -> anyhow::Result<ExtractionResult> {
        let mut entities = Vec::new();
        let mut relationships = Vec::new();

        // Extract capitalized multi-word phrases as entities.
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut i = 0;
        while i < words.len() {
            let w = words[i].trim_matches(|c: char| !c.is_alphanumeric());
            if !w.is_empty()
                && w.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                && w.len() > 1
            {
                // Gather consecutive capitalized words into one entity name.
                let mut name = w.to_string();
                let mut j = i + 1;
                while j < words.len() {
                    let next = words[j].trim_matches(|c: char| !c.is_alphanumeric());
                    if !next.is_empty()
                        && next.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                    {
                        name.push(' ');
                        name.push_str(next);
                        j += 1;
                    } else {
                        break;
                    }
                }
                entities.push(ExtractedEntity {
                    name: name.clone(),
                    entity_type: Some("concept".to_string()),
                    description: format!("Extracted from text: {name}"),
                });
                i = j;
            } else {
                i += 1;
            }
        }

        // Create relationships between consecutive entities.
        for pair in entities.windows(2) {
            relationships.push(ExtractedRelationship {
                source: pair[0].name.clone(),
                target: pair[1].name.clone(),
                relationship: "related_to".to_string(),
                fact: Some(format!("{} is related to {}", pair[0].name, pair[1].name)),
                confidence: Some("emerging".to_string()),
            });
        }

        Ok(ExtractionResult {
            entities,
            relationships,
        })
    }
}

// ---------------------------------------------------------------------------
// System prompt for entity/relationship extraction
// ---------------------------------------------------------------------------

const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are an entity and relationship extraction engine. Extract structured knowledge from the given text.

For each entity found, provide:
- name: The canonical name (proper capitalization, full name preferred)
- entity_type: One of: person, organization, concept, material, technology, location, event, product, metric
- description: One sentence describing this entity in the context of the text

For each relationship found, provide:
- source: Name of the source entity (must match an extracted entity name exactly)
- target: Name of the target entity (must match an extracted entity name exactly)
- relationship: One of: requires, enables, contains, part_of, instance_of, supports, contradicts, extends, refines, supersedes, causes, prevents, increases, decreases, supplies, consumes, produces, competes_with, listed_on, holds, related_to
- fact: A natural language sentence describing the relationship
- confidence: One of: established, emerging, contested

DO NOT extract:
- Pronouns (he, she, they, it, them)
- Abstract concepts with no specificity (things, stuff, ideas, growth, demand, opportunity)
- Generic temporal references (today, tomorrow, recently, soon)
- Bare relational terms (parent, child, friend) — qualify them ("Alice's parent")
- Single common words (the, an, good, bad, new, old)

When in doubt, do not extract the entity.

Return your response as JSON:
{"entities": [...], "relationships": [...]}"#;

// ---------------------------------------------------------------------------
// LLM-based extractor (calls Anthropic Claude API)
// ---------------------------------------------------------------------------

/// LLM-based extractor that calls the Anthropic API for entity/relationship
/// extraction. Supports an optional graph-store-backed response cache and a
/// gleaning loop to capture entities missed on the first pass.
pub struct LlmExtractor {
    client: reqwest::Client,
    api_key: String,
    model: String,
    graph_store: Option<std::sync::Arc<dyn crate::store::GraphStore>>,
    max_gleanings: u32,
    excluded_entity_types: Vec<String>,
}

impl LlmExtractor {
    pub fn new(api_key: String, model: String) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            api_key,
            model,
            graph_store: None,
            max_gleanings: 1,
            excluded_entity_types: vec![],
        }
    }

    pub fn with_cache(mut self, store: std::sync::Arc<dyn crate::store::GraphStore>) -> Self {
        self.graph_store = Some(store);
        self
    }

    pub fn with_max_gleanings(mut self, n: u32) -> Self {
        self.max_gleanings = n;
        self
    }

    pub fn with_excluded_types(mut self, types: Vec<String>) -> Self {
        self.excluded_entity_types = types;
        self
    }

    /// Send messages to the Anthropic API and return the text content of the
    /// first content block. Retries on 429 (rate limited) and 529 (overloaded)
    /// with exponential backoff.
    async fn call_anthropic(
        &self,
        messages: &[serde_json::Value],
    ) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": EXTRACTION_SYSTEM_PROMPT,
            "messages": messages,
        });

        let max_retries = 3;
        for attempt in 0..max_retries {
            let resp = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await?;

            let status = resp.status();

            // Rate limited or overloaded -- retry with backoff
            if (status.as_u16() == 429 || status.as_u16() == 529) && attempt < max_retries - 1 {
                let delay = 2u64.pow(attempt as u32 + 1); // 2s, 4s, 8s
                tracing::warn!(
                    "Anthropic API {status}, retrying in {delay}s (attempt {}/{})",
                    attempt + 1,
                    max_retries
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                continue;
            }

            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!(
                    "Anthropic API error ({}): {}",
                    status,
                    &body[..body.len().min(200)]
                );
            }

            let data: serde_json::Value = resp.json().await?;
            let text = data["content"][0]["text"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No text in Anthropic response"))?;

            return Ok(text.to_string());
        }

        anyhow::bail!("Anthropic API failed after {max_retries} retries")
    }

    /// Filter out entities whose type appears in the exclusion list and remove
    /// any relationships that reference a filtered entity.
    fn filter_excluded(&self, mut result: ExtractionResult) -> ExtractionResult {
        if self.excluded_entity_types.is_empty() {
            return result;
        }

        let excluded: std::collections::HashSet<&str> = self
            .excluded_entity_types
            .iter()
            .map(|s| s.as_str())
            .collect();

        // Collect names of entities that will be removed.
        let removed_names: std::collections::HashSet<String> = result
            .entities
            .iter()
            .filter(|e| {
                e.entity_type
                    .as_deref()
                    .is_some_and(|t| excluded.contains(t))
            })
            .map(|e| e.name.clone())
            .collect();

        result
            .entities
            .retain(|e| !e.entity_type.as_deref().is_some_and(|t| excluded.contains(t)));

        result.relationships.retain(|r| {
            !removed_names.contains(&r.source) && !removed_names.contains(&r.target)
        });

        result
    }
}

#[async_trait]
impl Extractor for LlmExtractor {
    async fn extract(&self, text: &str, prompt: &str) -> anyhow::Result<ExtractionResult> {
        // Build user message combining the custom prompt and the text to analyse.
        let user_content = if prompt.is_empty() {
            format!("Extract entities and relationships from the following text:\n\n{text}")
        } else {
            format!("{prompt}\n\nText:\n{text}")
        };

        let mut messages = vec![serde_json::json!({
            "role": "user",
            "content": user_content,
        })];

        // --- Cache check (initial extraction) ---
        let cache_key_initial = if let Some(ref store) = self.graph_store {
            let messages_str = serde_json::to_string(&messages)?;
            let key = crate::cache::cache_key(&self.model, &messages_str);
            if let Some(cached) = store.cache_get(&key).await? {
                tracing::debug!("LLM cache hit for extraction");
                let result = parse_extraction(&cached)?;
                return Ok(self.filter_excluded(result));
            }
            Some(key)
        } else {
            None
        };

        // --- Initial extraction ---
        let response_text = self.call_anthropic(&messages).await?;
        let mut result = parse_extraction(&response_text)?;

        // Cache the initial response.
        if let (Some(store), Some(key)) = (&self.graph_store, &cache_key_initial) {
            let _ = store.cache_set(key, &self.model, &response_text).await;
        }

        // Add assistant response to conversation for gleaning context.
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": response_text,
        }));

        // --- Gleaning loop ---
        for _ in 0..self.max_gleanings {
            // Ask for more entities.
            messages.push(serde_json::json!({
                "role": "user",
                "content": "MANY entities and relationships were missed in the last extraction. Add them below in the same JSON format.",
            }));

            let glean_response = self.call_anthropic(&messages).await?;
            if let Ok(extra) = parse_extraction(&glean_response) {
                merge_results(&mut result, extra);
            }

            messages.push(serde_json::json!({
                "role": "assistant",
                "content": glean_response,
            }));

            // Ask if more remain.
            messages.push(serde_json::json!({
                "role": "user",
                "content": "Are there still entities or relationships that need to be added? Answer Y or N only.",
            }));

            let check_response = self.call_anthropic(&messages).await?;
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": &check_response,
            }));

            if check_response.trim().to_uppercase() != "Y" {
                break;
            }
        }

        Ok(self.filter_excluded(result))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse an `ExtractionResult` from LLM output. Handles bare JSON, JSON
/// wrapped in ````json` code fences, and JSON embedded in surrounding prose.
pub(crate) fn parse_extraction(text: &str) -> anyhow::Result<ExtractionResult> {
    // Try parsing as-is first.
    if let Ok(result) = serde_json::from_str::<ExtractionResult>(text) {
        return Ok(result);
    }

    // Try extracting from ```json ... ``` block.
    if let Some(start) = text.find("```json") {
        let json_start = start + 7;
        if let Some(end) = text[json_start..].find("```") {
            let json_str = text[json_start..json_start + end].trim();
            if let Ok(result) = serde_json::from_str::<ExtractionResult>(json_str) {
                return Ok(result);
            }
        }
    }

    // Try finding any JSON object.
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            let json_str = &text[start..=end];
            if let Ok(result) = serde_json::from_str::<ExtractionResult>(json_str) {
                return Ok(result);
            }
        }
    }

    anyhow::bail!("Could not parse extraction result from LLM response")
}

/// Merge `extra` entities and relationships into `base`, deduplicating entities
/// by name (case-insensitive).
fn merge_results(base: &mut ExtractionResult, extra: ExtractionResult) {
    let existing_names: std::collections::HashSet<String> = base
        .entities
        .iter()
        .map(|e| e.name.to_lowercase())
        .collect();

    for entity in extra.entities {
        if !existing_names.contains(&entity.name.to_lowercase()) {
            base.entities.push(entity);
        }
    }

    // For relationships we do a simple source+target+relationship dedup.
    let existing_rels: std::collections::HashSet<(String, String, String)> = base
        .relationships
        .iter()
        .map(|r| {
            (
                r.source.to_lowercase(),
                r.target.to_lowercase(),
                r.relationship.to_lowercase(),
            )
        })
        .collect();

    for rel in extra.relationships {
        let key = (
            rel.source.to_lowercase(),
            rel.target.to_lowercase(),
            rel.relationship.to_lowercase(),
        );
        if !existing_rels.contains(&key) {
            base.relationships.push(rel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_extracts_capitalized_words() {
        let extractor = MockExtractor;
        let result = extractor
            .extract("Teck Resources mines copper in Canada", "")
            .await
            .unwrap();

        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"Teck Resources"),
            "expected 'Teck Resources' in {names:?}"
        );
        assert!(
            names.contains(&"Canada"),
            "expected 'Canada' in {names:?}"
        );
    }

    #[tokio::test]
    async fn mock_creates_relationships_between_consecutive_entities() {
        let extractor = MockExtractor;
        let result = extractor
            .extract("Data Centers consume Copper Wiring daily", "")
            .await
            .unwrap();

        assert!(
            !result.relationships.is_empty(),
            "expected at least one relationship"
        );
        let rel = &result.relationships[0];
        assert_eq!(rel.relationship, "related_to");
        assert_eq!(rel.source, result.entities[0].name);
        assert_eq!(rel.target, result.entities[1].name);
    }

    #[tokio::test]
    async fn mock_returns_empty_for_no_capitalized_words() {
        let extractor = MockExtractor;
        let result = extractor
            .extract("all lowercase words here", "")
            .await
            .unwrap();

        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn llm_extractor_construction() {
        // Just verify it constructs without panic.
        let _extractor =
            LlmExtractor::new("sk-test-key".to_string(), "claude-sonnet-4-20250514".to_string());
    }

    #[test]
    fn llm_extractor_builder_methods() {
        let extractor = LlmExtractor::new("key".to_string(), "model".to_string())
            .with_max_gleanings(3)
            .with_excluded_types(vec!["metric".to_string()]);

        assert_eq!(extractor.max_gleanings, 3);
        assert_eq!(extractor.excluded_entity_types, vec!["metric"]);
        assert!(extractor.graph_store.is_none());
    }

    // -- parse_extraction tests --

    #[test]
    fn parse_extraction_valid_json() {
        let json = r#"{"entities": [{"name": "Copper", "entity_type": "material", "description": "A metal"}], "relationships": []}"#;
        let result = parse_extraction(json).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "Copper");
    }

    #[test]
    fn parse_extraction_json_in_code_block() {
        let text = r#"Here is the extraction:
```json
{"entities": [{"name": "Teck", "entity_type": "organization", "description": "A mining company"}], "relationships": []}
```
Hope that helps!"#;
        let result = parse_extraction(text).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "Teck");
    }

    #[test]
    fn parse_extraction_json_with_surrounding_text() {
        let text = r#"I found the following entities:
{"entities": [{"name": "Canada", "entity_type": "location", "description": "A country"}], "relationships": [{"source": "Teck", "target": "Canada", "relationship": "related_to", "fact": "Teck operates in Canada", "confidence": "established"}]}
That covers the key entities."#;
        let result = parse_extraction(text).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.relationships.len(), 1);
    }

    #[test]
    fn parse_extraction_invalid_json_returns_error() {
        let text = "This is not JSON at all, just plain text with no braces.";
        assert!(parse_extraction(text).is_err());
    }

    #[test]
    fn parse_extraction_malformed_json_returns_error() {
        let text = r#"{"entities": "not an array"}"#;
        assert!(parse_extraction(text).is_err());
    }

    // -- merge_results tests --

    #[test]
    fn merge_deduplicates_entities_by_name() {
        let mut base = ExtractionResult {
            entities: vec![ExtractedEntity {
                name: "Copper".to_string(),
                entity_type: Some("material".to_string()),
                description: "A metal".to_string(),
            }],
            relationships: vec![],
        };
        let extra = ExtractionResult {
            entities: vec![
                ExtractedEntity {
                    name: "copper".to_string(), // same, different case
                    entity_type: Some("material".to_string()),
                    description: "Duplicate".to_string(),
                },
                ExtractedEntity {
                    name: "Silver".to_string(),
                    entity_type: Some("material".to_string()),
                    description: "Another metal".to_string(),
                },
            ],
            relationships: vec![],
        };
        merge_results(&mut base, extra);
        assert_eq!(base.entities.len(), 2); // Copper + Silver, not duplicate copper
        let names: Vec<&str> = base.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Copper"));
        assert!(names.contains(&"Silver"));
    }

    // -- filter_excluded tests --

    #[test]
    fn filter_excluded_removes_entities_and_dangling_relationships() {
        let extractor = LlmExtractor::new("key".to_string(), "model".to_string())
            .with_excluded_types(vec!["metric".to_string()]);

        let result = ExtractionResult {
            entities: vec![
                ExtractedEntity {
                    name: "Copper".to_string(),
                    entity_type: Some("material".to_string()),
                    description: "A metal".to_string(),
                },
                ExtractedEntity {
                    name: "Price".to_string(),
                    entity_type: Some("metric".to_string()),
                    description: "A metric".to_string(),
                },
            ],
            relationships: vec![
                ExtractedRelationship {
                    source: "Copper".to_string(),
                    target: "Price".to_string(),
                    relationship: "related_to".to_string(),
                    fact: None,
                    confidence: None,
                },
            ],
        };

        let filtered = extractor.filter_excluded(result);
        assert_eq!(filtered.entities.len(), 1);
        assert_eq!(filtered.entities[0].name, "Copper");
        // Relationship referencing "Price" should be removed too.
        assert!(filtered.relationships.is_empty());
    }
}
