# Implementation Patterns from Reference Systems

Analysis of five knowledge graph systems mapped to our Rust graph-engine at `graph-engine/`. Each pattern includes the source algorithm, specific code references, and Rust adoption notes targeting our sqlx/Postgres stack.

---

## 1. Extraction Improvements

### 1.1 Gleaning (from GraphRAG)

**Source**: `graphrag/index/operations/extract_graph/graph_extractor.py` -- `_process_document()` method, lines 85-122.

**Algorithm**: After the initial extraction call, enter a loop up to `max_gleanings` times (default: 1, configurable). Each iteration:

1. Send `CONTINUE_PROMPT` to the LLM (appended to conversation history)
2. Append the LLM's additional extractions to `results`
3. Unless this is the final gleaning, send `LOOP_PROMPT` asking "are there more?"
4. If the LLM responds anything other than `"Y"`, break

**Exact prompt text** (from `graphrag/prompts/index/extract_graph.py`):

```
CONTINUE_PROMPT = "MANY entities and relationships were missed in the last extraction.
Remember to ONLY emit entities that match any of the previously extracted types.
Add them below using the same format:\n"

LOOP_PROMPT = "It appears some entities and relationships may have still been missed.
Answer Y if there are still entities or relationships that need to be added,
or N if there are none. Please answer with a single letter Y or N.\n"
```

**Key detail**: The conversation history accumulates -- each gleaning sees all prior extractions, so the LLM avoids re-emitting what it already found.

**Rust implementation notes**:
- Add `max_gleanings: u32` to the `Extractor` trait or as a parameter to `LlmExtractor`
- Build conversation history as a `Vec<Message>` in the extract loop
- After initial extraction, loop: push CONTINUE_PROMPT as user message, call LLM, accumulate response, push LOOP_PROMPT, check for "Y"
- Parse all accumulated text at the end (not per-gleaning) since records use `##` delimiter
- Default `max_gleanings = 1` -- one extra pass catches ~15-30% more entities per the GraphRAG paper

### 1.2 Negative-Example Prompting (from Graphiti)

**Source**: `graphiti_core/utils/maintenance/node_operations.py` -- `_build_entity_types_context()` function, and `graphiti_core/prompts/dedupe_nodes.py`.

**Algorithm**: The default entity type description in Graphiti includes an explicit exclusion list of bad entities:

```python
'entity_type_description': (
    'A specific, identifiable entity that does not fit any of the other listed '
    'types. Must still be a concrete, meaningful thing -- specific enough to be '
    'uniquely identifiable. GOOD: a named entity not covered by the other types. '
    'BAD: "luck", "ideas", "tomorrow", "things", "them", "everybody", '
    '"a sense of wonder", "great times". '
    'When in doubt, do not extract the entity.'
),
```

**Additionally**, Graphiti supports `excluded_entity_types: list[str]` which filters out entities post-extraction before they enter resolution (line 205 in `node_operations.py`):
```python
if excluded_entity_types and entity_type_name in excluded_entity_types:
    logger.debug(f'Excluding entity of type "{entity_type_name}"')
    continue
```

**How to integrate into our extractor**:
- Add a "BAD examples" section to the extraction prompt for the `LlmExtractor`
- Maintain a configurable exclusion list at the `Extractor` level: `excluded_types: Vec<String>`
- Filter extracted entities before passing them to the resolver -- cheap and effective noise reduction
- For our investment channel, exclude abstract nouns like "growth", "demand", "opportunity" which currently pollute entity resolution

### 1.3 Constrained Relation Extraction (from KGGen)

**Source**: `kg-gen/src/kg_gen/steps/_2_get_relations.py` -- `_create_relations_model()` function, lines 79-98.

**Algorithm**: KGGen dynamically creates Pydantic models with `Literal` types constraining subject/object to exactly the entities already extracted:

```python
def _create_relations_model(entities: List[str]):
    EntityLiteral = Literal[tuple(entities)]  # type: ignore
    RelationItem = create_model(
        "RelationItem",
        subject=(EntityLiteral, ...),
        predicate=(str, ...),
        object=(EntityLiteral, ...),
    )
    RelationsResponse = create_model(
        "RelationsResponse",
        relations=(List[RelationItem], ...),
    )
    return RelationItem, RelationsResponse
```

