//! Backward-compatible wrappers around `store::postgres::PostgresGraphStore`.
//!
//! These free functions accept `&PgPool` and delegate to the trait implementation,
//! preserving the API that existing integration tests rely on.

use sqlx::PgPool;

use crate::store::postgres::PostgresGraphStore;
use crate::store::GraphStore;
use crate::types::{Channel, Entity, Relationship};

pub async fn insert_entity(pool: &PgPool, entity: &Entity) -> anyhow::Result<()> {
    PostgresGraphStore::new(pool.clone()).insert_entity(entity).await
}

pub async fn insert_relationship(pool: &PgPool, rel: &Relationship) -> anyhow::Result<()> {
    PostgresGraphStore::new(pool.clone())
        .insert_relationship(rel)
        .await
}

pub async fn insert_entity_channel(
    pool: &PgPool,
    entity_id: &str,
    channel_id: i32,
    document_id: &str,
) -> anyhow::Result<()> {
    PostgresGraphStore::new(pool.clone())
        .insert_entity_channel(entity_id, channel_id, document_id)
        .await
}

pub async fn insert_entity_alias(
    pool: &PgPool,
    alias: &str,
    entity_id: &str,
    document_id: Option<&str>,
) -> anyhow::Result<()> {
    PostgresGraphStore::new(pool.clone())
        .insert_entity_alias(alias, entity_id, document_id)
        .await
}

pub async fn insert_entity_chunk(
    pool: &PgPool,
    entity_id: &str,
    chunk_id: &str,
) -> anyhow::Result<()> {
    PostgresGraphStore::new(pool.clone())
        .insert_entity_chunk(entity_id, chunk_id)
        .await
}

pub async fn find_entity_by_name(pool: &PgPool, name: &str) -> anyhow::Result<Option<Entity>> {
    PostgresGraphStore::new(pool.clone())
        .find_entity_by_name(name)
        .await
}

pub async fn find_entity_by_alias(pool: &PgPool, alias: &str) -> anyhow::Result<Option<Entity>> {
    PostgresGraphStore::new(pool.clone())
        .find_entity_by_alias(alias)
        .await
}

pub async fn get_entity_channels(pool: &PgPool, entity_id: &str) -> anyhow::Result<Vec<String>> {
    PostgresGraphStore::new(pool.clone())
        .get_entity_channels(entity_id)
        .await
}

pub async fn get_entity_relationships(
    pool: &PgPool,
    entity_id: &str,
    include_expired: bool,
) -> anyhow::Result<Vec<(Relationship, String, String)>> {
    PostgresGraphStore::new(pool.clone())
        .get_entity_relationships(entity_id, include_expired)
        .await
}

pub async fn traverse(
    pool: &PgPool,
    start_entity_id: &str,
    max_depth: i32,
    channel_ids: Option<&[i32]>,
    include_expired: bool,
) -> anyhow::Result<Vec<(Entity, i32)>> {
    PostgresGraphStore::new(pool.clone())
        .traverse(start_entity_id, max_depth, channel_ids, include_expired)
        .await
}

pub async fn supersede_relationship(pool: &PgPool, relationship_id: &str) -> anyhow::Result<()> {
    PostgresGraphStore::new(pool.clone())
        .supersede_relationship(relationship_id)
        .await
}

pub async fn delete_channel(pool: &PgPool, channel_id: i32) -> anyhow::Result<()> {
    // The old API took a channel_id (i32). Look up the name and delegate.
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM channels WHERE id = $1")
        .bind(channel_id)
        .fetch_optional(pool)
        .await?;

    match row {
        Some((name,)) => {
            PostgresGraphStore::new(pool.clone())
                .delete_channel(&name)
                .await
        }
        None => anyhow::bail!("channel with id {} not found", channel_id),
    }
}

pub async fn list_channels(pool: &PgPool) -> anyhow::Result<Vec<Channel>> {
    PostgresGraphStore::new(pool.clone())
        .list_channels()
        .await
}
