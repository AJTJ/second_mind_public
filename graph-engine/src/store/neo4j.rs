use async_trait::async_trait;
use chrono::{DateTime, Utc};
use neo4rs::{query, Graph, Node, Row};
use tracing::info;

use super::GraphStore;
use crate::types::*;

/// Neo4j-backed graph store using Cypher queries.
///
/// Node labels: `:Entity`, `:Channel`, `:Document`, `:Chunk`, `:Community`
/// Edge types:  `:RELATES` (with properties), `:IN_CHANNEL`, `:FROM_DOCUMENT`,
///              `:HAS_CHUNK`, `:ENTITY_CHUNK`, `:HAS_MEMBER`, `:ALIAS_OF`
pub struct Neo4jGraphStore {
    graph: Graph,
}

impl Neo4jGraphStore {
    pub async fn new(uri: &str, user: &str, password: &str) -> anyhow::Result<Self> {
        let graph = Graph::new(uri, user, password).await?;
        Ok(Self { graph })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a string property from a Neo4j node, returning None if missing.
fn node_str(node: &Node, key: &str) -> Option<String> {
    node.get::<String>(key).ok()
}

/// Extract a required string property from a Neo4j node.
fn node_str_req(node: &Node, key: &str) -> String {
    node.get::<String>(key).unwrap_or_default()
}

/// Extract an optional i64 property, returning 0 if missing.
fn node_i32(node: &Node, key: &str) -> i32 {
    node.get::<i64>(key).unwrap_or(0) as i32
}

/// Parse an ISO-8601 datetime string from a node property, defaulting to now.
fn node_datetime(node: &Node, key: &str) -> DateTime<Utc> {
    node.get::<String>(key)
        .ok()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now)
}

/// Parse properties JSON from a node, defaulting to empty object.
fn node_json(node: &Node, key: &str) -> serde_json::Value {
    node.get::<String>(key)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn entity_from_node(node: &Node) -> Entity {
    Entity {
        id: node_str_req(node, "id"),
        canonical_name: node_str_req(node, "canonical_name"),
        entity_type: node_str(node, "entity_type"),
        properties: node_json(node, "properties"),
        embedding: None,
        created_at: node_datetime(node, "created_at"),
    }
}

fn relationship_from_row(row: &Row) -> anyhow::Result<Relationship> {
    Ok(Relationship {
        id: row.get::<String>("id")?,
        source_id: row.get::<String>("source_id")?,
        target_id: row.get::<String>("target_id")?,
        relationship: row.get::<String>("relationship")?,
        fact: row.get::<String>("fact").ok(),
        properties: row
            .get::<String>("properties")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({})),
        confidence: row
            .get::<String>("confidence")
            .unwrap_or_else(|_| "established".to_string())
            .parse()
            .unwrap_or(Confidence::Established),
        channel_id: row.get::<i64>("channel_id").unwrap_or(0) as i32,
        document_id: row.get::<String>("document_id").unwrap_or_default(),
        valid_from: row
            .get::<String>("valid_from")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(Utc::now),
        valid_until: row
            .get::<String>("valid_until")
            .ok()
            .and_then(|s| s.parse().ok()),
        ingested_at: row
            .get::<String>("ingested_at")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(Utc::now),
        created_at: row
            .get::<String>("created_at")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(Utc::now),
        weight: row.get::<f64>("weight").unwrap_or(1.0),
    })
}

#[async_trait]
impl GraphStore for Neo4jGraphStore {
    // -- Health ---------------------------------------------------------------

    async fn health_check(&self) -> anyhow::Result<()> {
        self.graph.run(query("RETURN 1")).await?;
        Ok(())
    }

    // -- Schema ---------------------------------------------------------------