This is passed as a JSON schema to the LLM via `strict: True`, which forces the model to only emit valid entity names as subjects/objects. When strict validation fails, a fallback parser (`parse_relations_response()`) does raw JSON parsing and filters to valid entities.

**Rust implementation notes**:
- Two-phase extraction: extract entities first, then extract relations with entity names embedded in the schema
- Build a JSON schema dynamically where `subject` and `object` are `enum` types listing all extracted entity names
- Pass as Claude's `tool_use` schema with strict mode
- Fallback: parse JSON response and filter relations where source/target are not in the entity set
- This eliminates hallucinated entity references in relationships -- a problem our current system has since relationships resolve entities by name, creating phantom nodes

### 1.4 Delimiter Corruption Recovery (from LightRAG)

**Source**: `lightrag/utils.py` -- `fix_tuple_delimiter_corruption()` function, line 3004.

**Algorithm**: LLMs frequently corrupt the delimiter format (e.g., `<|#|>` becomes `<|##|>`, `<|\#|>`, `<|>`, or `<||>`). The function applies a cascade of regex fixes:

```python
def fix_tuple_delimiter_corruption(record, delimiter_core, tuple_delimiter):
    escaped = re.escape(delimiter_core)

    # Fix: <|##|> -> <|#|>, <|#||#|> -> <|#|>
    record = re.sub(rf"<\|{escaped}\|*?{escaped}\|>", tuple_delimiter, record)

    # Fix: <|\#|> -> <|#|>
    record = re.sub(rf"<\|\\{escaped}\|>", tuple_delimiter, record)

    # Fix: <|> -> <|#|>, <||> -> <|#|>
    record = re.sub(r"<\|+>", tuple_delimiter, record)

    return record
```

**Additional resilience** in LightRAG's `_handle_single_entity_extraction()` (line 386):
- Validates field count (must be exactly 4 for entities, 5 for relations)
- Sanitizes and normalizes extracted text (removes inner quotes, trims)
- Rejects entities with empty names, invalid types, or empty descriptions
- Handles comma-separated entity types by taking first non-empty token

**Rust implementation notes**:
- Add a `sanitize_extraction_output()` function before parsing
- Use `regex` crate for the corruption patterns
- Apply after LLM response, before splitting by record delimiter
- Also validate field counts per record type and log/skip malformed records
- Critical for robustness since our `LlmExtractor` will use similar delimiter-based output

---

## 2. Entity Resolution Improvements

### 2.1 Three-Tier Cascade (from Graphiti)

**Source**: `graphiti_core/utils/maintenance/dedup_helpers.py` and `node_operations.py` -- `resolve_extracted_nodes()` orchestrator plus `_resolve_with_similarity()` and `_resolve_with_llm()`.

**Algorithm** (exact flow from `resolve_extracted_nodes()`, line 490):

**Phase 0 -- Semantic Candidate Retrieval**: For each extracted node, embed the name and run cosine similarity search against existing nodes (min score 0.6, limit 15 candidates). This narrows the candidate set before any resolution.

**Tier 1 -- Exact Normalized Name Match** (always runs, no entropy gate):
```python
normalized_exact = re.sub(r'[\s]+', ' ', name.lower()).strip()
existing_matches = indexes.normalized_existing.get(normalized_exact, [])
if len(existing_matches) == 1:
    # resolved
elif len(existing_matches) > 1:
    # ambiguous -> escalate to LLM
```

**Tier 2 -- MinHash/LSH Fuzzy Match** (gated by entropy):

Constants:
```python
_NAME_ENTROPY_THRESHOLD = 1.5
_MIN_NAME_LENGTH = 6
_MIN_TOKEN_COUNT = 2
_FUZZY_JACCARD_THRESHOLD = 0.9
_MINHASH_PERMUTATIONS = 32
_MINHASH_BAND_SIZE = 4
```

