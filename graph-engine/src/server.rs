use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Json, Router};

use crate::chunker::ChunkerConfig;
use crate::embedder::Embedder;
use crate::extractor::Extractor;
use crate::pipeline;
use crate::store::{GraphStore, VectorStore};
use crate::types::*;

pub struct AppState {
    pub graph_store: Arc<dyn GraphStore>,
    pub vector_store: Arc<dyn VectorStore>,
    pub embedder: Arc<dyn Embedder>,
    pub extractor: Arc<dyn Extractor>,
    pub chunker_config: ChunkerConfig,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", axum::routing::get(health))
        .route("/api/v1/add", axum::routing::post(add_document))
        .route("/api/v1/integrate", axum::routing::post(integrate))
        // Backward compatibility alias (remove after intake engine migration)
        .route("/api/v1/cognify", axum::routing::post(integrate))
        .route("/api/v1/search", axum::routing::post(search))
        .route("/api/v1/datasets", axum::routing::get(list_datasets))
        .route(
            "/api/v1/datasets",
            axum::routing::delete(delete_all_datasets),
        )
        .route(
            "/api/v1/datasets/{name}",
            axum::routing::delete(delete_dataset),
        )
        .route(
            "/api/v1/communities",
            axum::routing::get(list_communities),
        )
        .route(
            "/api/v1/communities/rebuild",
            axum::routing::post(rebuild_communities),
        )
        .with_state(state)
        .layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024)) // 4MB
}

async fn health() -> &'static str {
    "ok"
}

async fn add_document(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    match pipeline::add_document(
        state.graph_store.as_ref(),
        state.vector_store.as_ref(),
        state.embedder.as_ref(),
        &state.chunker_config,
        &req.dataset_name,
        &req.content,
        req.source_ref.as_deref(),
        req.sha256.as_deref(),
    )
    .await
    {
        Ok(doc_id) => (
            StatusCode::OK,
            Json(ApiResponse::ok(&format!("Added document {doc_id}"))),
        ),
        Err(e) => {
            tracing::error!(error = %e, "add_document failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error("Failed to add document")),
            )
        }
    }
}

async fn integrate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IntegrateRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    match pipeline::integrate(
        state.graph_store.as_ref(),
        state.vector_store.as_ref(),
        state.embedder.as_ref(),
        state.extractor.as_ref(),
        &req.datasets,
        req.custom_prompt.as_deref(),
    )
    .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(ApiResponse::ok(&format!(
                "Integrated: {} entities, {} relationships ({} chunks processed, {} failed)",
                result.entities_created,
                result.relationships_created,
                result.chunks_processed,
                result.chunks_failed,
            ))),
        ),
        Err(e) => {
            tracing::error!(error = %e, "integrate failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error("Failed to integrate")),
            )
        }
    }
}

async fn search(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchApiRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match pipeline::search(
        state.graph_store.as_ref(),
        state.vector_store.as_ref(),
        state.embedder.as_ref(),
        &req.query,
        req.datasets.as_deref(),
        req.search_type.as_deref(),
        req.top_k,
    )
    .await
    {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(e) => {
            tracing::error!(error = %e, "search failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Search failed"})),
            )
        }
    }
}

async fn list_datasets(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.graph_store.list_channels().await {
        Ok(channels) => {
            let result: Vec<serde_json::Value> = channels
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id.to_string(),
                        "name": c.name,
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!(result)))
        }
        Err(e) => {
            tracing::error!(error = %e, "list_datasets failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Failed to list datasets"})),
            )
        }
    }
}

async fn delete_dataset(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> (StatusCode, Json<ApiResponse>) {
    match pipeline::delete_channel(state.graph_store.as_ref(), &name).await {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::ok("Dataset deleted"))),
        Err(e) => {
            tracing::error!(error = %e, "delete_dataset failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error("Failed to delete dataset")),
            )
        }
    }
}

async fn delete_all_datasets(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<ApiResponse>) {
    match pipeline::delete_all_channels(state.graph_store.as_ref()).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::ok("All datasets deleted")),
        ),
        Err(e) => {
            tracing::error!(error = %e, "delete_all_datasets failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error("Failed to delete all datasets")),
            )
        }
    }
}

async fn list_communities(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.graph_store.list_communities().await {
        Ok(communities) => {
            let result: Vec<serde_json::Value> = communities
                .into_iter()
                .map(|(c, member_count)| {
                    serde_json::json!({
                        "id": c.id,
                        "level": c.level,
                        "name": c.name,
                        "summary": c.summary,
                        "parent_id": c.parent_id,
                        "created_at": c.created_at,
                        "member_count": member_count,
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!(result)))
        }
        Err(e) => {
            tracing::error!(error = %e, "list_communities failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Failed to list communities"})),
            )
        }
    }
}

async fn rebuild_communities(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<ApiResponse>) {
    match crate::communities::detect_communities(state.graph_store.as_ref()).await {
        Ok(communities) => (
            StatusCode::OK,
            Json(ApiResponse::ok(&format!(
                "Detected {} communities",
                communities.len()
            ))),
        ),
        Err(e) => {
            tracing::error!(error = %e, "rebuild_communities failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error("Failed to rebuild communities")),
            )
        }
    }
}
