/// Neo4j integration tests.
/// Requires NEO4J_TEST_URI, NEO4J_TEST_USER, NEO4J_TEST_PASSWORD env vars.
/// Also requires TEST_DATABASE_URL for the Postgres vector store.
///
/// Run with:
/// NEO4J_TEST_URI=bolt://localhost:7687 NEO4J_TEST_USER=neo4j NEO4J_TEST_PASSWORD=testpassword \
/// TEST_DATABASE_URL=postgresql://test:test@localhost:5433/second_mind_test \
/// cargo test --test neo4j_integration

use second_mind::store::neo4j::Neo4jGraphStore;
use second_mind::store::postgres::PostgresVectorStore;
use second_mind::store::GraphStore;
use second_mind::types::*;
use sqlx::PgPool;

async fn setup_neo4j() -> Option<(Neo4jGraphStore, PgPool)> {
    let uri = match std::env::var("NEO4J_TEST_URI") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("NEO4J_TEST_URI not set, skipping Neo4j tests");
            return None;
        }
    };
    let user = std::env::var("NEO4J_TEST_USER").unwrap_or_else(|_| "neo4j".to_string());
    let password = std::env::var("NEO4J_TEST_PASSWORD").unwrap_or_else(|_| "testpassword".to_string());

    let pg_url = match std::env::var("TEST_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("TEST_DATABASE_URL not set, skipping Neo4j tests");
            return None;
        }
    };

    let store = Neo4jGraphStore::new(&uri, &user, &password).await.ok()?;
    store.initialize().await.ok()?;

    let pool = PgPool::connect(&pg_url).await.ok()?;
    second_mind::store::postgres::PostgresGraphStore::new(pool.clone())
        .initialize()
        .await
        .ok()?;

    Some((store, pool))
}

macro_rules! require_neo4j {
    ($setup:expr) => {
        match $setup {
            Some(s) => s,
            None => return,
        }
    };
}

fn uid() -> String {
    ulid::Ulid::new().to_string()
}

// ==========================================================================
// Schema
// ==========================================================================

#[tokio::test]
async fn test_neo4j_initialize_is_idempotent() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    store.initialize().await.expect("second init should succeed");
}

#[tokio::test]
async fn test_neo4j_ensure_channel() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let name = format!("neo4j-ch-{}", uid());
    let id1 = store.ensure_channel(&name).await.expect("ensure_channel failed");
    let id2 = store.ensure_channel(&name).await.expect("second ensure failed");
    assert_eq!(id1, id2, "same channel name should return same ID");
    assert!(id1 > 0, "channel ID should be positive");
}

// ==========================================================================
// Entities
// ==========================================================================

#[tokio::test]
async fn test_neo4j_insert_and_find_entity() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();
    let name = format!("neo4j-entity-{id}");

    let entity = Entity {
        id: id.clone(),
        canonical_name: name.clone(),
        entity_type: Some("material".to_string()),
        properties: serde_json::json!({"description": "test entity"}),
        embedding: None,
        created_at: chrono::Utc::now(),
    };
    store.insert_entity(&entity).await.expect("insert failed");

    let found = store.find_entity_by_name(&name).await.expect("find failed");
    assert!(found.is_some(), "should find entity by name");
    let found = found.unwrap();
    assert_eq!(found.id, id);
    assert_eq!(found.canonical_name, name);
    assert_eq!(found.entity_type.as_deref(), Some("material"));
}

#[tokio::test]
async fn test_neo4j_find_entity_not_found() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let found = store.find_entity_by_name("nonexistent-entity-xyz").await.expect("find failed");
    assert!(found.is_none());
}