Entropy gate: Skip fuzzy matching for names shorter than 6 chars with fewer than 2 tokens, or with Shannon entropy < 1.5. This prevents "AI" from matching "A1" etc.

Shingles: Character 3-grams from the cleaned name (spaces removed).

MinHash: 32 permutations using `blake2b(f'{seed}:{shingle}', digest_size=8)`.

LSH: Bands of size 4 (8 bands total). If any band matches, the candidate enters the Jaccard verification phase.

Jaccard threshold: 0.9 -- very strict, only near-identical names pass.

**Tier 3 -- LLM Resolution** (for unresolved nodes):
All unresolved nodes are batched into a single LLM call using the `dedupe_nodes.nodes()` prompt. The LLM returns `duplicate_candidate_id` for each (or -1 for no match). Response validation discards out-of-range IDs and logs warnings for missing/extra IDs.

**Rust implementation notes**:
- Phase 0: Use our existing `vectors::search_entities()` with a lower threshold (0.6 cosine min)
- Tier 1: Already implemented in our `resolver.rs` as exact name match -- keep this
- Tier 2: Implement MinHash/LSH in pure Rust:
  - `blake2` crate for hashing
  - Precompute shingle sets and signatures for existing entities (can be cached in memory or a `entity_signatures` table)
  - Shannon entropy is trivial: `chars.counts().map(|c| -p*log2(p)).sum()`
  - For our scale (thousands, not millions of entities), a simple in-memory index suffices
- Tier 3: Batch unresolved entities into one Claude API call with the dedup prompt
- The key insight: exact match covers ~70% of cases, fuzzy covers ~20%, LLM handles the remaining ~10% -- huge cost savings vs all-LLM resolution

### 2.2 SEMHASH Normalization (from KGGen)

**Source**: `kg-gen/src/kg_gen/utils/deduplicate.py` -- `DeduplicateList` class.

**Algorithm**: Three-step normalization before deduplication:

1. **Unicode normalization**: `unicodedata.normalize("NFKC", text)` -- collapses compatibility characters (e.g., ligatures, fullwidth forms)
2. **Singularization**: Per-token singularization using `inflect.engine().singular_noun(tok)` -- "Data Centers" becomes "Data Center"
3. **Semantic hashing**: Uses `SemHash.from_records()` which internally creates character n-gram fingerprints and clusters items by fingerprint similarity

Threshold: `0.95` for `self_deduplicate()` -- very conservative.

After deduplication, entity names in relations are remapped to their canonical (deduplicated) representative.

**Rust implementation notes**:
- Unicode normalization: `unicode-normalization` crate, apply NFKC
- Singularization: No great Rust library exists. Options:
  - Simple suffix rules ("ies" -> "y", "es" -> "", "s" -> "") cover 80% of cases
  - Or call out to a small lookup table for common plurals
- Apply both normalizations in `resolver::normalize_name()` before the current lowercase + whitespace collapse
- The character n-gram fingerprint (SemHash) is essentially what Graphiti's MinHash does -- we can combine these approaches
- Add NFKC normalization to our existing `normalize_name()` as a quick win

### 2.3 Rank Fusion Candidate Selection (from KGGen)

**Source**: `kg-gen/src/kg_gen/utils/llm_deduplicate.py` -- `get_relevant_items()` method, lines 57-83.

**Algorithm**: For each entity to deduplicate, retrieve candidates using both BM25 and embedding similarity, then combine with equal weights:

```python
bm25_scores = self.node_bm25.get_scores(query_tokens)
query_embedding = self.retrieval_model.encode([query])
embedding_scores = cosine_similarity(query_embedding, embeddings).flatten()
combined_scores = 0.5 * bm25_scores + 0.5 * embedding_scores
top_indices = np.argsort(combined_scores)[::-1][:top_k]
```

BM25 tokenization: simple `text.lower().split()`.

**Clustering**: KMeans with cluster_size=128, then per-cluster LLM deduplication. This bounds LLM context size.

