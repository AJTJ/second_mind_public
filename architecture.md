# Learning Engine — Architecture

Knowledge graph with extensible channels, entity extraction, and hybrid retrieval. Each channel pairs a dataset with a processing prompt that controls what gets extracted.

## System Design

```
Claude Code (MCP tools, /explore skill)
              |
              | MCP (port 8001)
              |
         Intake Engine (Rust)
         audit log + source library + MCP tools
              |
              | HTTP API (port 8000)
              |
         Graph Engine (Rust)
         chunking + extraction + resolution + search
              |
        ┌─────┴─────┐
    Neo4j         Postgres + pgvector
    entities      embeddings
    relationships LLM cache
    traversal     metadata
```

### How It Works

1. Claude calls the intake engine's MCP tools with a dataset and prompt name
2. The intake engine logs the decision, stores sources in the content-addressed library, and forwards to the graph engine:
   - `add()` — chunks the document, embeds chunks, stores in Postgres
   - `integrate()` — extracts entities and relationships via LLM, resolves against existing graph, stores with temporal validity
3. Search runs graph traversal and vector similarity in parallel, merges results

### Why the Intake Engine Layer

The graph engine handles storage and processing, but the intake engine adds:
- **Audit trail** — every ingestion is logged with full provenance (source, prompt version, timestamp)
- **Replay** — re-process any entry with the original or a different prompt
- **Content-addressed library** — SHA-256 hashed source storage for deduplication and replay
- **Failure recovery** — `AddedNotIntegrated` status tracks partial failures for retry

### Channels and Lenses

A channel is a dataset name paired with a processing prompt. The prompt defines the lens — what to extract, how to categorize, what to keep or discard. Create a new channel by adding a `.md` file to `channels/` and using its name as `prompt_name` during ingestion.

Channels share a single graph with entity-level channel tagging. Entities can belong to multiple channels. Search can filter by channel or span all.

**Example: personal channel**
- Dataset: `personal`, Prompt: `personal.md`
- Lens: "How does this help Aaron get things done?"
- Categories: PROJECT, QUESTION, RESOURCE, PARKED. Discards noise.

**Example: research channel**
- Dataset: `research`, Prompt: `research.md`
- Lens: "What are the key claims, concepts, and evidence?"
- Extracts claims with confidence levels. Preserves objectively.

**Example: investment channel**
- Dataset: `investment`, Prompt: `investment.md`
- Lens: "What does this mean for resource demand and investment positioning?"
- Extracts: resource dependencies, supply/demand dynamics, company exposure, Canadian-accessible vehicles (TSX, ETFs, TFSA/RRSP eligible). Flags data staleness.

### What the Graph Engine Provides

- Entity extraction with gleaning (multi-pass, +15-30% recall) via Claude API
- Entity resolution: normalize → singularize → exact match → alias lookup
- Bi-temporal validity on relationships (valid_from, valid_until, ingested_at)
- Contradiction detection before supersession
- Markdown-aware chunking with configurable size
- Vector embeddings via Ollama (`qwen3-embedding:4b`, 2560 dims)
- Graph storage and vector search in Postgres (pgvector)
- Community detection via label propagation
- LLM response cache (SHA-256 content-addressed)
- Hybrid retrieval (graph traversal + vector similarity)

### What We Build

**Graph Engine** (`graph-engine/`, 3,200 lines Rust, 92 tests):
- Axum REST API on port 8000
- Markdown-aware chunking via `text-splitter`
- Entity/relationship extraction via Claude API with gleaning (multi-pass, +15-30% recall)
- Entity resolution: normalize → singularize → exact match → alias lookup
- Bi-temporal validity on relationships (valid_from, valid_until, ingested_at)
- Contradiction detection before supersession (identical → skip, novel → insert, contradictory → supersede)
- LLM response cache (SHA-256 content-addressed, Postgres-backed)
- Community detection via label propagation
- Hybrid search: graph traversal + vector similarity (pgvector)
- Negative-example prompting and entity type filtering

Techniques adopted from: GraphRAG (gleaning), Graphiti (temporal model, negative examples), KGGen (SEMHASH normalization), nano-graphrag (LLM cache). See `graph-engine/FEATURES.md` for full lineage.

**Intake Engine** (`intake-engine/`, Rust):
- MCP server on port 8001, sole entry point for Claude
- Append-only JSONL audit log with last-write-wins deduplication
- Content-addressed file library (SHA-256)
- Dual CLI + MCP interface
- Log compaction via `just compact`

**Explorer Agent** (`explorer/`, deprecated):
- Replaced by the `/explore` skill in Claude Code. Same methodology, no separate API cost.

## Infrastructure

### Docker Compose

```
docker-compose.yml
├── intake-engine   (127.0.0.1:8001) — Rust MCP server
├── graph-engine    (internal :8000)  — Rust knowledge graph API
├── neo4j           (internal :7687)  — Graph database (entities, relationships, traversal)
├── postgres        (internal)        — pgvector (embeddings, cache, metadata)
└── ollama          (internal)        — Embedding model (qwen3-embedding:4b)
```

Prompts and data are bind-mounted from the host (`:cached` for macOS). The graph engine and Postgres are only reachable within the Docker network. The intake engine is the sole exposed service.

### Required Config

Single `.env` file at `docker/.env` serves all components:

| Key | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` | Claude API key (entity extraction + research) |
| `DATABASE_URL` | Postgres connection (graph engine) |
| `EMBEDDING_ENDPOINT` | Ollama URL (default: `http://ollama:11434/api/embed`) |
| `EMBEDDING_MODEL` | Embedding model (default: `qwen3-embedding:4b`) |
| `COGNEE_URL` | Backend URL for intake engine (default: `http://graph-engine:8000`) |
| `COGNEE_USER` / `COGNEE_PASS` | Auth credentials |