#[tokio::test]
async fn test_neo4j_entity_alias() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();
    let name = format!("neo4j-alias-entity-{id}");
    let alias = format!("neo4j-alias-{id}");

    let entity = Entity {
        id: id.clone(),
        canonical_name: name.clone(),
        entity_type: Some("concept".to_string()),
        properties: serde_json::json!({}),
        embedding: None,
        created_at: chrono::Utc::now(),
    };
    store.insert_entity(&entity).await.expect("insert failed");
    store.insert_entity_alias(&alias, &id, None).await.expect("alias insert failed");

    let found = store.find_entity_by_alias(&alias).await.expect("find by alias failed");
    assert!(found.is_some(), "should find entity by alias");
    assert_eq!(found.unwrap().id, id);
}

// ==========================================================================
// Relationships
// ==========================================================================

#[tokio::test]
async fn test_neo4j_insert_and_get_relationship() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();
    let ch_name = format!("neo4j-rel-ch-{id}");
    let channel_id = store.ensure_channel(&ch_name).await.unwrap();

    // Create two entities
    let e1_id = format!("neo4j-rel-e1-{id}");
    let e2_id = format!("neo4j-rel-e2-{id}");
    for (eid, ename) in [(&e1_id, "copper"), (&e2_id, "data centers")] {
        store.insert_entity(&Entity {
            id: eid.to_string(),
            canonical_name: format!("{ename}-{id}"),
            entity_type: Some("concept".to_string()),
            properties: serde_json::json!({}),
            embedding: None,
            created_at: chrono::Utc::now(),
        }).await.unwrap();
    }

    // Insert document (needed for FK)
    store.insert_document(&format!("doc-{id}"), channel_id, "test", "hash").await.unwrap();

    // Insert relationship
    let rel = Relationship {
        id: format!("rel-{id}"),
        source_id: e1_id.clone(),
        target_id: e2_id.clone(),
        relationship: "supplies".to_string(),
        fact: Some("Copper supplies data center infrastructure".to_string()),
        properties: serde_json::json!({}),
        confidence: Confidence::Established,
        channel_id,
        document_id: format!("doc-{id}"),
        valid_from: chrono::Utc::now(),
        valid_until: None,
        ingested_at: chrono::Utc::now(),
        created_at: chrono::Utc::now(),
        weight: 1.0,
    };
    store.insert_relationship(&rel).await.expect("insert rel failed");

    let rels = store.get_entity_relationships(&e1_id, false).await.expect("get rels failed");
    assert!(!rels.is_empty(), "should have at least one relationship");
    let (r, source_name, target_name) = &rels[0];
    assert_eq!(r.relationship, "supplies");
    assert!(source_name.contains("copper"));
    assert!(target_name.contains("data centers"));
}

// ==========================================================================
// Traversal
// ==========================================================================

#[tokio::test]
async fn test_neo4j_traverse() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();
    let ch_name = format!("neo4j-trav-ch-{id}");
    let channel_id = store.ensure_channel(&ch_name).await.unwrap();
    let doc_id = format!("doc-trav-{id}");
    store.insert_document(&doc_id, channel_id, "test", "hash").await.unwrap();

    // Create chain: A → B → C
    let ids: Vec<String> = (0..3).map(|i| format!("trav-{id}-{i}")).collect();
    let names = ["alpha", "beta", "gamma"];
    for (eid, ename) in ids.iter().zip(names.iter()) {
        store.insert_entity(&Entity {
            id: eid.clone(),
            canonical_name: format!("{ename}-{id}"),
            entity_type: Some("concept".to_string()),
            properties: serde_json::json!({}),
            embedding: None,
            created_at: chrono::Utc::now(),
        }).await.unwrap();
    }

    // A→B, B→C
    for i in 0..2 {
        store.insert_relationship(&Relationship {
            id: format!("trav-rel-{id}-{i}"),
            source_id: ids[i].clone(),
            target_id: ids[i + 1].clone(),
            relationship: "related_to".to_string(),
            fact: None,
            properties: serde_json::json!({}),
            confidence: Confidence::Emerging,
            channel_id,
            document_id: doc_id.clone(),
            valid_from: chrono::Utc::now(),
            valid_until: None,
            ingested_at: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            weight: 1.0,
        }).await.unwrap();
    }

    // Traverse from A, depth 2
    let results = store.traverse(&ids[0], 2, None, false).await.expect("traverse failed");
    assert!(results.len() >= 3, "should find A, B, and C (found {})", results.len());

    // Traverse from A, depth 1
    let results = store.traverse(&ids[0], 1, None, false).await.expect("traverse failed");
    let found_names: Vec<&str> = results.iter().map(|(e, _)| e.canonical_name.as_str()).collect();
    assert!(
        results.iter().any(|(e, _)| e.id == ids[1]),
        "depth 1 should find B, found: {:?}", found_names
    );
}

