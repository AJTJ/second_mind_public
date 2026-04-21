# Graph Engine — Plan

Rust-native knowledge graph engine replacing Cognee. Subgraph-aware storage with Postgres (pgvector for embeddings, recursive CTEs for traversal). Entity extraction via LLM, embedding via Ollama. Axum REST API compatible with the intake engine's existing backend trait.

Every design decision below is referenced to the research that supports it.

## Directory

```
graph-engine/
  src/
    main.rs           — Axum server + CLI
    server.rs         — REST API handlers
    chunker.rs        — Document chunking (text-splitter, markdown-aware)
    extractor.rs      — Entity/relationship extraction (LLM, structured output)
    embedder.rs       — Embedding via Ollama HTTP API
    graph.rs          — Graph storage (Postgres tables + recursive CTEs)
    vectors.rs        — Vector similarity search (pgvector)
    communities.rs    — Community detection + hierarchical summarization
    resolver.rs       — Entity resolution (deduplication, coreference)
    temporal.rs       — Bi-temporal validity tracking
    schema.rs         — Database schema + migrations
    types.rs          — Shared types
  migrations/
    001_initial.sql
  Cargo.toml
```

## Data Model (Postgres)

```sql
-- pgvector extension
CREATE EXTENSION IF NOT EXISTS vector;

-- Channels (subgraphs)
CREATE TABLE channels (
    id SERIAL PRIMARY KEY,
    name TEXT UNIQUE NOT NULL
);

-- Documents (ingested sources)
CREATE TABLE documents (
    id TEXT PRIMARY KEY,                -- ULID
    channel_id INT REFERENCES channels,
    source_ref TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    ingested_at TIMESTAMPTZ DEFAULT now()
);

-- Chunks (text segments — stored alongside graph structure)
-- Research: graphs with source text score 90% retrieval accuracy vs 15-20% for entity-only graphs.
-- Ref: "Ontology Learning and KG Construction Impact on RAG" (arXiv:2511.05991, 2025)
CREATE TABLE chunks (
    id TEXT PRIMARY KEY,                -- ULID
    document_id TEXT REFERENCES documents,
    content TEXT NOT NULL,              -- original text preserved for answer grounding
    chunk_index INT NOT NULL,
    embedding vector(2560),             -- pgvector
    created_at TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX chunks_embedding_idx ON chunks USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);

-- Entities (extracted by LLM)
CREATE TABLE entities (
    id TEXT PRIMARY KEY,                -- ULID
    canonical_name TEXT NOT NULL,       -- resolved name after deduplication
    entity_type TEXT,                   -- person, concept, material, company, etc.
    properties JSONB DEFAULT '{}',
    embedding vector(2560),             -- entity-level embedding for similarity search
    created_at TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX entities_embedding_idx ON entities USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
CREATE INDEX entities_name_idx ON entities (canonical_name);
CREATE INDEX entities_type_idx ON entities (entity_type);

-- Entity-channel membership (subgraph tagging)
-- Enables: search within one channel, extend into others, or search all.
CREATE TABLE entity_channels (
    entity_id TEXT REFERENCES entities,
    channel_id INT REFERENCES channels,
    document_id TEXT REFERENCES documents,
    PRIMARY KEY (entity_id, channel_id, document_id)
);

-- Entity aliases (for resolution — maps variant names to canonical entities)
-- Research: without coreference resolution, 30% of graph is duplicates.
-- Ref: CORE-KG (arXiv:2510.26512, 2025)
CREATE TABLE entity_aliases (
    alias TEXT NOT NULL,
    entity_id TEXT REFERENCES entities,
    source_document_id TEXT REFERENCES documents,
    PRIMARY KEY (alias, entity_id)
);
CREATE INDEX entity_aliases_alias_idx ON entity_aliases (alias);

-- Relationships (edges) with bi-temporal validity
-- Research: bi-temporal modeling (event time + ingestion time) scored 94.8% on deep memory retrieval.
-- Ref: Zep/Graphiti (arXiv:2501.13956, 2025)
CREATE TABLE relationships (
    id TEXT PRIMARY KEY,                -- ULID
    source_id TEXT REFERENCES entities,
    target_id TEXT REFERENCES entities,
    relationship TEXT NOT NULL,         -- from core predicate vocabulary (see below)
    properties JSONB DEFAULT '{}',
    confidence TEXT DEFAULT 'established',  -- established, emerging, contested
    channel_id INT REFERENCES channels,
    document_id TEXT REFERENCES documents,
    -- Bi-temporal validity
    valid_from TIMESTAMPTZ DEFAULT now(),   -- when the fact became true
    valid_until TIMESTAMPTZ,               -- NULL = still valid, set when superseded
    ingested_at TIMESTAMPTZ DEFAULT now(),  -- when the system learned it
    created_at TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX relationships_source_idx ON relationships (source_id);
CREATE INDEX relationships_target_idx ON relationships (target_id);
CREATE INDEX relationships_type_idx ON relationships (relationship);

-- Entity-chunk provenance (which chunks introduced which entities)
CREATE TABLE entity_chunks (
    entity_id TEXT REFERENCES entities,
    chunk_id TEXT REFERENCES chunks,
    PRIMARY KEY (entity_id, chunk_id)
);

-- Communities (hierarchical groupings of densely connected entities)
-- Research: community summarization achieves 72-83% comprehensiveness over flat retrieval.
-- Ref: Microsoft GraphRAG (arXiv:2404.16130, 2024)
CREATE TABLE communities (
    id TEXT PRIMARY KEY,                -- ULID
    level INT NOT NULL,                 -- hierarchy level (0 = root, higher = more specific)
    name TEXT,                          -- auto-generated community label
    summary TEXT,                       -- LLM-generated summary of the community
    summary_embedding vector(2560),
    parent_id TEXT REFERENCES communities,
    created_at TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX communities_level_idx ON communities (level);

-- Community membership
CREATE TABLE community_members (
    community_id TEXT REFERENCES communities,
    entity_id TEXT REFERENCES entities,
    PRIMARY KEY (community_id, entity_id)
);
```

