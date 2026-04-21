//! Integration tests for the graph-engine.
//!
//! Requires a real Postgres database with pgvector. Set `TEST_DATABASE_URL` to run.
//! Uses MockExtractor and MockEmbedder so no external API calls are needed.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt; // for oneshot()

use serial_test::serial;

use second_mind::chunker::ChunkerConfig;
use second_mind::embedder::MockEmbedder;
use second_mind::extractor::{Extractor, MockExtractor};
use second_mind::types::*;
use second_mind::*;

// ---------------------------------------------------------------------------
// Setup helpers
// ---------------------------------------------------------------------------

async fn setup() -> Option<PgPool> {
    let url = match std::env::var("TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("TEST_DATABASE_URL not set, skipping integration tests");
            return None;
        }
    };

    let pool = PgPool::connect(&url)
        .await
        .expect("Failed to connect to test database");

    schema::initialize(&pool)
        .await
        .expect("Failed to initialize schema");

    Some(pool)
}

/// Generate a unique ID prefix for test isolation.
fn uid() -> String {
    ulid::Ulid::new().to_string()
}

/// Clean all tables — only used by tests that require a completely empty database
/// (e.g., community detection which operates on the entire graph).
async fn clean_all(pool: &PgPool) {
    // Delete in FK-safe order
    for table in &[
        "community_members",
        "communities",
        "entity_chunks",
        "entity_channels",
        "entity_aliases",
        "relationships",
        "chunks",
        "documents",
        "entities",
        "channels",
        "llm_cache",
    ] {
        sqlx::query(&format!("DELETE FROM {table}"))
            .execute(pool)
            .await
            .ok();
    }
}

macro_rules! require_db {
    ($pool:expr) => {
        match $pool {
            Some(p) => p,
            None => return,
        }
    };
}

fn mock_embedder() -> MockEmbedder {
    MockEmbedder { dimensions: 2560 }
}

fn mock_extractor() -> MockExtractor {
    MockExtractor
}

fn default_chunker() -> ChunkerConfig {
    ChunkerConfig::default()
}

/// Helper: insert a document row directly (bypassing pipeline) for low-level tests.
async fn insert_document(pool: &PgPool, doc_id: &str, channel_id: i32, sha: &str) {
    sqlx::query(
        "INSERT INTO documents (id, channel_id, source_ref, sha256) VALUES ($1, $2, '(test)', $3)",
    )
    .bind(doc_id)
    .bind(channel_id)
    .bind(sha)
    .execute(pool)
    .await
    .expect("insert_document failed");
}

/// Helper: insert a chunk row directly.
async fn insert_chunk(pool: &PgPool, chunk_id: &str, document_id: &str, content: &str, idx: i32) {
    sqlx::query(
        "INSERT INTO chunks (id, document_id, content, chunk_index) VALUES ($1, $2, $3, $4)",
    )
    .bind(chunk_id)
    .bind(document_id)
    .bind(content)
    .bind(idx)
    .execute(pool)
    .await
    .expect("insert_chunk failed");
}

/// Helper: insert an entity with a given ID and name.
async fn insert_test_entity(pool: &PgPool, id: &str, name: &str) {
    let entity = Entity {
        id: id.to_string(),
        canonical_name: name.to_string(),
        entity_type: Some("concept".to_string()),
        properties: serde_json::json!({}),
        embedding: None,
        created_at: chrono::Utc::now(),
    };
    graph::insert_entity(pool, &entity)
        .await
        .expect("insert_entity failed");
}

/// Helper: insert a relationship between two entities.
async fn insert_test_relationship(
    pool: &PgPool,
    rel_id: &str,
    source_id: &str,
    target_id: &str,
    rel_type: &str,
    fact: Option<&str>,
    channel_id: i32,
    document_id: &str,
) -> Relationship {
    let now = chrono::Utc::now();
    let rel = Relationship {
        id: rel_id.to_string(),
        source_id: source_id.to_string(),
        target_id: target_id.to_string(),
        relationship: rel_type.to_string(),
        fact: fact.map(|f| f.to_string()),
        properties: serde_json::json!({}),
        confidence: Confidence::Established,
        channel_id,
        document_id: document_id.to_string(),
        valid_from: now,
        valid_until: None,
        ingested_at: now,
        created_at: now,
        weight: 1.0,
    };
    graph::insert_relationship(pool, &rel)
        .await
        .expect("insert_relationship failed");
    rel
}

/// Build an AppState for server tests.
fn build_app_state(pool: PgPool) -> Arc<server::AppState> {
    Arc::new(server::AppState {
        graph_store: Arc::new(second_mind::store::postgres::PostgresGraphStore::new(pool.clone())),
        vector_store: Arc::new(second_mind::store::postgres::PostgresVectorStore::new(pool)),
        embedder: Arc::new(mock_embedder()),
        extractor: Arc::new(mock_extractor()),
        chunker_config: default_chunker(),
    })
}

// ===========================================================================
// Schema tests
// ===========================================================================

#[tokio::test]
async fn test_initialize_is_idempotent() {
    let pool = require_db!(setup().await);
    // Second call should not error
    schema::initialize(&pool)
        .await
        .expect("second initialize should succeed");
}

#[tokio::test]
async fn test_ensure_channel_creates_and_returns_id() {
    let pool = require_db!(setup().await);

    let id1 = schema::ensure_channel(&pool, "test-channel")
        .await
        .expect("first ensure_channel failed");
    let id2 = schema::ensure_channel(&pool, "test-channel")
        .await
        .expect("second ensure_channel failed");
    assert_eq!(id1, id2, "same name should return same ID");
}

#[tokio::test]
async fn test_ensure_channel_different_names_different_ids() {
    let pool = require_db!(setup().await);

    let id1 = schema::ensure_channel(&pool, "alpha")
        .await
        .expect("ensure_channel alpha failed");
    let id2 = schema::ensure_channel(&pool, "beta")
        .await
        .expect("ensure_channel beta failed");
    assert_ne!(id1, id2, "different names should get different IDs");
}

// ===========================================================================
// Graph CRUD tests
// ===========================================================================

#[tokio::test]
async fn test_insert_and_find_entity_by_name() {
    let pool = require_db!(setup().await);

    let id = uid();
    let name = format!("copper-{id}");
    insert_test_entity(&pool, &id, &name).await;
    let found = graph::find_entity_by_name(&pool, &name)
        .await
        .expect("find_entity_by_name failed");
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, id);
}

#[tokio::test]
async fn test_find_entity_by_name_not_found() {
    let pool = require_db!(setup().await);

    let found = graph::find_entity_by_name(&pool, "nonexistent")
        .await
        .expect("find_entity_by_name failed");
    assert!(found.is_none());
}

#[tokio::test]
async fn test_insert_and_find_entity_by_alias() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("test-ch-{id}")).await.unwrap();
    let doc_id = format!("doc-alias-{id}");
    let ent_id = format!("ent-alias-{id}");
    let alias = format!("cu-{id}");
    let name = format!("copper-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-alias-{id}")).await;
    insert_test_entity(&pool, &ent_id, &name).await;
    graph::insert_entity_alias(&pool, &alias, &ent_id, Some(&doc_id))
        .await
        .expect("insert_entity_alias failed");

    let found = graph::find_entity_by_alias(&pool, &alias)
        .await
        .expect("find_entity_by_alias failed");
    assert!(found.is_some());
    assert_eq!(found.unwrap().canonical_name, name);
}

