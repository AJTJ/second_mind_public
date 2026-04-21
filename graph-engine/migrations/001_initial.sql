-- Second Mind graph engine schema
-- pgvector for embeddings, standard tables for graph structure

CREATE EXTENSION IF NOT EXISTS vector;

-- Channels (subgraphs)
CREATE TABLE channels (
    id SERIAL PRIMARY KEY,
    name TEXT UNIQUE NOT NULL
);

-- Documents (ingested sources)
CREATE TABLE documents (
    id TEXT PRIMARY KEY,
    channel_id INT REFERENCES channels NOT NULL,
    source_ref TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Chunks (text segments stored alongside graph structure)
CREATE TABLE chunks (
    id TEXT PRIMARY KEY,
    document_id TEXT REFERENCES documents NOT NULL,
    content TEXT NOT NULL,
    chunk_index INT NOT NULL,
    embedding vector(2560),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Entities (extracted by LLM)
CREATE TABLE entities (
    id TEXT PRIMARY KEY,
    canonical_name TEXT NOT NULL,
    entity_type TEXT,
    properties JSONB NOT NULL DEFAULT '{}',
    embedding vector(2560),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX entities_name_idx ON entities (canonical_name);
CREATE INDEX entities_type_idx ON entities (entity_type);

-- Entity-channel membership (subgraph tagging)
CREATE TABLE entity_channels (
    entity_id TEXT REFERENCES entities NOT NULL,
    channel_id INT REFERENCES channels NOT NULL,
    document_id TEXT REFERENCES documents NOT NULL,
    PRIMARY KEY (entity_id, channel_id, document_id)
);

-- Entity aliases (variant names → canonical entity)
CREATE TABLE entity_aliases (
    alias TEXT NOT NULL,
    entity_id TEXT REFERENCES entities NOT NULL,
    source_document_id TEXT REFERENCES documents,
    PRIMARY KEY (alias, entity_id)
);
CREATE INDEX entity_aliases_alias_idx ON entity_aliases (alias);

-- Relationships (edges) with bi-temporal validity
CREATE TABLE relationships (
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
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX relationships_source_idx ON relationships (source_id);
CREATE INDEX relationships_target_idx ON relationships (target_id);
CREATE INDEX relationships_type_idx ON relationships (relationship);
CREATE INDEX relationships_valid_idx ON relationships (valid_until) WHERE valid_until IS NULL;

-- Entity-chunk provenance
CREATE TABLE entity_chunks (
    entity_id TEXT REFERENCES entities NOT NULL,
    chunk_id TEXT REFERENCES chunks NOT NULL,
    PRIMARY KEY (entity_id, chunk_id)
);

-- Communities (hierarchical groupings)
CREATE TABLE communities (
    id TEXT PRIMARY KEY,
    level INT NOT NULL,
    name TEXT,
    summary TEXT,
    summary_embedding vector(2560),
    parent_id TEXT REFERENCES communities,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX communities_level_idx ON communities (level);

-- Community membership
CREATE TABLE community_members (
    community_id TEXT REFERENCES communities NOT NULL,
    entity_id TEXT REFERENCES entities NOT NULL,
    PRIMARY KEY (community_id, entity_id)
);

-- LLM response cache (content-addressable)
CREATE TABLE llm_cache (
    hash TEXT PRIMARY KEY,
    model TEXT NOT NULL,
    response TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Vector indices (created after initial data load for better index quality)
-- Run these manually after first batch of data:
-- CREATE INDEX chunks_embedding_idx ON chunks USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
-- CREATE INDEX entities_embedding_idx ON entities USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
