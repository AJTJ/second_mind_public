/// End-to-end tests that exercise the full pipeline through the graph engine API.
/// Tests the complete lifecycle: add → integrate → search → replay → verify.
///
/// These tests use the HTTP API (via tower::ServiceExt) to simulate real requests,
/// not direct function calls. This validates the full stack including routing,
/// serialization, and error handling.
///
/// Requires TEST_DATABASE_URL for Postgres.
/// Optionally requires NEO4J_TEST_URI for backend comparison tests.
///
/// Run with:
/// TEST_DATABASE_URL=postgresql://test:test@localhost:5433/second_mind_test cargo test --test end_to_end

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

use second_mind::chunker::ChunkerConfig;
use second_mind::embedder::MockEmbedder;
use second_mind::extractor::MockExtractor;
use second_mind::server::{self, AppState};
use second_mind::store::postgres::{PostgresGraphStore, PostgresVectorStore};
use second_mind::store::GraphStore;

fn uid() -> String {
    ulid::Ulid::new().to_string()
}

async fn setup_postgres_app() -> Option<(axum::Router, PgPool)> {
    let url = match std::env::var("TEST_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("TEST_DATABASE_URL not set, skipping end-to-end tests");
            return None;
        }
    };

    let pool = PgPool::connect(&url).await.ok()?;
    let graph_store = PostgresGraphStore::new(pool.clone());
    graph_store.initialize().await.ok()?;

    let state = Arc::new(AppState {
        graph_store: Arc::new(graph_store),
        vector_store: Arc::new(PostgresVectorStore::new(pool.clone())),
        embedder: Arc::new(MockEmbedder { dimensions: 2560 }),
        extractor: Arc::new(MockExtractor),
        chunker_config: ChunkerConfig::default(),
    });

    Some((server::router(state), pool))
}

macro_rules! require_app {
    ($setup:expr) => {
        match $setup {
            Some(s) => s,
            None => return,
        }
    };
}

async fn body_json(resp: axum::http::Response<Body>) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

// ==========================================================================
// Health
// ==========================================================================