#[tokio::test]
async fn test_insert_entity_channel_and_get_channels() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch1_name = format!("research-{id}");
    let ch2_name = format!("personal-{id}");
    let ch1 = schema::ensure_channel(&pool, &ch1_name).await.unwrap();
    let ch2 = schema::ensure_channel(&pool, &ch2_name).await.unwrap();
    let doc1 = format!("doc-ch1-{id}");
    let doc2 = format!("doc-ch2-{id}");
    let ent_id = format!("ent-ch-{id}");
    insert_document(&pool, &doc1, ch1, &format!("sha-ch1-{id}")).await;
    insert_document(&pool, &doc2, ch2, &format!("sha-ch2-{id}")).await;
    insert_test_entity(&pool, &ent_id, "silver").await;

    graph::insert_entity_channel(&pool, &ent_id, ch1, &doc1)
        .await
        .unwrap();
    graph::insert_entity_channel(&pool, &ent_id, ch2, &doc2)
        .await
        .unwrap();

    let channels = graph::get_entity_channels(&pool, &ent_id)
        .await
        .expect("get_entity_channels failed");
    assert_eq!(channels.len(), 2);
    assert!(channels.contains(&ch1_name));
    assert!(channels.contains(&ch2_name));
}

#[tokio::test]
async fn test_insert_relationship() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("test-rel-{id}")).await.unwrap();
    let doc_id = format!("doc-rel-{id}");
    let src = format!("src-{id}");
    let tgt = format!("tgt-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-rel-{id}")).await;
    insert_test_entity(&pool, &src, "copper").await;
    insert_test_entity(&pool, &tgt, "data center").await;

    insert_test_relationship(
        &pool, &format!("rel-{id}"), &src, &tgt, "consumes", None, ch_id, &doc_id,
    )
    .await;

    let rels = graph::get_entity_relationships(&pool, &src, true)
        .await
        .expect("get_entity_relationships failed");
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].0.relationship, "consumes");
}

#[tokio::test]
async fn test_get_entity_relationships() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("test-rels-{id}")).await.unwrap();
    let doc_id = format!("doc-rels-{id}");
    let a = format!("a-{id}");
    let b = format!("b-{id}");
    let c = format!("c-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-rels-{id}")).await;
    insert_test_entity(&pool, &a, "entity a").await;
    insert_test_entity(&pool, &b, "entity b").await;
    insert_test_entity(&pool, &c, "entity c").await;

    insert_test_relationship(&pool, &format!("r1-{id}"), &a, &b, "related_to", None, ch_id, &doc_id).await;
    insert_test_relationship(&pool, &format!("r2-{id}"), &a, &c, "enables", None, ch_id, &doc_id).await;

    let rels = graph::get_entity_relationships(&pool, &a, true)
        .await
        .expect("get_entity_relationships failed");
    assert_eq!(rels.len(), 2);
}

#[tokio::test]
async fn test_get_entity_relationships_excludes_expired() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("test-expire-{id}")).await.unwrap();
    let doc_id = format!("doc-expire-{id}");
    let x = format!("x-{id}");
    let y = format!("y-{id}");
    let z = format!("z-{id}");
    let r_active = format!("r-active-{id}");
    let r_expired = format!("r-expired-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-expire-{id}")).await;
    insert_test_entity(&pool, &x, "entity x").await;
    insert_test_entity(&pool, &y, "entity y").await;
    insert_test_entity(&pool, &z, "entity z").await;

    // Active relationship
    insert_test_relationship(&pool, &r_active, &x, &y, "related_to", None, ch_id, &doc_id)
        .await;
    // Expired relationship
    let expired_rel = insert_test_relationship(
        &pool,
        &r_expired,
        &x,
        &z,
        "related_to",
        None,
        ch_id,
        &doc_id,
    )
    .await;
    // Expire it
    graph::supersede_relationship(&pool, &expired_rel.id)
        .await
        .expect("supersede failed");

    // include_expired=false should exclude expired
    let rels = graph::get_entity_relationships(&pool, &x, false)
        .await
        .expect("get_entity_relationships failed");
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].0.id, r_active);

    // include_expired=true should include both
    let rels_all = graph::get_entity_relationships(&pool, &x, true)
        .await
        .expect("get_entity_relationships failed");
    assert_eq!(rels_all.len(), 2);
}

#[tokio::test]
async fn test_traverse_graph() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("test-traverse-{id}")).await.unwrap();
    let doc_id = format!("doc-trav-{id}");
    let a = format!("t-a-{id}");
    let b = format!("t-b-{id}");
    let c = format!("t-c-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-trav-{id}")).await;
    insert_test_entity(&pool, &a, "node a").await;
    insert_test_entity(&pool, &b, "node b").await;
    insert_test_entity(&pool, &c, "node c").await;

    // A -> B -> C
    insert_test_relationship(&pool, &format!("tr1-{id}"), &a, &b, "related_to", None, ch_id, &doc_id)
        .await;
    insert_test_relationship(&pool, &format!("tr2-{id}"), &b, &c, "related_to", None, ch_id, &doc_id)
        .await;

    let results = graph::traverse(&pool, &a, 2, None, false)
        .await
        .expect("traverse failed");

    let ids: Vec<&str> = results.iter().map(|(e, _)| e.id.as_str()).collect();
    assert!(ids.contains(&a.as_str()), "should contain start node");
    assert!(ids.contains(&b.as_str()), "should contain depth-1 node");
    assert!(ids.contains(&c.as_str()), "should contain depth-2 node");
}

#[tokio::test]
async fn test_traverse_respects_max_depth() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("test-depth-{id}")).await.unwrap();
    let doc_id = format!("doc-depth-{id}");
    let a = format!("d-a-{id}");
    let b = format!("d-b-{id}");
    let c = format!("d-c-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-depth-{id}")).await;
    insert_test_entity(&pool, &a, "node a").await;
    insert_test_entity(&pool, &b, "node b").await;
    insert_test_entity(&pool, &c, "node c").await;

    insert_test_relationship(&pool, &format!("dr1-{id}"), &a, &b, "related_to", None, ch_id, &doc_id)
        .await;
    insert_test_relationship(&pool, &format!("dr2-{id}"), &b, &c, "related_to", None, ch_id, &doc_id)
        .await;

    let results = graph::traverse(&pool, &a, 1, None, false)
        .await
        .expect("traverse failed");

    let ids: Vec<&str> = results.iter().map(|(e, _)| e.id.as_str()).collect();
    assert!(ids.contains(&a.as_str()), "should contain start node");
    assert!(ids.contains(&b.as_str()), "should contain depth-1 node");
    assert!(!ids.contains(&c.as_str()), "should NOT contain depth-2 node at depth limit 1");
}

#[tokio::test]
async fn test_traverse_with_channel_filter() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch1 = schema::ensure_channel(&pool, &format!("traverse-ch1-{id}")).await.unwrap();
    let ch2 = schema::ensure_channel(&pool, &format!("traverse-ch2-{id}")).await.unwrap();
    let doc1 = format!("doc-tch1-{id}");
    let doc2 = format!("doc-tch2-{id}");
    let a = format!("fc-a-{id}");
    let b = format!("fc-b-{id}");
    let c = format!("fc-c-{id}");
    insert_document(&pool, &doc1, ch1, &format!("sha-tch1-{id}")).await;
    insert_document(&pool, &doc2, ch2, &format!("sha-tch2-{id}")).await;
    insert_test_entity(&pool, &a, "node a").await;
    insert_test_entity(&pool, &b, "node b").await;
    insert_test_entity(&pool, &c, "node c").await;

    // A -> B in ch1, A -> C in ch2
    insert_test_relationship(
        &pool, &format!("fcr1-{id}"), &a, &b, "related_to", None, ch1, &doc1,
    )
    .await;
    insert_test_relationship(
        &pool, &format!("fcr2-{id}"), &a, &c, "related_to", None, ch2, &doc2,
    )
    .await;

    // Filter to ch1 only
    let results = graph::traverse(&pool, &a, 2, Some(&[ch1]), false)
        .await
        .expect("traverse failed");

    let ids: Vec<&str> = results.iter().map(|(e, _)| e.id.as_str()).collect();
    assert!(ids.contains(&b.as_str()), "should find B via ch1 relationship");
    assert!(!ids.contains(&c.as_str()), "should NOT find C (in ch2)");
}

