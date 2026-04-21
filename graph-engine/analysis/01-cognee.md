# Cognee Analysis — Second Mind Integration

Analysis of Cognee 0.5.6-local as used in the second-mind project, based on integration code, configuration, patches, search logs (24 queries), and intake logs (33 entries across 3 datasets).

---

## 1. Extraction

### Method

Cognee's entity extraction runs during the `cognify` step. The intake engine sends a `POST /api/v1/cognify` with:

```json
{
  "datasets": ["research"],
  "custom_prompt": "<contents of prompts/research.md>",
  "chunksPerBatch": 5
}
```

The LLM used for extraction is configured as `LLM_MODEL=claude-sonnet-4-20250514` via LiteLLM. Cognee's internal Anthropic adapter (patched — see below) calls Claude with `instructor` for structured output, using Pydantic response models to extract typed entities and relationships.

The `custom_prompt` parameter is the key customization point. Three prompts are in use:
- **research.md** — "Extract key claims with confidence levels, core concepts, relationships (supports/contradicts/extends/requires)"
- **personal.md** — "Categorize by utility (PROJECT/QUESTION/RESOURCE/PARKED), extract next steps, connections"
- **investment.md** — "Identify demand drivers, resource dependencies, company exposure, Canadian-accessible vehicles, confidence levels"

The actual extraction prompt in the patched `anthropic_adapter.py` line 96-97 is:

```
Use the given format to extract information from the following input: {text_input}. {system_prompt}
```

Where `system_prompt` contains the custom prompt. This means the custom prompt is appended to a generic extraction instruction — it does NOT replace Cognee's internal extraction schema. The Pydantic `response_model` still controls what fields get extracted.

### What works well

- **Custom prompts actually influence extraction.** The search results show domain-specific entities: "copper demand", "TFSA/RRSP tax implications", "Teck Resources" — these come from the investment prompt directing extraction toward those concepts.
- **Three distinct lenses on the same infrastructure.** The channel pattern (dataset + prompt) is elegant and extensible. Adding a new lens is just adding a `.md` file.
- **Structured output via instructor.** Using Pydantic models ensures entity types are consistent and machine-parseable.

### Failure modes

- **The custom prompt is subordinate to Cognee's schema.** The extraction format is controlled by Cognee's internal `response_model` (Pydantic BaseModel), not the custom prompt. The prompt can influence *what* gets extracted but not *how* it's structured. If Cognee's schema doesn't have a field for "confidence level" or "Canadian accessibility", those get lost even though the prompt asks for them.
- **Required a patch to work at all.** The `anthropic_adapter.py` patch adds `max_tokens=min(self.max_completion_tokens, 4096)` — without this, the Anthropic API call fails because Cognee's upstream code doesn't pass the required `max_tokens` parameter. This is a Cognee bug.
- **`max_tokens` capped at 4096.** The patched adapter hard-caps at 4096 tokens, which limits extraction depth for large documents. For a 30K token document, 4096 tokens of extraction output may truncate entities.
- **Retry is coarse.** The adapter retries with `stop_after_delay(128)` and `wait_exponential_jitter(8, 128)`, but the intake engine adds its own 3-attempt retry on top (with 30s/60s backoff). This creates nested retry storms — up to 3 outer * N inner retries.

### Worth adopting

**Yes — the custom_prompt parameter pattern.** Being able to direct extraction per-ingestion without code changes is valuable. But the implementation needs control over the extraction schema, not just the prompt text. A custom graph engine should let the prompt define both what to look for AND the output schema.

---

## 2. Chunking

### Method

Cognee handles chunking internally before embedding. The only external control is `chunksPerBatch` (default 5, configurable via `COGNEE_CHUNKS_PER_BATCH`), which limits concurrent embedding requests to Ollama. This is a batching parameter, not a chunking parameter.

The intake engine enforces a pre-chunking size limit: documents exceeding `EMBEDDING_MAX_TOKENS` (default 32K, estimated as `chars / 4`) are rejected outright. This is a blunt instrument — reject or accept, no splitting.

Cognee's internal chunking strategy is opaque from the integration layer. Based on the search results returning chunk-level matches, Cognee does split documents into chunks, but:
- Chunk size is not configurable from the API
- Chunk overlap is not configurable
- Chunk boundaries are not visible in search results

