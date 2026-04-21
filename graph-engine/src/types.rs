use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// --- Channel ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: i32,
    pub name: String,
}

// --- Document ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub channel_id: i32,
    pub source_ref: String,
    pub sha256: String,
    pub ingested_at: DateTime<Utc>,
}

// --- Chunk ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub document_id: String,
    pub content: String,
    pub chunk_index: i32,
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
}

// --- Entity ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub canonical_name: String,
    pub entity_type: Option<String>,
    pub properties: serde_json::Value,
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
}

// --- Entity Alias ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityAlias {
    pub alias: String,
    pub entity_id: String,
    pub source_document_id: Option<String>,
}

// --- Relationship (edge with bi-temporal validity) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub relationship: String,
    pub fact: Option<String>,
    pub properties: serde_json::Value,
    pub confidence: Confidence,
    pub channel_id: i32,
    pub document_id: String,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub ingested_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// Weight increases each time the same relationship is confirmed from a new
    /// document (episode accumulation). Defaults to 1.0 for new relationships.
    #[serde(default = "default_weight")]
    pub weight: f64,
}

fn default_weight() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Established,
    Emerging,
    Contested,
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Established => write!(f, "established"),
            Self::Emerging => write!(f, "emerging"),
            Self::Contested => write!(f, "contested"),
        }
    }
}

impl std::str::FromStr for Confidence {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "established" => Ok(Self::Established),
            "emerging" => Ok(Self::Emerging),
            "contested" => Ok(Self::Contested),
            _ => Ok(Self::Established),
        }
    }
}

// --- Community ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Community {
    pub id: String,
    pub level: i32,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub summary_embedding: Option<Vec<f32>>,
    pub parent_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

// --- Rich traversal types ---

/// A full path through the graph: a sequence of (entity, relationship, entity) triples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalPath {
    pub entities: Vec<Entity>,
    pub relationships: Vec<PathRelationship>,
    pub depth: i32,
}

/// A single relationship edge within a traversal path, carrying entity names for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathRelationship {
    pub source_name: String,
    pub target_name: String,
    pub relationship: String,
    pub fact: Option<String>,
    pub confidence: String,
}

// --- Extraction types (from LLM) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub name: String,
    pub entity_type: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedRelationship {
    pub source: String,
    pub target: String,
    pub relationship: String,
    pub fact: Option<String>,
    pub confidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub entities: Vec<ExtractedEntity>,
    pub relationships: Vec<ExtractedRelationship>,
}