**Rust implementation notes**:
- BM25: Implement using term frequency / inverse document frequency over entity names stored in Postgres. Alternatively, use Postgres full-text search with `ts_rank` as a BM25 proxy
- Combine with our existing pgvector cosine similarity
- SQL approach: `SELECT id, (0.5 * ts_rank(to_tsvector(canonical_name), plainto_tsquery($1)) + 0.5 * (1 - (embedding <=> $2))) as score FROM entities ORDER BY score DESC LIMIT $3`
- This gives us better candidate retrieval than pure vector search, especially for exact/partial name matches that embedding models sometimes miss

---

## 3. Community Detection

### 3.1 Leiden Algorithm Integration (from GraphRAG)

**Source**: `graphrag/graphs/hierarchical_leiden.py` and `graphrag/index/operations/cluster_graph.py`.

**Algorithm**: GraphRAG uses `graspologic_native` (Rust-based!) for hierarchical Leiden clustering:

```python
gn.hierarchical_leiden(
    edges=edges,                    # list[tuple[str, str, float]]
    max_cluster_size=10,            # default
    seed=0xDEADBEEF,
    starting_communities=None,
    resolution=1.0,
    randomness=0.001,
    use_modularity=True,
    iterations=1,
)
```

Returns `HierarchicalCluster` objects with `node`, `cluster`, `level`, `parent_cluster`, `is_final_cluster` fields.

The `cluster_graph()` function (line 20) converts a relationships DataFrame into an edge list, normalizes edge direction (undirected), optionally extracts the largest connected component, then runs hierarchical Leiden. Output: `list[tuple[level, cluster_id, parent_cluster_id, list[node_ids]]]`.

**Rust implementation notes**:
- The `graspologic` native library IS Rust (`graspologic-native` crate). We can depend on it directly or use the `igraph` Rust bindings
- Alternative: `petgraph` + a Leiden implementation. The `leiden` crate exists but is less mature
- For Postgres-stored graphs:
  1. Query all active relationships: `SELECT source_id, target_id, 1.0 as weight FROM relationships WHERE valid_until IS NULL`
  2. Build in-memory edge list
  3. Run Leiden clustering
  4. Store results in a `communities` table: `(community_id, level, parent_community_id, entity_id)`
  5. Run as a background job after cognify, not inline
- `max_cluster_size = 10` is the key parameter -- small clusters produce more focused reports

### 3.2 Hierarchical Report Generation (from GraphRAG / nano-graphrag)

**Source**: `graphrag/index/operations/summarize_communities/community_reports_extractor.py` and `graphrag/prompts/index/community_report.py`.

**Algorithm**: For each community, gather its entities and relationships, format as CSV-like text, and ask the LLM to generate a structured report.

**Report JSON structure** (from `CommunityReportResponse` Pydantic model):
```json
{
    "title": "short, specific community name with key entity names",
    "summary": "executive summary of structure and relationships",
    "rating": 0.0-10.0,  // impact severity
    "rating_explanation": "single sentence justification",
    "findings": [
        {
            "summary": "insight title",
            "explanation": "multi-paragraph grounded explanation with [Data: Entities (ids)] references"
        }
    ]
}
```

**nano-graphrag** uses the same structure (copied from GraphRAG) in `nano_graphrag/prompt.py` line 63.

**Bottom-up summarization**: Lower-level community reports become inputs for higher-level community reports. GraphRAG's `_compute_leiden_communities()` returns a hierarchy mapping (`parent_cluster`), and reports at level N incorporate reports from level N-1.

**Rust implementation notes**:
- Define a `CommunityReport` struct with serde serialization
- Store in `community_reports` table: `(community_id, level, title, summary, rating, rating_explanation, findings JSONB, created_at)`
- Generate bottom-up: process level 0 communities first, then level 1 using level 0 reports as additional context
- For search, treat community reports as additional searchable context
- Generate reports asynchronously after community detection completes

### 3.3 Community-Based Search (from GraphRAG)

**Source**: `graphrag/query/structured_search/global_search/search.py` -- `GlobalSearch.search()` method.

**Algorithm** -- Map-Reduce over community reports:

