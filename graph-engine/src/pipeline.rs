use futures::stream::{self, StreamExt};
use tracing::{info, warn};

use crate::chunker::ChunkerConfig;
use crate::embedder::Embedder;
use crate::extractor::Extractor;
use crate::store::{GraphStore, VectorStore};
use crate::types::*;

/// Maximum content size accepted for a single document (512 KB).
const MAX_CONTENT_BYTES: usize = 512 * 1024;

/// Concurrent embedding calls (bounded to avoid overwhelming Ollama).
const EMBED_CONCURRENCY: usize = 4;

/// Concurrent LLM extraction calls (bounded for API rate limits).
const EXTRACT_CONCURRENCY: usize = 2;

/// Add a document: chunk it, embed concurrently, and store everything.
/// Returns the document ID.
pub async fn add_document(
    store: &dyn GraphStore,
    vectors: &dyn VectorStore,
    embedder: &dyn Embedder,
    chunker_config: &ChunkerConfig,
    dataset_name: &str,
    content: &str,
    source_ref: Option<&str>,
    sha256: Option<&str>,
) -> anyhow::Result<String> {
    // --- Validation ---
    anyhow::ensure!(!dataset_name.is_empty(), "dataset_name must not be empty");
    anyhow::ensure!(!content.is_empty(), "content must not be empty");
    anyhow::ensure!(
        content.len() <= MAX_CONTENT_BYTES,
        "content exceeds maximum size of {} bytes",
        MAX_CONTENT_BYTES
    );

    let channel_id = store.ensure_channel(dataset_name).await?;

    let sha = match sha256 {
        Some(s) => s.to_string(),
        None => {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(content.as_bytes()))
        }
    };

    // --- Duplicate check ---
    if let Some(existing_id) = store.find_document_by_hash(channel_id, &sha).await? {
        info!(
            doc_id = %existing_id,
            "duplicate document detected by sha256, returning existing"
        );
        return Ok(existing_id);
    }

    // --- Insert document ---
    let doc_id = ulid::Ulid::new().to_string();
    let source = source_ref.unwrap_or("(inline)");

    store
        .insert_document(&doc_id, channel_id, source, &sha)
        .await?;

    // --- Chunk ---
    let chunks = crate::chunker::chunk_document(content, chunker_config);

    // Insert all chunks first (sequential — needs ordered chunk_index)
    let mut chunk_ids = Vec::with_capacity(chunks.len());
    for (i, chunk_text) in chunks.iter().enumerate() {
        let chunk_id = ulid::Ulid::new().to_string();
        store
            .insert_chunk(&chunk_id, &doc_id, chunk_text, i as i32)
            .await?;
        chunk_ids.push(chunk_id);
    }

    // Embed all chunks concurrently
    let embed_tasks: Vec<_> = chunks
        .iter()
        .zip(chunk_ids.iter())
        .map(|(text, id)| async move {
            match embedder.embed(text).await {
                Ok(embedding) => {
                    if let Err(e) = vectors.set_chunk_embedding(id, &embedding).await {
                        warn!(chunk_id = %id, error = %e, "failed to store chunk embedding");
                    }
                }
                Err(e) => {
                    warn!(chunk_id = %id, error = %e, "failed to embed chunk");
                }
            }
        })
        .collect();

    stream::iter(embed_tasks)
        .buffer_unordered(EMBED_CONCURRENCY)
        .collect::<Vec<()>>()
        .await;

    info!(doc_id = %doc_id, chunks = chunks.len(), "document added");
    Ok(doc_id)
}