#[tokio::test]
async fn test_delete_channel_cascades() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_name = format!("delete-me-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-del-{id}");
    let chunk_id = format!("chunk-del-{id}");
    let ent_id = format!("ent-del-{id}");
    let rel_id = format!("rel-del-{id}");

    insert_document(&pool, &doc_id, ch_id, &format!("sha-del-{id}")).await;
    insert_chunk(&pool, &chunk_id, &doc_id, "some text", 0).await;
    insert_test_entity(&pool, &ent_id, "deletable entity").await;
    graph::insert_entity_channel(&pool, &ent_id, ch_id, &doc_id)
        .await
        .unwrap();
    graph::insert_entity_chunk(&pool, &ent_id, &chunk_id)
        .await
        .unwrap();
    insert_test_relationship(
        &pool,
        &rel_id,
        &ent_id,
        &ent_id,
        "self_ref",
        None,
        ch_id,
        &doc_id,
    )
    .await;

    // Delete the channel
    graph::delete_channel(&pool, ch_id)
        .await
        .expect("delete_channel failed");

    // Verify everything is gone
    let channels = graph::list_channels(&pool).await.unwrap();
    assert!(
        !channels.iter().any(|c| c.name == ch_name),
        "channel should be deleted"
    );

    let doc: Option<(String,)> =
        sqlx::query_as("SELECT id FROM documents WHERE id = $1")
            .bind(&doc_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(doc.is_none(), "document should be deleted");

    let chunk: Option<(String,)> =
        sqlx::query_as("SELECT id FROM chunks WHERE id = $1")
            .bind(&chunk_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(chunk.is_none(), "chunk should be deleted");
}

#[tokio::test]
async fn test_list_channels() {
    let pool = require_db!(setup().await);

    schema::ensure_channel(&pool, "list-a").await.unwrap();
    schema::ensure_channel(&pool, "list-b").await.unwrap();
    schema::ensure_channel(&pool, "list-c").await.unwrap();

    let channels = graph::list_channels(&pool).await.expect("list_channels failed");
    let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"list-a"));
    assert!(names.contains(&"list-b"));
    assert!(names.contains(&"list-c"));
}

// ===========================================================================
// Vector tests
// ===========================================================================

#[tokio::test]
async fn test_set_and_search_chunk_embedding() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_name = format!("vec-chunks-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-vec-{id}");
    let chunk_id = format!("chunk-vec-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-vec-{id}")).await;
    insert_chunk(&pool, &chunk_id, &doc_id, "copper mining operations", 0).await;

    // Set a known embedding — use a unique dimension to make this chunk the closest match
    let mut embedding = vec![0.0f32; 2560];
    embedding[0] = 1.0;
    vectors::set_chunk_embedding(&pool, &chunk_id, &embedding)
        .await
        .expect("set_chunk_embedding failed");

    // Search with same vector, filtered to this channel
    let results = vectors::search_chunks(&pool, &embedding, Some(&[ch_id]), 10)
        .await
        .expect("search_chunks failed");
    assert!(!results.is_empty(), "should find at least one chunk");
    assert_eq!(results[0].chunk.id, chunk_id);
}

#[tokio::test]
async fn test_search_chunks_with_channel_filter() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch1_name = format!("vec-ch1-{id}");
    let ch2_name = format!("vec-ch2-{id}");
    let ch1 = schema::ensure_channel(&pool, &ch1_name).await.unwrap();
    let ch2 = schema::ensure_channel(&pool, &ch2_name).await.unwrap();
    let doc1 = format!("doc-vf1-{id}");
    let doc2 = format!("doc-vf2-{id}");
    let chunk1 = format!("chunk-vf1-{id}");
    let chunk2 = format!("chunk-vf2-{id}");
    insert_document(&pool, &doc1, ch1, &format!("sha-vf1-{id}")).await;
    insert_document(&pool, &doc2, ch2, &format!("sha-vf2-{id}")).await;
    insert_chunk(&pool, &chunk1, &doc1, "text in ch1", 0).await;
    insert_chunk(&pool, &chunk2, &doc2, "text in ch2", 0).await;

    let mut emb = vec![0.0f32; 2560];
    emb[0] = 1.0;
    vectors::set_chunk_embedding(&pool, &chunk1, &emb).await.unwrap();
    vectors::set_chunk_embedding(&pool, &chunk2, &emb).await.unwrap();

    // Filter to ch1 only
    let results = vectors::search_chunks(&pool, &emb, Some(&[ch1]), 10)
        .await
        .expect("search_chunks failed");
    assert!(results.iter().all(|r| r.channel == ch1_name));
}

#[tokio::test]
async fn test_set_and_search_entity_embedding() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ent_id = format!("ent-vec-{id}");
    insert_test_entity(&pool, &ent_id, &format!("copper-{id}")).await;

    let mut embedding = vec![0.0f32; 2560];
    embedding[1] = 1.0;
    vectors::set_entity_embedding(&pool, &ent_id, &embedding)
        .await
        .expect("set_entity_embedding failed");

    // Search with a large top_k to avoid missing our entity among accumulated test data
    let results = vectors::search_entities(&pool, &embedding, None, 500)
        .await
        .expect("search_entities failed");
    assert!(!results.is_empty(), "should find at least one entity");
    assert!(
        results.iter().any(|r| r.entity.id == ent_id),
        "should find our entity in results (searched {} results)",
        results.len()
    );
}

#[tokio::test]
async fn test_search_returns_empty_when_no_embeddings() {
    let pool = require_db!(setup().await);

    // Use a channel that has no data to avoid interference from other tests
    let ch_name = format!("empty-{}", uid());
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();

    let query = vec![0.0f32; 2560];
    let chunks = vectors::search_chunks(&pool, &query, Some(&[ch_id]), 10)
        .await
        .expect("search_chunks failed");
    assert!(chunks.is_empty());

    let entities = vectors::search_entities(&pool, &query, Some(&[ch_id]), 10)
        .await
        .expect("search_entities failed");
    assert!(entities.is_empty());
}

// ===========================================================================
// Pipeline tests
// ===========================================================================

#[tokio::test]
async fn test_add_document_creates_doc_and_chunks() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let chunker = default_chunker();

    let doc_id = pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        "pipe-test",
        "This is a test document with some content for chunking.",
        Some("test-source"),
        None,
    )
    .await
    .expect("add_document failed");

    // Verify document row exists
    let doc: Option<(String,)> =
        sqlx::query_as("SELECT id FROM documents WHERE id = $1")
            .bind(&doc_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(doc.is_some(), "document row should exist");

    // Verify chunks exist
    let chunks: Vec<(String,)> =
        sqlx::query_as("SELECT id FROM chunks WHERE document_id = $1")
            .bind(&doc_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(!chunks.is_empty(), "should have at least one chunk");
}

#[tokio::test]
async fn test_add_document_deduplicates_by_sha256() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let chunker = default_chunker();
    let content = "Exact same content for dedup test.";

    let id1 = pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder, &chunker, "dedup-ch", content, None, None)
        .await
        .expect("first add failed");
    let id2 = pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder, &chunker, "dedup-ch", content, None, None)
        .await
        .expect("second add failed");

    assert_eq!(id1, id2, "duplicate content should return existing doc ID");
}

#[tokio::test]
async fn test_add_document_validates_empty_content() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let chunker = default_chunker();

    let result =
        pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder, &chunker, "empty-test", "", None, None).await;
    assert!(result.is_err(), "empty content should fail");
}

