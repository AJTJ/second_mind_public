# Graph Landscape Update — 2026-04-18

## Updates to Tracked Systems

### GraphRAG (microsoft/graphrag)
- Commits since last pull: 0
- Last commit: 0da2a4dd (Release v3.0.9)
- No changes.

### Graphiti (getzep/graphiti)
- Commits since last pull: 0
- Last commit: 98d8344
- No code changes. New branch `preston/forward-port-from-zep-proprietary` appeared — may indicate upcoming features from Zep's proprietary codebase.

### KGGen (stair-lab/kg-gen)
- Commits since last pull: 0
- Last commit: 6259b4c
- No changes.

### LightRAG (HKUDS/LightRAG)
- Commits since last pull: 16
- Previous: 94c5d95f → Current: a5a3c5a2
- Changes: dependency bumps (vite, axios, react), Docker image signing with cosign, base64 embedding support for providers like Yandex Cloud, code formatting cleanup
- Relevant to us: No. The base64 embedding feature (commit 60922b3f) is for providers that don't support binary encoding. We use Ollama directly.

### nano-graphrag (gusye1234/nano-graphrag)
- Commits since last pull: 0
- Last commit: acb35c0
- No changes. Project appears dormant (last real feature commit was January 2026).

## Feature Comparison: Us vs Them

### What we have that they don't

| Feature | Us | GraphRAG | Graphiti | LightRAG | KGGen | nano-graphrag |
|---|---|---|---|---|---|---|
| Rust implementation | yes | — | — | — | — | — |
| Subgraph-aware search (channel filtering with extension) | yes | — | — | — | — | — |
| Multi-perspective extraction (same doc, different channels) | yes | — | — | — | — | — |
| Quality gate (input validation, skip garbage) | yes | — | — | — | — | — |
| Transactional pipeline (no partial state) | yes | — | — | — | — | — |
| Cognify tracks extraction state (skip processed chunks) | yes | — | — | — | — | — |
| Postgres-only storage (graph + vectors in one DB) | yes | — | — | partial | — | — |

### What we adopted from them

| Feature | Source | Status |
|---|---|---|
| Gleaning (multi-pass extraction) | GraphRAG | implemented |
| Negative-example prompting | Graphiti | implemented |
| Entity type filtering | Graphiti | implemented |
| Contradiction detection (identical/novel/contradictory) | Graphiti | implemented |
| SEMHASH normalization + singularization | KGGen | implemented |
| LLM response cache (SHA-256 keyed) | nano-graphrag | implemented |
| Community detection (label propagation) | GraphRAG + nano-graphrag | implemented |
| Embedder as trait (mockable) | novel | implemented |
| Extractor as trait (mockable) | novel | implemented |

### What they have that we don't (yet)

| Feature | System | Priority | Notes |
|---|---|---|---|
| Leiden algorithm (vs our label propagation) | GraphRAG | low | label propagation sufficient at current scale |
| Hierarchical community summaries | GraphRAG | medium | needs LLM calls, deferred until graph has data |
| Map-reduce global search over community reports | GraphRAG | medium | depends on community summaries |
| Three-tier entity resolution (exact → MinHash/LSH → LLM) | Graphiti | medium | we have exact + alias + singularization, no fuzzy tier |
| Bitemporal edge model with LLM contradiction verification | Graphiti | low | we detect contradictions but don't use LLM to verify |
| Composable search with pluggable rerankers | Graphiti | medium | we have fixed search logic |
| Density-aware chunking (skip chunking for short docs) | Graphiti | low | easy add |
| Dual-level retrieval (entity VDB + relationship keyword VDB) | LightRAG | low | chose communities over this approach |
| Dynamic token budget for context assembly | LightRAG | medium | prevents context overflow at scale |
| Rank fusion candidate selection (BM25 + embedding) | KGGen | low | for entity resolution, not needed at current scale |
| Constrained relation extraction (Literal types) | KGGen | medium | prevents phantom entities |
| Edge deduplication (predicate normalization) | KGGen | low | we normalize entity names but not predicates |
| DRIFT iterative search | GraphRAG | low | expensive, complex, marginal benefit |
| Content-hash dedup at document/chunk level | nano-graphrag | low | we use SHA-256 for documents but not chunks |

### What validates our design

Things tracked systems have adopted or emphasize that we already do:

1. **Transactional writes** — Graphiti added saga-based transactions for multi-step operations. We had this from the start.
2. **Embedder/extractor as swappable traits** — GraphRAG and LightRAG both have pluggable model backends. Our trait-based design matches.
3. **Channel/namespace concept** — Cognee's dataset isolation is the closest, but our entity-level channel tagging is more flexible.
4. **Extraction state tracking** — No other system tracks which chunks have been extracted. All reprocess everything on re-cognify.
5. **Quality gates** — No other system validates input before extraction. Cognee's 62.5% noise contamination validates our decision.

## Registry Changes

Updated: LightRAG pulled to a5a3c5a2 (16 new commits, no relevant changes)
No new systems discovered (no --discover flag this run).