// ==========================================================================
// Temporal
// ==========================================================================

#[tokio::test]
async fn test_neo4j_temporal_supersede() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();
    let ch_name = format!("neo4j-temp-ch-{id}");
    let channel_id = store.ensure_channel(&ch_name).await.unwrap();
    let doc_id = format!("doc-temp-{id}");
    store.insert_document(&doc_id, channel_id, "test", "hash").await.unwrap();

    let e1 = format!("temp-e1-{id}");
    let e2 = format!("temp-e2-{id}");
    for eid in [&e1, &e2] {
        store.insert_entity(&Entity {
            id: eid.clone(),
            canonical_name: format!("temp-entity-{eid}"),
            entity_type: None,
            properties: serde_json::json!({}),
            embedding: None,
            created_at: chrono::Utc::now(),
        }).await.unwrap();
    }

    let rel_id = format!("temp-rel-{id}");
    store.insert_relationship(&Relationship {
        id: rel_id.clone(),
        source_id: e1.clone(),
        target_id: e2.clone(),
        relationship: "supplies".to_string(),
        fact: Some("old fact".to_string()),
        properties: serde_json::json!({}),
        confidence: Confidence::Established,
        channel_id,
        document_id: doc_id.clone(),
        valid_from: chrono::Utc::now(),
        valid_until: None,
        ingested_at: chrono::Utc::now(),
        created_at: chrono::Utc::now(),
        weight: 1.0,
    }).await.unwrap();

    // Find active — should exist
    let active = store.find_active_relationships(&e1, &e2, "supplies").await.unwrap();
    assert!(!active.is_empty(), "should find active relationship");

    // Supersede
    store.supersede_relationship(&rel_id).await.expect("supersede failed");

    // Find active — should be empty
    let active = store.find_active_relationships(&e1, &e2, "supplies").await.unwrap();
    assert!(active.is_empty(), "should not find superseded relationship");
}

// ==========================================================================
// Full-text search
// ==========================================================================

#[tokio::test]
async fn test_neo4j_fulltext_search() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();
    let name = format!("FulltextCopper{id}");

    store.insert_entity(&Entity {
        id: format!("ft-neo4j-{id}"),
        canonical_name: name.clone(),
        entity_type: Some("material".to_string()),
        properties: serde_json::json!({}),
        embedding: None,
        created_at: chrono::Utc::now(),
    }).await.unwrap();

    // Neo4j fulltext index needs a moment to index
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let results = store.search_entities_fulltext(&name, 10).await.expect("fulltext search failed");
    assert!(
        results.iter().any(|(e, _)| e.canonical_name == name),
        "should find entity by fulltext search, got {} results", results.len()
    );
}

// ==========================================================================
// Documents and chunks
// ==========================================================================