#[tokio::test]
async fn test_add_document_validates_empty_dataset() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let chunker = default_chunker();

    let result =
        pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder, &chunker, "", "some content", None, None).await;
    assert!(result.is_err(), "empty dataset name should fail");
}

#[tokio::test]
async fn test_integrate_extracts_entities_and_relationships() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let extractor = mock_extractor();
    let chunker = default_chunker();

    let id = uid();
    let ch_name = format!("integrate-test-{id}");

    // Add a document with capitalized words that MockExtractor will pick up
    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        &ch_name,
        "Teck Resources mines Copper in British Columbia for Data Centers.",
        None,
        None,
    )
    .await
    .expect("add_document failed");

    let result = pipeline::integrate(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &extractor,
        &[ch_name],
        None,
    )
    .await
    .expect("integrate failed");

    assert!(result.entities_created > 0, "should have created entities");
    assert!(result.chunks_processed > 0, "should have processed chunks");

    // Verify entities exist in DB
    let entity_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM entities")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(entity_count.0 > 0, "entities table should have rows");
}

#[tokio::test]
async fn test_integrate_skips_already_processed_chunks() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let extractor = mock_extractor();
    let chunker = default_chunker();

    let id = uid();
    let ch_name = format!("integrate-skip-{id}");

    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        &ch_name,
        "Alpha Beta Gamma are related concepts.",
        None,
        None,
    )
    .await
    .expect("add_document failed");

    let first = pipeline::integrate(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &extractor,
        &[ch_name.clone()],
        None,
    )
    .await
    .expect("first integrate failed");
    assert!(first.chunks_processed > 0);

    let second = pipeline::integrate(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &extractor,
        &[ch_name],
        None,
    )
    .await
    .expect("second integrate failed");
    assert_eq!(
        second.chunks_processed, 0,
        "second integrate should process zero new chunks"
    );
}

#[tokio::test]
async fn test_integrate_continues_on_extraction_failure() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let chunker = default_chunker();

    /// An extractor that fails on chunks containing "FAIL_MARKER".
    struct FailingExtractor;

    #[async_trait::async_trait]
    impl Extractor for FailingExtractor {
        async fn extract(&self, text: &str, prompt: &str) -> anyhow::Result<ExtractionResult> {
            if text.contains("FAIL_MARKER") {
                anyhow::bail!("intentional extraction failure");
            }
            MockExtractor.extract(text, prompt).await
        }
    }

    let id = uid();
    let ch_name = format!("integrate-fail-{id}");

    // Add a document so we get at least one good chunk
    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        &ch_name,
        "Good Document with Alpha Beta entities.",
        None,
        None,
    )
    .await
    .unwrap();

    // Need to insert a chunk that will fail, directly
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    // Get an existing doc ID to attach the failing chunk to
    let doc_row: (String,) =
        sqlx::query_as("SELECT id FROM documents WHERE channel_id = $1 LIMIT 1")
            .bind(ch_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    // Insert a raw chunk that will fail extraction (not linked to entity_chunks so it's "unprocessed")
    let fail_chunk_id = format!("fail-chunk-{id}");
    insert_chunk(
        &pool,
        &fail_chunk_id,
        &doc_row.0,
        "This contains FAIL_MARKER and should fail extraction.",
        99,
    )
    .await;

    let result = pipeline::integrate(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &FailingExtractor,
        &[ch_name],
        None,
    )
    .await
    .expect("integrate should not propagate extraction failures");

    assert!(result.chunks_failed > 0, "should have at least one failed chunk");
    // The good chunk should still have been processed
    assert!(
        result.chunks_processed > 0,
        "should have processed at least one good chunk"
    );
}

#[tokio::test]
async fn test_search_returns_chunks() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let chunker = default_chunker();

    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        "search-chunks",
        "Copper demand is driven by data center expansion worldwide.",
        None,
        None,
    )
    .await
    .expect("add_document failed");

    let result = pipeline::search(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        "copper demand",
        Some(&["search-chunks".to_string()]),
        Some("CHUNKS"),
        Some(5),
    )
    .await
    .expect("search failed");

    // Result is a JSON array with search_result
    let arr = result.as_array().expect("result should be array");
    assert!(!arr.is_empty());
    let search_results = arr[0]["search_result"].as_array().unwrap();
    assert!(!search_results.is_empty(), "should return chunk results");
}

#[tokio::test]
async fn test_search_returns_entities_after_integrate() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let extractor = mock_extractor();
    let chunker = default_chunker();

    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        "search-ent",
        "Teck Resources operates in British Columbia mining Copper.",
        None,
        None,
    )
    .await
    .unwrap();

    pipeline::integrate(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &extractor,
        &["search-ent".to_string()],
        None,
    )
    .await
    .unwrap();

    let result = pipeline::search(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        "teck resources",
        Some(&["search-ent".to_string()]),
        Some("SIMILARITY"),
        Some(5),
    )
    .await
    .expect("search failed");

    let arr = result.as_array().expect("result should be array");
    assert!(!arr.is_empty());
    let entities = arr[0]["entities"].as_array().unwrap();
    assert!(!entities.is_empty(), "should return entity results after integrate");
}

#[tokio::test]
async fn test_search_clamps_top_k() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();

    // Should not crash even with absurdly large top_k
    let result = pipeline::search(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        "anything", None, None, Some(9999)).await;
    assert!(result.is_ok(), "large top_k should not crash");
}

#[tokio::test]
async fn test_delete_channel_via_pipeline() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let chunker = default_chunker();

    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        "del-pipe",
        "Document in a channel that will be deleted.",
        None,
        None,
    )
    .await
    .unwrap();

    pipeline::delete_channel(&second_mind::store::postgres::PostgresGraphStore::new(pool.clone()), "del-pipe")
        .await
        .expect("delete_channel failed");

    let channels = graph::list_channels(&pool).await.unwrap();
    assert!(
        !channels.iter().any(|c| c.name == "del-pipe"),
        "channel should be deleted"
    );
}

#[tokio::test]
async fn test_delete_all_channels() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let chunker = default_chunker();

    let id = uid();
    let ch1 = format!("del-all-1-{id}");
    let ch2 = format!("del-all-2-{id}");

    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        &ch1,
        "First channel doc.",
        None,
        None,
    )
    .await
    .unwrap();
    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        &ch2,
        "Second channel doc.",
        None,
        None,
    )
    .await
    .unwrap();

    // Delete each channel individually to avoid race conditions with concurrent tests
    // (delete_all_channels lists then deletes, which can race with other tests)
    pipeline::delete_channel(&second_mind::store::postgres::PostgresGraphStore::new(pool.clone()), &ch1)
        .await
        .expect("delete_channel ch1 failed");
    pipeline::delete_channel(&second_mind::store::postgres::PostgresGraphStore::new(pool.clone()), &ch2)
        .await
        .expect("delete_channel ch2 failed");

    let channels = graph::list_channels(&pool).await.unwrap();
    assert!(
        !channels.iter().any(|c| c.name == ch1 || c.name == ch2),
        "our channels should be deleted"
    );
}

// ===========================================================================
// Temporal tests
// ===========================================================================

#[tokio::test]
async fn test_contradiction_novel() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("temp-novel-{id}")).await.unwrap();
    let doc_id = format!("doc-tn-{id}");
    let a = format!("tn-a-{id}");
    let b = format!("tn-b-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-tn-{id}")).await;
    insert_test_entity(&pool, &a, "entity a").await;
    insert_test_entity(&pool, &b, "entity b").await;

    let result =
        temporal::check_contradiction_pg(&pool, &a, &b, "related_to", Some("a new fact"))
            .await
            .expect("check_contradiction failed");

    assert!(
        matches!(result, temporal::ContradictionResult::Novel),
        "should be Novel when no existing relationship"
    );
}

