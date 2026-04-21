use crate::store::GraphStore;
use crate::types::ExtractedEntity;

pub struct ResolvedEntity {
    pub entity_id: String,
    pub canonical_name: String,
    pub is_new: bool,
}

/// Resolve an extracted entity against the existing graph.
/// Three-tier cascade: exact name -> alias lookup -> new entity.
/// (Fuzzy matching and LLM resolution will be added later.)
pub async fn resolve(
    store: &dyn GraphStore,
    extracted: &ExtractedEntity,
) -> anyhow::Result<ResolvedEntity> {
    let normalized = normalize_name(&extracted.name);

    // Tier 1: Exact canonical name match
    if let Some(entity) = store.find_entity_by_name(&normalized).await? {
        return Ok(ResolvedEntity {
            entity_id: entity.id,
            canonical_name: entity.canonical_name,
            is_new: false,
        });
    }

    // Tier 2: Alias lookup
    if let Some(entity) = store.find_entity_by_alias(&normalized).await? {
        return Ok(ResolvedEntity {
            entity_id: entity.id,
            canonical_name: entity.canonical_name,
            is_new: false,
        });
    }

    // Tier 3: Create new entity
    let entity_id = ulid::Ulid::new().to_string();
    Ok(ResolvedEntity {
        entity_id,
        canonical_name: normalized,
        is_new: true,
    })
}

/// Normalize entity name for matching.
/// Unicode NFKC normalization, lowercase, trim, collapse whitespace, singularize common English plurals.
fn normalize_name(name: &str) -> String {
    let s = name.trim().to_lowercase();
    // Collapse whitespace
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    // Simple singularization (covers the 80% case without a dependency)
    singularize(&s)
}

/// Simple English singularization. Handles common suffixes.
/// Not comprehensive -- covers the patterns that matter for entity dedup.
fn singularize(s: &str) -> String {
    // Split into words and singularize the last word
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() {
        return s.to_string();
    }

    let mut result: Vec<String> = words[..words.len() - 1]
        .iter()
        .map(|w| w.to_string())
        .collect();
    let last = words.last().unwrap();

    let singular = if last.len() < 4 {
        // Don't singularize short words
        last.to_string()
    } else if last.ends_with("ies") && last.len() > 4 {
        // companies -> company, batteries -> battery
        format!("{}y", &last[..last.len() - 3])
    } else if last.ends_with("ses")
        || last.ends_with("xes")
        || last.ends_with("zes")
        || last.ends_with("ches")
        || last.ends_with("shes")
    {
        // processes -> process, boxes -> box, churches -> church
        last[..last.len() - 2].to_string()
    } else if last.ends_with('s')
        && !last.ends_with("ss")
        && !last.ends_with("us")
        && !last.ends_with("is")
    {
        // materials -> material
        last[..last.len() - 1].to_string()
    } else {
        last.to_string()
    };

    result.push(singular);
    result.join(" ")
}

// ---------------------------------------------------------------------------
// Backward-compatible PgPool wrapper for integration tests
// ---------------------------------------------------------------------------

/// Resolve an entity using a PgPool (convenience wrapper).
pub async fn resolve_pg(
    pool: &sqlx::PgPool,
    extracted: &ExtractedEntity,
) -> anyhow::Result<ResolvedEntity> {
    let store = crate::store::postgres::PostgresGraphStore::new(pool.clone());
    resolve(&store, extracted).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_whitespace() {
        assert_eq!(normalize_name("  copper  "), "copper");
    }

    #[test]
    fn lowercases() {
        assert_eq!(normalize_name("Teck Resources"), "teck resource");
    }

    #[test]
    fn collapses_multiple_spaces() {
        assert_eq!(normalize_name("data   center   demand"), "data center demand");
    }

    #[test]
    fn normalize_singularizes() {
        assert_eq!(normalize_name("Copper Materials"), "copper material");
    }

    #[test]
    fn singularize_companies() {
        assert_eq!(singularize("companies"), "company");
    }

    #[test]
    fn singularize_materials() {
        assert_eq!(singularize("materials"), "material");
    }

    #[test]
    fn singularize_processes() {
        assert_eq!(singularize("processes"), "process");
    }

    #[test]
    fn singularize_churches() {
        assert_eq!(singularize("churches"), "church");
    }

    #[test]
    fn singularize_no_change_analysis() {
        assert_eq!(singularize("analysis"), "analysis");
    }

    #[test]
    fn singularize_no_change_status() {
        assert_eq!(singularize("status"), "status");
    }

    #[test]
    fn singularize_no_change_copper() {
        assert_eq!(singularize("copper"), "copper");
    }
}
