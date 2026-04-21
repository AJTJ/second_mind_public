pub mod neo4j;
pub mod postgres;

use async_trait::async_trait;

use crate::types::*;

/// Abstraction over graph storage backends (Postgres, Neo4j, etc.).
#[async_trait]
pub trait GraphStore: Send + Sync {
    // Schema
    async fn initialize(&self) -> anyhow::Result<()>;
    async fn ensure_channel(&self, name: &str) -> anyhow::Result<i32>;

    // Entities
    async fn insert_entity(&self, entity: &Entity) -> anyhow::Result<()>;
    async fn find_entity_by_name(&self, name: &str) -> anyhow::Result<Option<Entity>>;
    async fn find_entity_by_alias(&self, alias: &str) -> anyhow::Result<Option<Entity>>;
    async fn insert_entity_channel(
        &self,
        entity_id: &str,
        channel_id: i32,
        document_id: &str,
    ) -> anyhow::Result<()>;
    async fn insert_entity_alias(
        &self,
        alias: &str,
        entity_id: &str,
        document_id: Option<&str>,
    ) -> anyhow::Result<()>;
    async fn insert_entity_chunk(&self, entity_id: &str, chunk_id: &str) -> anyhow::Result<()>;
    async fn get_entity_channels(&self, entity_id: &str) -> anyhow::Result<Vec<String>>;

    // Relationships
    async fn insert_relationship(&self, rel: &Relationship) -> anyhow::Result<()>;
    async fn get_entity_relationships(
        &self,
        entity_id: &str,
        include_expired: bool,
    ) -> anyhow::Result<Vec<(Relationship, String, String)>>;

    // Traversal
    async fn traverse(
        &self,
        start_entity_id: &str,
        max_depth: i32,
        channel_ids: Option<&[i32]>,
        include_expired: bool,
    ) -> anyhow::Result<Vec<(Entity, i32)>>;

    // Temporal
    async fn find_active_relationships(
        &self,
        source_id: &str,
        target_id: &str,
        relationship: &str,
    ) -> anyhow::Result<Vec<(String, Option<String>)>>;
    async fn supersede_relationship(&self, relationship_id: &str) -> anyhow::Result<()>;
    async fn get_current_relationships(
        &self,
        entity_id: &str,
    ) -> anyhow::Result<Vec<Relationship>>;

    // Documents and chunks
    async fn insert_document(
        &self,
        id: &str,
        channel_id: i32,
        source_ref: &str,
        sha256: &str,
    ) -> anyhow::Result<()>;
    async fn find_document_by_hash(
        &self,
        channel_id: i32,
        sha256: &str,
    ) -> anyhow::Result<Option<String>>;
    async fn insert_chunk(
        &self,
        id: &str,
        document_id: &str,
        content: &str,
        chunk_index: i32,
    ) -> anyhow::Result<()>;
    /// Returns (chunk_id, content, document_id) for chunks not yet processed (integrated).
    async fn get_unprocessed_chunks(
        &self,
        channel_id: i32,
    ) -> anyhow::Result<Vec<(String, String, String)>>;
    async fn get_chunk_document_id(&self, chunk_id: &str) -> anyhow::Result<Option<String>>;

    // Channels
    async fn list_channels(&self) -> anyhow::Result<Vec<Channel>>;
    async fn delete_channel(&self, name: &str) -> anyhow::Result<()>;
    async fn delete_all_channels(&self) -> anyhow::Result<()>;

    // --- Health check ---

    /// Test that the underlying store is reachable. Default: assume healthy.
    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }

    // Communities (delegated to CommunityStore super-trait)
    async fn clear_communities(&self) -> anyhow::Result<()>;
    async fn insert_community(&self, community: &Community) -> anyhow::Result<()>;
    async fn insert_community_member(
        &self,
        community_id: &str,
        entity_id: &str,
    ) -> anyhow::Result<()>;
    async fn list_communities(&self) -> anyhow::Result<Vec<(Community, i64)>>;
    async fn get_community_members(&self, community_id: &str) -> anyhow::Result<Vec<Entity>>;

    // For community detection -- need all entities and edges
    /// Returns (id, canonical_name) for every entity.
    async fn get_all_entities(&self) -> anyhow::Result<Vec<(String, String)>>;
    /// Returns (source_id, target_id) for every active (non-expired) edge.
    async fn get_all_active_edges(&self) -> anyhow::Result<Vec<(String, String)>>;

    // Cache
    async fn cache_get(&self, key: &str) -> anyhow::Result<Option<String>>;
    async fn cache_set(&self, key: &str, model: &str, response: &str) -> anyhow::Result<()>;

    // --- Rich traversal ---

    /// Rich traversal that returns full paths through the graph.
    /// Each path is a sequence of (entity, relationship, entity) triples.
    async fn traverse_rich(
        &self,
        _start_entity_id: &str,
        _max_depth: i32,
        _channel_ids: Option<&[i32]>,
        _include_expired: bool,
    ) -> anyhow::Result<Vec<TraversalPath>> {
        Ok(vec![])
    }

    // --- Full-text search ---

    /// Full-text search on entity names (fuzzy, no embedding needed).
    async fn search_entities_fulltext(
        &self,
        _query: &str,
        _top_k: i32,
    ) -> anyhow::Result<Vec<(Entity, f64)>> {
        Ok(vec![])
    }

    // --- Entity description enrichment ---

    /// Update an entity's description in its properties JSON.
    async fn update_entity_description(
        &self,
        _entity_id: &str,
        _description: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    // --- Relationship weight ---

    /// Increment the weight of a relationship (episode accumulation).
    async fn increment_relationship_weight(&self, _relationship_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Abstraction over vector storage backends.
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn set_chunk_embedding(&self, chunk_id: &str, embedding: &[f32]) -> anyhow::Result<()>;
    async fn set_entity_embedding(
        &self,
        entity_id: &str,
        embedding: &[f32],
    ) -> anyhow::Result<()>;
    async fn search_chunks(
        &self,
        query_embedding: &[f32],
        channel_ids: Option<&[i32]>,
        top_k: i32,
    ) -> anyhow::Result<Vec<ChunkResult>>;
    async fn search_entities(
        &self,
        query_embedding: &[f32],
        channel_ids: Option<&[i32]>,
        top_k: i32,
    ) -> anyhow::Result<Vec<EntityResult>>;
}