**Map phase** (parallel): For each community report (or batch of reports that fit in context):
1. Format the report into the MAP_SYSTEM_PROMPT
2. Ask LLM to extract key points relevant to the query
3. LLM returns JSON: `{"points": [{"description": "...", "score": 0-100}]}`
4. Run all batches in parallel with `asyncio.gather()`, bounded by semaphore (default 32 concurrent)

**Reduce phase**:
1. Collect all key points from all map responses
2. Filter out points with score = 0
3. Sort by score descending
4. Truncate to fit `max_data_tokens` (default 8000)
5. Format as "----Analyst N----\nImportance Score: X\n[answer]"
6. Feed into REDUCE_SYSTEM_PROMPT for final answer synthesis

**Rust implementation notes**:
- Add a `search_type = "GLOBAL"` option to our `pipeline::search()`
- When GLOBAL, skip vector search; instead load community reports for the requested channels
- Parallelize map calls using `tokio::spawn` + semaphore
- Reduce: sort by score, truncate to token budget, synthesize
- This excels for broad questions ("what do I know about AI infrastructure?") where vector search misses the forest for the trees

---

## 4. Temporal Improvements

### 4.1 Contradiction Detection + Resolution (from Graphiti)

**Source**: `graphiti_core/utils/maintenance/edge_operations.py` -- `resolve_extracted_edge()` (line 495) and `resolve_edge_contradictions()` (line 457). Plus `graphiti_core/prompts/dedupe_edges.py`.

**Algorithm**: Graphiti's edge resolution is a three-step process:

**Step 1 -- Fast exact-text dedup** (line 545):
```python
normalized_fact = _normalize_string_exact(extracted_edge.fact)
for edge in related_edges:
    if (edge.source_node_uuid == extracted_edge.source_node_uuid
        and edge.target_node_uuid == extracted_edge.target_node_uuid
        and _normalize_string_exact(edge.fact) == normalized_fact):
        return edge  # exact duplicate, reuse
```

**Step 2 -- LLM-based duplicate + contradiction detection**: Send existing edges and invalidation candidates to the LLM with continuous indexing. The prompt asks for two things simultaneously:

```
duplicate_facts: [idx]     -- edges that say the same thing (from EXISTING FACTS only)
contradicted_facts: [idx]  -- edges the new fact contradicts (from either list)
```

The prompt includes carefully constructed examples:
- "Alice joined Acme Corp in 2020" vs same = duplicate
- "Alice works at Acme Corp as software engineer" vs "...as senior engineer" = contradiction (NOT duplicate)
- "Bob ran 5 miles Tuesday" vs "Bob ran 3 miles Wednesday" = neither (different events)

**Step 3 -- Temporal invalidation** (line 457, `resolve_edge_contradictions()`):

For each contradicted edge, apply temporal logic:
```python
# Skip if edge was already invalid before new edge became valid
if edge.invalid_at <= resolved_edge.valid_at:
    continue
# Skip if new edge was invalid before edge became valid
if resolved_edge.invalid_at <= edge.valid_at:
    continue
# If the old edge predates the new one, expire it
if edge.valid_at < resolved_edge.valid_at:
    edge.invalid_at = resolved_edge.valid_at
    edge.expired_at = now
```

Additionally, the resolved edge itself can be expired if a contradicting edge has a MORE RECENT valid_at (line 672):
```python
if candidate.valid_at > resolved_edge.valid_at:
    resolved_edge.invalid_at = candidate.valid_at
    resolved_edge.expired_at = now
    break  # only need one newer contradicting edge
```

**Key insight**: Graphiti uses BOTH `valid_at`/`invalid_at` (assertion time -- when the fact was true) AND `expired_at` (system time -- when the system learned the fact was superseded). This is true bitemporal modeling.

**Rust implementation notes**:
- Our current `temporal.rs` only does same-type supersession (same source, target, relationship type). Graphiti's approach is more nuanced:
  - Same endpoints + same fact text = duplicate (skip)
  - Same endpoints + contradicting fact = temporal invalidation
  - Different endpoints but semantically contradicting = invalidation candidate