/// Integrate: extract entities from chunks concurrently, then resolve and store sequentially.
pub async fn integrate(
    store: &dyn GraphStore,
    vectors: &dyn VectorStore,
    embedder: &dyn Embedder,
    extractor: &dyn Extractor,
    datasets: &[String],
    custom_prompt: Option<&str>,
) -> anyhow::Result<IntegrationResult> {
    anyhow::ensure!(!datasets.is_empty(), "datasets must not be empty");

    // Fast-fail if the store is unreachable
    store.health_check().await?;

    let prompt = custom_prompt.unwrap_or("Extract entities and relationships.");
    let mut result = IntegrationResult {
        entities_created: 0,
        relationships_created: 0,
        chunks_processed: 0,
        chunks_failed: 0,
    };

    for dataset in datasets {
        let channel_id = store.ensure_channel(dataset).await?;
        let chunks = store.get_unprocessed_chunks(channel_id).await?;

        if chunks.is_empty() {
            continue;
        }

        info!(dataset = %dataset, chunks = chunks.len(), "starting extraction");

        // Phase 1: Extract all chunks concurrently (LLM calls)
        let extraction_tasks: Vec<_> = chunks
            .iter()
            .map(|(chunk_id, chunk_content, document_id)| {
                let chunk_id = chunk_id.clone();
                let content = chunk_content.clone();
                let doc_id = document_id.clone();
                let prompt = prompt.to_string();
                async move {
                    let extraction = extractor.extract(&content, &prompt).await;
                    (chunk_id, doc_id, extraction)
                }
            })
            .collect();

        let extractions: Vec<_> = stream::iter(extraction_tasks)
            .buffer_unordered(EXTRACT_CONCURRENCY)
            .collect()
            .await;

        // Phase 2: Resolve and store sequentially (graph writes need ordering)
        for (chunk_id, document_id, extraction) in extractions {
            let extraction = match extraction {
                Ok(r) => r,
                Err(e) => {
                    warn!(chunk_id = %chunk_id, error = %e, "extraction failed, chunk will be retried on next integrate call");
                    result.chunks_failed += 1;
                    continue;
                }
            };

            // --- Entities ---
            for extracted in &extraction.entities {
                process_entity(store, vectors, embedder, extracted, channel_id, &document_id, &chunk_id).await?;
                result.entities_created += 1;
            }

            // --- Relationships ---
            for rel in &extraction.relationships {
                if process_relationship(store, rel, channel_id, &document_id).await? {
                    result.relationships_created += 1;
                }
            }

            result.chunks_processed += 1;
        }
    }

    info!(
        entities = result.entities_created,
        relationships = result.relationships_created,
        chunks = result.chunks_processed,
        failed = result.chunks_failed,
        "integration complete"
    );

    Ok(result)
}

/// Resolve and store a single extracted entity: insert or enrich, embed, and link to channel/chunk.
async fn process_entity(
    store: &dyn GraphStore,
    vectors: &dyn VectorStore,
    embedder: &dyn Embedder,
    extracted: &ExtractedEntity,
    channel_id: i32,
    document_id: &str,
    chunk_id: &str,
) -> anyhow::Result<()> {
    let resolved = crate::resolver::resolve(store, extracted).await?;

    if resolved.is_new {
        let entity = Entity {
            id: resolved.entity_id.clone(),
            canonical_name: resolved.canonical_name.clone(),
            entity_type: extracted.entity_type.clone(),
            properties: serde_json::json!({"description": extracted.description}),
            embedding: None,
            created_at: chrono::Utc::now(),
        };
        if let Err(e) = store.insert_entity(&entity).await {
            warn!(entity_id = %entity.id, error = %e, "failed to insert entity");
        }

        // Embed entity
        let embed_text = format!("{}: {}", entity.canonical_name, extracted.description);
        match embedder.embed(&embed_text).await {
            Ok(emb) => {
                if let Err(e) = vectors.set_entity_embedding(&entity.id, &emb).await {
                    warn!(entity_id = %entity.id, error = %e, "failed to store entity embedding");
                }
            }
            Err(e) => {
                warn!(entity_id = %entity.id, error = %e, "failed to embed entity");
            }
        }
    } else if !extracted.description.is_empty() {
        // Enrich description if new one is longer
        if let Ok(Some(existing)) = store.find_entity_by_name(&resolved.canonical_name).await {
            let existing_desc = existing
                .properties
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            if extracted.description.len() > existing_desc.len() {
                let _ = store
                    .update_entity_description(&resolved.entity_id, &extracted.description)
                    .await;
            }
        }
    }

    let _ = store
        .insert_entity_channel(&resolved.entity_id, channel_id, document_id)
        .await;
    let _ = store
        .insert_entity_chunk(&resolved.entity_id, chunk_id)
        .await;

    Ok(())
}