### MCP Connection

Claude Code connects to the intake engine's MCP server at `localhost:8001/mcp`. The intake engine exposes four tools:
- `ingest(sources, datasets, prompt_name)` — store + log + forward to graph engine
- `search(query, datasets, search_type, top_k)` — query graph engine
- `replay(entry_id, prompt_override)` — re-process entries
- `log(dataset, prompt, limit)` — view ingestion history

## Search Strategy

Four search types with different retrieval mechanisms. All searches run locally (Postgres + Ollama) at zero API cost. Only integrate uses the Claude API.

### Search Types

| Type | Mechanism | Best for | Returns |
|---|---|---|---|
| `GRAPH_COMPLETION` | Knowledge graph traversal + LLM completion | Relational queries ("how does X relate to Y"), conceptual questions | Synthesized answers from graph structure |
| `CHUNKS` | Vector similarity on raw document chunks | Specific facts, quotes, data points | Raw text chunks ranked by similarity |
| `SIMILARITY` | Vector similarity on embedded entities | Finding related concepts, fuzzy matching | Entity-level matches |
| `SUMMARIES` | Vector similarity on generated summaries | Broad overview queries, "what do I know about X" | Summary-level matches |

### Defaults

- **MCP tool**: `CHUNKS` (raw retrieval for programmatic callers)
- **CLI `just search`**: `SUMMARIES` (human-readable for terminal use)
- **Explorer KB check**: `GRAPH_COMPLETION` (relational context for research agent)

These differ intentionally — each entry point has a different consumer with different needs.

### Known Gaps

- Empty results are common when knowledge is sparse. A query returning `[]` may mean the data doesn't exist OR the wrong search type was used.
- No cascading fallback yet — if one search type returns empty, the system doesn't automatically try another.
- No cross-dataset result merging — searching multiple datasets returns separate result sets, not unified results.

## Entry Lifecycle

```
Pending → Added → Integrated       (happy path)
Pending → Failed                   (add failed)
Pending → Added → AddedNotIntegrated  (integrate failed, recoverable via replay)
Any → DatasetDeleted              (dataset/channel deleted)
```

## Custom Prompts

### Personal Channel Prompt

Used as `custom_prompt` when integrating personal dataset:

```
Evaluate this input through the lens: "How does this help Aaron get things done?"

For each piece of input:
1. Categorize by utility:
   - PROJECT: Something actionable with identifiable next steps
   - QUESTION: Something Aaron is still figuring out
   - RESOURCE: Something that serves an existing project or question
   - PARKED: Acknowledged but intentionally set aside

2. Extract:
   - Next steps (if actionable)
   - Connections to existing concepts in the graph
   - Whether this changes or contradicts something already stored

3. Do NOT store noise. If input has no utility, discard it.
```

### Research Channel Prompt

Used as `custom_prompt` when integrating research dataset:

```
Extract knowledge from this input objectively. Do not filter by personal utility.

For each piece of input:
1. Extract key claims with confidence levels (established, emerging, contested)
2. Identify core concepts and their definitions
3. Map relationships between concepts (supports, contradicts, extends, requires)
4. Note sources and evidence quality
5. Connect to existing concepts in the graph where relationships exist

Preserve the research as-is. Do not editorialize or evaluate usefulness.
```

## Project Structure

```
second-mind/
├── architecture.md              # This file
├── graph-engine/                # Rust knowledge graph engine (port 8000)
│   ├── src/
│   │   ├── main.rs              # Axum server + CLI
│   │   ├── server.rs            # REST API handlers (thin)
│   │   ├── pipeline.rs          # Business logic (add, integrate, search, delete)
│   │   ├── extractor.rs         # LLM extraction with gleaning + mock
│   │   ├── embedder.rs          # Ollama embedding client (trait-based)
│   │   ├── resolver.rs          # Entity resolution (normalize, singularize, match)
│   │   ├── temporal.rs          # Bi-temporal validity + contradiction detection
│   │   ├── communities.rs       # Label propagation community detection
│   │   ├── graph.rs             # Entity/relationship CRUD + traversal
│   │   ├── vectors.rs           # pgvector similarity search
│   │   ├── chunker.rs           # Markdown-aware text splitting
│   │   ├── cache.rs             # LLM response cache (SHA-256 keyed)
│   │   ├── schema.rs            # Database migrations
│   │   └── types.rs             # Shared types
│   ├── tests/integration.rs     # 53 integration tests (Postgres required)
│   ├── migrations/001_initial.sql
│   ├── analysis/                # Comparative analysis of 6 graph systems
│   ├── FEATURES.md              # Feature lineage (what came from where)
│   └── PLAN.md                  # Design rationale with research references
├── intake-engine/               # Rust MCP server (port 8001)
│   └── src/
│       ├── main.rs, server.rs, backend.rs, cognee.rs
│       ├── library.rs, log.rs, search_log.rs, prompts.rs, types.rs
├── channels/                    # Channel lenses (bind-mounted)
├── channels.example/            # Generic examples (ships publicly)
├── docker/
│   ├── docker-compose.yml       # Full stack
│   └── .env.example
├── data/                        # Runtime data (bind-mounted)
│   ├── intake.jsonl, searches.jsonl, library/
```

## Future Work

- Hierarchical community summarization (LLM-generated summaries per community)
- MinHash/LSH fuzzy entity resolution (tier 2 between exact match and LLM)
- Cascading search fallback (try alternate search types on empty results)
- Health endpoint on intake engine