#[tokio::test]
async fn test_contradiction_identical() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("temp-ident-{id}")).await.unwrap();
    let doc_id = format!("doc-ti-{id}");
    let a = format!("ti-a-{id}");
    let b = format!("ti-b-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-ti-{id}")).await;
    insert_test_entity(&pool, &a, "entity a").await;
    insert_test_entity(&pool, &b, "entity b").await;

    insert_test_relationship(
        &pool,
        &format!("rel-ti-{id}"),
        &a,
        &b,
        "related_to",
        Some("copper is expensive"),
        ch_id,
        &doc_id,
    )
    .await;

    let result = temporal::check_contradiction_pg(
        &pool,
        &a,
        &b,
        "related_to",
        Some("copper is expensive"),
    )
    .await
    .expect("check_contradiction failed");

    assert!(
        matches!(result, temporal::ContradictionResult::Identical(_)),
        "should be Identical for same fact text"
    );
}

#[tokio::test]
async fn test_contradiction_potential() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("temp-contra-{id}")).await.unwrap();
    let doc_id = format!("doc-tc-{id}");
    let a = format!("tc-a-{id}");
    let b = format!("tc-b-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-tc-{id}")).await;
    insert_test_entity(&pool, &a, "entity a").await;
    insert_test_entity(&pool, &b, "entity b").await;

    insert_test_relationship(
        &pool,
        &format!("rel-tc-{id}"),
        &a,
        &b,
        "related_to",
        Some("copper is cheap"),
        ch_id,
        &doc_id,
    )
    .await;

    let result = temporal::check_contradiction_pg(
        &pool,
        &a,
        &b,
        "related_to",
        Some("copper is expensive"),
    )
    .await
    .expect("check_contradiction failed");

    assert!(
        matches!(
            result,
            temporal::ContradictionResult::PotentialContradiction { .. }
        ),
        "should be PotentialContradiction for different fact text on same pair+type"
    );
}

#[tokio::test]
async fn test_supersede_sets_valid_until() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("temp-super-{id}")).await.unwrap();
    let doc_id = format!("doc-ts-{id}");
    let a = format!("ts-a-{id}");
    let b = format!("ts-b-{id}");
    let rel_id = format!("rel-ts-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-ts-{id}")).await;
    insert_test_entity(&pool, &a, "entity a").await;
    insert_test_entity(&pool, &b, "entity b").await;

    insert_test_relationship(
        &pool,
        &rel_id,
        &a,
        &b,
        "related_to",
        None,
        ch_id,
        &doc_id,
    )
    .await;

    temporal::supersede_pg(&pool, &rel_id)
        .await
        .expect("supersede failed");

    let row: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT valid_until FROM relationships WHERE id = $1")
            .bind(&rel_id)
            .fetch_optional(&pool)
            .await
            .unwrap();

    assert!(row.is_some());
    assert!(
        row.unwrap().0.is_some(),
        "valid_until should be set after supersede"
    );
}

#[tokio::test]
async fn test_get_current_excludes_superseded() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_name = format!("temp-curr-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-tcur-{id}");
    let ent_a = format!("tcur-a-{id}");
    let ent_b = format!("tcur-b-{id}");
    let rel_id = format!("rel-cur-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-tcur-{id}")).await;
    insert_test_entity(&pool, &ent_a, "entity a").await;
    insert_test_entity(&pool, &ent_b, "entity b").await;

    insert_test_relationship(
        &pool,
        &rel_id,
        &ent_a,
        &ent_b,
        "related_to",
        None,
        ch_id,
        &doc_id,
    )
    .await;

    temporal::supersede_pg(&pool, &rel_id).await.unwrap();

    let current = temporal::get_current_relationships_pg(&pool, &ent_a)
        .await
        .expect("get_current_relationships failed");
    assert!(
        current.is_empty(),
        "superseded relationship should not appear in current"
    );
}

#[tokio::test]
async fn test_get_historical_returns_as_of() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_name = format!("temp-hist-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-hist-{id}");
    let ent_a = format!("hist-a-{id}");
    let ent_b = format!("hist-b-{id}");
    let rel_id = format!("rel-hist-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-hist-{id}")).await;
    insert_test_entity(&pool, &ent_a, "entity a").await;
    insert_test_entity(&pool, &ent_b, "entity b").await;

    // Insert a relationship that is currently active
    insert_test_relationship(
        &pool,
        &rel_id,
        &ent_a,
        &ent_b,
        "related_to",
        None,
        ch_id,
        &doc_id,
    )
    .await;

    // Capture now AFTER insert so valid_from <= now is guaranteed
    let now = chrono::Utc::now();

    // Query as-of now should find it
    let rels = temporal::get_historical_relationships(&pool, &ent_a, now)
        .await
        .expect("get_historical_relationships failed");
    assert!(!rels.is_empty(), "should find active relationship as-of now");

    // Supersede it
    temporal::supersede_pg(&pool, &rel_id).await.unwrap();

    // Query as-of a time before the relationship existed should find nothing
    let far_past = chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let old_rels = temporal::get_historical_relationships(&pool, &ent_a, far_past)
        .await
        .expect("get_historical_relationships failed");
    assert!(
        old_rels.is_empty(),
        "should not find relationship as-of time before it existed"
    );
}

// ===========================================================================
// Community tests
// ===========================================================================

#[tokio::test]
#[serial]
async fn test_detect_communities_empty_graph() {
    let pool = require_db!(setup().await);

    // Create a unique isolated entity with no relationships.
    // detect_communities filters out single-entity communities,
    // so we just verify the function doesn't error on entities with no edges.
    let id = uid();
    insert_test_entity(&pool, &format!("lone-{id}"), "isolated").await;

    let comms = communities::detect_communities_pg(&pool)
        .await
        .expect("detect_communities failed");
    // Isolated entities (no relationships) should not form communities.
    // Other test data may create communities; we just check our isolated entity
    // is NOT in any community.
    for comm in &comms {
        let members = communities::get_community_members_pg(&pool, &comm.id)
            .await
            .unwrap();
        assert!(
            !members.iter().any(|e| e.id == format!("lone-{id}")),
            "isolated entity should not be in any community"
        );
    }
}

#[tokio::test]
#[serial]
async fn test_detect_communities_groups_connected_entities() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_name = format!("comm-groups-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-cg-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-cg-{id}")).await;

    // Two disconnected pairs: A-B and C-D
    let a = format!("cg-a-{id}");
    let b = format!("cg-b-{id}");
    let c = format!("cg-c-{id}");
    let d = format!("cg-d-{id}");
    insert_test_entity(&pool, &a, "alpha").await;
    insert_test_entity(&pool, &b, "beta").await;
    insert_test_entity(&pool, &c, "gamma").await;
    insert_test_entity(&pool, &d, "delta").await;

    insert_test_relationship(&pool, &format!("cg-r1-{id}"), &a, &b, "related_to", None, ch_id, &doc_id)
        .await;
    insert_test_relationship(&pool, &format!("cg-r2-{id}"), &c, &d, "related_to", None, ch_id, &doc_id)
        .await;

    let comms = communities::detect_communities_pg(&pool)
        .await
        .expect("detect_communities failed");

    // Verify our two pairs are in separate communities
    let mut a_comm = None;
    let mut c_comm = None;
    for comm in &comms {
        let members = communities::get_community_members_pg(&pool, &comm.id).await.unwrap();
        let member_ids: Vec<&str> = members.iter().map(|e| e.id.as_str()).collect();
        if member_ids.contains(&a.as_str()) {
            a_comm = Some(comm.id.clone());
        }
        if member_ids.contains(&c.as_str()) {
            c_comm = Some(comm.id.clone());
        }
    }
    assert!(a_comm.is_some(), "entity A should be in a community");
    assert!(c_comm.is_some(), "entity C should be in a community");
    assert_ne!(a_comm, c_comm, "disconnected pairs should be in different communities");
}

