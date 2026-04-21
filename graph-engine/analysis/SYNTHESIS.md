# Knowledge Graph Systems — Comparative Synthesis

Analysis of 6 systems to inform the graph-engine design. Each system was analyzed by reading actual source code, not documentation.

## Systems Analyzed

| System | Language | Focus | Stars |
|---|---|---|---|
| Cognee 0.5.6 | Python | Full pipeline, custom prompts | 15K |
| GraphRAG | Python | Community detection + hierarchical retrieval | 32K |
| Graphiti (Zep) | Python | Temporal validity + entity resolution | 25K |
| LightRAG | Python | Cost-efficient dual-level retrieval | 33K |
| KGGen | Python | Entity/relationship deduplication | 1K |
| nano-graphrag | Python | Minimal GraphRAG reimplementation | 4K |

---

## Best-in-Class Techniques by Category

### Extraction

**Winner: Graphiti** for prompt engineering. LightRAG for output robustness.

| System | Joint extraction | Gleaning | Self-reflection | Negative examples | Error recovery |
|---|---|---|---|---|---|
| Cognee | Unknown (black box) | No | No | No | No |
| GraphRAG | Yes | Yes (configurable rounds) | No | No | Regex parsing |
| Graphiti | No (pipeline) | No | No | **Yes (extensive)** | Pydantic validation |
| LightRAG | Yes | Yes (1 round default) | No | No | **Best** (delimiter corruption recovery) |
| KGGen | No (pipeline) | No | No | No | Constrained output + fallback |
| nano-graphrag | Yes | Yes (1 round) | DSPy path has critique-refine | No | Basic |

