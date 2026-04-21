use std::collections::HashMap;

use crate::store::GraphStore;
use crate::types::{Community, Entity};

/// Run community detection on the entity graph.
/// Uses label propagation: each entity starts as its own community,
/// then iteratively adopts the most common community label among its neighbors.
/// Converges when no labels change.
pub async fn detect_communities(store: &dyn GraphStore) -> anyhow::Result<Vec<Community>> {
    // Fetch all entities and their relationships (active only)
    let entities = store.get_all_entities().await?;
    let edges = store.get_all_active_edges().await?;

    if entities.is_empty() {
        return Ok(vec![]);
    }

    // Build adjacency list (undirected)
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (src, tgt) in &edges {
        adjacency.entry(src.clone()).or_default().push(tgt.clone());
        adjacency.entry(tgt.clone()).or_default().push(src.clone());
    }

    // Initialize labels: each entity is its own community
    let mut labels: HashMap<String, String> = entities
        .iter()
        .map(|(id, _)| (id.clone(), id.clone()))
        .collect();

    // Label propagation -- iterate until stable
    let max_iterations = 20;
    for _iteration in 0..max_iterations {
        let mut changed = false;

        for (entity_id, _) in &entities {
            let neighbors = match adjacency.get(entity_id) {
                Some(n) if !n.is_empty() => n,
                _ => continue,
            };

            // Find most common label among neighbors
            let mut label_counts: HashMap<&String, usize> = HashMap::new();
            for neighbor in neighbors {
                if let Some(label) = labels.get(neighbor) {
                    *label_counts.entry(label).or_insert(0) += 1;
                }
            }

            if let Some((best_label, _)) = label_counts.into_iter().max_by_key(|(_, count)| *count)
            {
                let current = labels.get(entity_id).unwrap();
                if best_label != current {
                    labels.insert(entity_id.clone(), best_label.clone());
                    changed = true;
                }
            }
        }

        if !changed {
            break;
        }
    }

    // Group entities by community label
    let mut community_groups: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (entity_id, entity_name) in &entities {
        let label = labels.get(entity_id).unwrap();
        community_groups
            .entry(label.clone())
            .or_default()
            .push((entity_id.clone(), entity_name.clone()));
    }

    // Filter out single-entity communities (not useful)
    community_groups.retain(|_, members| members.len() > 1);

    // Clear existing communities
    store.clear_communities().await?;

    // Create community records
    let mut communities = Vec::new();
    for (_, members) in &community_groups {
        let community_id = ulid::Ulid::new().to_string();

        // Name the community after the most-connected entity
        let name = members
            .iter()
            .max_by_key(|(id, _)| adjacency.get(id).map(|n| n.len()).unwrap_or(0))
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| "Unnamed".to_string());

        let community = Community {
            id: community_id.clone(),
            level: 0,
            name: Some(name),
            summary: None,
            summary_embedding: None,
            parent_id: None,
            created_at: chrono::Utc::now(),
        };

        store.insert_community(&community).await?;

        for (entity_id, _) in members {
            store
                .insert_community_member(&community_id, entity_id)
                .await?;
        }

        communities.push(community);
    }

    Ok(communities)
}

/// List all communities with their member counts.
pub async fn list_communities(store: &dyn GraphStore) -> anyhow::Result<Vec<(Community, i64)>> {
    store.list_communities().await
}

/// Get entities belonging to a community.
pub async fn get_community_members(
    store: &dyn GraphStore,
    community_id: &str,
) -> anyhow::Result<Vec<Entity>> {
    store.get_community_members(community_id).await
}

// ---------------------------------------------------------------------------
// Backward-compatible PgPool wrappers for integration tests
// ---------------------------------------------------------------------------

/// Detect communities using a PgPool (convenience wrapper).
pub async fn detect_communities_pg(pool: &sqlx::PgPool) -> anyhow::Result<Vec<Community>> {
    let store = crate::store::postgres::PostgresGraphStore::new(pool.clone());
    detect_communities(&store).await
}

/// List communities using a PgPool (convenience wrapper).
pub async fn list_communities_pg(
    pool: &sqlx::PgPool,
) -> anyhow::Result<Vec<(Community, i64)>> {
    let store = crate::store::postgres::PostgresGraphStore::new(pool.clone());
    list_communities(&store).await
}

/// Get community members using a PgPool (convenience wrapper).
pub async fn get_community_members_pg(
    pool: &sqlx::PgPool,
    community_id: &str,
) -> anyhow::Result<Vec<Entity>> {
    let store = crate::store::postgres::PostgresGraphStore::new(pool.clone());
    get_community_members(&store, community_id).await
}
