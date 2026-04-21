//! Backward-compatible wrappers around `store::postgres::PostgresGraphStore`.

use sqlx::PgPool;

use crate::store::postgres::PostgresGraphStore;
use crate::store::GraphStore;

/// Run the initial migration and ensure the schema is ready.
pub async fn initialize(pool: &PgPool) -> anyhow::Result<()> {
    PostgresGraphStore::new(pool.clone()).initialize().await
}

/// Ensure a channel exists, returning its id. Creates it if missing.
pub async fn ensure_channel(pool: &PgPool, name: &str) -> anyhow::Result<i32> {
    PostgresGraphStore::new(pool.clone())
        .ensure_channel(name)
        .await
}