### What works well

- **The 32K token pre-check catches documents that would blow out the embedding model.** The `qwen3-embedding:4b` has a 40K context window; rejecting at 32K leaves headroom.
- **`chunksPerBatch=5` prevents Ollama OOM on the 16GB VPS.** This was tuned through operational experience.

### Failure modes

- **No way to control chunk size.** If Cognee chunks a research paper into 500-token fragments, entity relationships that span paragraphs are lost. If it chunks into 5000-token fragments, embedding quality may degrade.
- **No document splitting.** Documents over 32K tokens are rejected, not split. The error message says "split the document before ingesting" — but there's no splitting tool in the intake engine.
- **Chunk boundaries may break entity co-occurrence.** If "Teck Resources" and "TSX-listed" appear in different chunks, the relationship between them depends entirely on entity resolution during cognify, not on chunking preserving the co-reference.

### Worth adopting

**No — the black-box chunking is a significant limitation.** A custom graph engine should expose chunk size, overlap, and boundary strategy as first-class configuration. Semantic chunking (splitting on topic shifts rather than token count) would be ideal for research documents.

---

## 3. Entity Resolution

### Method

Cognee uses LLM-powered entity extraction during cognify, where the instructor-patched Claude call extracts entities into typed Pydantic models. How Cognee deduplicates entities across chunks and documents is internal to Cognee and not observable from the integration layer.

### What works well

- **Entities do persist across documents.** Search results reference "Teck Resources" across multiple queries, suggesting the entity was extracted and stored as a single graph node from multiple ingested documents.
- **The knowledge graph (KuzuDB) provides a natural deduplication surface.** Graph databases inherently merge nodes by identity.

### Failure modes

- **No evidence of sophisticated entity resolution.** The search logs show repeated references to "systematic data access barriers" and "technical difficulties" as entities — these are noise from failed web scrapes that got extracted as if they were research findings. Cognee extracted them faithfully but without any quality filter.
- **Cross-document entity linking is unclear.** When "copper" appears in an investment document and a research document (different datasets), there's no evidence these link to the same entity. Dataset isolation may prevent cross-reference entirely (see section 9).
- **No entity merging visible.** "TFSA/RRSP tax implications" and "Canadian tax rates TFSA RRSP withholding tax" may or may not resolve to the same concept — the search results treat them as separate queries hitting similar but potentially separate entity neighborhoods.

### Worth adopting

**Partially — LLM-powered extraction is the right approach, but entity resolution needs explicit control.** A custom implementation should define canonical entity forms, merge rules, and confidence thresholds for entity matching.

---

## 4. Relationships

### Method

Cognee extracts relationships during cognify using the LLM. The custom prompts direct what kinds of relationships to look for:
- Research prompt: "supports, contradicts, extends, requires"
- Personal prompt: "connections to existing concepts"
- Investment prompt: "which resources serve multiple fields, which companies span multiple bottlenecks"

The extracted relationships are stored in KuzuDB as graph edges. The `GRAPH_COMPLETION` search type traverses these relationships to answer queries.

### What works well

- **GRAPH_COMPLETION produces synthesized answers that follow relationships.** The search logs show results like "AI infrastructure investment is driving increased copper demand through data centers and semiconductor manufacturing" — this synthesizes across multiple relationship hops (AI infrastructure -> data centers -> electrical infrastructure -> copper).
- **Custom prompts can direct relationship types.** The investment prompt explicitly asks for supply chain relationships, and the results reflect this.

### Failure modes

- **Relationship types are not standardized.** The prompts suggest relationship types in natural language ("supports, contradicts, extends"), but whether Cognee actually creates typed edges vs. generic "related_to" edges is opaque.
- **Noise relationships persist.** The search results contain relationships like "research tools -> technical difficulties -> data access barriers" — these are relationships between failure artifacts, not between real concepts. Once extracted, they pollute every subsequent graph traversal.
- **15 of 24 searches (62.5%) mention gaps/missing/unavailable/barriers.** This is the most damning metric. The graph has absorbed the explorer agent's failure reports as if they were research findings, and now those failure artifacts dominate retrieval. The graph has essentially memorized "I couldn't find this data" as the primary knowledge about most topics.

### Worth adopting