- Add an `expired_at` column to `relationships` table (distinct from `valid_until`)
- For LLM-based contradiction detection: batch related edges per new edge, call Claude with the dedup prompt
- Temporal logic can be pure Rust -- no LLM needed for the date comparison rules
- Priority: High. Our current naive supersession (any matching triple gets expired) is too aggressive

---

## 5. Retrieval Improvements

### 5.1 Dual-Level Retrieval (from LightRAG)

**Source**: `lightrag/operate.py` -- `_build_query_context()` (line 4239) and `get_keywords_from_query()` (line 3374).

**Algorithm**: LightRAG splits every query into two keyword types, then routes them to different VDBs:

**Keyword extraction** (via LLM, prompt at `lightrag/prompt.py` line 374):
```
high_level_keywords: overarching concepts, themes, core intent
low_level_keywords: specific entities, proper nouns, technical terms
```

Example for "How does international trade influence global economic stability?":
- high_level: ["International trade", "Global economic stability", "Economic impact"]
- low_level: ["Trade agreements", "Tariffs", "Currency exchange", "Imports", "Exports"]

**Routing**:
- `low_level_keywords` -> Entity VDB (`_get_node_data()`) -- searches entity descriptions/names
- `high_level_keywords` -> Relationship VDB (`_get_edge_data()`) -- searches relationship keywords/descriptions

LightRAG stores `relationship_keywords` as a separate field on each edge (comma-separated), which captures thematic/conceptual terms.

**Retrieval modes**:
- `local`: entities VDB only (low-level keywords)
- `global`: relationships VDB only (high-level keywords)
- `hybrid`: both VDBs, results merged via round-robin interleaving
- `mix`: hybrid + raw vector chunk search