// --- Search types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Graph,
    Vector,
    Hybrid,
    Community,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub channels: Option<Vec<String>>,
    pub extend_channels: Option<Vec<String>>,
    pub mode: Option<SearchMode>,
    pub top_k: Option<i32>,
    pub max_depth: Option<i32>,
    pub include_expired: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub entities: Vec<EntityResult>,
    pub chunks: Vec<ChunkResult>,
    pub relationships: Vec<RelationshipResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityResult {
    pub entity: Entity,
    pub channels: Vec<String>,
    pub relevance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkResult {
    pub chunk: Chunk,
    pub document_source: String,
    pub channel: String,
    pub distance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipResult {
    pub relationship: Relationship,
    pub source_name: String,
    pub target_name: String,
}

// --- API request/response types ---

#[derive(Debug, Deserialize)]
pub struct AddRequest {
    #[serde(alias = "datasetName")]
    pub dataset_name: String,
    pub content: String,
    pub source_ref: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IntegrateRequest {
    pub datasets: Vec<String>,
    pub custom_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearchApiRequest {
    pub query: String,
    #[serde(alias = "channels")]
    pub datasets: Option<Vec<String>>,
    pub search_type: Option<String>,
    pub top_k: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntegrationResult {
    pub entities_created: usize,
    pub relationships_created: usize,
    pub chunks_processed: usize,
    pub chunks_failed: usize,
}

#[derive(Debug, Serialize)]
pub struct ApiResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ApiResponse {
    pub fn ok(message: &str) -> Self {
        Self {
            status: "ok".to_string(),
            message: Some(message.to_string()),
            data: None,
        }
    }

    pub fn error(message: &str) -> Self {
        Self {
            status: "error".to_string(),
            message: Some(message.to_string()),
            data: None,
        }
    }

    pub fn with_data(message: &str, data: serde_json::Value) -> Self {
        Self {
            status: "ok".to_string(),
            message: Some(message.to_string()),
            data: Some(data),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal row type shared between graph.rs and temporal.rs
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
pub(crate) struct RelRow {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub relationship: String,
    pub fact: Option<String>,
    pub properties: serde_json::Value,
    pub confidence: String,
    pub channel_id: i32,
    pub document_id: String,
    pub valid_from: chrono::DateTime<chrono::Utc>,
    pub valid_until: Option<chrono::DateTime<chrono::Utc>>,
    pub ingested_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[sqlx(default)]
    pub weight: f64,
}

impl From<RelRow> for Relationship {
    fn from(r: RelRow) -> Self {
        Self {
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
            weight: if r.weight == 0.0 { 1.0 } else { r.weight },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_roundtrip() {
        assert_eq!("established".parse::<Confidence>().unwrap(), Confidence::Established);
        assert_eq!("emerging".parse::<Confidence>().unwrap(), Confidence::Emerging);
        assert_eq!("contested".parse::<Confidence>().unwrap(), Confidence::Contested);
        assert_eq!("unknown".parse::<Confidence>().unwrap(), Confidence::Established);
    }

    #[test]
    fn confidence_display() {
        assert_eq!(Confidence::Established.to_string(), "established");
        assert_eq!(Confidence::Emerging.to_string(), "emerging");
        assert_eq!(Confidence::Contested.to_string(), "contested");
    }

    #[test]
    fn api_response_ok() {
        let r = ApiResponse::ok("done");
        assert_eq!(r.status, "ok");
        assert_eq!(r.message.as_deref(), Some("done"));
        assert!(r.data.is_none());
    }

    #[test]
    fn api_response_error() {
        let r = ApiResponse::error("failed");
        assert_eq!(r.status, "error");
    }

    #[test]
    fn api_response_with_data() {
        let r = ApiResponse::with_data("found", serde_json::json!({"count": 5}));
        assert!(r.data.is_some());
    }

    #[test]
    fn extraction_result_serde() {
        let result = ExtractionResult {
            entities: vec![ExtractedEntity {
                name: "copper".to_string(),
                entity_type: Some("material".to_string()),
                description: "A conductive metal".to_string(),
            }],
            relationships: vec![ExtractedRelationship {
                source: "data centers".to_string(),
                target: "copper".to_string(),
                relationship: "consumes".to_string(),
                fact: Some("Data centers use copper for electrical wiring".to_string()),
                confidence: Some("established".to_string()),
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: ExtractionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entities.len(), 1);
        assert_eq!(back.relationships.len(), 1);
        assert_eq!(back.entities[0].name, "copper");
    }

    #[test]
    fn search_request_defaults() {
        let json = r#"{"query": "copper demand"}"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.query, "copper demand");
        assert!(req.channels.is_none());
        assert!(req.mode.is_none());
        assert!(req.top_k.is_none());
    }

    #[test]
    fn traversal_path_serde() {
        let path = TraversalPath {
            entities: vec![
                Entity {
                    id: "e1".to_string(),
                    canonical_name: "copper".to_string(),
                    entity_type: Some("material".to_string()),
                    properties: serde_json::json!({}),
                    embedding: None,
                    created_at: chrono::Utc::now(),
                },
                Entity {
                    id: "e2".to_string(),
                    canonical_name: "data center".to_string(),
                    entity_type: Some("concept".to_string()),
                    properties: serde_json::json!({}),
                    embedding: None,
                    created_at: chrono::Utc::now(),
                },
            ],
            relationships: vec![PathRelationship {
                source_name: "copper".to_string(),
                target_name: "data center".to_string(),
                relationship: "consumes".to_string(),
                fact: Some("Data centers consume copper wiring".to_string()),
                confidence: "established".to_string(),
            }],
            depth: 1,
        };

        let json = serde_json::to_string(&path).unwrap();
        let back: TraversalPath = serde_json::from_str(&json).unwrap();

        assert_eq!(back.entities.len(), 2);
        assert_eq!(back.entities[0].canonical_name, "copper");
        assert_eq!(back.entities[1].canonical_name, "data center");
        assert_eq!(back.relationships.len(), 1);
        assert_eq!(back.relationships[0].relationship, "consumes");
        assert_eq!(
            back.relationships[0].fact.as_deref(),
            Some("Data centers consume copper wiring")
        );
        assert_eq!(back.relationships[0].confidence, "established");
        assert_eq!(back.depth, 1);
    }

    #[test]
    fn relationship_weight_default() {
        // A Relationship JSON without a `weight` field should default to 1.0.
        let json = serde_json::json!({
            "id": "r1",
            "source_id": "s1",
            "target_id": "t1",
            "relationship": "related_to",
            "fact": null,
            "properties": {},
            "confidence": "established",
            "channel_id": 1,
            "document_id": "d1",
            "valid_from": "2026-01-01T00:00:00Z",
            "valid_until": null,
            "ingested_at": "2026-01-01T00:00:00Z",
            "created_at": "2026-01-01T00:00:00Z"
        });
        let rel: Relationship = serde_json::from_value(json).unwrap();
        assert!(
            (rel.weight - 1.0).abs() < f64::EPSILON,
            "weight should default to 1.0, got {}",
            rel.weight
        );
    }

    #[test]
    fn relationship_weight_roundtrip() {
        let now = chrono::Utc::now();
        let rel = Relationship {
            id: "r1".to_string(),
            source_id: "s1".to_string(),
            target_id: "t1".to_string(),
            relationship: "related_to".to_string(),
            fact: None,
            properties: serde_json::json!({}),
            confidence: Confidence::Established,
            channel_id: 1,
            document_id: "d1".to_string(),
            valid_from: now,
            valid_until: None,
            ingested_at: now,
            created_at: now,
            weight: 3.5,
        };

        let json = serde_json::to_string(&rel).unwrap();
        let back: Relationship = serde_json::from_str(&json).unwrap();
        assert!(
            (back.weight - 3.5).abs() < f64::EPSILON,
            "weight should roundtrip as 3.5, got {}",
            back.weight
        );
    }
}