    async fn initialize(&self) -> anyhow::Result<()> {
        let constraints = [
            "CREATE CONSTRAINT entity_id IF NOT EXISTS FOR (e:Entity) REQUIRE e.id IS UNIQUE",
            "CREATE CONSTRAINT channel_name IF NOT EXISTS FOR (c:Channel) REQUIRE c.name IS UNIQUE",
            "CREATE CONSTRAINT document_id IF NOT EXISTS FOR (d:Document) REQUIRE d.id IS UNIQUE",
            "CREATE CONSTRAINT chunk_id IF NOT EXISTS FOR (ch:Chunk) REQUIRE ch.id IS UNIQUE",
            "CREATE CONSTRAINT community_id IF NOT EXISTS FOR (co:Community) REQUIRE co.id IS UNIQUE",
        ];

        let indexes = [
            "CREATE INDEX entity_name IF NOT EXISTS FOR (e:Entity) ON (e.canonical_name)",
            "CREATE INDEX entity_type IF NOT EXISTS FOR (e:Entity) ON (e.entity_type)",
            "CREATE INDEX rel_source IF NOT EXISTS FOR ()-[r:RELATES]-() ON (r.source_id)",
            "CREATE INDEX rel_target IF NOT EXISTS FOR ()-[r:RELATES]-() ON (r.target_id)",
            "CREATE INDEX alias_name IF NOT EXISTS FOR (a:Alias) ON (a.alias)",
            "CREATE INDEX channel_counter IF NOT EXISTS FOR (cc:ChannelCounter) ON (cc.key)",
        ];

        let fulltext_indexes = [
            "CREATE FULLTEXT INDEX entity_fulltext IF NOT EXISTS FOR (e:Entity) ON EACH [e.canonical_name, e.entity_type]",
        ];

        for stmt in constraints.iter().chain(indexes.iter()) {
            self.graph.run(query(stmt)).await?;
        }

        for stmt in &fulltext_indexes {
            self.graph.run(query(stmt)).await?;
        }

        // Ensure a counter node exists for auto-incrementing channel IDs.
        self.graph
            .run(query(
                "MERGE (cc:ChannelCounter {key: 'channels'})
                 ON CREATE SET cc.value = 0",
            ))
            .await?;

        info!("Neo4j schema initialized");
        Ok(())
    }

    async fn ensure_channel(&self, name: &str) -> anyhow::Result<i32> {
        // Check if channel already exists with a valid ID
        let mut result = self
            .graph
            .execute(
                query("MATCH (c:Channel {name: $name}) WHERE c.id >= 0 RETURN c.id AS id")
                    .param("name", name.to_string()),
            )
            .await?;

        if let Some(row) = result.next().await? {
            let id: i64 = row.get("id")?;
            return Ok(id as i32);
        }

        // Channel doesn't exist or has no valid ID. Create with counter.
        let mut result = self
            .graph
            .execute(
                query(
                    "MERGE (cc:ChannelCounter {key: 'channels'})
                     ON CREATE SET cc.value = 0
                     SET cc.value = cc.value + 1
                     WITH cc.value AS new_id
                     MERGE (c:Channel {name: $name})
                     SET c.id = new_id
                     RETURN new_id",
                )
                .param("name", name.to_string()),
            )
            .await?;

        let row = result
            .next()
            .await?
            .ok_or_else(|| anyhow::anyhow!("ensure_channel counter returned no rows"))?;
        let new_id: i64 = row.get("new_id")?;
        Ok(new_id as i32)
    }

    // -- Entities -------------------------------------------------------------