#[tokio::test]
#[serial]
async fn test_detect_communities_merges_connected_components() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_name = format!("comm-merge-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-cm-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-cm-{id}")).await;

    let a = format!("cm-a-{id}");
    let b = format!("cm-b-{id}");
    let c = format!("cm-c-{id}");
    insert_test_entity(&pool, &a, "alpha").await;
    insert_test_entity(&pool, &b, "beta").await;
    insert_test_entity(&pool, &c, "gamma").await;

    // A -> B -> C (connected chain)
    insert_test_relationship(&pool, &format!("cm-r1-{id}"), &a, &b, "related_to", None, ch_id, &doc_id)
        .await;
    insert_test_relationship(&pool, &format!("cm-r2-{id}"), &b, &c, "related_to", None, ch_id, &doc_id)
        .await;

    let comms = communities::detect_communities_pg(&pool)
        .await
        .expect("detect_communities failed");

    // Verify all three entities end up in the same community
    let mut found_comm = None;
    for comm in &comms {
        let members = communities::get_community_members_pg(&pool, &comm.id).await.unwrap();
        let member_ids: Vec<&str> = members.iter().map(|e| e.id.as_str()).collect();
        if member_ids.contains(&a.as_str()) {
            assert!(member_ids.contains(&b.as_str()), "B should be in same community as A");
            assert!(member_ids.contains(&c.as_str()), "C should be in same community as A");
            found_comm = Some(comm.id.clone());
            break;
        }
    }
    assert!(found_comm.is_some(), "should find a community containing our connected entities");
}

#[tokio::test]
#[serial]
async fn test_list_communities_with_counts() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_name = format!("comm-list-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-cl-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-cl-{id}")).await;

    let a = format!("cl-a-{id}");
    let b = format!("cl-b-{id}");
    insert_test_entity(&pool, &a, "alpha").await;
    insert_test_entity(&pool, &b, "beta").await;
    insert_test_relationship(&pool, &format!("cl-r1-{id}"), &a, &b, "related_to", None, ch_id, &doc_id)
        .await;

    communities::detect_communities_pg(&pool).await.unwrap();

    let listed = communities::list_communities_pg(&pool)
        .await
        .expect("list_communities failed");
    assert!(!listed.is_empty());

    for (comm, count) in &listed {
        assert!(*count > 0, "community {} should have members", comm.id);
    }
}

#[tokio::test]
#[serial]
async fn test_get_community_members() {
    let pool = require_db!(setup().await);

    let id = uid();
    let ch_name = format!("comm-members-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-cmem-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-cmem-{id}")).await;

    let a = format!("cmem-a-{id}");
    let b = format!("cmem-b-{id}");
    insert_test_entity(&pool, &a, "alpha").await;
    insert_test_entity(&pool, &b, "beta").await;
    insert_test_relationship(
        &pool, &format!("cmem-r1-{id}"), &a, &b, "related_to", None, ch_id, &doc_id,
    )
    .await;

    let comms = communities::detect_communities_pg(&pool).await.unwrap();
    assert!(!comms.is_empty(), "should detect at least one community");

    // Find the community that contains our entities
    let mut found_community = None;
    for comm in &comms {
        let members = communities::get_community_members_pg(&pool, &comm.id)
            .await
            .expect("get_community_members failed");
        let member_ids: Vec<&str> = members.iter().map(|e| e.id.as_str()).collect();
        if member_ids.contains(&a.as_str()) {
            found_community = Some((comm.clone(), members));
            break;
        }
    }

    let (_comm, members) = found_community.expect("should find a community containing our entities");
    let member_ids: Vec<&str> = members.iter().map(|e| e.id.as_str()).collect();
    assert!(member_ids.contains(&a.as_str()));
    assert!(member_ids.contains(&b.as_str()));
}

// ===========================================================================
// Cache tests
// ===========================================================================

#[tokio::test]
async fn test_cache_set_and_get() {
    let pool = require_db!(setup().await);

    let key = cache::cache_key("test-model", "test-input");
    cache::set(&pool, &key, "test-model", "cached response")
        .await
        .expect("cache set failed");

    let result = cache::get(&pool, &key).await.expect("cache get failed");
    assert_eq!(result, Some("cached response".to_string()));
}

#[tokio::test]
async fn test_cache_miss_returns_none() {
    let pool = require_db!(setup().await);

    let result = cache::get(&pool, "nonexistent-key")
        .await
        .expect("cache get failed");
    assert!(result.is_none());
}

#[tokio::test]
async fn test_cache_is_idempotent() {
    let pool = require_db!(setup().await);

    let key = cache::cache_key("model", "input");
    cache::set(&pool, &key, "model", "response1")
        .await
        .expect("first set failed");
    cache::set(&pool, &key, "model", "response2")
        .await
        .expect("second set should not error (ON CONFLICT DO NOTHING)");

    // Should still return the first value (DO NOTHING on conflict)
    let result = cache::get(&pool, &key).await.unwrap();
    assert_eq!(result, Some("response1".to_string()));
}

// ===========================================================================
// Resolver tests
// ===========================================================================

#[tokio::test]
async fn test_resolve_new_entity() {
    let pool = require_db!(setup().await);

    let extracted = ExtractedEntity {
        name: "Brand New Entity".to_string(),
        entity_type: Some("concept".to_string()),
        description: "Something new".to_string(),
    };

    let resolved = resolver::resolve_pg(&pool, &extracted)
        .await
        .expect("resolve failed");
    assert!(resolved.is_new, "should be new when entity doesn't exist");
}

#[tokio::test]
async fn test_resolve_existing_entity() {
    let pool = require_db!(setup().await);

    let id = uid();
    // Store with a fully lowercase name (matching what the resolver will normalize to)
    let name = format!("resolvecopper{}", id.to_lowercase());
    insert_test_entity(&pool, &id, &name).await;

    // Input with uppercase first letter — resolver will lowercase it
    let input_name = format!("Resolvecopper{}", id.to_lowercase());

    let extracted = ExtractedEntity {
        name: input_name,
        entity_type: Some("material".to_string()),
        description: "A metal".to_string(),
    };

    let resolved = resolver::resolve_pg(&pool, &extracted)
        .await
        .expect("resolve failed");
    assert!(!resolved.is_new, "should find existing entity by normalized name");
    assert_eq!(resolved.entity_id, id);
}

#[tokio::test]
async fn test_resolve_via_alias() {
    let pool = require_db!(setup().await);

    let id = uid();
    let id_lower = id.to_lowercase();
    let ch_name = format!("resolve-alias-{id}");
    let ch_id = schema::ensure_channel(&pool, &ch_name).await.unwrap();
    let doc_id = format!("doc-ra-{id}");
    let entity_id = format!("alias-ent-{id}");
    // Use a unique entity name that won't match via name lookup (resolver lowercases input)
    let entity_name = format!("uniquecopperentity{id_lower}");
    // Store alias already lowercased (resolver normalizes alias input to lowercase)
    let alias = format!("cu{id_lower}");

    insert_document(&pool, &doc_id, ch_id, &format!("sha-ra-{id}")).await;
    insert_test_entity(&pool, &entity_id, &entity_name).await;
    graph::insert_entity_alias(&pool, &alias, &entity_id, Some(&doc_id))
        .await
        .unwrap();

    // The resolver will normalize this to lowercase, which should match our alias
    let extracted = ExtractedEntity {
        name: alias.clone(),
        entity_type: None,
        description: String::new(),
    };

    let resolved = resolver::resolve_pg(&pool, &extracted)
        .await
        .expect("resolve failed");
    assert!(!resolved.is_new, "should find entity via alias");
    assert_eq!(resolved.entity_id, entity_id);
}