/// Resolve and store a single extracted relationship, handling contradiction detection.
/// Returns `true` if a new relationship was created, `false` if skipped or incremented.
async fn process_relationship(
    store: &dyn GraphStore,
    rel: &ExtractedRelationship,
    channel_id: i32,
    document_id: &str,
) -> anyhow::Result<bool> {
    let source = crate::resolver::resolve(
        store,
        &ExtractedEntity {
            name: rel.source.clone(),
            entity_type: None,
            description: String::new(),
        },
    )
    .await?;

    let target = crate::resolver::resolve(
        store,
        &ExtractedEntity {
            name: rel.target.clone(),
            entity_type: None,
            description: String::new(),
        },
    )
    .await?;

    let now = chrono::Utc::now();

    // Contradiction detection
    match crate::temporal::check_contradiction(
        store,
        &source.entity_id,
        &target.entity_id,
        &rel.relationship,
        rel.fact.as_deref(),
    )
    .await?
    {
        crate::temporal::ContradictionResult::Identical(existing_id) => {
            if let Err(e) = store.increment_relationship_weight(&existing_id).await {
                warn!(
                    rel_id = %existing_id,
                    error = %e,
                    "failed to increment relationship weight"
                );
            }
            return Ok(false);
        }
        crate::temporal::ContradictionResult::Novel => {}
        crate::temporal::ContradictionResult::PotentialContradiction {
            existing_id,
            existing_fact,
        } => {
            info!(
                "contradiction detected: existing={:?}, new={:?}",
                existing_fact, rel.fact
            );
            crate::temporal::supersede(store, &existing_id).await?;
        }
    }

    let relationship = Relationship {
        id: ulid::Ulid::new().to_string(),
        source_id: source.entity_id,
        target_id: target.entity_id,
        relationship: rel.relationship.clone(),
        fact: rel.fact.clone(),
        properties: serde_json::json!({}),
        confidence: rel
            .confidence
            .as_deref()
            .and_then(|c| c.parse().ok())
            .unwrap_or(Confidence::Established),
        channel_id,
        document_id: document_id.to_string(),
        valid_from: now,
        valid_until: None,
        ingested_at: now,
        created_at: now,
        weight: 1.0,
    };

    if let Err(e) = store.insert_relationship(&relationship).await {
        warn!(rel_id = %relationship.id, error = %e, "failed to insert relationship");
        Ok(false)
    } else {
        Ok(true)
    }
}

