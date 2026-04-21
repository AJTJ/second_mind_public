//! Backward-compatible wrappers around `store::postgres::PostgresVectorStore`.

use sqlx::PgPool;

use crate::store::postgres::PostgresVectorStore;
use crate::store::VectorStore;
use crate::types::{ChunkResult, EntityResult};

pub async fn set_chunk_embedding(
    pool: &PgPool,
    chunk_id: &str,
    embedding: &[f32],
) -> anyhow::Result<()> {
    PostgresVectorStore::new(pool.clone())
        .set_chunk_embedding(chunk_id, embedding)
        .await
}

pub async fn set_entity_embedding(
    pool: &PgPool,
    entity_id: &str,
    embedding: &[f32],
) -> anyhow::Result<()> {
    PostgresVectorStore::new(pool.clone())
        .set_entity_embedding(entity_id, embedding)
        .await
}

pub async fn search_chunks(
    pool: &PgPool,
    query_embedding: &[f32],
    channel_ids: Option<&[i32]>,
    top_k: i32,
) -> anyhow::Result<Vec<ChunkResult>> {
    PostgresVectorStore::new(pool.clone())
        .search_chunks(query_embedding, channel_ids, top_k)
        .await
}

pub async fn search_entities(
    pool: &PgPool,
    query_embedding: &[f32],
    channel_ids: Option<&[i32]>,
    top_k: i32,
) -> anyhow::Result<Vec<EntityResult>> {
    PostgresVectorStore::new(pool.clone())
        .search_entities(query_embedding, channel_ids, top_k)
        .await
}