**Adopt for graph-engine:**
1. Joint entity+relationship extraction in one prompt (GraphRAG/LightRAG pattern)
2. Gleaning with YES/NO gate for early termination (GraphRAG pattern)
3. Negative-example prompting (Graphiti's exclusion lists)
4. Literal-type constrained relation extraction (KGGen's Pydantic approach)
5. Delimiter corruption recovery (LightRAG's robust parsing)

### Chunking

**Winner: Graphiti** for the insight that not all documents need chunking.

| System | Strategy | Configurable | Overlap | Semantic awareness |
|---|---|---|---|---|
| Cognee | Black box | No | Unknown | Unknown |
| GraphRAG | Fixed token count | Yes (chunk_size, overlap) | Yes | No |
| Graphiti | **Density-aware** (only chunks entity-dense docs) | Yes | N/A | Entity density check |
| LightRAG | Fixed token count | Yes | No | No |
| KGGen | Caller's responsibility | N/A | N/A | N/A |
| nano-graphrag | Token-level (encode, slice, decode) | Yes | Yes | No |

**Adopt for graph-engine:**
1. Use `text-splitter` crate with markdown awareness (better than any system's approach)
2. Density-aware chunking decision (Graphiti pattern — skip chunking for short docs)
3. Token-level precision (nano-graphrag's encode-slice-decode)
4. Configurable overlap (10% default)

### Entity Resolution

**Winner: Graphiti** for the three-tier cascade. KGGen for the deduplication algorithm.

| System | Method | Fuzzy matching | LLM fallback | Edge dedup |
|---|---|---|---|---|
| Cognee | Unknown (black box) | Unknown | Unknown | Unknown |
| GraphRAG | **None** (exact name match only) | No | No | No |
| Graphiti | **Three-tier** (exact → MinHash/LSH → LLM) | **Yes** (MinHash) | **Yes** | **Yes** (LLM detects duplicate edges) |
| LightRAG | Exact name match + LLM description merge | No | Partial (merge descriptions) | No |
| KGGen | **SEMHASH** (normalization) → KMeans clustering → **rank fusion** → LLM | **Yes** (n-gram fingerprinting) | **Yes** | **Yes** |
| nano-graphrag | Exact uppercase match | No | No | No |

**Adopt for graph-engine:**
1. Three-tier cascade: cheap normalization → fuzzy matching → LLM (combining Graphiti's architecture with KGGen's SEMHASH)
2. KGGen's rank fusion (BM25 + embedding) for candidate selection within clusters
3. Edge deduplication as first-class (KGGen pattern)
4. Graphiti's entropy gate for short names (prevents false positives)

### Relationships

**Winner: Graphiti** for fact-based edges with episode provenance.

| System | Typed | Directional | Free-text descriptions | Provenance | Temporal |
|---|---|---|---|---|---|
| Cognee | Unknown | Unknown | Unknown | Via intake log | No |
| GraphRAG | No (free-text) | Yes | **Yes** (summarized) | **Yes** (text_unit_ids) | Partial (claim date ranges) |
| Graphiti | Free-text SCREAMING_SNAKE | Yes | **Yes** (fact strings) | **Yes** (episode UUIDs) | **Yes** (bitemporal) |
| LightRAG | **Keyword-based** | **No** (undirected) | Yes | Yes (source_id) | No |
| KGGen | Free-text predicates | Yes | No | No | No |
| nano-graphrag | **None** (all "RELATED") | Yes | Yes | Yes (source_id) | No |

**Adopt for graph-engine:**
1. Core predicate vocabulary with free-text description (our plan's 20+ predicates + `properties.raw_relationship`)
2. Fact-based edge storage (Graphiti pattern — natural language fact string per edge)
3. Provenance via document/chunk IDs (GraphRAG's text_unit_ids pattern)
4. Bitemporal validity (Graphiti's valid_at/invalid_at/expired_at)
5. Directional edges (LightRAG's undirected approach loses information)

### Storage

**Winner: LightRAG** for pluggable backends. Graphiti for graph database integration.

| System | Graph store | Vector store | Metadata store | Pluggable |
|---|---|---|---|---|
| Cognee | KuzuDB (embedded) | LanceDB (embedded) | Postgres | No |
| GraphRAG | Parquet files / CosmosDB | In-memory | Parquet files | Partially |
| Graphiti | **Neo4j** / FalkorDB / Neptune | Neo4j vectors | Neo4j | **Yes** (multiple backends) |
| LightRAG | NetworkX / Neo4j / Postgres | nano-vectordb / Postgres | JSON / Postgres | **Yes** (most backends) |
| KGGen | None (extraction only) | None | None | N/A |
| nano-graphrag | NetworkX (in-memory) | JSON KV (in-memory) | JSON KV | Via dataclass fields |

**Adopt for graph-engine:**
1. Postgres for everything (our plan — graph tables + pgvector). One database, one process, scales well.
2. Clean storage trait/interface for future backend swaps (LightRAG's abstraction pattern)

### Retrieval

**Winner: LightRAG** for dual-level routing. GraphRAG for community-based global search.

| System | Graph traversal | Vector similarity | Community search | Hybrid | Fallback |
|---|---|---|---|---|---|
| Cognee | GRAPH_COMPLETION | CHUNKS, SIMILARITY | SUMMARIES (maybe) | No | No |
| GraphRAG | Local search (entity neighborhood) | Local search (entity + chunk vectors) | **Global search** (map-reduce over reports) | DRIFT (iterative) | DRIFT falls back to local |
| Graphiti | BFS from seed nodes | Vector search + rerankers | No | **Yes** (composable) | Via search config |
| LightRAG | Entity neighborhood | **Dual VDB** (entity + relationship) | No | **6 modes** | Mix mode combines |
| KGGen | 2-hop graph walk | Basic cosine | No | No | No |
| nano-graphrag | Entity neighborhood | Entity VDB | Community report VDB | Local + global | No |

**Adopt for graph-engine:**
1. Dual-channel retrieval: graph traversal + vector similarity always together (research-validated)
2. Community-based global search (GraphRAG's map-reduce over summaries)
3. LightRAG's dual VDB concept (separate entity + relationship indices)
4. Graphiti's composable search with pluggable rerankers
5. Multi-perspective query decomposition at search time (from MVRAG research)
6. 8K token result cap (GraphRAG's finding that smaller windows beat larger)

### Community Detection

**Winner: GraphRAG** (the only system that does this well).

| System | Algorithm | Hierarchical | Summaries | Incremental |
|---|---|---|---|---|
| Cognee | None visible | No | No | N/A |
| GraphRAG | **Leiden** | **Yes** (multi-level) | **Yes** (LLM-generated, structured) | No (full rebuild) |
| Graphiti | Label propagation | **Yes** (multi-level) | Yes (LLM-generated) | Yes |
| LightRAG | None (replaced by keyword routing) | No | No | N/A |
| KGGen | KMeans (for dedup only) | No | No | N/A |
| nano-graphrag | **Leiden** | **Yes** | **Yes** (severity-rated) | No (full rebuild) |

**Adopt for graph-engine:**
1. Leiden algorithm for community detection (GraphRAG/nano-graphrag)
2. Hierarchical levels with bottom-up summarization (nano-graphrag's approach)
3. Structured report format with severity rating (nano-graphrag)
4. Incremental community maintenance, not full rebuild (Graphiti's approach adapted)

### Temporal Handling

**Winner: Graphiti** (no contest — only system with real temporal modeling).

| System | Timestamps | Validity intervals | Contradiction detection | Decay |
|---|---|---|---|---|
| Cognee | Intake log only | No | No | No |
| GraphRAG | Claim date ranges (optional) | No | No | No |
| Graphiti | **Bitemporal** (valid_at, invalid_at, expired_at, reference_time) | **Yes** | **Yes** (LLM + temporal logic) | No |
| LightRAG | created_at stored | No | No | No |
| KGGen | None | No | No | No |
| nano-graphrag | None | No | No | No |

**Adopt for graph-engine:**
1. Full bitemporal model from Graphiti (valid_from, valid_until, ingested_at)
2. LLM-detected contradiction + rule-based temporal ordering (Graphiti's hybrid)
3. Out-of-order ingestion handling (Graphiti's reverse invalidation logic)
4. Temporal filtering in search (Graphiti's gap — we should fix this)

### Cross-Document Linking

**Winner: Graphiti** for multiple cross-referencing layers.

| System | Mechanism | Quality |
|---|---|---|
| Cognee | Possibly shared entity names across datasets | Poor (datasets appear isolated) |
| GraphRAG | Shared entity names (uppercase match) | Moderate (no fuzzy) |
| Graphiti | Entity dedup + edge episode tracking + sagas + communities | **Best** (5 distinct mechanisms) |
| LightRAG | Shared entity names (uppercase match) | Moderate (no fuzzy) |
| KGGen | Aggregate + deduplicate | Good (with dedup) |
| nano-graphrag | Shared entity names (uppercase match) | Moderate (no fuzzy) |

**Adopt for graph-engine:**
1. Entity resolution as the primary cross-document bridge (with our three-tier cascade)
2. Chunk/document provenance on every entity and edge
3. Community detection as a secondary cross-document synthesis layer

---

## Techniques No System Implements

These are gaps across all 6 systems that our graph-engine should address:

1. **Multi-perspective organization (channels).** No system supports processing the same document through different extraction lenses. Cognee's dataset+prompt is closest but with physical isolation.

2. **Quality gate between extraction and storage.** No system filters noise entities before storing them. Cognee's 62.5% noise contamination proves this matters.

3. **Subgraph-aware queries with extension.** "Search investment, extend into worldview" is not possible in any system. All use either full isolation or no isolation.

4. **Temporal filtering in search.** Even Graphiti stores temporal data but doesn't filter on it during retrieval.

5. **Configurable entity connectivity limits.** No system enforces the spreading-activation research finding that 5-15 relationships per entity is optimal.

---

## Priority Adoption Matrix for Graph-Engine

Ranked by impact on retrieval quality, referenced to source system and research.

| # | Technique | From | Impact | Effort | Research backing |
|---|---|---|---|---|---|
| 1 | Store source text alongside graph | Original (research-backed) | **Critical** | Low | 90% vs 15-20% retrieval (arXiv:2511.05991) |
| 2 | Three-tier entity resolution | Graphiti + KGGen | **Critical** | High | 30% duplication without (CORE-KG) |
| 3 | Joint extraction with gleaning | GraphRAG + LightRAG | **High** | Medium | Nearly doubles entity detection (GraphRAG) |
| 4 | Negative-example extraction prompts | Graphiti | **High** | Low | Reduces noise extraction |
| 5 | Bitemporal edge validity | Graphiti | **High** | Medium | 94.8% deep memory retrieval (Zep) |
| 6 | Community detection + summaries | GraphRAG + nano-graphrag | **High** | High | 72-83% comprehensiveness (GraphRAG) |
| 7 | Quality gate (noise filtering) | Novel | **High** | Medium | 62.5% noise in Cognee production data |
| 8 | Dual-channel retrieval | LightRAG + research | **High** | Medium | Outperforms either alone (Nature 2025) |
| 9 | Edge deduplication | KGGen | **Medium** | Medium | Reduces predicate sprawl |
| 10 | Fact-based edge storage | Graphiti | **Medium** | Low | Enables semantic search over relationships |
| 11 | LLM response cache | nano-graphrag | **Medium** | Low | Makes reruns free |
| 12 | Multi-perspective query decomposition | MVRAG research | **Medium** | Medium | 3x accuracy (arXiv:2404.12879) |
| 13 | Content-hash dedup at every level | nano-graphrag | **Medium** | Low | Prevents redundant processing |
| 14 | Delimiter corruption recovery | LightRAG | **Low** | Low | Defensive, prevents silent data loss |
| 15 | Density-aware chunking decision | Graphiti | **Low** | Low | Avoids unnecessary splitting |

---

## Open Design Questions

### Storage: Postgres-only vs. purpose-built indices

The plan uses Postgres for everything (graph tables + pgvector). The argument is operational simplicity. The counterargument: pgvector is adequate but not purpose-built for vector similarity at scale, and recursive CTEs are clumsy compared to graph query languages. At dozens of documents this doesn't matter. At thousands it might. Starting with Postgres and adding a dedicated vector index later if latency becomes a problem is a reasonable path. The decision should be driven by measured performance, not upfront architecture.

### Bidirectional pipeline feedback

Every system analyzed treats the pipeline as strictly forward: chunk → extract → resolve → store → index → retrieve. No step communicates backward. This is a potential area for novel integration:

- **Extraction-aware-of-graph.** Before extracting from a new document, include relevant existing entities in the extraction prompt. "The graph already knows: copper, Teck Resources, COPX. Use these exact names if you see them." Reduces entity resolution burden because the extractor produces canonical names from the start.
- **Resolution-aware-of-communities.** When resolving "Cu" vs "copper," check community membership. If "copper" is in the materials/mining community, "Cu" is almost certainly the same entity. Community context becomes a resolution signal.
- **Retrieval-aware-of-extraction-quality.** Entities extracted from a single chunk with low confidence get down-weighted in search. Entities confirmed across 10 documents get boosted.

No existing system implements these feedback loops. Whether they're worth the complexity is an open question — they add coupling between pipeline stages that are currently independent. Worth prototyping after the forward pipeline works.

### Quality gate: start simple

The quality gate between extraction and storage prevents noise from permanently entering the graph. Cognee's 62.5% noise contamination (from ingesting failed web scrapes as research data) demonstrates the need.

The gate should start with mechanical checks only — no heuristics, no judgment calls:
- Source text below minimum length (e.g., < 50 chars) → reject
- Source text matches a blocklist of known error patterns ("access denied", "rate limited", "technical difficulties") → reject
- Entity name is empty, or longer than 100 chars (likely a description that leaked into the name field) → drop that entity

These are not heuristic quality judgments. They're structural validity checks. Fancier filtering (entity meaningfulness, confidence scoring) can come later and is optional. The cheap source-text checks are not.

---

## What Our Graph-Engine Has That No One Else Does

Combining the best techniques from all 6 systems with the research findings, our planned graph-engine would be unique in:

1. **Subgraph-aware queries with cross-channel extension** — search one channel, allow traversal into specified others
2. **Multi-perspective extraction** — same document through different lenses, unified graph
3. **Quality gate** — mechanical noise filtering between extraction and storage
4. **Complete temporal model with search filtering** — Graphiti's bitemporal model but actually usable in queries
5. **Three-tier entity resolution combining the best of Graphiti and KGGen** — cheap normalization, fuzzy matching, LLM fallback, applied to both entities and edges
6. **Bidirectional pipeline feedback** (future) — extraction aware of existing graph, resolution aware of communities, retrieval aware of extraction quality
7. **Rust implementation** — every system analyzed is Python. Rust gives type safety, performance, and single-binary deployment

---

## Individual Report Locations

- [01-cognee.md](01-cognee.md)
- [02-graphrag.md](02-graphrag.md)
- [03-graphiti.md](03-graphiti.md)
- [04-lightrag.md](04-lightrag.md)
- [05-kggen.md](05-kggen.md)
- [06-nano-graphrag.md](06-nano-graphrag.md)
