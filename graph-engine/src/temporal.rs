use crate::store::GraphStore;
use crate::types::Relationship;

// Re-export the store-based functions for callers that use &dyn GraphStore.
// Backward-compatible PgPool-based overloads live below for integration tests.

/// Result of checking for contradictions between a new relationship and existing ones.
pub enum ContradictionResult {
    /// Identical fact -- no action needed, just add source.
    Identical(String), // existing relationship ID
    /// New fact, no existing relationship on this pair+type.
    Novel,
    /// Potential contradiction -- needs LLM verification or caller decision.
    PotentialContradiction {
        existing_id: String,
        existing_fact: Option<String>,
    },
}

/// Check if a new relationship contradicts, duplicates, or is novel relative to existing graph.
pub async fn check_contradiction(
    store: &dyn GraphStore,
    source_id: &str,
    target_id: &str,
    relationship: &str,
    new_fact: Option<&str>,
) -> anyhow::Result<ContradictionResult> {
    // Find existing active relationships between same entities with same type
    let existing = store
        .find_active_relationships(source_id, target_id, relationship)
        .await?;

    if existing.is_empty() {
        return Ok(ContradictionResult::Novel);
    }

    // Check for identical facts (exact text match after normalization)
    if let Some(new_fact_text) = new_fact {
        let normalized_new = new_fact_text.trim().to_lowercase();
        for (id, existing_fact) in &existing {
            if let Some(ef) = existing_fact {
                let normalized_existing = ef.trim().to_lowercase();
                if normalized_new == normalized_existing {
                    return Ok(ContradictionResult::Identical(id.clone()));
                }
            }
        }
    }

    // Different fact text on same pair+type -- potential contradiction
    let (id, fact) = existing.into_iter().next().unwrap();
    Ok(ContradictionResult::PotentialContradiction {
        existing_id: id,
        existing_fact: fact,
    })
}

/// Supersede a specific relationship (set valid_until to now).
pub async fn supersede(store: &dyn GraphStore, relationship_id: &str) -> anyhow::Result<()> {
    store.supersede_relationship(relationship_id).await
}

/// Query current (non-expired) relationships for an entity.
pub async fn get_current_relationships(
    store: &dyn GraphStore,
    entity_id: &str,
) -> anyhow::Result<Vec<Relationship>> {
    store.get_current_relationships(entity_id).await
}

// ---------------------------------------------------------------------------
// Backward-compatible PgPool wrappers for integration tests
// ---------------------------------------------------------------------------

/// Check contradiction using a PgPool (convenience wrapper).
pub async fn check_contradiction_pg(
    pool: &sqlx::PgPool,
    source_id: &str,
    target_id: &str,
    relationship: &str,
    new_fact: Option<&str>,
) -> anyhow::Result<ContradictionResult> {
    let store = crate::store::postgres::PostgresGraphStore::new(pool.clone());
    check_contradiction(&store, source_id, target_id, relationship, new_fact).await
}

/// Supersede a relationship using a PgPool (convenience wrapper).
pub async fn supersede_pg(pool: &sqlx::PgPool, relationship_id: &str) -> anyhow::Result<()> {
    let store = crate::store::postgres::PostgresGraphStore::new(pool.clone());
    supersede(&store, relationship_id).await
}

/// Get current relationships using a PgPool (convenience wrapper).
pub async fn get_current_relationships_pg(
    pool: &sqlx::PgPool,
    entity_id: &str,
) -> anyhow::Result<Vec<Relationship>> {
    let store = crate::store::postgres::PostgresGraphStore::new(pool.clone());
    get_current_relationships(&store, entity_id).await
}

/// Query relationships for an entity as they existed at a specific point in time.
/// This requires the Postgres backend directly since temporal range queries
/// with arbitrary timestamps are backend-specific.
pub async fn get_historical_relationships(
    pool: &sqlx::PgPool,
    entity_id: &str,
    as_of: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<Vec<Relationship>> {
    let rows: Vec<crate::types::RelRow> = sqlx::query_as(
        "SELECT id, source_id, target_id, relationship, fact, properties,
                confidence, channel_id, document_id, valid_from, valid_until,
                ingested_at, created_at, COALESCE(weight, 1.0) AS weight
         FROM relationships
         WHERE (source_id = $1 OR target_id = $1)
           AND valid_from <= $2
           AND (valid_until IS NULL OR valid_until > $2)",
    )
    .bind(entity_id)
    .bind(as_of)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}
