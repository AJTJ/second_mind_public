# nano-graphrag Analysis

Codebase: ~1,100 lines of Python. A minimal reimplementation of Microsoft's GraphRAG paper, preserving the core pipeline (chunk -> extract -> cluster -> report -> query) while stripping away the framework overhead.

Source: `~/references/graph-systems/nano-graphrag/`

---

## 1. Entity and Relationship Extraction

**Method:** Two extraction paths exist.

**Path A (default, prompt-based):** Each chunk is sent to `best_model_func` (GPT-4o by default) with the `entity_extraction` prompt from Microsoft's GraphRAG. The prompt specifies entity types (default: `organization, person, geo, event`) and asks the LLM to output tuples in a custom delimited format:

```
("entity"<|>ENTITY_NAME<|>ENTITY_TYPE<|>description)
("relationship"<|>SOURCE<|>TARGET<|>description<|>strength_float)
```

Records are separated by `##` and terminated by `<|COMPLETE|>`. After initial extraction, a "gleaning" loop runs up to `entity_extract_max_gleaning` times (default: 1), prompting "MANY entities were missed in the last extraction. Add them below..." and asking the LLM "are there still entities that need to be added?" (YES/NO) to decide whether to continue.

**Path B (DSPy-based, `entity_extraction/`):** Uses `dspy.ChainOfThought` with typed Pydantic models (`Entity`, `Relationship`) and a much larger entity type list (50+ types from R2R). Includes an optional self-refine loop: extract -> critique -> refine. The critique step evaluates completeness and accuracy; the refine step addresses the critique. This path also supports relationship ordering (1st, 2nd, 3rd order).

**What works well:**
- Gleaning catches entities the LLM missed on first pass. The YES/NO gate prevents wasted API calls when extraction is already complete.
- All chunks are processed concurrently via `asyncio.gather`, bounded by `limit_async_func_call` semaphore. This maximizes throughput.
- LLM response caching (`hashing_kv`) means re-running on the same data skips API calls entirely. Cache key is `md5(model + messages)`.
- Entity names are uppercased and cleaned (`clean_str` strips HTML entities and control chars), providing basic normalization.
- The DSPy path's self-refine loop (critique -> refine) is a genuinely useful pattern for improving extraction recall.

**Failure modes:**
- The default entity types (`organization, person, geo, event`) are extremely narrow. Technical concepts, tools, methods, research topics -- all missed unless you override `PROMPTS["DEFAULT_ENTITY_TYPES"]`. The DSPy path has 50+ types but they're not used by the default extraction function.
- The custom delimited format is fragile. Parsing relies on regex `\((.*)\)` to find records, then splits by `<|>`. If the LLM includes parentheses in descriptions or misformats a delimiter, records silently drop. There is no validation or retry on parse failure.
- Gleaning appends raw text (`final_result += glean_result`) without deduplication. If the LLM repeats entities from the first pass, they appear as separate records. The merge step handles this later, but it wastes tokens and increases LLM cost.
- No schema validation on extracted data. If `relationship_strength` isn't a parseable float, it silently defaults to 1.0. If entity_type is missing, the record is dropped.
- Entity types are not enforced -- the LLM can hallucinate types not in the provided list.

**Worth adopting:** Yes -- the gleaning pattern (multi-pass extraction with early termination). Also the LLM response cache keyed by message hash. Both are simple and effective.

---

## 2. Chunking

**Method:** Two chunking strategies:

**`chunking_by_token_size` (default):** Simple sliding window. Tokenizes each document, then slices into windows of `max_token_size` (default 1200) tokens with `overlap_token_size` (default 100) token overlap. Decodes token slices back to text via `tokenizer_wrapper.decode_batch`.

**`chunking_by_seperators`:** Token-level separator-aware splitting. Encodes a list of separator strings (paragraph breaks, sentence endings, CJK punctuation, whitespace) into tokens, then uses `SeparatorSplitter` to split at separator boundaries. Merges small splits up to `chunk_size`, then enforces overlap by prepending the last `chunk_overlap` tokens from the previous chunk.