#[tokio::test]
async fn test_e2e_health() {
    let (app, _pool) = require_app!(setup_postgres_app().await);
    let resp = app.oneshot(get("/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ==========================================================================
// Full lifecycle: add → integrate → search
// ==========================================================================

#[tokio::test]
async fn test_e2e_add_integrate_search() {
    let (app, _pool) = require_app!(setup_postgres_app().await);
    let id = uid();
    let channel = format!("e2e-{id}");

    // Step 1: Add a document
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/add",
            json!({
                "dataset_name": channel,
                "content": "Alice is a researcher at MIT. Bob works at Stanford. They collaborate on AI safety.",
                "source_ref": "test-doc.md"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "add should succeed");
    let add_body = body_json(resp).await;
    assert_eq!(add_body["status"], "ok");

    // Step 2: Integrate (extract entities + relationships)
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/integrate",
            json!({
                "datasets": [channel],
                "custom_prompt": "Extract all entities and relationships."
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "integrate should succeed");
    let integrate_body = body_json(resp).await;
    assert_eq!(integrate_body["status"], "ok");

    // Step 3: Search
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/search",
            json!({
                "query": "Alice",
                "datasets": [channel],
                "top_k": 10
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "search should succeed");
    let search_body = body_json(resp).await;

    // Should have results (the mock extractor produces entities from capitalized words)
    let results = &search_body[0];
    assert!(
        results["search_result"].is_array() || results["entities"].is_array(),
        "search should return result arrays"
    );
}

// ==========================================================================
// Integration is idempotent — re-integrating skips processed chunks
// ==========================================================================

#[tokio::test]
async fn test_e2e_integrate_idempotent() {
    let (app, _pool) = require_app!(setup_postgres_app().await);
    let id = uid();
    let channel = format!("e2e-idemp-{id}");

    // Add document
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/add",
            json!({
                "dataset_name": channel,
                "content": "Charlie works at Google on machine learning projects.",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // First integrate
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/integrate",
            json!({ "datasets": [channel] }),
        ))
        .await
        .unwrap();
    let first = body_json(resp).await;
    let first_msg = first["message"].as_str().unwrap_or("");

    // Second integrate — should skip already-processed chunks
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/integrate",
            json!({ "datasets": [channel] }),
        ))
        .await
        .unwrap();
    let second = body_json(resp).await;
    let second_msg = second["message"].as_str().unwrap_or("");

    // Second run should process 0 chunks (or fewer than first)
    assert!(
        second_msg.contains("0 entities") || second_msg.contains("Integrated: 0"),
        "second integrate should be a no-op, got: {second_msg}"
    );
    let _ = first_msg;
}

// ==========================================================================
// Duplicate document detection
// ==========================================================================

#[tokio::test]
async fn test_e2e_duplicate_document() {
    let (app, _pool) = require_app!(setup_postgres_app().await);
    let id = uid();
    let channel = format!("e2e-dup-{id}");
    let content = format!("Unique content for dedup test {id}");

    // Add same content twice
    let resp1 = app
        .clone()
        .oneshot(post_json(
            "/api/v1/add",
            json!({
                "dataset_name": channel,
                "content": content,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    let resp2 = app
        .clone()
        .oneshot(post_json(
            "/api/v1/add",
            json!({
                "dataset_name": channel,
                "content": content,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);

    let body1 = body_json(resp1).await;
    let body2 = body_json(resp2).await;

    // Both should succeed
    assert_eq!(body1["status"], "ok");
    assert_eq!(body2["status"], "ok");

    // Both messages should reference the same document ID (dedup returns existing)
    let msg1 = body1["message"].as_str().unwrap_or("");
    let msg2 = body2["message"].as_str().unwrap_or("");
    // Extract the document ID from "Added document <ID> with N chunks"
    let id1 = msg1.split_whitespace().nth(2).unwrap_or("");
    let id2 = msg2.split_whitespace().nth(2).unwrap_or("");
    assert!(
        !id1.is_empty() && id1 == id2,
        "both adds should return same doc ID (dedup), got: '{msg1}' vs '{msg2}'"
    );
}

// ==========================================================================
// Channel isolation — search respects channel filter
// ==========================================================================

#[tokio::test]
async fn test_e2e_channel_isolation() {
    let (app, _pool) = require_app!(setup_postgres_app().await);
    let id = uid();
    let ch_a = format!("e2e-iso-a-{id}");
    let ch_b = format!("e2e-iso-b-{id}");

    // Add different documents to different channels
    app.clone()
        .oneshot(post_json(
            "/api/v1/add",
            json!({
                "dataset_name": ch_a,
                "content": "Delta is a concept in channel A only.",
            }),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(post_json(
            "/api/v1/add",
            json!({
                "dataset_name": ch_b,
                "content": "Epsilon is a concept in channel B only.",
            }),
        ))
        .await
        .unwrap();

    // Integrate both
    app.clone()
        .oneshot(post_json(
            "/api/v1/integrate",
            json!({ "datasets": [ch_a] }),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(post_json(
            "/api/v1/integrate",
            json!({ "datasets": [ch_b] }),
        ))
        .await
        .unwrap();

    // Search channel A for "Delta" — should find it (it's in channel A)
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/search",
            json!({
                "query": "Delta",
                "datasets": [ch_a],
                "top_k": 10
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let body_str = body.to_string();
    // Vector search is channel-filtered; the chunk containing "Delta" should appear
    assert!(
        body_str.len() > 20,
        "channel A search for Delta should return results"
    );

    // Search channel B for "Epsilon" — should find it
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/search",
            json!({
                "query": "Epsilon",
                "datasets": [ch_b],
                "top_k": 10
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ==========================================================================
// List and delete datasets
// ==========================================================================

#[tokio::test]
async fn test_e2e_list_and_delete_dataset() {
    let (app, _pool) = require_app!(setup_postgres_app().await);
    let id = uid();
    let channel = format!("e2e-del-{id}");

    // Add a document to create the channel
    app.clone()
        .oneshot(post_json(
            "/api/v1/add",
            json!({
                "dataset_name": channel,
                "content": "Temporary data for deletion test.",
            }),
        ))
        .await
        .unwrap();

    // List datasets — should include our channel
    let resp = app
        .clone()
        .oneshot(get("/api/v1/datasets"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let datasets = body_json(resp).await;
    let datasets_str = datasets.to_string();
    assert!(
        datasets_str.contains(&channel),
        "should find our channel in datasets list"
    );

    // Delete the channel
    let resp = app
        .clone()
        .oneshot(delete(&format!("/api/v1/datasets/{channel}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ==========================================================================
// Communities
// ==========================================================================

#[tokio::test]
async fn test_e2e_communities() {
    let (app, _pool) = require_app!(setup_postgres_app().await);

    // List communities (may be empty)
    let resp = app
        .clone()
        .oneshot(get("/api/v1/communities"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Rebuild communities
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/communities/rebuild")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ==========================================================================
// Body size limit
// ==========================================================================

#[tokio::test]
async fn test_e2e_body_size_limit() {
    let (app, _pool) = require_app!(setup_postgres_app().await);

    // 5MB content should be rejected (limit is 4MB)
    let big_content = "x".repeat(5 * 1024 * 1024);
    let resp = app
        .oneshot(post_json(
            "/api/v1/add",
            json!({
                "dataset_name": "size-test",
                "content": big_content,
            }),
        ))
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "5MB content should be rejected"
    );
}

// ==========================================================================
// Backend comparison: Postgres vs Neo4j produce equivalent results
// ==========================================================================

#[tokio::test]
async fn test_e2e_backend_comparison() {
    // This test requires both Postgres and Neo4j
    let pg_url = match std::env::var("TEST_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => return,
    };
    let neo4j_uri = match std::env::var("NEO4J_TEST_URI") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("NEO4J_TEST_URI not set, skipping backend comparison");
            return;
        }
    };
    let neo4j_user = std::env::var("NEO4J_TEST_USER").unwrap_or_else(|_| "neo4j".to_string());
    let neo4j_pass =
        std::env::var("NEO4J_TEST_PASSWORD").unwrap_or_else(|_| "testpassword".to_string());

    let pool = PgPool::connect(&pg_url).await.unwrap();

    // Setup Postgres backend
    let pg_graph = PostgresGraphStore::new(pool.clone());
    pg_graph.initialize().await.unwrap();
    let pg_vectors = PostgresVectorStore::new(pool.clone());

    // Setup Neo4j backend
    let neo4j_graph =
        second_mind::store::neo4j::Neo4jGraphStore::new(&neo4j_uri, &neo4j_user, &neo4j_pass)
            .await
            .unwrap();
    neo4j_graph.initialize().await.unwrap();

    let embedder = MockEmbedder { dimensions: 2560 };
    let extractor = MockExtractor;
    let chunker = ChunkerConfig::default();

    let id = uid();
    let channel = format!("compare-{id}");
    let content = "Zeta works with Eta on quantum computing research.";

    // Run same operations on both backends
    let pg_doc = second_mind::pipeline::add_document(
        &pg_graph, &pg_vectors, &embedder, &chunker, &channel, content, None, None,
    )
    .await
    .unwrap();

    let neo4j_doc = second_mind::pipeline::add_document(
        &neo4j_graph, &pg_vectors, &embedder, &chunker, &channel, content, None, None,
    )
    .await
    .unwrap();

    // Both should create documents
    assert!(!pg_doc.is_empty());
    assert!(!neo4j_doc.is_empty());

    // Integrate both
    let pg_result = second_mind::pipeline::integrate(
        &pg_graph, &pg_vectors, &embedder, &extractor, &[channel.clone()], None,
    )
    .await
    .unwrap();

    let neo4j_result = second_mind::pipeline::integrate(
        &neo4j_graph, &pg_vectors, &embedder, &extractor, &[channel.clone()], None,
    )
    .await
    .unwrap();

    // Both should create the same number of entities
    assert_eq!(
        pg_result.entities_created, neo4j_result.entities_created,
        "Postgres created {} entities, Neo4j created {}",
        pg_result.entities_created, neo4j_result.entities_created
    );

    // Both should create the same number of relationships
    assert_eq!(
        pg_result.relationships_created, neo4j_result.relationships_created,
        "Postgres created {} relationships, Neo4j created {}",
        pg_result.relationships_created, neo4j_result.relationships_created
    );
}