## Core Predicate Vocabulary

Research: real-world KGs converge on dozens to hundreds of reusable predicates, not thousands. A fixed core set with extension mechanism balances consistency with coverage. (Dillinger, "Nature of KG Predicates"; Yang, "Fixed vs Dynamic Schema")

Research: "contradicts" should be first-class. Schema-violating information forms stronger, more distinct memories. (van Kesteren et al., "Schema and Novelty Augment Memory Formation," Trends in Neuroscience, 2012)

```
-- Structural
requires, enables, contains, part_of, instance_of

-- Analytical
supports, contradicts, extends, refines, supersedes

-- Causal
causes, prevents, increases, decreases

-- Domain (investment)
supplies, consumes, produces, competes_with, listed_on, holds

-- Temporal
precedes, follows, concurrent_with

-- Meta
derived_from, cited_by, same_as
```

New predicates can be proposed by the extractor and reviewed. Unmapped relationships use a `related_to` fallback with the raw text stored in `properties.raw_relationship`.

## Pipeline

```
Document
  ↓
Chunker (adaptive, markdown-aware)
  ↓
Chunks → Embed (Ollama) → Store chunks + embeddings
  ↓
Extractor (LLM, structured output)
  ↓
Raw entities + relationships
  ↓
Resolver (deduplicate, merge aliases, link to existing entities)
  ↓
Store entities, relationships, channel membership
  ↓
Community Detection (Leiden algorithm, periodic)
  ↓
Community Summarization (LLM)
```

### Stage Details

**Chunking.** Adaptive, markdown-aware via `text-splitter` crate. Respects heading and paragraph boundaries. Target chunk size: 256-512 tokens (research shows smaller chunks optimize for fact retrieval; larger for context — we bias toward facts since the graph provides context). Overlap of 10% at boundaries to preserve cross-boundary entities.

Research: adaptive chunking improves retrieval F1 from 0.24 to 0.64 (167% improvement) vs fixed-size.
Ref: PMC12649634, 2025

**Extraction.** Joint entity-relationship extraction in a single LLM call per chunk. The prompt requests structured JSON output with entity types, relationship predicates from the core vocabulary, and confidence levels.

Research: joint extraction outperforms pipeline (extract entities, then predict relations). The pipeline approach creates O(N²) entity pairs, most of which are noise.
Ref: arXiv:2511.08143, 2025

A self-reflection pass ("are there entities you missed?") follows the initial extraction.

Research: self-reflection nearly doubles entity detection without introducing noise.
Ref: Microsoft GraphRAG (arXiv:2404.16130, 2024)

**Resolution.** After extraction, each entity is checked against existing entities:
1. Exact name match → merge
2. Alias table match → merge
3. Embedding similarity above threshold → propose merge (auto-merge if above high threshold, flag for review if between thresholds)
4. No match → create new entity, store name as alias

Research: KGGen's iterative LLM-based clustering achieves 66% resolution accuracy vs GraphRAG's 48%. Entity resolution is where the most quality is left on the table.
Ref: Mo et al., KGGen, NeurIPS 2025

Research: for heterogeneous personal knowledge, LLM-based resolution outperforms fine-tuned models by 40-68% F1 on unseen entity types.
Ref: Peeters & Bizer, EDBT 2025

**Community Detection.** Runs periodically (not per-ingestion). Leiden algorithm partitions the entity graph into communities at multiple hierarchy levels. Each community gets an LLM-generated summary and summary embedding.