**Rust implementation notes**:
- Add a `keywords` TEXT column to `relationships` table
- During extraction, ask the LLM for relationship keywords (LightRAG's prompt includes `relationship_keywords` as a field)
- At query time, extract high/low keywords from the query (one LLM call)
- Low-level -> our existing `vectors::search_entities()`
- High-level -> new `vectors::search_relationships_by_keywords()` using a keyword embedding stored on relationships
- Merge results using round-robin (alternating local/global) to ensure diversity

### 5.2 Dynamic Token Budget (from LightRAG)

**Source**: `lightrag/operate.py` -- `_apply_token_truncation()` (line 3783) and `_build_context_str()` (line 4056).

**Algorithm**: LightRAG manages three separate token budgets for context assembly:

```python
DEFAULT_MAX_ENTITY_TOKENS = 4000
DEFAULT_MAX_RELATION_TOKENS = 4000
DEFAULT_MAX_TOTAL_TOKENS = 12000
```

The `_apply_token_truncation()` function:
1. Takes the merged entity and relation lists (already ranked by relevance)
2. Truncates entities list to `max_entity_tokens`
3. Truncates relations list to `max_relation_tokens`
4. If combined exceeds `max_total_tokens`, further truncates proportionally

The `truncate_list_by_token_size()` utility iterates through items, accumulating token counts, and stops when the budget is exhausted.

**Entity/relation summary handling** also uses token budgets (line 167, `_handle_entity_relation_summary()`):
- If total tokens < `summary_context_size` and list count < `force_llm_summary_on_merge`: just concatenate (no LLM)
- If within context window: single LLM summarization
- Otherwise: map-reduce -- chunk descriptions into groups, summarize each group, then recursively summarize the summaries

**Rust implementation notes**:
- Add token counting to search result assembly (approximate: `chars / 4`)
- Set budgets per result type: entity context, relationship context, chunk context
- Truncate ranked results to budget before returning to the caller
- For our system: entity budget = 3000 tokens, relationship budget = 3000 tokens, chunk budget = 4000 tokens
- This prevents context overflow when the graph has many relevant results

---

## 6. Caching

### 6.1 LLM Response Cache (from nano-graphrag)

**Source**: `nano_graphrag/_llm.py` -- `openai_complete_if_cache()` function (line 50), and `nano_graphrag/_utils.py` -- `compute_args_hash()` (line 220).

**Algorithm**: Content-addressable caching of LLM responses:

```python
def compute_args_hash(*args):
    return md5(str(args).encode()).hexdigest()

# In the LLM call function:
args_hash = compute_args_hash(model, messages)
if_cache_return = await hashing_kv.get_by_id(args_hash)
if if_cache_return is not None:
    return if_cache_return["return"]

# After LLM call:
await hashing_kv.upsert({
    args_hash: {"return": response.choices[0].message.content, "model": model}
})
```

The cache key is `MD5(str(model_name, full_messages_array))`. The value stores the response content and model name.

**LightRAG extends this** with typed caching (from `lightrag/utils.py`):
- `cache_type` field distinguishing "extract", "summary", "query"
- Additional metadata: prompt text, mode, query parameters
- TTL not implemented in either -- cache is permanent until manually cleared

**Rust implementation notes**:
- Add an `llm_cache` table: `(hash TEXT PRIMARY KEY, model TEXT, response TEXT, cache_type TEXT, created_at TIMESTAMPTZ)`
- Hash function: `sha256(format!("{}:{}", model, serde_json::to_string(&messages)?))` -- use SHA256 over MD5
- Check cache before every LLM call in the extractor and search paths
- This is extremely high value for our system because:
  - Cognify re-processes chunks on failure/restart -- cached responses avoid re-spending API tokens
  - Entity description summarization is deterministic for the same inputs
  - Search queries with identical context hit the same extraction prompts
- Estimated savings: 30-50% of LLM API costs during iterative development

---

## Priority Adoption Order

Ranked by impact on our graph-engine, considering current gaps and implementation effort:

### Tier 1 -- High Impact, Moderate Effort

1. **Gleaning (1.1)** -- Our `LlmExtractor` is currently a stub. When implementing it, gleaning should be built in from the start. One extra LLM call per chunk catches 15-30% more entities. Low marginal cost.

2. **LLM Response Cache (6.1)** -- Add before any other LLM integration. Every subsequent feature benefits from cached responses during development and re-processing. Simple table + hash lookup.

3. **Contradiction Detection (4.1)** -- Our current temporal logic blindly supersedes matching triples. Graphiti's approach (exact text dedup + LLM contradiction detection + temporal rules) is dramatically more accurate. This prevents data loss from false supersession.

### Tier 2 -- High Impact, Higher Effort

4. **Three-Tier Entity Resolution (2.1)** -- Our resolver does exact match + alias lookup. Adding MinHash/LSH as tier 2 and LLM as tier 3 would catch "Teck Resources" / "Teck" / "TECK RESOURCES LIMITED" without expensive LLM calls for every entity.

5. **Constrained Relation Extraction (1.3)** -- Two-phase extraction (entities first, then relations constrained to those entities) eliminates phantom entity creation from relationship source/target names that don't match any extracted entity.

6. **Community Detection + Reports (3.1, 3.2)** -- Leiden clustering enables a fundamentally different search mode (global/thematic). The `graspologic-native` crate means the core algorithm is already in Rust.

### Tier 3 -- Moderate Impact, Good Polish

7. **SEMHASH Normalization (2.2)** -- Adding Unicode NFKC normalization and singularization to `normalize_name()` is a small change with meaningful dedup improvement.

8. **Dual-Level Retrieval (5.1)** -- Adding keyword-based relationship search alongside entity vector search improves retrieval for thematic queries. Requires adding a `keywords` field to relationships.

9. **Negative-Example Prompting (1.2)** -- Adding exclusion lists to the extraction prompt is trivial and reduces entity noise immediately.

10. **Dynamic Token Budget (5.2)** -- Prevents context overflow as the graph grows. Simple token counting + truncation.

11. **Delimiter Corruption Recovery (1.4)** -- Defensive parsing that prevents extraction failures from LLM output format errors. Important for production robustness.

12. **Community-Based Search (3.3)** -- Map-reduce over community reports. Depends on community detection being in place first.

### Tier 4 -- Nice to Have

13. **Rank Fusion Candidate Selection (2.3)** -- BM25 + embedding fusion for candidate retrieval. Our Postgres full-text search can approximate BM25. Helps when entity names have exact substring matches that embeddings miss.