/// Search the knowledge graph.
///
/// Search types:
/// - `KEYWORD` — full-text search on entity names only. No embeddings, no graph traversal. Fast.
/// - `CHUNKS` — vector similarity on document chunks.
/// - `SIMILARITY` — vector similarity on entities.
/// - `GRAPH_COMPLETION` — vector entity search + relationship traversal + rich paths.
/// - `SUMMARIES` — same as CHUNKS (placeholder for future community summaries).
/// - `None` — hybrid: all signals combined.
pub async fn search(
    store: &dyn GraphStore,
    vectors: &dyn VectorStore,
    embedder: &dyn Embedder,
    query: &str,
    datasets: Option<&[String]>,
    search_type: Option<&str>,
    top_k: Option<i32>,
) -> anyhow::Result<serde_json::Value> {
    anyhow::ensure!(!query.is_empty(), "query must not be empty");

    let top_k = top_k.unwrap_or(10).clamp(1, 100);
    let keyword_only = matches!(search_type, Some("KEYWORD"));

    // Resolve channel IDs
    let channel_ids = if let Some(names) = datasets {
        let mut ids = Vec::new();
        for name in names {
            if let Ok(id) = store.ensure_channel(name).await {
                ids.push(id);
            }
        }
        Some(ids)
    } else {
        None
    };

    // KEYWORD mode: fulltext only, skip embeddings entirely
    if keyword_only {
        let fulltext = store
            .search_entities_fulltext(query, top_k)
            .await
            .unwrap_or_default();

        let dataset_label = datasets
            .map(|d| d.join(", "))
            .unwrap_or_else(|| "all".to_string());

        return Ok(serde_json::json!([{
            "dataset_name": dataset_label,
            "search_result": [],
            "entities": [],
            "fulltext_matches": fulltext.iter().map(|(e, score)| serde_json::json!({
                "name": e.canonical_name,
                "type": e.entity_type,
                "score": score,
            })).collect::<Vec<_>>(),
            "relationships": [],
            "paths": [],
        }]));
    }

    let query_embedding = embedder.embed(query).await?;

    // Determine what to search based on search_type
    let search_chunks = matches!(search_type, None | Some("CHUNKS") | Some("SUMMARIES"));
    let search_entities = matches!(
        search_type,
        None | Some("SIMILARITY") | Some("GRAPH_COMPLETION")
    );
    let include_relationships = matches!(search_type, None | Some("GRAPH_COMPLETION"));

    // Run chunk search, entity search, and fulltext in parallel
    let (chunk_results, entity_results, fulltext_entities) = tokio::join!(
        async {
            if search_chunks {
                vectors
                    .search_chunks(&query_embedding, channel_ids.as_deref(), top_k)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        },
        async {
            if search_entities {
                vectors
                    .search_entities(&query_embedding, channel_ids.as_deref(), top_k)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        },
        async {
            store
                .search_entities_fulltext(query, top_k)
                .await
                .unwrap_or_default()
        }
    );

    let mut all_relationships = Vec::new();
    if include_relationships {
        for er in entity_results.iter().take(5) {
            if let Ok(rels) = store
                .get_entity_relationships(&er.entity.id, false)
                .await
            {
                for (rel, source_name, target_name) in rels {
                    all_relationships.push(serde_json::json!({
                        "source": source_name,
                        "target": target_name,
                        "relationship": rel.relationship,
                        "fact": rel.fact,
                        "confidence": rel.confidence.to_string(),
                        "weight": rel.weight,
                        "valid_from": rel.valid_from,
                        "valid_until": rel.valid_until,
                    }));
                }
            }
        }
    }

    // --- Rich traversal from top entities ---
    let mut all_paths = Vec::new();
    if include_relationships {
        let mut top_entity_ids: Vec<String> = entity_results
            .iter()
            .take(3)
            .map(|er| er.entity.id.clone())
            .collect();
        for (ft_entity, _score) in fulltext_entities.iter().take(2) {
            if !top_entity_ids.contains(&ft_entity.id) {
                top_entity_ids.push(ft_entity.id.clone());
            }
        }

        for entity_id in top_entity_ids.iter().take(5) {
            if let Ok(paths) = store
                .traverse_rich(entity_id, 2, channel_ids.as_deref(), false)
                .await
            {
                for path in paths {
                    all_paths.push(serde_json::json!({
                        "depth": path.depth,
                        "entities": path.entities.iter().map(|e| serde_json::json!({
                            "name": e.canonical_name,
                            "type": e.entity_type,
                        })).collect::<Vec<_>>(),
                        "relationships": path.relationships.iter().map(|r| serde_json::json!({
                            "source": r.source_name,
                            "target": r.target_name,
                            "relationship": r.relationship,
                            "fact": r.fact,
                            "confidence": r.confidence,
                        })).collect::<Vec<_>>(),
                    }));
                }
            }
        }
    }

    let dataset_label = datasets
        .map(|d| d.join(", "))
        .unwrap_or_else(|| "all".to_string());

    let response = serde_json::json!([{
        "dataset_name": dataset_label,
        "search_result": chunk_results.iter().map(|cr| serde_json::json!({
            "text": cr.chunk.content,
            "distance": cr.distance,
            "source": cr.document_source,
            "channel": cr.channel,
        })).collect::<Vec<_>>(),
        "entities": entity_results.iter().map(|er| serde_json::json!({
            "name": er.entity.canonical_name,
            "type": er.entity.entity_type,
            "relevance": er.relevance,
            "channels": er.channels,
        })).collect::<Vec<_>>(),
        "relationships": all_relationships,
        "paths": all_paths,
        "fulltext_matches": fulltext_entities.iter().map(|(entity, score)| serde_json::json!({
            "name": entity.canonical_name,
            "type": entity.entity_type,
            "score": score,
        })).collect::<Vec<_>>(),
    }]);

    Ok(response)
}

/// Delete a channel by name and all data scoped to it.
pub async fn delete_channel(store: &dyn GraphStore, channel_name: &str) -> anyhow::Result<()> {
    store.delete_channel(channel_name).await
}

/// Delete all channels and their data.
pub async fn delete_all_channels(store: &dyn GraphStore) -> anyhow::Result<()> {
    store.delete_all_channels().await
}
