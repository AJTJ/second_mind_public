use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pgvector::Vector;
use sqlx::PgPool;
use tracing::{error, info};

use super::{GraphStore, VectorStore};
use crate::types::*;

// ---------------------------------------------------------------------------
// PostgresGraphStore
// ---------------------------------------------------------------------------

pub struct PostgresGraphStore {
    pool: PgPool,
}

impl PostgresGraphStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Expose the underlying pool for callers that still need raw access
    /// (e.g. integration tests, LlmExtractor cache).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl GraphStore for PostgresGraphStore {
    // -- Health ---------------------------------------------------------------

    async fn health_check(&self) -> anyhow::Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    // -- Schema ---------------------------------------------------------------

    async fn initialize(&self) -> anyhow::Result<()> {
        // pgvector extension MUST be created before any table that uses the vector type.
        // Fail fast with a clear error if this doesn't work.
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&self.pool)
            .await
            .map_err(|e| {
                error!(
                    "Failed to create pgvector extension. \
                     Ensure the 'vector' extension is installed in PostgreSQL \
                     (e.g. via the pgvector package). Error: {e}"
                );
                anyhow::anyhow!(
                    "pgvector extension creation failed — \
                     cannot proceed with schema initialization: {e}"
                )
            })?;
        info!("pgvector extension ready");

        let statements = [
            // Channels
            "CREATE TABLE IF NOT EXISTS channels (
                id SERIAL PRIMARY KEY,
                name TEXT UNIQUE NOT NULL
            )",
            // Documents
            "CREATE TABLE IF NOT EXISTS documents (
                id TEXT PRIMARY KEY,
                channel_id INT REFERENCES channels NOT NULL,
                source_ref TEXT NOT NULL,
                sha256 TEXT NOT NULL,
                ingested_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            // Chunks
            "CREATE TABLE IF NOT EXISTS chunks (
                id TEXT PRIMARY KEY,
                document_id TEXT REFERENCES documents NOT NULL,
                content TEXT NOT NULL,
                chunk_index INT NOT NULL,
                embedding vector(2560),
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            // Entities
            "CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                canonical_name TEXT NOT NULL,
                entity_type TEXT,
                properties JSONB NOT NULL DEFAULT '{}',
                embedding vector(2560),
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS entities_name_idx ON entities (canonical_name)",
            "CREATE INDEX IF NOT EXISTS entities_type_idx ON entities (entity_type)",
            // Entity-channel membership
            "CREATE TABLE IF NOT EXISTS entity_channels (
                entity_id TEXT REFERENCES entities NOT NULL,
                channel_id INT REFERENCES channels NOT NULL,
                document_id TEXT REFERENCES documents NOT NULL,
                PRIMARY KEY (entity_id, channel_id, document_id)
            )",
            // Entity aliases
            "CREATE TABLE IF NOT EXISTS entity_aliases (
                alias TEXT NOT NULL,
                entity_id TEXT REFERENCES entities NOT NULL,
                source_document_id TEXT REFERENCES documents,
                PRIMARY KEY (alias, entity_id)
            )",
            "CREATE INDEX IF NOT EXISTS entity_aliases_alias_idx ON entity_aliases (alias)",
            // Relationships
            "CREATE TABLE IF NOT EXISTS relationships (
                id TEXT PRIMARY KEY,
                source_id TEXT REFERENCES entities NOT NULL,
                target_id TEXT REFERENCES entities NOT NULL,
                relationship TEXT NOT NULL,
                fact TEXT,
                properties JSONB NOT NULL DEFAULT '{}',
                confidence TEXT NOT NULL DEFAULT 'established',
                channel_id INT REFERENCES channels NOT NULL,
                document_id TEXT REFERENCES documents NOT NULL,
                valid_from TIMESTAMPTZ NOT NULL DEFAULT now(),
                valid_until TIMESTAMPTZ,
                ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                weight DOUBLE PRECISION NOT NULL DEFAULT 1.0
            )",
            "CREATE INDEX IF NOT EXISTS relationships_source_idx ON relationships (source_id)",
            "CREATE INDEX IF NOT EXISTS relationships_target_idx ON relationships (target_id)",
            "CREATE INDEX IF NOT EXISTS relationships_type_idx ON relationships (relationship)",
            "CREATE INDEX IF NOT EXISTS relationships_valid_idx ON relationships (valid_until) WHERE valid_until IS NULL",
            // Entity-chunk provenance
            "CREATE TABLE IF NOT EXISTS entity_chunks (
                entity_id TEXT REFERENCES entities NOT NULL,
                chunk_id TEXT REFERENCES chunks NOT NULL,
                PRIMARY KEY (entity_id, chunk_id)
            )",
            // Communities
            "CREATE TABLE IF NOT EXISTS communities (
                id TEXT PRIMARY KEY,
                level INT NOT NULL,
                name TEXT,
                summary TEXT,
                summary_embedding vector(2560),
                parent_id TEXT REFERENCES communities,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS communities_level_idx ON communities (level)",
            // Community membership
            "CREATE TABLE IF NOT EXISTS community_members (
                community_id TEXT REFERENCES communities NOT NULL,
                entity_id TEXT REFERENCES entities NOT NULL,
                PRIMARY KEY (community_id, entity_id)
            )",
            // LLM response cache
            "CREATE TABLE IF NOT EXISTS llm_cache (
                hash TEXT PRIMARY KEY,
                model TEXT NOT NULL,
                response TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
        ];

        for stmt in &statements {
            sqlx::query(stmt).execute(&self.pool).await?;
        }

        // Migrations: add columns that may not exist on older schemas.
        // These are idempotent -- the DO NOTHING handles existing columns.
        let migrations = [
            "ALTER TABLE relationships ADD COLUMN IF NOT EXISTS weight DOUBLE PRECISION NOT NULL DEFAULT 1.0",
        ];
        for stmt in &migrations {
            sqlx::query(stmt).execute(&self.pool).await?;
        }

        info!("schema initialized ({} statements)", statements.len());
        Ok(())
    }

    async fn ensure_channel(&self, name: &str) -> anyhow::Result<i32> {
        sqlx::query("INSERT INTO channels (name) VALUES ($1) ON CONFLICT (name) DO NOTHING")
            .bind(name)
            .execute(&self.pool)
            .await?;

        let row: (i32,) = sqlx::query_as("SELECT id FROM channels WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await?;

        Ok(row.0)
    }

    // -- Entities -------------------------------------------------------------

    async fn insert_entity(&self, entity: &Entity) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO entities (id, canonical_name, entity_type, properties, created_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE SET
                canonical_name = EXCLUDED.canonical_name,
                entity_type = EXCLUDED.entity_type,
                properties = EXCLUDED.properties",
        )
        .bind(&entity.id)
        .bind(&entity.canonical_name)
        .bind(&entity.entity_type)
        .bind(&entity.properties)
        .bind(entity.created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn find_entity_by_name(&self, name: &str) -> anyhow::Result<Option<Entity>> {
        let row = sqlx::query_as::<_, EntityRow>(
            "SELECT id, canonical_name, entity_type, properties, created_at
             FROM entities
             WHERE canonical_name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    async fn find_entity_by_alias(&self, alias: &str) -> anyhow::Result<Option<Entity>> {
        let row = sqlx::query_as::<_, EntityRow>(
            "SELECT e.id, e.canonical_name, e.entity_type, e.properties, e.created_at
             FROM entities e
             JOIN entity_aliases a ON a.entity_id = e.id
             WHERE a.alias = $1",
        )
        .bind(alias)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    async fn insert_entity_channel(
        &self,
        entity_id: &str,
        channel_id: i32,
        document_id: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO entity_channels (entity_id, channel_id, document_id)
             VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(entity_id)
        .bind(channel_id)
        .bind(document_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn insert_entity_alias(
        &self,
        alias: &str,
        entity_id: &str,
        document_id: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO entity_aliases (alias, entity_id, source_document_id)
             VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(alias)
        .bind(entity_id)
        .bind(document_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn insert_entity_chunk(&self, entity_id: &str, chunk_id: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO entity_chunks (entity_id, chunk_id)
             VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(entity_id)
        .bind(chunk_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_entity_channels(&self, entity_id: &str) -> anyhow::Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT c.name
             FROM entity_channels ec
             JOIN channels c ON c.id = ec.channel_id
             WHERE ec.entity_id = $1",
        )
        .bind(entity_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    // -- Relationships --------------------------------------------------------

    async fn insert_relationship(&self, rel: &Relationship) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO relationships
                (id, source_id, target_id, relationship, fact, properties,
                 confidence, channel_id, document_id, valid_from, valid_until,
                 ingested_at, created_at, weight)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(&rel.id)
        .bind(&rel.source_id)
        .bind(&rel.target_id)
        .bind(&rel.relationship)
        .bind(&rel.fact)
        .bind(&rel.properties)
        .bind(rel.confidence.to_string())
        .bind(rel.channel_id)
        .bind(&rel.document_id)
        .bind(rel.valid_from)
        .bind(rel.valid_until)
        .bind(rel.ingested_at)
        .bind(rel.created_at)
        .bind(rel.weight)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_entity_relationships(
        &self,
        entity_id: &str,
        include_expired: bool,
    ) -> anyhow::Result<Vec<(Relationship, String, String)>> {
        let rows: Vec<RelationshipRow> = sqlx::query_as::<_, RelationshipRow>(
            "SELECT r.id, r.source_id, r.target_id, r.relationship, r.fact,
                    r.properties, r.confidence, r.channel_id, r.document_id,
                    r.valid_from, r.valid_until, r.ingested_at, r.created_at,
                    COALESCE(r.weight, 1.0) AS weight,
                    src.canonical_name AS source_name,
                    tgt.canonical_name AS target_name
             FROM relationships r
             JOIN entities src ON src.id = r.source_id
             JOIN entities tgt ON tgt.id = r.target_id
             WHERE (r.source_id = $1 OR r.target_id = $1)
               AND ($2::bool OR r.valid_until IS NULL)",
        )
        .bind(entity_id)
        .bind(include_expired)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                let rel = Relationship {
                    id: r.id,
                    source_id: r.source_id,
                    target_id: r.target_id,
                    relationship: r.relationship,
                    fact: r.fact,
                    properties: r.properties,
                    confidence: r.confidence.parse().unwrap_or(Confidence::Established),
                    channel_id: r.channel_id,
                    document_id: r.document_id,
                    valid_from: r.valid_from,
                    valid_until: r.valid_until,
                    ingested_at: r.ingested_at,
                    created_at: r.created_at,
                    weight: r.weight,
                };
                Ok((rel, r.source_name, r.target_name))
            })
            .collect()
    }

    // -- Traversal ------------------------------------------------------------

    async fn traverse(
        &self,
        start_entity_id: &str,
        max_depth: i32,
        channel_ids: Option<&[i32]>,
        include_expired: bool,
    ) -> anyhow::Result<Vec<(Entity, i32)>> {
        let rows: Vec<TraversalRow> = sqlx::query_as::<_, TraversalRow>(
            "WITH RECURSIVE traverse AS (
                SELECT e.id, e.canonical_name, e.entity_type, e.properties, e.created_at,
                       0 AS depth, ARRAY[e.id] AS path
                FROM entities e
                WHERE e.id = $1
                UNION ALL
                SELECT e2.id, e2.canonical_name, e2.entity_type, e2.properties, e2.created_at,
                       t.depth + 1, t.path || e2.id
                FROM traverse t
                JOIN relationships r ON r.source_id = t.id
                JOIN entities e2 ON e2.id = r.target_id
                WHERE t.depth < $2
                  AND NOT e2.id = ANY(t.path)
                  AND ($3::int[] IS NULL OR r.channel_id = ANY($3))
                  AND ($4::bool OR r.valid_until IS NULL)
            )
            SELECT DISTINCT ON (id) id, canonical_name, entity_type, properties, created_at, depth
            FROM traverse
            ORDER BY id, depth",
        )
        .bind(start_entity_id)
        .bind(max_depth)
        .bind(channel_ids)
        .bind(include_expired)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let entity = Entity {
                    id: r.id,
                    canonical_name: r.canonical_name,
                    entity_type: r.entity_type,
                    properties: r.properties,
                    embedding: None,
                    created_at: r.created_at,
                };
                (entity, r.depth)
            })
            .collect())
    }

    // -- Temporal -------------------------------------------------------------

    async fn find_active_relationships(
        &self,
        source_id: &str,
        target_id: &str,
        relationship: &str,
    ) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, fact FROM relationships
             WHERE source_id = $1 AND target_id = $2 AND relationship = $3 AND valid_until IS NULL",
        )
        .bind(source_id)
        .bind(target_id)
        .bind(relationship)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    async fn supersede_relationship(&self, relationship_id: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE relationships SET valid_until = now() WHERE id = $1 AND valid_until IS NULL",
        )
        .bind(relationship_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_current_relationships(
        &self,
        entity_id: &str,
    ) -> anyhow::Result<Vec<Relationship>> {
        let rows: Vec<RelRow> = sqlx::query_as(
            "SELECT id, source_id, target_id, relationship, fact, properties,
                    confidence, channel_id, document_id, valid_from, valid_until,
                    ingested_at, created_at, COALESCE(weight, 1.0) AS weight
             FROM relationships
             WHERE (source_id = $1 OR target_id = $1) AND valid_until IS NULL",
        )
        .bind(entity_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    // -- Documents and chunks -------------------------------------------------

    async fn insert_document(
        &self,
        id: &str,
        channel_id: i32,
        source_ref: &str,
        sha256: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO documents (id, channel_id, source_ref, sha256, ingested_at)
             VALUES ($1, $2, $3, $4, now())
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(channel_id)
        .bind(source_ref)
        .bind(sha256)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn find_document_by_hash(
        &self,
        channel_id: i32,
        sha256: &str,
    ) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM documents WHERE channel_id = $1 AND sha256 = $2")
                .bind(channel_id)
                .bind(sha256)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|(id,)| id))
    }

    async fn insert_chunk(
        &self,
        id: &str,
        document_id: &str,
        content: &str,
        chunk_index: i32,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO chunks (id, document_id, content, chunk_index) VALUES ($1, $2, $3, $4)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(document_id)
        .bind(content)
        .bind(chunk_index)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_unprocessed_chunks(
        &self,
        channel_id: i32,
    ) -> anyhow::Result<Vec<(String, String, String)>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT c.id, c.content, c.document_id FROM chunks c
             JOIN documents d ON c.document_id = d.id
             WHERE d.channel_id = $1
               AND NOT EXISTS (SELECT 1 FROM entity_chunks ec WHERE ec.chunk_id = c.id)",
        )
        .bind(channel_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    async fn get_chunk_document_id(&self, chunk_id: &str) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT document_id FROM chunks WHERE id = $1")
                .bind(chunk_id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|(id,)| id))
    }

    // -- Channels -------------------------------------------------------------

    async fn list_channels(&self) -> anyhow::Result<Vec<Channel>> {
        let rows: Vec<(i32, String)> =
            sqlx::query_as("SELECT id, name FROM channels ORDER BY id")
                .fetch_all(&self.pool)
                .await?;

        Ok(rows
            .into_iter()
            .map(|(id, name)| Channel { id, name })
            .collect())
    }

    async fn delete_channel(&self, name: &str) -> anyhow::Result<()> {
        let row: Option<(i32,)> = sqlx::query_as("SELECT id FROM channels WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;

        let channel_id = match row {
            Some((id,)) => id,
            None => anyhow::bail!("channel '{}' not found", name),
        };

        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM entity_channels WHERE channel_id = $1")
            .bind(channel_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM relationships WHERE channel_id = $1")
            .bind(channel_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "DELETE FROM entity_chunks
             WHERE chunk_id IN (
                 SELECT c.id FROM chunks c
                 JOIN documents d ON c.document_id = d.id
                 WHERE d.channel_id = $1
             )",
        )
        .bind(channel_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "DELETE FROM chunks
             WHERE document_id IN (SELECT id FROM documents WHERE channel_id = $1)",
        )
        .bind(channel_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "DELETE FROM entity_aliases
             WHERE source_document_id IN (SELECT id FROM documents WHERE channel_id = $1)",
        )
        .bind(channel_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM documents WHERE channel_id = $1")
            .bind(channel_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM channels WHERE id = $1")
            .bind(channel_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        info!(channel = %name, "channel deleted");
        Ok(())
    }

    async fn delete_all_channels(&self) -> anyhow::Result<()> {
        let channels = GraphStore::list_channels(self).await?;

        for channel in &channels {
            GraphStore::delete_channel(self, &channel.name).await?;
        }

        info!(count = channels.len(), "all channels deleted");
        Ok(())
    }

    // -- Communities ----------------------------------------------------------

    async fn clear_communities(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM community_members")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM communities")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn insert_community(&self, community: &Community) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO communities (id, level, name) VALUES ($1, $2, $3)")
            .bind(&community.id)
            .bind(community.level)
            .bind(&community.name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn insert_community_member(
        &self,
        community_id: &str,
        entity_id: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO community_members (community_id, entity_id) VALUES ($1, $2)")
            .bind(community_id)
            .bind(entity_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_communities(&self) -> anyhow::Result<Vec<(Community, i64)>> {
        let rows: Vec<(
            String,
            i32,
            Option<String>,
            Option<String>,
            Option<String>,
            DateTime<Utc>,
            i64,
        )> = sqlx::query_as(
            "SELECT c.id, c.level, c.name, c.summary, c.parent_id, c.created_at,
             COUNT(cm.entity_id) as member_count
             FROM communities c
             LEFT JOIN community_members cm ON c.id = cm.community_id
             GROUP BY c.id, c.level, c.name, c.summary, c.parent_id, c.created_at
             ORDER BY member_count DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, level, name, summary, parent_id, created_at, count)| {
                    (
                        Community {
                            id,
                            level,
                            name,
                            summary,
                            summary_embedding: None,
                            parent_id,
                            created_at,
                        },
                        count,
                    )
                },
            )
            .collect())
    }

    async fn get_community_members(&self, community_id: &str) -> anyhow::Result<Vec<Entity>> {
        let rows: Vec<(
            String,
            String,
            Option<String>,
            serde_json::Value,
            DateTime<Utc>,
        )> = sqlx::query_as(
            "SELECT e.id, e.canonical_name, e.entity_type, e.properties, e.created_at
             FROM entities e
             JOIN community_members cm ON e.id = cm.entity_id
             WHERE cm.community_id = $1",
        )
        .bind(community_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, canonical_name, entity_type, properties, created_at)| Entity {
                    id,
                    canonical_name,
                    entity_type,
                    properties,
                    embedding: None,
                    created_at,
                },
            )
            .collect())
    }

    // -- All entities/edges (for community detection) -------------------------

    async fn get_all_entities(&self) -> anyhow::Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, canonical_name FROM entities")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn get_all_active_edges(&self) -> anyhow::Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT source_id, target_id FROM relationships WHERE valid_until IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // -- Cache ----------------------------------------------------------------

    async fn cache_get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT response FROM llm_cache WHERE hash = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(r,)| r))
    }

    async fn cache_set(&self, key: &str, model: &str, response: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO llm_cache (hash, model, response) VALUES ($1, $2, $3)
             ON CONFLICT (hash) DO NOTHING",
        )
        .bind(key)
        .bind(model)
        .bind(response)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // -- Rich traversal -------------------------------------------------------

    async fn traverse_rich(
        &self,
        start_entity_id: &str,
        max_depth: i32,
        channel_ids: Option<&[i32]>,
        include_expired: bool,
    ) -> anyhow::Result<Vec<TraversalPath>> {
        // Use recursive CTE to find paths, then fetch relationships along each path.
        // We reuse the existing traverse to get connected entities with depth,
        // then for each entity pair in the path, fetch the connecting relationships.
        let entities = self
            .traverse(start_entity_id, max_depth, channel_ids, include_expired)
            .await?;

        let mut paths = Vec::new();

        for (entity, depth) in &entities {
            if *depth == 0 {
                continue; // skip start node itself
            }

            // Fetch relationships between the start entity and this entity
            let rels: Vec<PathRelRow> = sqlx::query_as(
                "SELECT r.relationship, r.fact, r.confidence,
                        src.canonical_name AS source_name,
                        tgt.canonical_name AS target_name
                 FROM relationships r
                 JOIN entities src ON src.id = r.source_id
                 JOIN entities tgt ON tgt.id = r.target_id
                 WHERE ((r.source_id = $1 AND r.target_id = $2) OR (r.source_id = $2 AND r.target_id = $1))
                   AND ($3::bool OR r.valid_until IS NULL)
                   AND ($4::int[] IS NULL OR r.channel_id = ANY($4))",
            )
            .bind(start_entity_id)
            .bind(&entity.id)
            .bind(include_expired)
            .bind(channel_ids)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();

            if rels.is_empty() {
                continue;
            }

            // Build the start entity
            let start = self.find_entity_by_name_or_id(start_entity_id).await?;
            let start_entity = match start {
                Some(e) => e,
                None => continue,
            };

            let path_rels: Vec<PathRelationship> = rels
                .into_iter()
                .map(|r| PathRelationship {
                    source_name: r.source_name,
                    target_name: r.target_name,
                    relationship: r.relationship,
                    fact: r.fact,
                    confidence: r.confidence,
                })
                .collect();

            paths.push(TraversalPath {
                entities: vec![start_entity, entity.clone()],
                relationships: path_rels,
                depth: *depth,
            });
        }

        Ok(paths)
    }

    // -- Full-text search -----------------------------------------------------

    async fn search_entities_fulltext(
        &self,
        search_query: &str,
        top_k: i32,
    ) -> anyhow::Result<Vec<(Entity, f64)>> {
        let rows: Vec<(String, String, Option<String>, serde_json::Value, DateTime<Utc>)> =
            sqlx::query_as(
                "SELECT id, canonical_name, entity_type, properties, created_at
                 FROM entities
                 WHERE canonical_name ILIKE '%' || $1 || '%'
                 ORDER BY length(canonical_name)
                 LIMIT $2",
            )
            .bind(search_query)
            .bind(top_k as i64)
            .fetch_all(&self.pool)
            .await?;

        let query_lower = search_query.to_lowercase();
        Ok(rows
            .into_iter()
            .map(|(id, name, etype, props, created)| {
                let score = if name.to_lowercase() == query_lower {
                    1.0
                } else if name.to_lowercase().starts_with(&query_lower) {
                    0.8
                } else {
                    0.5
                };
                (
                    Entity {
                        id,
                        canonical_name: name,
                        entity_type: etype,
                        properties: props,
                        embedding: None,
                        created_at: created,
                    },
                    score,
                )
            })
            .collect())
    }

    // -- Entity description enrichment ----------------------------------------

    async fn update_entity_description(
        &self,
        entity_id: &str,
        description: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE entities
             SET properties = jsonb_set(properties, '{description}', $1::jsonb)
             WHERE id = $2",
        )
        .bind(serde_json::to_string(description)?) // produces a JSON string value
        .bind(entity_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // -- Relationship weight --------------------------------------------------

    async fn increment_relationship_weight(&self, relationship_id: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE relationships SET weight = COALESCE(weight, 1.0) + 1.0 WHERE id = $1",
        )
        .bind(relationship_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

impl PostgresGraphStore {
    /// Helper: find entity by ID (used internally for traverse_rich).
    async fn find_entity_by_name_or_id(&self, id: &str) -> anyhow::Result<Option<Entity>> {
        let row = sqlx::query_as::<_, EntityRow>(
            "SELECT id, canonical_name, entity_type, properties, created_at
             FROM entities
             WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }
}

// ---------------------------------------------------------------------------
// PostgresVectorStore
// ---------------------------------------------------------------------------

pub struct PostgresVectorStore {
    pool: PgPool,
}

impl PostgresVectorStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl VectorStore for PostgresVectorStore {
    async fn set_chunk_embedding(&self, chunk_id: &str, embedding: &[f32]) -> anyhow::Result<()> {
        let vec = Vector::from(embedding.to_vec());
        sqlx::query("UPDATE chunks SET embedding = $1 WHERE id = $2")
            .bind(vec)
            .bind(chunk_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_entity_embedding(
        &self,
        entity_id: &str,
        embedding: &[f32],
    ) -> anyhow::Result<()> {
        let vec = Vector::from(embedding.to_vec());
        sqlx::query("UPDATE entities SET embedding = $1 WHERE id = $2")
            .bind(vec)
            .bind(entity_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn search_chunks(
        &self,
        query_embedding: &[f32],
        channel_ids: Option<&[i32]>,
        top_k: i32,
    ) -> anyhow::Result<Vec<ChunkResult>> {
        let query_vec = Vector::from(query_embedding.to_vec());

        let rows: Vec<ChunkSearchRow> = sqlx::query_as::<_, ChunkSearchRow>(
            "SELECT c.id, c.document_id, c.content, c.chunk_index, c.created_at,
                    d.source_ref, ch.name AS channel_name,
                    c.embedding <=> $1::vector AS distance
             FROM chunks c
             JOIN documents d ON c.document_id = d.id
             JOIN channels ch ON d.channel_id = ch.id
             WHERE c.embedding IS NOT NULL
               AND ($2::int[] IS NULL OR d.channel_id = ANY($2))
             ORDER BY c.embedding <=> $1::vector
             LIMIT $3",
        )
        .bind(&query_vec)
        .bind(channel_ids)
        .bind(top_k)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| ChunkResult {
                chunk: Chunk {
                    id: r.id,
                    document_id: r.document_id,
                    content: r.content,
                    chunk_index: r.chunk_index,
                    embedding: None,
                    created_at: r.created_at,
                },
                document_source: r.source_ref,
                channel: r.channel_name,
                distance: r.distance as f32,
            })
            .collect())
    }

    async fn search_entities(
        &self,
        query_embedding: &[f32],
        channel_ids: Option<&[i32]>,
        top_k: i32,
    ) -> anyhow::Result<Vec<EntityResult>> {
        let query_vec = Vector::from(query_embedding.to_vec());

        let rows: Vec<EntitySearchRow> = sqlx::query_as::<_, EntitySearchRow>(
            "SELECT e.id, e.canonical_name, e.entity_type, e.properties, e.created_at,
                    e.embedding <=> $1::vector AS distance
             FROM entities e
             WHERE e.embedding IS NOT NULL
               AND ($2::int[] IS NULL OR EXISTS (
                   SELECT 1 FROM entity_channels ec
                   WHERE ec.entity_id = e.id AND ec.channel_id = ANY($2)
               ))
             ORDER BY e.embedding <=> $1::vector
             LIMIT $3",
        )
        .bind(&query_vec)
        .bind(channel_ids)
        .bind(top_k)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::with_capacity(rows.len());
        for r in rows {
            let channels: Vec<(String,)> = sqlx::query_as(
                "SELECT DISTINCT c.name
                 FROM entity_channels ec
                 JOIN channels c ON c.id = ec.channel_id
                 WHERE ec.entity_id = $1",
            )
            .bind(&r.id)
            .fetch_all(&self.pool)
            .await?;

            results.push(EntityResult {
                entity: Entity {
                    id: r.id,
                    canonical_name: r.canonical_name,
                    entity_type: r.entity_type,
                    properties: r.properties,
                    embedding: None,
                    created_at: r.created_at,
                },
                channels: channels.into_iter().map(|c| c.0).collect(),
                relevance: 1.0 - r.distance as f32,
            });
        }

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Internal row types for sqlx::query_as
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct EntityRow {
    id: String,
    canonical_name: String,
    entity_type: Option<String>,
    properties: serde_json::Value,
    created_at: DateTime<Utc>,
}

impl From<EntityRow> for Entity {
    fn from(r: EntityRow) -> Self {
        Self {
            id: r.id,
            canonical_name: r.canonical_name,
            entity_type: r.entity_type,
            properties: r.properties,
            embedding: None,
            created_at: r.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct RelationshipRow {
    id: String,
    source_id: String,
    target_id: String,
    relationship: String,
    fact: Option<String>,
    properties: serde_json::Value,
    confidence: String,
    channel_id: i32,
    document_id: String,
    valid_from: DateTime<Utc>,
    valid_until: Option<DateTime<Utc>>,
    ingested_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    weight: f64,
    source_name: String,
    target_name: String,
}

#[derive(sqlx::FromRow)]
struct TraversalRow {
    id: String,
    canonical_name: String,
    entity_type: Option<String>,
    properties: serde_json::Value,
    created_at: DateTime<Utc>,
    depth: i32,
}

#[derive(sqlx::FromRow)]
struct ChunkSearchRow {
    id: String,
    document_id: String,
    content: String,
    chunk_index: i32,
    created_at: DateTime<Utc>,
    source_ref: String,
    channel_name: String,
    distance: f64,
}

#[derive(sqlx::FromRow)]
struct EntitySearchRow {
    id: String,
    canonical_name: String,
    entity_type: Option<String>,
    properties: serde_json::Value,
    created_at: DateTime<Utc>,
    distance: f64,
}

#[derive(sqlx::FromRow)]
struct PathRelRow {
    relationship: String,
    fact: Option<String>,
    confidence: String,
    source_name: String,
    target_name: String,
}