#[tokio::test]
async fn test_neo4j_document_and_chunk() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();
    let ch_name = format!("neo4j-doc-ch-{id}");
    let channel_id = store.ensure_channel(&ch_name).await.unwrap();

    let doc_id = format!("doc-{id}");
    store.insert_document(&doc_id, channel_id, "test.pdf", "abc123").await.unwrap();

    // Duplicate document check
    let found = store.find_document_by_hash(channel_id, "abc123").await.unwrap();
    assert!(found.is_some(), "should find document by hash");
    assert_eq!(found.unwrap(), doc_id);

    // Insert chunk
    let chunk_id = format!("chunk-{id}");
    store.insert_chunk(&chunk_id, &doc_id, "some text content", 0).await.unwrap();

    // Get unprocessed chunks
    let unprocessed = store.get_unprocessed_chunks(channel_id).await.unwrap();
    assert!(
        unprocessed.iter().any(|(cid, _, _)| cid == &chunk_id),
        "should find unprocessed chunk"
    );
}

// ==========================================================================
// Communities
// ==========================================================================

#[tokio::test]
async fn test_neo4j_communities() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();

    let comm = Community {
        id: format!("comm-{id}"),
        level: 0,
        name: Some(format!("test community {id}")),
        summary: None,
        summary_embedding: None,
        parent_id: None,
        created_at: chrono::Utc::now(),
    };
    store.insert_community(&comm).await.expect("insert community failed");

    // Insert member
    let ent_id = format!("comm-ent-{id}");
    store.insert_entity(&Entity {
        id: ent_id.clone(),
        canonical_name: format!("comm-entity-{id}"),
        entity_type: None,
        properties: serde_json::json!({}),
        embedding: None,
        created_at: chrono::Utc::now(),
    }).await.unwrap();
    store.insert_community_member(&comm.id, &ent_id).await.expect("insert member failed");

    let communities = store.list_communities().await.expect("list communities failed");
    assert!(
        communities.iter().any(|(c, count)| c.id == comm.id && *count >= 1),
        "should list community with member count"
    );

    let members = store.get_community_members(&comm.id).await.expect("get members failed");
    assert!(
        members.iter().any(|e| e.id == ent_id),
        "should find entity in community"
    );
}

// ==========================================================================
// Channel operations
// ==========================================================================

#[tokio::test]
async fn test_neo4j_list_channels() {
    let (store, _pool) = require_neo4j!(setup_neo4j().await);
    let id = uid();
    let name = format!("neo4j-list-ch-{id}");
    store.ensure_channel(&name).await.unwrap();

    let channels = store.list_channels().await.expect("list channels failed");
    assert!(
        channels.iter().any(|c| c.name == name),
        "should find channel in list"
    );
}

// ==========================================================================
// Pipeline test: full flow through Neo4j + Postgres vectors
// ==========================================================================

#[tokio::test]
async fn test_neo4j_pipeline_add_and_search() {
    let (neo4j_store, pool) = require_neo4j!(setup_neo4j().await);
    let vector_store = PostgresVectorStore::new(pool.clone());

    let embedder = second_mind::embedder::MockEmbedder { dimensions: 2560 };
    let extractor = second_mind::extractor::MockExtractor;
    let chunker = second_mind::chunker::ChunkerConfig::default();

    let id = uid();
    let ch_name = format!("neo4j-pipeline-{id}");

    // Add document
    let doc_id = second_mind::pipeline::add_document(
        &neo4j_store,
        &vector_store,
        &embedder,
        &chunker,
        &ch_name,
        "Alice works with Bob on important Research Projects.",
        None,
        None,
    )
    .await
    .expect("add_document failed");
    assert!(!doc_id.is_empty());

    // Integrate
    let result = second_mind::pipeline::integrate(
        &neo4j_store,
        &vector_store,
        &embedder,
        &extractor,
        &[ch_name.clone()],
        None,
    )
    .await
    .expect("integrate failed");
    assert!(result.entities_created > 0, "should create entities: {:?}", result);

    // Search
    let search_result = second_mind::pipeline::search(
        &neo4j_store,
        &vector_store,
        &embedder,
        "Alice",
        Some(&[ch_name]),
        None,
        Some(10),
    )
    .await
    .expect("search failed");

    // Should return something (at least fulltext matches)
    let search_str = search_result.to_string();
    assert!(search_str.len() > 10, "search should return non-trivial results");
}