Both strategies operate at the token level (encode first, split tokens, decode back), which avoids the character-vs-token mismatch problem.

**What works well:**
- Token-level chunking ensures chunks respect the token budget precisely. No estimating chars/token.
- The separator list is comprehensive: includes CJK punctuation, full-width characters, zero-width spaces. Good for multilingual content.
- Chunk deduplication by content hash (`compute_mdhash_id(chunk["content"], prefix="chunk-")`) prevents re-processing identical chunks across documents.
- Document-level deduplication too: `filter_keys` checks if the full document hash already exists before chunking.

**Failure modes:**
- Default 1200-token chunks are large for entity extraction. Microsoft's GraphRAG paper uses 300-600 tokens. Larger chunks mean more entities per chunk, which increases extraction difficulty and the chance of missed entities.
- No semantic awareness. Chunks can split mid-sentence (token-level slicing doesn't know about sentence boundaries unless using the separator variant).
- The overlap mechanism in `chunking_by_token_size` is simple sliding window -- it doesn't try to keep semantic units intact at boundaries.
- `chunking_by_seperators` has a subtle bug potential: if no separator matches within `chunk_size` tokens, `_split_chunk` falls back to hard slicing at `chunk_size - chunk_overlap` intervals, which can split words.

**Worth adopting:** The token-level approach (encode then slice) is worth adopting over character-based chunking. The separator-aware variant is better for structured text. The chunk/doc deduplication via content hashing is a clean pattern.

---

## 3. Entity Resolution

**Method:** Entity resolution happens in `_merge_nodes_then_upsert`. When multiple chunks extract the same entity name (after uppercasing), their data is merged:

1. **Entity type:** Majority vote via `Counter`. If chunk A says "OPENAI" is an ORGANIZATION and chunk B says COMPANY, the most frequent type wins.
2. **Description:** All unique descriptions are joined with `<SEP>` separator, then summarized by the LLM if the concatenated description exceeds `entity_summary_to_max_tokens` (default 500 tokens). The summary prompt asks to "concatenate all of these into a single, comprehensive description" and "resolve contradictions."
3. **Source IDs:** Union of all source chunk IDs, joined with `<SEP>`.

For edges, `_merge_edges_then_upsert` follows the same pattern: descriptions are concatenated and summarized, weights are **summed** (more mentions = higher weight), and source IDs are unioned.

**What works well:**
- LLM-powered description merging handles semantic deduplication that string matching cannot. Two descriptions saying the same thing differently get merged into one coherent summary.
- Weight accumulation for edges is a good signal: frequently co-mentioned relationships score higher.
- The merge function checks for existing nodes/edges in the graph before merging, supporting incremental insertion.

**Failure modes:**
- **Only exact name matches are resolved.** "OpenAI" and "OPENAI INC" and "Open AI" are three different entities. There is zero fuzzy matching, embedding-based similarity, or alias resolution. This is the single biggest weakness for knowledge graph quality.
- Entity type majority vote can produce wrong results with small samples. Two chunks saying PERSON and one saying ORGANIZATION yields PERSON even if ORGANIZATION is correct.
- Description summarization uses `cheap_model_func` (GPT-4o-mini by default), which may lose important details from the concatenated descriptions.
- When edges reference entity names that don't exist as nodes, stub nodes are created with type `"UNKNOWN"` and minimal data. These phantom nodes pollute the graph.
- No coreference resolution. "the company," "it," "the firm" referring to the same entity are never linked.

**Worth adopting:** The LLM-powered description merge/summarization when descriptions grow long. The weight accumulation pattern for edges. But entity resolution without fuzzy name matching is fundamentally broken for real-world use.

---

## 4. Relationships

**Method:** Relationships are **generic, untyped.** Every edge is the same kind -- there's no relationship type field. The edge data consists of:
- `description`: Free-text description of the relationship
- `weight`: Numeric strength (from LLM extraction, accumulated across mentions)
- `source_id`: Which chunks this relationship was extracted from
- `order`: Relationship order (1st/2nd/3rd degree, only from DSPy path)

In the graph, edges are stored as undirected. When merging, `maybe_edges[tuple(sorted(k))]` ensures (A,B) and (B,A) are treated as the same edge. Neo4j storage uses a single `RELATED` relationship type for everything.

**What works well:**
- Undirected edge deduplication via sorted tuple keys prevents duplicate edges.
- The weight accumulation across mentions is a useful signal for edge importance.
- Edge descriptions are rich free-text, summarized by LLM when they grow long.
- The DSPy path's relationship ordering (1st/2nd/3rd degree) captures indirect relationships.

**Failure modes:**
- No relationship typing. "PERSON works_at ORGANIZATION" and "PERSON invested_in ORGANIZATION" are indistinguishable except by reading the description. This prevents type-based graph queries.
- Undirected edges lose directionality. "A funds B" and "B funds A" are merged into one edge, destroying causal/directional semantics.
- The `weight` from LLM extraction (a numeric "relationship strength") is subjective and inconsistent across chunks. Summing these arbitrary numbers doesn't produce meaningful absolute values.
- No relationship deduplication beyond exact entity name pairs. Two relationships between the same entities with different semantics (e.g., "competes with" and "partners with") get merged into a single edge with concatenated descriptions.

**Worth adopting:** No. The lack of relationship typing and forced undirectedness are significant limitations. The description-based approach works for summarization but fails for structured graph queries.

---

## 5. Storage

**Method:** Three storage layers with pluggable backends:

**Graph storage:**
- `NetworkXStorage` (default): In-memory `nx.Graph`, persisted as GraphML XML. Loaded on init, written on `index_done_callback`.
- `Neo4jStorage`: Full Neo4j driver with GDS (Graph Data Science) library for Leiden clustering. Uses `MERGE` for upserts, namespace-based node labels for isolation.

**Key-value storage:**
- `JsonKVStorage`: In-memory dict, persisted as JSON files. Separate files per namespace (full_docs, text_chunks, llm_response_cache, community_reports).

**Vector storage:**
- `NanoVectorDBStorage` (default): Wrapper around `nano_vectordb` library, persisted as JSON.
- `HNSWVectorStorage`: `hnswlib` with pickle-serialized metadata. Supports configurable `ef_construction`, `M`, `max_elements`.

**What works well:**
- Clean abstraction boundaries. `BaseGraphStorage`, `BaseKVStorage`, `BaseVectorStorage` define async interfaces. Swapping backends is straightforward.
- NetworkX persistence as GraphML is human-readable and inspectable.
- The `filter_keys` pattern on KV storage enables efficient deduplication before processing.
- LLM response caching via a dedicated KV store is elegant -- same interface, automatic persistence.
- HNSWVectorStorage uses `xxhash` for fast ID hashing and supports configurable HNSW parameters.

**Failure modes:**
- `JsonKVStorage` loads everything into memory. For large knowledge bases, this becomes a memory problem -- every chunk, every community report, every cached LLM response is in a Python dict.
- NetworkX GraphML serialization is O(n) on every `index_done_callback`, which fires after every insert/query cycle. With large graphs, this is slow.
- No concurrent write safety. Multiple processes writing to the same JSON/GraphML files will corrupt data.
- NanoVectorDB stores vectors as JSON, which is extremely space-inefficient for float arrays.
- No backup/recovery mechanism. A crash mid-write can corrupt the storage files.
- Community reports are **dropped entirely** on each new insertion (`await self.community_reports.drop()`), because incremental community updates aren't supported. This means every insert requires full recomputation.

**Worth adopting:** The abstraction layer design (base classes with pluggable backends) is clean and worth emulating. The LLM response cache pattern is useful. The actual storage implementations are too simple for production use.

---

## 6. Retrieval (Query Modes)

**Method:** Three query modes: local, global, and naive.

### Local Query
1. Embed the query, search the entity vector DB for top-k similar entities.
2. For each matched entity, fetch from the graph: the entity node data, its degree, its edges, and one-hop neighbor data.
3. Find related community reports by looking at the `clusters` field on matched entities, filtered by `query_param.level`.
4. Find related text chunks by tracing `source_id` from entities and their neighbors.
5. Assemble context as CSV tables: Reports, Entities, Relationships, Sources.
6. Send to LLM with `local_rag_response` prompt.

Context budget is split: 33% text units (4000 tokens), 40% local context/relationships (4800 tokens), 27% community reports (3200 tokens). Each section is independently truncated by token count.

### Global Query
1. Get all community schemas, filter by `level <= query_param.level` and `rating >= global_min_community_rating`.
2. Sort by occurrence (how many chunks reference the community), take top `global_max_consider_community` (default 512).
3. **Map phase:** Group communities into token-budget-sized batches. For each group, ask LLM to generate key points with importance scores (JSON output).
4. **Reduce phase:** Collect all points, filter by score > 0, sort by score, truncate by token budget. Send to LLM with `global_reduce_rag_response` prompt for final synthesis.

### Naive Query
Classic vector RAG: embed query, search chunk vector DB, retrieve top-k chunks, truncate by token budget, send to LLM.

**What works well:**
- The local query's multi-signal context (entities + relationships + communities + source text) provides rich, grounded answers. The graph structure adds context that pure vector search misses.
- The global map-reduce pattern handles arbitrarily large community sets by batching. It's the same approach as the original GraphRAG paper.
- Token budget management is explicit and tunable per context section.
- Community reports include the severity rating, which allows filtering low-quality communities.
- Entity matching in local queries prioritizes by vector similarity, then enriches from the graph structure. This is a good two-stage retrieval pattern.
- The `only_need_context` flag lets you inspect the assembled context without the final LLM call -- useful for debugging.

**Failure modes:**
- Local query starts from entity vector search. If the query doesn't match any entity names/descriptions, it returns the fail response even if relevant relationships exist. Relationship search is only reached after entity matching.
- No hybrid search. The entity VDB stores `entity_name + description` as content, so retrieval depends on embedding similarity. Short entity names like "AI" have weak embeddings.
- Global query is expensive: one LLM call per community batch in the map phase, plus the reduce call. For large graphs with many communities, this can mean dozens of API calls per query.
- The `level` parameter for community filtering is user-specified and hard to tune without understanding the graph structure. Wrong level = irrelevant or too-generic communities.
- Text unit retrieval in local mode counts "relation_counts" (how many one-hop neighbors share the same source chunk) but this heuristic can promote irrelevant chunks that happen to be well-connected.
- No query rewriting or decomposition. Complex multi-hop questions are sent as-is.

**Worth adopting:** Yes -- the local query's multi-signal context assembly (entity + relationship + community + source) is the core value of GraphRAG over plain vector search. The map-reduce global query is proven for corpus-level questions. The explicit token budgeting per context section is practical.

---

## 7. Community Detection

**Method:** Leiden algorithm via `graspologic.partition.hierarchical_leiden`. The process:

1. Extract the largest connected component from the graph (`stable_largest_connected_component`).
2. Run hierarchical Leiden with `max_cluster_size` (default 10) and a fixed random seed.
3. Each node gets a `clusters` field: a JSON array of `{level, cluster}` assignments (one per hierarchy level).
4. Community schema is computed by iterating all nodes, grouping by cluster assignment, and computing: member nodes, member edges, source chunk IDs, occurrence score, and sub-community relationships (node subset containment).
5. For each community (bottom-up by level), an LLM generates a structured report: title, summary, impact severity rating (0-10), rating explanation, and 5-10 detailed findings.

For Neo4j, Leiden runs via the GDS library (`gds.leiden.write`), writing community IDs directly to node properties.

**What works well:**
- Hierarchical Leiden produces multi-level community structure, enabling queries at different granularity levels.
- Reports are generated bottom-up: leaf communities first, then parent communities can reference sub-community reports for summarization. This produces hierarchically coherent summaries.
- The `stable_largest_connected_component` + `_stabilize_graph` functions ensure deterministic community assignments across runs (same data = same communities).
- Community "occurrence" score (fraction of max chunk coverage) provides a useful relevance signal.
- The community report format (JSON with structured findings) is well-designed for downstream use.

**Failure modes:**
- **No incremental update.** Every insertion drops all community reports and recomputes from scratch. For large graphs, this makes incremental ingestion prohibitively expensive.
- The largest connected component filter silently drops disconnected subgraphs. Isolated entities or small clusters are never assigned to communities and never get reports.
- `max_graph_cluster_size=10` is very small. Real-world knowledge graphs produce communities much larger than 10 nodes. This setting creates many tiny communities, increasing the number of LLM calls for report generation.
- Community report generation is serial by level (all level-N communities in parallel, then level N-1, etc.). This creates a dependency chain that can't be fully parallelized.
- The structured report prompt asks for "legal compliance" and "reputation" which are specific to the original GraphRAG threat-analysis use case. These fields are meaningless for general knowledge graphs.

**Worth adopting:** Yes -- hierarchical Leiden is the right algorithm for this. The bottom-up report generation with sub-community references is a good pattern. The community schema structure (nodes, edges, chunk_ids, sub_communities, occurrence) is well-designed.

---

## 8. Temporal Awareness

**Method:** None. There is no temporal modeling anywhere in the codebase. No timestamps on entities, relationships, or documents. No temporal decay in retrieval scoring. No versioning of entity descriptions.

The `claim_extraction` prompt includes a `claim_date` field (ISO-8601 start/end), but this prompt is defined in `prompt.py` and **never called** -- it's dead code from the original GraphRAG prompt set.

**Failure modes:**
- An entity's description is the latest LLM summary of all mentions. If information changes over time, old and new facts are merged into one "comprehensive" description with no indication of which is current.
- Relationship weights accumulate monotonically. A relationship that was important 2 years ago but irrelevant now still carries its accumulated weight.
- No way to query "what was true at time T" or "what changed between T1 and T2."

**Worth adopting:** No. Nothing to adopt. Temporal awareness must be added externally.

---

## 9. Cross-Document Connections

**Method:** Cross-document connections form through entity name collisions. When Document A mentions "OPENAI" and Document B mentions "OPENAI," both extraction results merge into the same entity node via `_merge_nodes_then_upsert` (exact uppercase string match). Their descriptions get concatenated and summarized, their source chunk IDs get unioned.

Similarly, if Document A mentions a relationship between OPENAI and MICROSOFT, and Document B mentions the same pair, the edge descriptions and weights merge.

Community detection then groups connected entities across documents into communities, and community reports synthesize cross-document knowledge.

**What works well:**
- The merge-on-name-match mechanism is simple and requires zero configuration. It works well for unambiguous proper nouns.
- The `source_id` field tracks provenance: every entity and relationship records which chunks contributed to it. This enables tracing back to source documents.
- Community reports are the real cross-document synthesis layer. They aggregate information from multiple entities (and therefore multiple documents) into coherent narratives.
- The entity VDB stores merged entity descriptions, so a query about "OPENAI" returns information from all documents that mentioned it.

**Failure modes:**
- **Name collision is the only bridge.** If Document A calls it "artificial intelligence" and Document B calls it "AI," no connection forms. There's no embedding-based entity linking, no alias resolution, no coreference.
- **False merges from name ambiguity.** "Apple" the company and "Apple" the fruit become one entity. No disambiguation mechanism exists.
- Cross-document relationship discovery is limited to shared entity names. If two documents discuss related but differently-named concepts, the system cannot infer a connection.
- The community detection only operates on the largest connected component. Documents whose entities don't connect to the main graph are isolated and excluded from cross-document synthesis.

**Worth adopting:** The provenance tracking via `source_id` with `<SEP>` separator is worth adopting. The entity-as-bridge pattern works but needs fuzzy matching to be useful in practice.

---

## 10. Standout Techniques of the Minimal Approach

### What's genuinely good:

**1. LLM response cache with content-addressable hashing.** `compute_args_hash(model, messages)` -> MD5 -> KV lookup. Same input = cached response. This is embedded directly in the LLM call functions, making it transparent. Every rerun of the pipeline is essentially free after the first run.

**2. Async-first with explicit concurrency limits.** `limit_async_func_call(max_size)` implements a manual semaphore (avoiding nest-asyncio issues). All chunk processing, entity merging, and community report generation use `asyncio.gather` with bounded concurrency. The architecture is naturally parallelizable.

**3. Pluggable everything via dataclass defaults.** The `GraphRAG` class is a single dataclass where every function (LLM, embedding, chunking, extraction) and every storage backend is a replaceable field. You can swap from OpenAI to Bedrock by changing two fields. This is better than most "configurable" frameworks that require subclassing.

**4. Token budget management.** `truncate_list_by_token_size` is used everywhere: context assembly, community packing, query result preparation. It's a simple utility that prevents context window overflow by counting actual tokens, not estimating.

**5. Content-hash deduplication at every level.** Documents get `md5(content)` IDs, chunks get `md5(content)` IDs, entities get `md5(name)` IDs. `filter_keys` checks existence before processing. Re-inserting the same document is a no-op.

**6. The community report structure.** JSON with title, summary, severity rating, rating explanation, and structured findings. This is more useful than a flat text summary because it enables filtering (by rating), sorting (by severity), and hierarchical aggregation.

**7. Two extraction paths.** The prompt-based path is fast and cheap. The DSPy path with typed Pydantic models, self-refinement, and critique is higher quality but more expensive. Having both as pluggable alternatives is practical.

### Where minimalism hurts:

**1. No entity resolution beyond exact string match.** This is the critical gap. Real-world text uses synonyms, abbreviations, coreferences, and name variations. Without fuzzy matching, the graph fragments into disconnected entity islands.

**2. No incremental community updates.** Every insert wipes and rebuilds all communities. This makes the system impractical for continuous knowledge ingestion.

**3. No relationship typing.** Everything is "RELATED." Graph queries like "find all companies that acquired other companies" are impossible without reading descriptions.

**4. No error recovery.** A failed LLM call during extraction drops that chunk silently. A crash mid-indexing leaves partially written storage files. There are no transactions, no rollback, no retry at the operation level (only at the LLM call level via tenacity).

**5. In-memory storage.** Both the JSON KV store and NetworkX graph live entirely in memory. This works for small datasets but creates hard scaling limits.

---

## Summary: Adoption Recommendations

| Technique | Adopt? | Notes |
|---|---|---|
| LLM response cache (content-hash -> KV) | **Yes** | Simple, effective, transparent. Add to any LLM pipeline. |
| Gleaning (multi-pass extraction + early stop) | **Yes** | Improves recall for 2-3x the extraction cost. |
| Token-level chunking | **Yes** | Precise budget control vs. character estimation. |
| Content-hash deduplication | **Yes** | At document, chunk, and entity levels. |
| Multi-signal local context (entity+edge+community+source) | **Yes** | Core GraphRAG value proposition. |
| Map-reduce global query | **Conditional** | Useful for corpus-level questions. Expensive. |
| Hierarchical Leiden + bottom-up reports | **Yes** | Right algorithm, good report structure. |
| Community severity rating + occurrence scoring | **Yes** | Enables quality-based filtering. |
| Pluggable storage abstractions | **Yes** | Clean interface design, easy to swap backends. |
| DSPy self-refine extraction (critique -> refine) | **Conditional** | Better quality, 3x cost. Worth it for small, high-value corpora. |
| Entity resolution (exact match only) | **No** | Must add fuzzy/embedding-based matching. |
| Untyped relationships | **No** | Need typed edges for structured queries. |
| In-memory JSON storage | **No** | Replace with persistent DB for production. |
| Full community rebuild on insert | **No** | Need incremental community maintenance. |