Research: community-level summaries at intermediate levels (not root, not leaf) achieve the strongest retrieval. Root-level uses 9-43x fewer tokens with only 15-20% comprehensiveness loss.
Ref: Microsoft GraphRAG (arXiv:2404.16130, 2024)

Research: community detection as a pre-retrieval index focuses search on semantically coherent subgraphs.
Ref: CommunityKG-RAG (arXiv:2408.08535, 2025)

**Temporal Validity.** When new information contradicts an existing relationship, the old edge gets `valid_until = now()` rather than being deleted. Both versions remain queryable. Default search returns only currently-valid edges. Historical search returns all edges with their validity windows.

Research: bi-temporal modeling scored 94.8% on deep memory retrieval benchmark.
Ref: Zep/Graphiti (arXiv:2501.13956, 2025)

## Search

### Dual-Channel Retrieval

Every search runs both graph traversal and vector similarity, then merges results.

Research: dual-channel retrieval outperforms either alone.
Ref: KG-RAG (Nature Scientific Reports, 2025)

### Search Modes

**1. Graph traversal.** Start from matched entities, walk N hops along relationships. Filter by channel, relationship type, validity window.

```sql
WITH RECURSIVE traverse AS (
    SELECT e.id, e.name, e.entity_type, 0 as depth, ARRAY[e.id] as path
    FROM entities e
    WHERE e.canonical_name ILIKE $1
    UNION ALL
    SELECT e2.id, e2.name, e2.entity_type, t.depth + 1, t.path || e2.id
    FROM traverse t
    JOIN relationships r ON r.source_id = t.id
        AND r.valid_until IS NULL                    -- only current relationships
        AND ($2::int[] IS NULL OR r.channel_id = ANY($2))  -- channel filter
    JOIN entities e2 ON e2.id = r.target_id
    WHERE t.depth < $3                               -- max depth
        AND NOT e2.id = ANY(t.path)                  -- cycle prevention
)
SELECT DISTINCT ON (id) * FROM traverse;
```

**2. Vector similarity on chunks.** Find the most relevant text chunks. Returns source text for answer grounding.

**3. Vector similarity on entities.** Find semantically similar entities regardless of name match.

**4. Community search.** Match query against community summary embeddings. Return the community's entities and summary. Best for broad questions.

Research: retrieved subgraph size should be proportional to query complexity. Simple queries need small subgraphs; complex queries need larger ones.
Ref: SubgraphRAG, ICLR 2025

### Multi-Perspective Query Decomposition

When searching across channels, the query is rewritten from each channel's perspective before execution. Results from each perspective are merged and deduplicated.

Research: multi-perspective query rewriting nearly triples retrieval accuracy in knowledge-dense domains.
Ref: MVRAG (arXiv:2404.12879, 2024)

### Context Window

Research: 8K token context windows outperformed 16K/32K/64K in GraphRAG benchmarks. Smaller context forced more focused retrieval.
Ref: Microsoft GraphRAG (arXiv:2404.16130, 2024)

Search results are capped at 8K tokens. If more results are available, they are ranked and truncated rather than all included.

## REST API (Axum)

Compatible with intake engine's existing backend trait:

```
POST   /api/v1/add              — add document (chunk + embed)
POST   /api/v1/integrate        — extract entities + relationships (LLM)
POST   /api/v1/search           — hybrid search (graph + vector + community)
GET    /api/v1/datasets         — list channels
DELETE /api/v1/datasets/:id     — delete channel
GET    /health                  — health check

New endpoints (not in Cognee):
GET    /api/v1/entities/:id     — inspect an entity, its aliases, relationships, channels
GET    /api/v1/communities      — list communities with summaries
POST   /api/v1/resolve          — trigger entity resolution pass
POST   /api/v1/communities/rebuild — trigger community detection + summarization
```

## Dependencies

```toml
[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "json", "chrono"] }
pgvector = { version = "0.4", features = ["sqlx"] }
text-splitter = { version = "0.29", features = ["tokenizers"] }
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ulid = "1"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap = { version = "4", features = ["derive"] }
tower-http = { version = "0.6", features = ["cors", "trace"] }
```

## Build Order

1. **Schema + migrations** — all tables, indices, pgvector extension
2. **Types** — Entity, Relationship, Chunk, Channel, Community, SearchResult, TemporalEdge
3. **Graph module** — insert entities/relationships, recursive traversal, channel-filtered queries, temporal filtering
4. **Vectors module** — insert embeddings, similarity search on chunks and entities with channel filters
5. **Chunker** — text-splitter integration, markdown-aware, configurable chunk size with overlap
6. **Embedder** — Ollama HTTP client for chunks and entities
7. **Resolver** — entity deduplication via name match, alias lookup, and embedding similarity
8. **Extractor** — trait + mock for testing, LLM impl wired later. Joint entity-relationship extraction with self-reflection pass.
9. **Temporal** — bi-temporal validity on edges, supersession logic, historical queries
10. **Communities** — Leiden community detection, hierarchical summarization, community search
11. **Server** — Axum REST API, dual-channel search (graph + vector), multi-perspective query decomposition
12. **Tests** — unit tests for every module, integration tests for the full pipeline with mock extractor