**Yes — graph-based relationship extraction is the core value proposition.** But relationship quality depends entirely on input quality. A custom implementation needs: (1) relationship type ontology, (2) quality scoring on edges, (3) ability to prune noise relationships.

---

## 5. Storage

### Method

Three storage layers:
- **KuzuDB** (embedded graph) — entities and relationships, stored in a Docker volume (`cognee-data`)
- **LanceDB** (embedded vectors) — embeddings for chunks, entities, and summaries
- **PostgreSQL 17 + pgvector** — Cognee's relational metadata, user auth, pipeline state

Dataset isolation is by Cognee's `datasetName` parameter. Each dataset gets a UUID (`dataset_id`). Search results include `dataset_id`, `dataset_name`, and `dataset_tenant_id` (always null in this deployment).

The intake engine adds a fourth layer outside Cognee:
- **Content-addressed library** — SHA-256 hashed files in `data/library/texts/`, `snapshots/`, `files/`, `audio/`
- **JSONL audit log** — every ingestion and its status

### What works well

- **Dataset namespace isolation works.** Searching `["investment-v1"]` returns only investment dataset results. Searching without a dataset filter returns results from `research` dataset. The search logs confirm this — the investment-v1 query (line 24) returned dataset_id `867b4f56` while research queries returned `f58bc12a`.
- **The intake engine's audit log is a genuine safeguard.** 33 entries logged with full provenance (11 pending -> 11 added -> 11 cognified, zero errors). The lifecycle tracking caught the `AddedNotCognified` failure mode during development.
- **Content-addressed library enables replay.** Sources are stored by SHA-256 hash, so replaying with a different prompt doesn't require re-fetching.
- **Resource limits are set.** Cognee gets 3GB RAM / 2 CPUs, Postgres gets 512MB / 1 CPU, Ollama gets 8GB / 2 CPUs. These prevent any single service from starving the VPS.

### Failure modes

