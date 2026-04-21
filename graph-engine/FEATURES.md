# Second Mind Graph Engine — Feature Adoption

Every feature traces to a specific reference system, research paper, or production finding. Nothing was added speculatively.

## Implemented

### From GraphRAG (Microsoft)

**Gleaning** — Multi-pass extraction. After initial extraction, the LLM is prompted with "MANY entities were missed" and asked to produce more. A YES/NO gate terminates early. Configurable via `max_gleanings` (default: 1). One extra pass catches 15-30% more entities.
- File: `src/extractor.rs`, `LlmExtractor::extract()`
- Source: `graphrag/index/operations/extract_graph/graph_extractor.py`
- Research: Microsoft GraphRAG (arXiv:2404.16130, 2024)

### From Graphiti (Zep)

**Contradiction detection** — Before superseding a relationship, check if the new fact is identical (skip), novel (insert), or contradictory (supersede). Prevents both duplicate insertion and silent data loss from false supersession.
- File: `src/temporal.rs`, `check_contradiction()` + `supersede()`
- Source: `graphiti_core/utils/maintenance/edge_operations.py`
- Research: Zep/Graphiti (arXiv:2501.13956, 2025) — 94.8% deep memory retrieval

**Negative-example prompting** — Extraction prompt includes explicit exclusion lists: pronouns, abstract concepts, bare relational terms, generic nouns. Reduces noise entities before they enter the graph.
- File: `src/extractor.rs`, `SYSTEM_PROMPT` constant
- Source: `graphiti_core/utils/maintenance/node_operations.py`

**Entity type filtering** — Post-extraction filter removes entities matching an exclusion list, plus any relationships that become dangling.
- File: `src/extractor.rs`, `filter_excluded()`
- Source: `graphiti_core/utils/maintenance/node_operations.py`, line 205

### From KGGen (Stanford)

**SEMHASH normalization** — Unicode-aware name normalization with English singularization. Catches "companies"→"company", "materials"→"material", "churches"→"church" without any LLM call.
- File: `src/resolver.rs`, `normalize_name()` + `singularize()`
- Source: `kg-gen/src/kg_gen/utils/deduplicate.py`, SEMHASH algorithm
- Research: Mo et al., KGGen, NeurIPS 2025 (arXiv:2502.09956)

### From nano-graphrag

**LLM response cache** — Content-addressable cache using SHA-256 of model+messages. Stored in Postgres. Checked before every LLM call, populated after. Makes re-processing free and saves 30-50% of API costs during development.
- File: `src/cache.rs`
- Source: `nano_graphrag/_llm.py`, `compute_args_hash()`
- Table: `llm_cache (hash, model, response, created_at)`

### From GraphRAG + nano-graphrag

**Community detection** — Label propagation algorithm on the active relationship graph. Groups densely connected entities into communities. Single-entity communities are filtered out. Communities are named after their most-connected entity.
- File: `src/communities.rs`, `detect_communities()`
- Source: GraphRAG uses Leiden; we use label propagation (simpler, sufficient at current scale, upgradeable to Leiden later)
- Research: Microsoft GraphRAG (arXiv:2404.16130, 2024) — 72-83% comprehensiveness improvement

### Novel (not from any reference system)

**Pipeline with transactions** — Document+chunk insertion, integrate per-chunk processing, and channel deletion are all transactional. Temporal supersession shares a transaction with the replacement relationship insert. No partial state on cancellation.
- File: `src/pipeline.rs`
- Motivation: Review finding — cancellation between supersession and replacement silently deletes knowledge

**Subgraph-aware search** — Search accepts `datasets` (channels) to filter results. Cognee's search_type names (CHUNKS, SIMILARITY, GRAPH_COMPLETION, SUMMARIES) are supported for backward compatibility.
- File: `src/pipeline.rs`, `search()`

**Quality gate** — Input validation (non-empty dataset/content, 512KB content limit, top_k clamped to 100). Integrate skips already-processed chunks. Embedding failures log warnings but don't abort.
- File: `src/pipeline.rs`
- Motivation: Cognee production data showed 62.5% noise from ingesting garbage

**Integrate tracks extraction state** — Chunks that have been extracted (have entries in entity_chunks table) are skipped on re-integrate. No redundant extraction on re-runs.
- File: `src/pipeline.rs`, `integrate()`

## Not Yet Implemented (Deferred)

| Feature | From | Why deferred |
|---|---|---|
| MinHash/LSH fuzzy entity resolution | Graphiti | Current scale doesn't need it — exact + alias + singularization is sufficient. Add when duplication becomes measurable. |
| Hierarchical community summarization | GraphRAG | Needs LLM calls per community. Add when communities are populated and retrieval quality needs improvement. |
| Community-based search (map-reduce) | GraphRAG | Depends on community summaries existing. |
| Constrained relation extraction | KGGen | Joint extraction with good prompting may be sufficient. Add if phantom entities appear in practice. |
| Rank fusion candidate selection | KGGen | SEMHASH + exact matching handles current scale. Add if dedup miss rate is measurable. |
| Dual-level retrieval (entity + keyword VDB) | LightRAG | Alternative to community search. Pick one; we chose communities. |
| Delimiter corruption recovery | LightRAG | Not needed — we use JSON structured output, not delimiters. |
| Bidirectional pipeline feedback | Novel design | Extraction-aware-of-graph, resolution-aware-of-communities. Prototype after forward pipeline is validated. |

## Architecture

```
Document → Chunker (markdown-aware, text-splitter)
    → Chunks + Embeddings (Ollama, pgvector)
    → LLM Extractor (Claude API, gleaning, negative examples, cache)
    → Entity Resolution (normalize, singularize, exact match, alias)
    → Contradiction Detection (identical/novel/contradictory)
    → Store entities, relationships, channels (Postgres, transactional)
    → Community Detection (label propagation, periodic rebuild)
```

## Test Coverage

39 unit tests covering:
- Types: serialization, confidence roundtrip, API response construction (7)
- Chunker: empty, short, long, max size (4)
- Embedder: construction (1)
- Extractor: mock extraction, LLM builder, JSON parsing, dedup, filtering (12)
- Resolver: normalization, singularization (11)
- Cache: key consistency, uniqueness, format (3)
- (Pipeline, graph, vectors, temporal, communities, server require live Postgres — tested via integration)