## Stubbed for Later

- `LlmExtractor` (calls Claude — wired after initial build, no API cost during development)
- Community summarization (requires LLM — stubbed with placeholder summaries for testing)
- Multi-perspective query decomposition (requires LLM for query rewriting — stubbed with pass-through)

All LLM-dependent features use a `trait` with mock implementations for testing.

## Intake Engine Integration

The intake engine's `Backend` trait abstracts over the HTTP client. Switching from Cognee to graph-engine is a URL change:

```
COGNEE_URL=http://graph-engine:8000
```

## Research References

All design decisions above are grounded in specific research. Full reference list:

### Extraction
- Wang et al., "GPT-NER: Named Entity Recognition via Large Language Models," NAACL 2025 (arXiv:2304.10428) — LLMs hallucinate entities, need self-verification
- Chen et al., "Exploring Nested NER with LLMs," EMNLP 2024 — output format materially changes extraction quality
- arXiv:2511.08143, 2025 — joint extraction outperforms entity-then-relation pipeline
- Microsoft GraphRAG (arXiv:2404.16130, 2024) — self-reflection pass nearly doubles entity detection

### Entity Resolution
- CORE-KG (arXiv:2510.26512, 2025) — 30% duplication without coreference, 73% noise increase without structured prompts
- Mo et al., KGGen, NeurIPS 2025 (arXiv:2502.09956) — iterative LLM clustering achieves 66% resolution accuracy
- Peeters & Bizer, EDBT 2025 (arXiv:2310.11244v4) — LLM-based resolution 40-68% better than fine-tuned models on unseen types

### Relationships
- KARMA, NeurIPS 2025 Spotlight (arXiv:2502.06472) — even 9-agent systems achieve only 83% correctness
- van Kesteren et al., Trends in Neuroscience, 2012 — contradictions form stronger memories, "contradicts" should be first-class

### Chunking
- PMC12649634, 2025 — adaptive chunking: 167% retrieval improvement over fixed-size
- SLIDE (arXiv:2503.17952, 2025) — chunk boundaries sever entity references
- arXiv:2505.21700, 2025 — smaller chunks for facts, larger for context, no universal optimal

### Graph Structure
- arXiv:2511.05991, 2025 — source text alongside graph: 90% vs 15-20% retrieval accuracy
- Dillinger, "Nature of KG Predicates" — real KGs converge on dozens to hundreds of predicates
- arXiv:2411.14480, 2025 — sparse meaningful connections outperform dense noisy ones
- Collins & Loftus, 1975 — spreading activation: 5-15 high-quality links per entity optimal

### Communities
- Microsoft GraphRAG (arXiv:2404.16130, 2024) — hierarchical communities: 72-83% comprehensiveness, 8K windows beat larger
- CommunityKG-RAG (arXiv:2408.08535, 2025) — community detection as pre-retrieval index

### Temporal
- Zep/Graphiti (arXiv:2501.13956, 2025) — bi-temporal validity: 94.8% deep memory retrieval
- arXiv:2403.04782, 2025 — temporal KG decay policies improve relevance

### Retrieval
- Barnett et al., IEEE/ACM CAIN 2024 (arXiv:2401.05856) — seven RAG failure points
- KG-RAG, Nature Scientific Reports, 2025 — dual-channel retrieval outperforms either alone
- SubgraphRAG, ICLR 2025 (arXiv:2410.20724) — subgraph size proportional to query complexity
- arXiv:2508.08344, 2025 — graph structure value is in connections text search cannot find

### Multi-Perspective
- MVRAG (arXiv:2404.12879, 2024) — multi-view query rewriting triples accuracy
- KDD 2022 — fact-view and context-view provide complementary retrieval signals

### Cognitive Science
- Craik & Lockhart, 1972 — deeper semantic processing creates more retrievable memories
- Collins & Loftus, 1975 — spreading activation, fan effect limits optimal connectivity
- van Kesteren et al., 2012 — schema-incongruent information forms stronger distinct memories

### Completeness
- BioKGrapher (PMC11536026, 2024) — automated KG captures ~60% of what humans would
- KGGen, NeurIPS 2025 — best extractors miss one-third of known facts
- ReGraphRAG, EMNLP 2025 — fragmentation is the default outcome, not an edge case
- arXiv:2510.20345, 2025 — pipeline errors compound across stages