- **Cognee's storage is a black box.** There's no way to inspect what's in KuzuDB or LanceDB without going through Cognee's search API. No graph export, no entity listing, no relationship browsing.
- **Volume-coupled state.** Wiping Cognee volumes (`just cognee-wipe`) destroys all graph and vector data. The intake log and library survive (they're bind-mounted from host), but the Cognee-internal state is gone. Recovery requires re-ingesting and re-cognifying everything.
- **No incremental backup.** The `cognee-data` Docker volume contains KuzuDB and LanceDB files, but there's no snapshot/restore mechanism short of `docker cp`.
- **Dataset deletion requires UUID lookup.** Cognee's API uses UUIDs internally but human-readable names externally. The adapter has to call `GET /api/v1/datasets`, iterate, and match by name before deleting.

### Worth adopting

**Partially — the KuzuDB + LanceDB combination is sound, but direct access (not through Cognee's API) would be better.** The intake engine's audit log and content-addressed library are already custom and should be kept. The graph and vector storage should be directly owned, not accessed through an intermediary.

---

## 6. Retrieval

### Method

Four search types, all via `POST /api/v1/search`:

| Type | Mechanism | Used in logs |
|---|---|---|
| `GRAPH_COMPLETION` | Graph traversal + LLM completion | 24/24 queries |
| `CHUNKS` | Vector similarity on raw chunks | 0 (MCP default, but not used in logged sessions) |
| `SIMILARITY` | Vector similarity on entities | 0 |
| `SUMMARIES` | Vector similarity on summaries | 0 |

All 24 logged searches used `GRAPH_COMPLETION`. No search type diversity in production use.

### What works well

- **GRAPH_COMPLETION produces readable, synthesized answers.** Results are narrative paragraphs, not raw chunks. Example: "AI infrastructure investment is driving increased copper demand through data centers and semiconductor manufacturing. The buildout of AI infrastructure requires extensive electrical infrastructure that is copper-intensive."
- **Results include dataset attribution.** Each result is tagged with `dataset_id` and `dataset_name`, making it clear where knowledge came from.
- **Non-empty result rate is 87.5% (21/24).** Most queries return something.

### Failure modes

- **GRAPH_COMPLETION is the only type used — no comparative data.** The architecture doc describes four types with different strengths, but in practice only one is exercised. There's no evidence of whether CHUNKS or SIMILARITY would return better results for specific queries.
- **3 queries returned empty results `[]` with no fallback.** These queries ("AI infrastructure investment copper demand data centers Canada Teck Resources TSX ETFs TFSA RRSP", "Canadian tax rates TFSA RRSP withholding tax", "AI copper demand growth forecasts tonnage timeframes") are long, specific queries that may have failed on graph traversal. CHUNKS search might have found relevant content.
- **62.5% of non-empty results are dominated by gap/failure artifacts.** The graph faithfully stores and retrieves "systematic data access barriers", "technical difficulties", "research tool failures" as its primary knowledge. When asked about copper prices, it responds with "copper spot prices are currently inaccessible due to technical barriers." This is technically correct retrieval of bad data, not a retrieval failure.
- **No result scoring or confidence.** Search results don't include relevance scores, so the caller can't distinguish high-confidence from low-confidence matches.
- **The search log only captures 500-char previews.** Full result analysis would require re-running queries.

### Worth adopting

**Yes — multi-type search is the right architecture.** But needs: (1) automatic fallback when one type returns empty, (2) result quality scoring, (3) deduplication across search types when combining results. The GRAPH_COMPLETION type specifically is valuable because it synthesizes across relationships rather than returning raw chunks.

---

## 7. Community Detection

### Method

No evidence of community detection in the integration layer. The Cognee API does not expose community-related endpoints, and the search types don't include community-based retrieval. The `SUMMARIES` search type may use document-level or community-level summaries internally, but this is opaque.

### What works well

- Nothing observable. If Cognee does community detection internally, it's not surfaced through the API.

### Failure modes

- **No way to discover topic clusters.** A query like "what topics do I know about?" has no direct mechanism. The user would need to search broadly and infer clusters from results.
- **No hierarchical summarization.** Without communities, there's no way to get "level 2" summaries (summaries of summaries) for broad overview queries.

### Worth adopting

**The concept is worth adopting (GraphRAG-style community detection), but Cognee doesn't provide it through its API.** A custom implementation could use the Leiden algorithm on the KuzuDB graph to detect communities and generate hierarchical summaries.

---

## 8. Temporal Handling

### Method

Cognee has no visible temporal handling. Documents are ingested with timestamps in the intake engine's audit log, but Cognee's graph doesn't appear to track when entities were created, when relationships were established, or when facts expire.

The investment prompt explicitly asks to "note dates — price and supply data goes stale fast", but there's no mechanism to enforce temporal decay or flag stale data in retrieval.

### What works well

- **The intake engine logs timestamps.** Every ingestion has an ISO 8601 timestamp, so the provenance of when data entered the system is tracked.
- **The investment prompt acknowledges temporal sensitivity.** At least the prompt design recognizes the problem.

### Failure modes

- **No temporal decay.** Copper prices from April 2026 will still be returned as current data in July 2026. There's no staleness flag, no expiry, no temporal weighting in retrieval.
- **No temporal ordering in results.** When multiple documents mention copper prices, the most recent isn't prioritized.
- **Search results show stale data being treated as current.** "Copper futures trading around $5.88/lb" — when this data was captured isn't visible in search results.

### Worth adopting

**No — Cognee provides nothing here.** A custom implementation should add temporal metadata to entities (created_at, valid_from, valid_until, confidence_decay_rate) and temporal weighting in retrieval.

---

## 9. Cross-Reference (Cross-Dataset)

### Method

Cognee's search API accepts an optional `datasets` parameter. When omitted, it appears to search across all datasets. When specified, it filters to those datasets. The search logs show:
- 23 queries with `datasets: null` (search all) — all returned results from `research` dataset only
- 1 query with `datasets: ["investment-v1"]` — returned results from `investment-v1` only

### What works well

- **Cross-dataset search is syntactically possible.** The API supports it.

### Failure modes

- **No evidence of actual cross-dataset entity linking.** When searching without a dataset filter, results come from `research` only — never from both `research` and `investment-v1`. This suggests either: (a) Cognee searches datasets independently and returns the first match, or (b) the graph doesn't link entities across dataset boundaries.
- **`dataset_tenant_id` is always null.** This field exists in the response but is never populated, suggesting multi-tenancy features aren't configured.
- **Same concepts in different datasets are likely separate nodes.** "Copper demand" ingested into `research` and `investment-v1` probably creates two separate entity nodes with no relationship between them. The graph can't tell you "here's what both your research lens and investment lens say about copper."

### Worth adopting

**No — cross-dataset linking is architecturally absent.** A custom implementation should either: (1) use a single graph with dataset labels on edges/nodes, not separate subgraphs, or (2) explicitly create cross-reference edges during extraction when entities match across datasets.

---

## 10. Standout Techniques

### What Cognee Does Well

1. **The `custom_prompt` parameter on cognify is genuinely useful.** Being able to change extraction behavior per-call without code changes, while sharing all infrastructure, is the right abstraction. This is the single most valuable pattern to preserve.

2. **GRAPH_COMPLETION search type produces synthesized answers.** Rather than returning raw chunks and making the caller assemble meaning, it traverses relationships and generates a narrative answer. This is meaningfully better than pure vector similarity for relational queries.

3. **Instructor-based structured extraction.** Using Pydantic models to enforce output structure from the LLM is more reliable than regex parsing or prompt-only approaches. The extraction output is typed and validated.

4. **Embedded graph + vector (KuzuDB + LanceDB).** No external services for graph or vector storage. Everything runs in-process within the Cognee container. This simplifies operations and reduces failure surface.

5. **Minimal API surface.** Six endpoints handle everything: login, add, cognify, search, list datasets, delete dataset. The simplicity made the intake engine adapter straightforward to implement (460 lines including all retry logic).

### What Cognee Does Poorly

1. **Garbage in, garbage faithfully stored and retrieved.** The biggest operational failure: 62.5% of search results are dominated by "data access barriers" and "technical difficulties" that were extracted as entities from failed research runs. Cognee has no quality filter, no noise detection, and no way to prune bad entities after ingestion.

2. **Black-box internals.** No way to inspect the graph, list entities, browse relationships, view chunk boundaries, or understand why a search returned what it did. Debugging requires guessing.

3. **No temporal awareness.** Facts don't have timestamps, don't decay, and can't be superseded. This is fatal for investment data.

4. **Chunking is invisible and unconfigurable.** You can't set chunk size, overlap, or boundary strategy. For a knowledge system processing diverse document types (research papers, financial data, personal notes), one-size-fits-all chunking is inadequate.

5. **The Anthropic adapter required patching.** A required API parameter (`max_tokens`) was missing from Cognee's upstream code. The patch is mounted as a read-only volume override. This works but means Cognee upgrades require re-validating the patch.

6. **Cognify is slow and fragile.** 4+ minutes for small documents on this VPS. The intake engine needs 3-attempt retry with 30s/60s backoff delays. This is the most timeout-prone operation in the system.

---

## Summary Scorecard

| Capability | Quality | Notes |
|---|---|---|
| Extraction | Good | Custom prompts work, but schema is fixed |
| Chunking | Poor | Invisible, unconfigurable |
| Entity Resolution | Unknown | Not observable through API |
| Relationships | Good | Graph traversal produces synthesized answers |
| Storage | Adequate | Works but opaque, no direct access |
| Retrieval | Mixed | GRAPH_COMPLETION is good; no fallback, no scoring |
| Community Detection | Absent | Not exposed through API |
| Temporal Handling | Absent | No timestamps, decay, or ordering |
| Cross-Reference | Poor | Datasets appear to be isolated subgraphs |
| Noise Handling | Absent | Faithfully stores and retrieves garbage |

### Key Metrics from Production Data

- **33 intake entries**: 11 pending, 11 added, 11 cognified (0 errors — happy path works reliably)
- **3 datasets**: research (12 entries), investment-v1 (15 entries), investment (6 entries)
- **24 searches**: 100% GRAPH_COMPLETION, 87.5% non-empty, 62.5% contaminated by noise entities
- **1 patch required**: anthropic_adapter.py (max_tokens bug)
- **Cognify timeout budget**: 600 seconds with 3 retries and 30s/60s backoff delays

### Bottom Line

Cognee provides a functional knowledge graph pipeline with a good API abstraction (custom_prompt, multi-type search) but is a black box that offers no control over the critical details: chunking, entity resolution, relationship typing, temporal handling, or noise filtering. The biggest production issue is not a Cognee bug but an architectural gap — there's no quality gate between extraction and storage, so noise gets permanently embedded in the graph.