    async fn insert_entity(&self, entity: &Entity) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MERGE (e:Entity {id: $id})
                     ON CREATE SET
                         e.canonical_name = $canonical_name,
                         e.entity_type = $entity_type,
                         e.properties = $properties,
                         e.created_at = $created_at
                     ON MATCH SET
                         e.canonical_name = $canonical_name,
                         e.entity_type = $entity_type,
                         e.properties = $properties",
                )
                .param("id", entity.id.clone())
                .param("canonical_name", entity.canonical_name.clone())
                .param(
                    "entity_type",
                    entity.entity_type.clone().unwrap_or_default(),
                )
                .param("properties", serde_json::to_string(&entity.properties)?)
                .param("created_at", entity.created_at.to_rfc3339()),
            )
            .await?;
        Ok(())
    }

    async fn find_entity_by_name(&self, name: &str) -> anyhow::Result<Option<Entity>> {
        let mut result = self
            .graph
            .execute(
                query("MATCH (e:Entity {canonical_name: $name}) RETURN e")
                    .param("name", name),
            )
            .await?;

        match result.next().await? {
            Some(row) => {
                let node: Node = row.get("e")?;
                Ok(Some(entity_from_node(&node)))
            }
            None => Ok(None),
        }
    }

    async fn find_entity_by_alias(&self, alias: &str) -> anyhow::Result<Option<Entity>> {
        let mut result = self
            .graph
            .execute(
                query(
                    "MATCH (a:Alias {alias: $alias})-[:ALIAS_OF]->(e:Entity)
                     RETURN e",
                )
                .param("alias", alias),
            )
            .await?;

        match result.next().await? {
            Some(row) => {
                let node: Node = row.get("e")?;
                Ok(Some(entity_from_node(&node)))
            }
            None => Ok(None),
        }
    }

    async fn insert_entity_channel(
        &self,
        entity_id: &str,
        channel_id: i32,
        document_id: &str,
    ) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MATCH (e:Entity {id: $eid}), (c:Channel {id: $cid})
                     MERGE (e)-[:IN_CHANNEL {document_id: $did}]->(c)",
                )
                .param("eid", entity_id.to_string())
                .param("cid", channel_id as i64)
                .param("did", document_id.to_string()),
            )
            .await?;
        Ok(())
    }

    async fn insert_entity_alias(
        &self,
        alias: &str,
        entity_id: &str,
        document_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MATCH (e:Entity {id: $eid})
                     MERGE (a:Alias {alias: $alias, entity_id: $eid})
                     ON CREATE SET a.source_document_id = $did
                     MERGE (a)-[:ALIAS_OF]->(e)",
                )
                .param("alias", alias.to_string())
                .param("eid", entity_id.to_string())
                .param(
                    "did",
                    document_id.unwrap_or("").to_string(),
                ),
            )
            .await?;
        Ok(())
    }

    async fn insert_entity_chunk(&self, entity_id: &str, chunk_id: &str) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MATCH (e:Entity {id: $eid}), (ch:Chunk {id: $cid})
                     MERGE (e)-[:ENTITY_CHUNK]->(ch)",
                )
                .param("eid", entity_id.to_string())
                .param("cid", chunk_id.to_string()),
            )
            .await?;
        Ok(())
    }

    async fn get_entity_channels(&self, entity_id: &str) -> anyhow::Result<Vec<String>> {
        let mut result = self
            .graph
            .execute(
                query(
                    "MATCH (e:Entity {id: $eid})-[:IN_CHANNEL]->(c:Channel)
                     RETURN DISTINCT c.name AS name",
                )
                .param("eid", entity_id.to_string()),
            )
            .await?;

        let mut channels = Vec::new();
        while let Some(row) = result.next().await? {
            channels.push(row.get::<String>("name")?);
        }
        Ok(channels)
    }

    // -- Relationships --------------------------------------------------------

    async fn insert_relationship(&self, rel: &Relationship) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MATCH (s:Entity {id: $src}), (t:Entity {id: $tgt})
                     CREATE (s)-[r:RELATES {
                         id: $id,
                         relationship: $relationship,
                         fact: $fact,
                         properties: $properties,
                         confidence: $confidence,
                         channel_id: $channel_id,
                         document_id: $document_id,
                         source_id: $src,
                         target_id: $tgt,
                         valid_from: $valid_from,
                         valid_until: $valid_until,
                         ingested_at: $ingested_at,
                         created_at: $created_at,
                         weight: $weight
                     }]->(t)",
                )
                .param("id", rel.id.clone())
                .param("src", rel.source_id.clone())
                .param("tgt", rel.target_id.clone())
                .param("relationship", rel.relationship.clone())
                .param("fact", rel.fact.clone().unwrap_or_default())
                .param("properties", serde_json::to_string(&rel.properties)?)
                .param("confidence", rel.confidence.to_string())
                .param("channel_id", rel.channel_id as i64)
                .param("document_id", rel.document_id.clone())
                .param("valid_from", rel.valid_from.to_rfc3339())
                .param(
                    "valid_until",
                    rel.valid_until
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                )
                .param("ingested_at", rel.ingested_at.to_rfc3339())
                .param("created_at", rel.created_at.to_rfc3339())
                .param("weight", rel.weight),
            )
            .await?;
        Ok(())
    }

    async fn get_entity_relationships(
        &self,
        entity_id: &str,
        include_expired: bool,
    ) -> anyhow::Result<Vec<(Relationship, String, String)>> {
        let cypher = if include_expired {
            "MATCH (s:Entity)-[r:RELATES]->(t:Entity)
             WHERE s.id = $eid OR t.id = $eid
             RETURN r.id AS id, r.source_id AS source_id, r.target_id AS target_id,
                    r.relationship AS relationship, r.fact AS fact,
                    r.properties AS properties, r.confidence AS confidence,
                    r.channel_id AS channel_id, r.document_id AS document_id,
                    r.valid_from AS valid_from, r.valid_until AS valid_until,
                    r.ingested_at AS ingested_at, r.created_at AS created_at,
                    COALESCE(r.weight, 1.0) AS weight,
                    s.canonical_name AS source_name, t.canonical_name AS target_name"
        } else {
            "MATCH (s:Entity)-[r:RELATES]->(t:Entity)
             WHERE (s.id = $eid OR t.id = $eid) AND (r.valid_until IS NULL OR r.valid_until = '')
             RETURN r.id AS id, r.source_id AS source_id, r.target_id AS target_id,
                    r.relationship AS relationship, r.fact AS fact,
                    r.properties AS properties, r.confidence AS confidence,
                    r.channel_id AS channel_id, r.document_id AS document_id,
                    r.valid_from AS valid_from, r.valid_until AS valid_until,
                    r.ingested_at AS ingested_at, r.created_at AS created_at,
                    COALESCE(r.weight, 1.0) AS weight,
                    s.canonical_name AS source_name, t.canonical_name AS target_name"
        };

        let mut result = self
            .graph
            .execute(query(cypher).param("eid", entity_id.to_string()))
            .await?;

        let mut rels = Vec::new();
        while let Some(row) = result.next().await? {
            let source_name: String = row.get("source_name")?;
            let target_name: String = row.get("target_name")?;
            let rel = relationship_from_row(&row)?;
            rels.push((rel, source_name, target_name));
        }
        Ok(rels)
    }

    // -- Traversal ------------------------------------------------------------

    async fn traverse(
        &self,
        start_entity_id: &str,
        max_depth: i32,
        channel_ids: Option<&[i32]>,
        include_expired: bool,
    ) -> anyhow::Result<Vec<(Entity, i32)>> {
        // Build a dynamic Cypher query based on filters.
        // Neo4j variable-length paths: (start)-[*1..N]-(connected)
        let mut where_clauses = Vec::new();
        if !include_expired {
            where_clauses
                .push("ALL(r IN relationships(path) WHERE r.valid_until IS NULL OR r.valid_until = '')".to_string());
        }
        if let Some(ids) = channel_ids {
            if !ids.is_empty() {
                let ids_str: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
                where_clauses.push(format!(
                    "ALL(r IN relationships(path) WHERE r.channel_id IN [{}])",
                    ids_str.join(", ")
                ));
            }
        }

        let where_clause = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let cypher = format!(
            "MATCH path = (start:Entity {{id: $start_id}})-[:RELATES*0..{max_depth}]-(connected:Entity)
             {where_clause}
             WITH DISTINCT connected, min(length(path)) AS depth
             RETURN connected, depth
             ORDER BY depth"
        );

        let mut result = self
            .graph
            .execute(query(&cypher).param("start_id", start_entity_id.to_string()))
            .await?;

        let mut entities = Vec::new();
        while let Some(row) = result.next().await? {
            let node: Node = row.get("connected")?;
            let depth: i64 = row.get("depth")?;
            entities.push((entity_from_node(&node), depth as i32));
        }
        Ok(entities)
    }

    // -- Temporal -------------------------------------------------------------

    async fn find_active_relationships(
        &self,
        source_id: &str,
        target_id: &str,
        relationship: &str,
    ) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let mut result = self
            .graph
            .execute(
                query(
                    "MATCH (s:Entity {id: $src})-[r:RELATES {relationship: $rel}]->(t:Entity {id: $tgt})
                     WHERE r.valid_until IS NULL OR r.valid_until = ''
                     RETURN r.id AS id, r.fact AS fact",
                )
                .param("src", source_id.to_string())
                .param("tgt", target_id.to_string())
                .param("rel", relationship.to_string()),
            )
            .await?;

        let mut rows = Vec::new();
        while let Some(row) = result.next().await? {
            let id: String = row.get("id")?;
            let fact: Option<String> = row.get::<String>("fact").ok().and_then(|f| {
                if f.is_empty() {
                    None
                } else {
                    Some(f)
                }
            });
            rows.push((id, fact));
        }
        Ok(rows)
    }

    async fn supersede_relationship(&self, relationship_id: &str) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.graph
            .run(
                query(
                    "MATCH ()-[r:RELATES {id: $id}]->()
                     WHERE r.valid_until IS NULL OR r.valid_until = ''
                     SET r.valid_until = $now",
                )
                .param("id", relationship_id.to_string())
                .param("now", now),
            )
            .await?;
        Ok(())
    }

    async fn get_current_relationships(
        &self,
        entity_id: &str,
    ) -> anyhow::Result<Vec<Relationship>> {
        let mut result = self
            .graph
            .execute(
                query(
                    "MATCH (s:Entity)-[r:RELATES]->(t:Entity)
                     WHERE (s.id = $eid OR t.id = $eid) AND (r.valid_until IS NULL OR r.valid_until = '')
                     RETURN r.id AS id, r.source_id AS source_id, r.target_id AS target_id,
                            r.relationship AS relationship, r.fact AS fact,
                            r.properties AS properties, r.confidence AS confidence,
                            r.channel_id AS channel_id, r.document_id AS document_id,
                            r.valid_from AS valid_from, r.valid_until AS valid_until,
                            r.ingested_at AS ingested_at, r.created_at AS created_at,
                            COALESCE(r.weight, 1.0) AS weight",
                )
                .param("eid", entity_id.to_string()),
            )
            .await?;

        let mut rels = Vec::new();
        while let Some(row) = result.next().await? {
            rels.push(relationship_from_row(&row)?);
        }
        Ok(rels)
    }

    // -- Documents and chunks -------------------------------------------------

    async fn insert_document(
        &self,
        id: &str,
        channel_id: i32,
        source_ref: &str,
        sha256: &str,
    ) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MATCH (c:Channel {id: $cid})
                     MERGE (d:Document {id: $id})
                     ON CREATE SET
                         d.channel_id = $cid,
                         d.source_ref = $source_ref,
                         d.sha256 = $sha256,
                         d.ingested_at = $ingested_at
                     MERGE (d)-[:IN_CHANNEL]->(c)",
                )
                .param("id", id.to_string())
                .param("cid", channel_id as i64)
                .param("source_ref", source_ref.to_string())
                .param("sha256", sha256.to_string())
                .param("ingested_at", Utc::now().to_rfc3339()),
            )
            .await?;
        Ok(())
    }

    async fn find_document_by_hash(
        &self,
        channel_id: i32,
        sha256: &str,
    ) -> anyhow::Result<Option<String>> {
        let mut result = self
            .graph
            .execute(
                query(
                    "MATCH (d:Document {channel_id: $cid, sha256: $sha256})
                     RETURN d.id AS id",
                )
                .param("cid", channel_id as i64)
                .param("sha256", sha256.to_string()),
            )
            .await?;

        match result.next().await? {
            Some(row) => Ok(Some(row.get::<String>("id")?)),
            None => Ok(None),
        }
    }

    async fn insert_chunk(
        &self,
        id: &str,
        document_id: &str,
        content: &str,
        chunk_index: i32,
    ) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MATCH (d:Document {id: $did})
                     MERGE (ch:Chunk {id: $id})
                     ON CREATE SET
                         ch.document_id = $did,
                         ch.content = $content,
                         ch.chunk_index = $chunk_index,
                         ch.created_at = $created_at
                     MERGE (ch)-[:FROM_DOCUMENT]->(d)",
                )
                .param("id", id.to_string())
                .param("did", document_id.to_string())
                .param("content", content.to_string())
                .param("chunk_index", chunk_index as i64)
                .param("created_at", Utc::now().to_rfc3339()),
            )
            .await?;
        Ok(())
    }

    async fn get_unprocessed_chunks(
        &self,
        channel_id: i32,
    ) -> anyhow::Result<Vec<(String, String, String)>> {
        let mut result = self
            .graph
            .execute(
                query(
                    "MATCH (ch:Chunk)-[:FROM_DOCUMENT]->(d:Document)-[:IN_CHANNEL]->(c:Channel {id: $cid})
                     WHERE NOT EXISTS { MATCH (e:Entity)-[:ENTITY_CHUNK]->(ch) }
                     RETURN ch.id AS id, ch.content AS content, ch.document_id AS document_id",
                )
                .param("cid", channel_id as i64),
            )
            .await?;

        let mut chunks = Vec::new();
        while let Some(row) = result.next().await? {
            chunks.push((
                row.get::<String>("id")?,
                row.get::<String>("content")?,
                row.get::<String>("document_id")?,
            ));
        }
        Ok(chunks)
    }

    async fn get_chunk_document_id(&self, chunk_id: &str) -> anyhow::Result<Option<String>> {
        let mut result = self
            .graph
            .execute(
                query("MATCH (ch:Chunk {id: $id}) RETURN ch.document_id AS document_id")
                    .param("id", chunk_id.to_string()),
            )
            .await?;

        match result.next().await? {
            Some(row) => Ok(Some(row.get::<String>("document_id")?)),
            None => Ok(None),
        }
    }

    // -- Channels -------------------------------------------------------------

    async fn list_channels(&self) -> anyhow::Result<Vec<Channel>> {
        let mut result = self
            .graph
            .execute(query(
                "MATCH (c:Channel) RETURN c.id AS id, c.name AS name ORDER BY c.id",
            ))
            .await?;

        let mut channels = Vec::new();
        while let Some(row) = result.next().await? {
            channels.push(Channel {
                id: row.get::<i64>("id")? as i32,
                name: row.get::<String>("name")?,
            });
        }
        Ok(channels)
    }

    async fn delete_channel(&self, name: &str) -> anyhow::Result<()> {
        // Delete all data associated with this channel in dependency order.
        // In Neo4j, we must detach-delete nodes to remove their relationships too.

        // Delete entity-channel edges for this channel
        self.graph
            .run(
                query(
                    "MATCH (c:Channel {name: $name})
                     MATCH (e:Entity)-[r:IN_CHANNEL]->(c)
                     DELETE r",
                )
                .param("name", name.to_string()),
            )
            .await?;

        // Delete RELATES edges scoped to this channel
        self.graph
            .run(
                query(
                    "MATCH (c:Channel {name: $name})
                     MATCH ()-[r:RELATES]->()
                     WHERE r.channel_id = c.id
                     DELETE r",
                )
                .param("name", name.to_string()),
            )
            .await?;

        // Delete entity-chunk links for chunks in documents of this channel
        self.graph
            .run(
                query(
                    "MATCH (c:Channel {name: $name})
                     MATCH (ch:Chunk)-[:FROM_DOCUMENT]->(d:Document)-[:IN_CHANNEL]->(c)
                     MATCH (e:Entity)-[r:ENTITY_CHUNK]->(ch)
                     DELETE r",
                )
                .param("name", name.to_string()),
            )
            .await?;

        // Delete chunks (detach to remove FROM_DOCUMENT edges)
        self.graph
            .run(
                query(
                    "MATCH (c:Channel {name: $name})
                     MATCH (ch:Chunk)-[:FROM_DOCUMENT]->(d:Document)-[:IN_CHANNEL]->(c)
                     DETACH DELETE ch",
                )
                .param("name", name.to_string()),
            )
            .await?;

        // Delete aliases sourced from documents in this channel
        self.graph
            .run(
                query(
                    "MATCH (c:Channel {name: $name})
                     MATCH (d:Document)-[:IN_CHANNEL]->(c)
                     MATCH (a:Alias {source_document_id: d.id})
                     DETACH DELETE a",
                )
                .param("name", name.to_string()),
            )
            .await?;

        // Delete documents
        self.graph
            .run(
                query(
                    "MATCH (c:Channel {name: $name})
                     MATCH (d:Document)-[r:IN_CHANNEL]->(c)
                     DETACH DELETE d",
                )
                .param("name", name.to_string()),
            )
            .await?;

        // Delete the channel itself
        self.graph
            .run(
                query("MATCH (c:Channel {name: $name}) DETACH DELETE c")
                    .param("name", name.to_string()),
            )
            .await?;

        info!(channel = %name, "channel deleted (Neo4j)");
        Ok(())
    }

    async fn delete_all_channels(&self) -> anyhow::Result<()> {
        let channels = GraphStore::list_channels(self).await?;
        for channel in &channels {
            GraphStore::delete_channel(self, &channel.name).await?;
        }
        info!(count = channels.len(), "all channels deleted (Neo4j)");
        Ok(())
    }

    // -- Communities ----------------------------------------------------------

    async fn clear_communities(&self) -> anyhow::Result<()> {
        self.graph
            .run(query(
                "MATCH (co:Community) DETACH DELETE co",
            ))
            .await?;
        Ok(())
    }

    async fn insert_community(&self, community: &Community) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "CREATE (co:Community {
                         id: $id,
                         level: $level,
                         name: $name,
                         created_at: $created_at
                     })",
                )
                .param("id", community.id.clone())
                .param("level", community.level as i64)
                .param("name", community.name.clone().unwrap_or_default())
                .param("created_at", community.created_at.to_rfc3339()),
            )
            .await?;
        Ok(())
    }

    async fn insert_community_member(
        &self,
        community_id: &str,
        entity_id: &str,
    ) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MATCH (co:Community {id: $coid}), (e:Entity {id: $eid})
                     MERGE (co)-[:HAS_MEMBER]->(e)",
                )
                .param("coid", community_id.to_string())
                .param("eid", entity_id.to_string()),
            )
            .await?;
        Ok(())
    }

    async fn list_communities(&self) -> anyhow::Result<Vec<(Community, i64)>> {
        let mut result = self
            .graph
            .execute(query(
                "MATCH (co:Community)
                 OPTIONAL MATCH (co)-[:HAS_MEMBER]->(e:Entity)
                 WITH co, count(e) AS member_count
                 RETURN co.id AS id, co.level AS level, co.name AS name,
                        co.summary AS summary, co.parent_id AS parent_id,
                        co.created_at AS created_at, member_count
                 ORDER BY member_count DESC",
            ))
            .await?;

        let mut communities = Vec::new();
        while let Some(row) = result.next().await? {
            let id: String = row.get("id")?;
            let level: i64 = row.get("level")?;
            let name: Option<String> = row.get::<String>("name").ok().and_then(|n| {
                if n.is_empty() {
                    None
                } else {
                    Some(n)
                }
            });
            let summary: Option<String> = row.get::<String>("summary").ok().and_then(|s| {
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            });
            let parent_id: Option<String> =
                row.get::<String>("parent_id").ok().and_then(|p| {
                    if p.is_empty() {
                        None
                    } else {
                        Some(p)
                    }
                });
            let created_at: DateTime<Utc> = row
                .get::<String>("created_at")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(Utc::now);
            let member_count: i64 = row.get("member_count")?;

            communities.push((
                Community {
                    id,
                    level: level as i32,
                    name,
                    summary,
                    summary_embedding: None,
                    parent_id,
                    created_at,
                },
                member_count,
            ));
        }
        Ok(communities)
    }

    async fn get_community_members(&self, community_id: &str) -> anyhow::Result<Vec<Entity>> {
        let mut result = self
            .graph
            .execute(
                query(
                    "MATCH (co:Community {id: $coid})-[:HAS_MEMBER]->(e:Entity)
                     RETURN e",
                )
                .param("coid", community_id.to_string()),
            )
            .await?;

        let mut entities = Vec::new();
        while let Some(row) = result.next().await? {
            let node: Node = row.get("e")?;
            entities.push(entity_from_node(&node));
        }
        Ok(entities)
    }

    // -- All entities/edges ---------------------------------------------------

    async fn get_all_entities(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut result = self
            .graph
            .execute(query(
                "MATCH (e:Entity) RETURN e.id AS id, e.canonical_name AS name",
            ))
            .await?;

        let mut entities = Vec::new();
        while let Some(row) = result.next().await? {
            entities.push((
                row.get::<String>("id")?,
                row.get::<String>("name")?,
            ));
        }
        Ok(entities)
    }

    async fn get_all_active_edges(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut result = self
            .graph
            .execute(query(
                "MATCH (s:Entity)-[r:RELATES]->(t:Entity)
                 WHERE r.valid_until IS NULL OR r.valid_until = ''
                 RETURN r.source_id AS source_id, r.target_id AS target_id",
            ))
            .await?;

        let mut edges = Vec::new();
        while let Some(row) = result.next().await? {
            edges.push((
                row.get::<String>("source_id")?,
                row.get::<String>("target_id")?,
            ));
        }
        Ok(edges)
    }

    // -- Cache ----------------------------------------------------------------
    // Neo4j is not ideal for key-value lookups. Return no-ops so the pipeline
    // degrades gracefully (LlmExtractor will just re-call the LLM on misses).

    async fn cache_get(&self, _key: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn cache_set(&self, _key: &str, _model: &str, _response: &str) -> anyhow::Result<()> {
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
        // Strategy: use the existing traverse to get connected entities,
        // then for each connected entity fetch direct relationships.
        // This avoids complex Cypher path parsing while still returning
        // rich traversal data.
        let entities = self
            .traverse(start_entity_id, max_depth, channel_ids, include_expired)
            .await?;

        // Find the start entity
        let mut start_entity = None;
        for (entity, depth) in &entities {
            if *depth == 0 {
                start_entity = Some(entity.clone());
                break;
            }
        }
        let start_entity = match start_entity {
            Some(e) => e,
            None => return Ok(vec![]),
        };

        let mut paths = Vec::new();

        for (entity, depth) in &entities {
            if *depth == 0 {
                continue;
            }

            // Fetch relationships between start and this entity
            let rel_cypher = if include_expired {
                "MATCH (s:Entity)-[r:RELATES]->(t:Entity)
                 WHERE ((s.id = $start_id AND t.id = $end_id) OR (s.id = $end_id AND t.id = $start_id))
                 RETURN r.relationship AS relationship, r.fact AS fact,
                        r.confidence AS confidence,
                        s.canonical_name AS source_name,
                        t.canonical_name AS target_name"
            } else {
                "MATCH (s:Entity)-[r:RELATES]->(t:Entity)
                 WHERE ((s.id = $start_id AND t.id = $end_id) OR (s.id = $end_id AND t.id = $start_id))
                   AND (r.valid_until IS NULL OR r.valid_until = '')
                 RETURN r.relationship AS relationship, r.fact AS fact,
                        r.confidence AS confidence,
                        s.canonical_name AS source_name,
                        t.canonical_name AS target_name"
            };

            let mut rel_result = self
                .graph
                .execute(
                    query(rel_cypher)
                        .param("start_id", start_entity_id.to_string())
                        .param("end_id", entity.id.clone()),
                )
                .await?;

            let mut path_rels = Vec::new();
            while let Some(rel_row) = rel_result.next().await? {
                let relationship: String = rel_row.get("relationship")?;
                let fact: Option<String> = rel_row.get::<String>("fact").ok().and_then(|f| {
                    if f.is_empty() { None } else { Some(f) }
                });
                let confidence: String = rel_row
                    .get::<String>("confidence")
                    .unwrap_or_else(|_| "established".to_string());
                let source_name: String = rel_row.get("source_name")?;
                let target_name: String = rel_row.get("target_name")?;

                path_rels.push(PathRelationship {
                    source_name,
                    target_name,
                    relationship,
                    fact,
                    confidence,
                });
            }

            if !path_rels.is_empty() {
                paths.push(TraversalPath {
                    entities: vec![start_entity.clone(), entity.clone()],
                    relationships: path_rels,
                    depth: *depth,
                });
            }
        }

        Ok(paths)
    }

    // -- Full-text search -----------------------------------------------------

    async fn search_entities_fulltext(
        &self,
        search_query: &str,
        top_k: i32,
    ) -> anyhow::Result<Vec<(Entity, f64)>> {
        let mut result = self
            .graph
            .execute(
                query(
                    "CALL db.index.fulltext.queryNodes('entity_fulltext', $query) YIELD node, score
                     RETURN node, score
                     ORDER BY score DESC
                     LIMIT $top_k",
                )
                .param("query", search_query.to_string())
                .param("top_k", top_k as i64),
            )
            .await?;

        let mut entities = Vec::new();
        while let Some(row) = result.next().await? {
            let node: Node = row.get("node")?;
            let score: f64 = row.get("score")?;
            entities.push((entity_from_node(&node), score));
        }
        Ok(entities)
    }

    // -- Entity description enrichment ----------------------------------------

    async fn update_entity_description(
        &self,
        entity_id: &str,
        description: &str,
    ) -> anyhow::Result<()> {
        // Read existing properties, merge description, write back
        let mut result = self
            .graph
            .execute(
                query("MATCH (e:Entity {id: $id}) RETURN e.properties AS properties")
                    .param("id", entity_id.to_string()),
            )
            .await?;

        if let Some(row) = result.next().await? {
            let mut props: serde_json::Value = row
                .get::<String>("properties")
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            props["description"] = serde_json::Value::String(description.to_string());

            self.graph
                .run(
                    query("MATCH (e:Entity {id: $id}) SET e.properties = $properties")
                        .param("id", entity_id.to_string())
                        .param("properties", serde_json::to_string(&props)?),
                )
                .await?;
        }
        Ok(())
    }

    // -- Relationship weight --------------------------------------------------

    async fn increment_relationship_weight(&self, relationship_id: &str) -> anyhow::Result<()> {
        self.graph
            .run(
                query(
                    "MATCH ()-[r:RELATES {id: $id}]->()
                     SET r.weight = COALESCE(r.weight, 1.0) + 1.0",
                )
                .param("id", relationship_id.to_string()),
            )
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Neo4j GDS community detection (backend-specific, not on trait)
// ---------------------------------------------------------------------------

impl Neo4jGraphStore {
    /// Try to use Neo4j GDS for community detection. Returns true if GDS is available.
    pub async fn detect_communities_gds(&self) -> anyhow::Result<bool> {
        // Check if GDS is available
        let check = self
            .graph
            .execute(query("RETURN gds.version() AS v"))
            .await;
        if check.is_err() {
            info!("Neo4j GDS not available, falling back to label propagation");
            return Ok(false);
        }

        // Drop any existing projection (ignore errors if it doesn't exist)
        let _ = self
            .graph
            .run(query(
                "CALL gds.graph.drop('sm-communities', false)",
            ))
            .await;

        // Project graph
        self.graph
            .run(query(
                "CALL gds.graph.project('sm-communities', 'Entity', 'RELATES') YIELD graphName",
            ))
            .await?;

        // Run Leiden community detection
        self.graph
            .run(query(
                "CALL gds.leiden.write('sm-communities', {writeProperty: 'communityId'}) YIELD communityCount",
            ))
            .await?;

        // Drop projection
        self.graph
            .run(query("CALL gds.graph.drop('sm-communities')"))
            .await?;

        info!("GDS community detection complete");
        Ok(true)
    }
}