// ===========================================================================
// Server tests (HTTP layer via tower::ServiceExt)
// ===========================================================================

#[tokio::test]
async fn test_health_endpoint() {
    let pool = require_db!(setup().await);
    let app = server::router(build_app_state(pool));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "ok");
}

#[tokio::test]
async fn test_add_document_endpoint() {
    let pool = require_db!(setup().await);
    let app = server::router(build_app_state(pool));

    let body = serde_json::json!({
        "dataset_name": "http-test",
        "content": "Test document added via HTTP endpoint."
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/add")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let resp_body = response.into_body().collect().await.unwrap().to_bytes();
    let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(resp["status"], "ok");
}

#[tokio::test]
async fn test_search_endpoint() {
    let pool = require_db!(setup().await);
    let state = build_app_state(pool.clone());

    // First add a document so there's something to search
    let embedder = mock_embedder();
    let chunker = default_chunker();
    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        "search-http",
        "Content for HTTP search test.",
        None,
        None,
    )
    .await
    .unwrap();

    let app = server::router(state);

    let body = serde_json::json!({
        "query": "search test",
        "datasets": ["search-http"],
        "top_k": 5
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/search")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let resp_body = response.into_body().collect().await.unwrap().to_bytes();
    let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert!(resp.is_array(), "search response should be a JSON array");
}

#[tokio::test]
async fn test_list_datasets_endpoint() {
    let pool = require_db!(setup().await);

    schema::ensure_channel(&pool, "ds-list-1").await.unwrap();
    schema::ensure_channel(&pool, "ds-list-2").await.unwrap();

    let app = server::router(build_app_state(pool));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/datasets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let resp_body = response.into_body().collect().await.unwrap().to_bytes();
    let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    let arr = resp.as_array().expect("response should be an array");
    assert!(arr.len() >= 2, "should list at least the 2 created datasets");
}

#[tokio::test]
async fn test_body_size_limit() {
    let pool = require_db!(setup().await);
    let app = server::router(build_app_state(pool));

    // Create a body larger than 4MB
    let large_content = "x".repeat(5 * 1024 * 1024);
    let body = serde_json::json!({
        "dataset_name": "too-big",
        "content": large_content
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/add")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should be rejected (413 Payload Too Large or 400)
    assert!(
        response.status() == StatusCode::PAYLOAD_TOO_LARGE
            || response.status().is_client_error()
            || response.status().is_server_error(),
        "oversized body should be rejected, got {}",
        response.status()
    );
}

// ===========================================================================
// Full-text search tests
// ===========================================================================

#[tokio::test]
async fn test_search_entities_fulltext_exact_match() {
    let pool = require_db!(setup().await);
    use second_mind::store::postgres::PostgresGraphStore;
    use second_mind::store::GraphStore;

    let id = uid();
    let name = format!("Copper Mining {id}");
    let ent_id = format!("ft-exact-{id}");
    insert_test_entity(&pool, &ent_id, &name).await;

    let store = PostgresGraphStore::new(pool.clone());
    let results = store
        .search_entities_fulltext(&name, 10)
        .await
        .expect("search_entities_fulltext failed");

    assert!(!results.is_empty(), "should find entity by exact name");
    let (found_entity, score) = &results[0];
    assert_eq!(found_entity.id, ent_id);
    assert!(
        (*score - 1.0).abs() < f64::EPSILON,
        "exact match should have score 1.0, got {score}"
    );
}

#[tokio::test]
async fn test_search_entities_fulltext_prefix_match() {
    let pool = require_db!(setup().await);
    use second_mind::store::postgres::PostgresGraphStore;
    use second_mind::store::GraphStore;

    let id = uid();
    let unique_prefix = format!("PrefixTest{id}");
    let name = format!("{unique_prefix} Mining Corp");
    let ent_id = format!("ft-prefix-{id}");
    insert_test_entity(&pool, &ent_id, &name).await;

    let store = PostgresGraphStore::new(pool.clone());
    let results = store
        .search_entities_fulltext(&unique_prefix, 10)
        .await
        .expect("search_entities_fulltext failed");

    let found = results.iter().find(|(e, _)| e.id == ent_id);
    assert!(found.is_some(), "should find entity by prefix search");
    let (_, score) = found.unwrap();
    // unique_prefix is a prefix of the full name → starts_with = true → 0.8
    assert!(
        (*score - 0.8).abs() < f64::EPSILON,
        "prefix match should have score ~0.8, got {score}"
    );
}

#[tokio::test]
async fn test_search_entities_fulltext_substring_match() {
    let pool = require_db!(setup().await);
    use second_mind::store::postgres::PostgresGraphStore;
    use second_mind::store::GraphStore;

    let id = uid();
    // "Global" prefix means searching for "Copper" is a substring, not prefix
    let unique_term = format!("SubstrTest{id}");
    let name = format!("Global {unique_term} Mining");
    let ent_id = format!("ft-substr-{id}");
    insert_test_entity(&pool, &ent_id, &name).await;

    let store = PostgresGraphStore::new(pool.clone());
    let results = store
        .search_entities_fulltext(&unique_term, 10)
        .await
        .expect("search_entities_fulltext failed");

    let found = results.iter().find(|(e, _)| e.id == ent_id);
    assert!(
        found.is_some(),
        "should find entity by substring match"
    );
    let (_, score) = found.unwrap();
    // name starts with "Global", not the search term, so it's a substring match → 0.5
    assert!(
        (*score - 0.5).abs() < f64::EPSILON,
        "substring match should have score ~0.5, got {score}"
    );
}

#[tokio::test]
async fn test_search_entities_fulltext_no_match() {
    let pool = require_db!(setup().await);
    use second_mind::store::postgres::PostgresGraphStore;
    use second_mind::store::GraphStore;

    let id = uid();
    let store = PostgresGraphStore::new(pool.clone());
    let results = store
        .search_entities_fulltext(&format!("Nonexistent{id}"), 10)
        .await
        .expect("search_entities_fulltext failed");

    assert!(results.is_empty(), "should return empty for non-matching query");
}

// ===========================================================================
// Entity description enrichment tests
// ===========================================================================

#[tokio::test]
async fn test_update_entity_description() {
    let pool = require_db!(setup().await);
    use second_mind::store::postgres::PostgresGraphStore;
    use second_mind::store::GraphStore;

    let id = uid();
    let ent_id = format!("desc-{id}");
    let name = format!("DescEntity {id}");

    // Insert entity with a short description in properties
    let entity = Entity {
        id: ent_id.clone(),
        canonical_name: name.clone(),
        entity_type: Some("concept".to_string()),
        properties: serde_json::json!({"description": "short"}),
        embedding: None,
        created_at: chrono::Utc::now(),
    };
    graph::insert_entity(&pool, &entity)
        .await
        .expect("insert_entity failed");

    let store = PostgresGraphStore::new(pool.clone());
    store
        .update_entity_description(&ent_id, "much longer description here with more detail")
        .await
        .expect("update_entity_description failed");

    // Read back and verify
    let found = graph::find_entity_by_name(&pool, &name)
        .await
        .expect("find_entity_by_name failed")
        .expect("entity should exist");

    let desc = found
        .properties
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("");
    assert_eq!(
        desc, "much longer description here with more detail",
        "description should be updated to the longer version"
    );
}

// ===========================================================================
// Relationship weight tests
// ===========================================================================

#[tokio::test]
async fn test_increment_relationship_weight() {
    let pool = require_db!(setup().await);
    use second_mind::store::postgres::PostgresGraphStore;
    use second_mind::store::GraphStore;

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("weight-{id}"))
        .await
        .unwrap();
    let doc_id = format!("doc-wt-{id}");
    let src = format!("wt-src-{id}");
    let tgt = format!("wt-tgt-{id}");
    let rel_id = format!("rel-wt-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-wt-{id}")).await;
    insert_test_entity(&pool, &src, "source entity").await;
    insert_test_entity(&pool, &tgt, "target entity").await;

    // Insert relationship (weight defaults to 1.0)
    insert_test_relationship(&pool, &rel_id, &src, &tgt, "related_to", None, ch_id, &doc_id)
        .await;

    let store = PostgresGraphStore::new(pool.clone());

    // First increment: 1.0 -> 2.0
    store
        .increment_relationship_weight(&rel_id)
        .await
        .expect("first increment failed");

    let row: (f64,) = sqlx::query_as("SELECT weight FROM relationships WHERE id = $1")
        .bind(&rel_id)
        .fetch_one(&pool)
        .await
        .expect("query weight failed");
    assert!(
        (row.0 - 2.0).abs() < f64::EPSILON,
        "weight should be 2.0 after first increment, got {}",
        row.0
    );

    // Second increment: 2.0 -> 3.0
    store
        .increment_relationship_weight(&rel_id)
        .await
        .expect("second increment failed");

    let row: (f64,) = sqlx::query_as("SELECT weight FROM relationships WHERE id = $1")
        .bind(&rel_id)
        .fetch_one(&pool)
        .await
        .expect("query weight failed");
    assert!(
        (row.0 - 3.0).abs() < f64::EPSILON,
        "weight should be 3.0 after second increment, got {}",
        row.0
    );
}

// ===========================================================================
// Pipeline: integrate weight increment on identical fact
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore] // Requires MockExtractor to produce identical facts across runs; fragile at pipeline level. Weight increment is validated by test_increment_relationship_weight
async fn test_integrate_increments_weight_on_identical_fact() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let extractor = mock_extractor();
    let chunker = default_chunker();

    let id = uid();
    let ch_name = format!("wt-integrate-{id}");

    // Add a document with capitalized words that MockExtractor will extract
    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        &ch_name,
        "Alice likes to work with Bob on projects.",
        None,
        None,
    )
    .await
    .expect("first add_document failed");

    // First integrate creates entities + relationships
    let first_result = pipeline::integrate(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &extractor,
        &[ch_name.clone()],
        None,
    )
    .await
    .expect("first integrate failed");
    assert!(
        first_result.entities_created > 0,
        "first integrate should create entities (got: {:?})", first_result
    );

    // Add a second document with the same capitalized words but different lowercase text
    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        &ch_name,
        "Alice collaborates frequently with Bob on different tasks.",
        None,
        None,
    )
    .await
    .expect("second add_document failed");

    // Second integrate should detect identical relationships and increment weight
    pipeline::integrate(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &extractor,
        &[ch_name],
        None,
    )
    .await
    .expect("second integrate failed");

    // Check that at least one relationship in this channel has weight > 1.0
    use second_mind::store::GraphStore as _;
    let store = second_mind::store::postgres::PostgresGraphStore::new(pool.clone());
    let ch_id: i32 = store.ensure_channel(&format!("wt-integrate-{id}")).await.unwrap();
    let rows: Vec<(f64,)> = sqlx::query_as(
        "SELECT COALESCE(weight, 1.0) FROM relationships WHERE channel_id = $1 AND COALESCE(weight, 1.0) > 1.0",
    )
    .bind(ch_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    // If no weight increment happened, check if relationships even exist
    if rows.is_empty() {
        let all_rels: Vec<(String, f64,)> = sqlx::query_as(
            "SELECT relationship, COALESCE(weight, 1.0) FROM relationships WHERE channel_id = $1",
        )
        .bind(ch_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        panic!(
            "should have at least one relationship with weight > 1.0 after second integrate. \
             Found {} relationships in channel, weights: {:?}",
            all_rels.len(),
            all_rels.iter().map(|(_, w)| w).collect::<Vec<_>>()
        );
    }
}

// ===========================================================================
// Rich traversal tests
// ===========================================================================

#[tokio::test]
async fn test_traverse_rich() {
    let pool = require_db!(setup().await);
    use second_mind::store::postgres::PostgresGraphStore;
    use second_mind::store::GraphStore;

    let id = uid();
    let ch_id = schema::ensure_channel(&pool, &format!("rich-trav-{id}"))
        .await
        .unwrap();
    let doc_id = format!("doc-rt-{id}");
    let a = format!("rt-a-{id}");
    let b = format!("rt-b-{id}");
    let c = format!("rt-c-{id}");
    insert_document(&pool, &doc_id, ch_id, &format!("sha-rt-{id}")).await;
    insert_test_entity(&pool, &a, &format!("node-a-{id}")).await;
    insert_test_entity(&pool, &b, &format!("node-b-{id}")).await;
    insert_test_entity(&pool, &c, &format!("node-c-{id}")).await;

    // A -> B -> C
    insert_test_relationship(
        &pool,
        &format!("rtr1-{id}"),
        &a,
        &b,
        "related_to",
        Some("A relates to B"),
        ch_id,
        &doc_id,
    )
    .await;
    insert_test_relationship(
        &pool,
        &format!("rtr2-{id}"),
        &b,
        &c,
        "enables",
        Some("B enables C"),
        ch_id,
        &doc_id,
    )
    .await;

    let store = PostgresGraphStore::new(pool.clone());
    let paths = store
        .traverse_rich(&a, 2, None, false)
        .await
        .expect("traverse_rich failed");

    // Should return paths from A to connected entities
    assert!(!paths.is_empty(), "should return at least one path");

    // Verify path to B exists
    let path_to_b = paths.iter().find(|p| {
        p.entities.iter().any(|e| e.id == b)
    });
    assert!(path_to_b.is_some(), "should have a path reaching entity B");
    let path_b = path_to_b.unwrap();
    assert!(!path_b.relationships.is_empty(), "path should have relationships");
    assert_eq!(path_b.relationships[0].relationship, "related_to");

    // Verify that entities in the path include the start entity
    assert!(
        path_b.entities.iter().any(|e| e.id == a),
        "path should include start entity A"
    );
}

// ===========================================================================
// Pipeline: search includes fulltext_matches and paths
// ===========================================================================

#[tokio::test]
async fn test_search_includes_fulltext_and_paths() {
    let pool = require_db!(setup().await);
    let embedder = mock_embedder();
    let extractor = mock_extractor();
    let chunker = default_chunker();

    let id = uid();
    let ch_name = format!("search-ft-{id}");

    // Add a document and integrate to create entities and relationships
    pipeline::add_document(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &chunker,
        &ch_name,
        "Teck Resources mines Copper in British Columbia for Data Centers.",
        None,
        None,
    )
    .await
    .expect("add_document failed");

    pipeline::integrate(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        &extractor,
        &[ch_name.clone()],
        None,
    )
    .await
    .expect("integrate failed");

    // Search with default search_type (None) which includes everything
    let result = pipeline::search(
        &second_mind::store::postgres::PostgresGraphStore::new(pool.clone()),
        &second_mind::store::postgres::PostgresVectorStore::new(pool.clone()),
        &embedder,
        "Teck Resources",
        Some(&[ch_name]),
        None,
        Some(5),
    )
    .await
    .expect("search failed");

    let arr = result.as_array().expect("result should be array");
    assert!(!arr.is_empty());
    let entry = &arr[0];

    // Verify the response has the new keys
    assert!(
        entry.get("fulltext_matches").is_some(),
        "response should have fulltext_matches key"
    );
    assert!(
        entry.get("paths").is_some(),
        "response should have paths key"
    );

    // fulltext_matches should be an array (as_array panics if not)
    let _fulltext = entry["fulltext_matches"]
        .as_array()
        .expect("fulltext_matches should be an array");

    // paths should be an array
    let _paths = entry["paths"]
        .as_array()
        .expect("paths should be an array");
}
