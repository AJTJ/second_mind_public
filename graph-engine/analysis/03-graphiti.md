# Graphiti (Zep) - Knowledge Graph System Analysis

Date: 2026-04-13
Source: `~/references/graph-systems/graphiti/` (commit at time of analysis)

---

## 1. Extraction

### Method

Graphiti uses a **pipeline architecture**: entity extraction and edge extraction are separate LLM calls run sequentially. Nodes are extracted first, then edges are extracted given the resolved node list.

Three extraction prompts exist, selected by `EpisodeType`:
- `extract_message` -- conversational "Speaker: content" format
- `extract_json` -- structured JSON data
- `extract_text` -- unstructured prose

All three use structured output via Pydantic models (`ExtractedEntities` with `ExtractedEntity` items). Each entity gets a `name` and an `entity_type_id` referencing a provided entity types list.

Edge extraction (`extract_edges.py::edge()`) receives the resolved entity list and produces `ExtractedEdges` -- triples of `(source_entity_name, target_entity_name, relation_type, fact, valid_at, invalid_at)`.

Entity types are user-extensible: callers pass `entity_types: dict[str, type[BaseModel]]` with Pydantic models whose docstrings become type descriptions in the prompt. The system always includes a fallback "Entity" type at ID 0.

**File**: `graphiti_core/prompts/extract_nodes.py`, `graphiti_core/prompts/extract_edges.py`
**Orchestration**: `graphiti_core/utils/maintenance/node_operations.py::extract_nodes()`, `graphiti_core/utils/maintenance/edge_operations.py::extract_edges()`

### What works well

- **Extremely detailed negative-example prompting.** The extraction prompts have extensive exclusion lists (pronouns, abstract concepts, bare kinship terms, generic nouns, ambiguous bare nouns). This is the most thorough extraction prompt engineering I have seen in any open-source graph system. Example: "NEVER extract bare relational or kinship terms (dad, mom...)" with instruction to qualify as "Nisha's dad".
- **Speaker extraction is first-class.** Message-type episodes always extract the speaker as entity #1, preventing the common failure where conversational actors get lost.
- **Entity-name validation on edges.** After edge extraction, `extract_edges()` validates that `source_entity_name` and `target_entity_name` match actual node names. Invalid names are dropped with warnings (lines 155-178 of edge_operations.py). Self-edges are also dropped.
- **Custom extraction instructions.** The `custom_extraction_instructions` parameter is injected into every prompt, allowing domain-specific guidance without forking the prompt library.
- **Temporal extraction is integrated into edge extraction.** The edge prompt asks for `valid_at` and `invalid_at` in ISO 8601, using `REFERENCE_TIME` to resolve relative expressions like "last week".

### Failure modes

- **Pipeline coupling.** Edge extraction depends on the exact `name` strings from node extraction. If the LLM returns a slightly different name in the edge response (common with aliases, possessives, or abbreviations), the edge is silently dropped. The validation at lines 155-178 logs a warning but does not attempt fuzzy matching.
- **No joint extraction.** Extracting nodes and edges in separate calls means the LLM cannot reason about which entities matter based on their relationships. An entity that only matters because of a critical relationship might be omitted during node extraction.
- **Episode-level context window.** Previous episodes are passed as raw content strings. For long conversation histories, this can exceed token limits. The system retrieves `RELEVANT_SCHEMA_LIMIT` (10) previous episodes, which may be insufficient for long-running conversations or may overflow for verbose episodes.
- **Entity type classification is single-call.** Classification happens during extraction (not as a separate refinement step), meaning the LLM must simultaneously identify entities AND classify them. This creates tension between extraction recall and classification precision.

### Worth adopting

Yes -- the negative-example prompting technique for entity extraction. The explicit exclusion of abstract concepts, pronouns, bare kinship terms, and generic nouns is directly applicable to any knowledge graph extraction pipeline. The `custom_extraction_instructions` injection pattern is also clean and reusable.

---

## 2. Chunking

### Method

Graphiti has a sophisticated, density-aware chunking system in `graphiti_core/utils/content_chunking.py`.

**Decision logic** (`should_chunk()`):
1. If content is below `CHUNK_MIN_TOKENS`, never chunk (short content is fine regardless).
2. If content is large, estimate entity density.
3. Only chunk if density is high (many entities per token).

**Density estimation** varies by type:
- **JSON**: Counts array elements or object keys at depth 2. High element-per-token ratio = dense.
- **Text**: Counts capitalized words (excluding sentence starters) as a proxy for named entities.

**Chunking strategies**:
- `chunk_json_content()` -- Splits arrays at element boundaries, objects at key boundaries. Preserves valid JSON in each chunk.
- `chunk_text_content()` -- Splits at paragraph boundaries first, then sentences, then fixed-size as last resort.
- `chunk_message_content()` -- Preserves message boundaries ("Speaker: content" format). Never splits mid-message.
- All strategies include configurable overlap (`CHUNK_OVERLAP_TOKENS`).

A separate `generate_covering_chunks()` function (for bulk operations) uses a greedy set-cover algorithm to ensure every pair of items appears in at least one chunk -- based on the "Handshake Flights Problem" / covering design theory. This is used for node deduplication across episodes.

### What works well

- **Density-aware decision.** Not all large documents need chunking. Prose with low entity density (e.g., a research paper with few proper nouns) preserves context better as a single episode. Only entity-dense content (lists of companies, structured data) benefits from splitting.
- **Structure-preserving JSON chunking.** Each JSON chunk remains valid JSON. Array elements and object keys are never split mid-value.
- **Message-boundary respect.** Conversations are never split mid-message, preventing the "orphaned utterance" problem.
- **Covering design for bulk dedup.** The `generate_covering_chunks()` function ensures that every pair of extracted entities will appear together in at least one chunk during deduplication, even when the total entity set is too large for a single LLM call.

### Failure modes

- **Capitalization heuristic for text density is fragile.** Academic text, all-caps headers, camelCase identifiers, and non-Latin scripts all break the capitalized-word proxy. A document about "NASA" and "CERN" would have zero "capitalized" words (all-caps are excluded).
- **Overlap is character-based, not semantic.** Overlap takes the last N characters of the previous chunk, potentially splitting meaningful phrases.
- **No cross-chunk entity deduplication at the extraction level.** Chunks are processed as independent episodes. The same entity appearing in two chunks creates duplicate extraction candidates that must be resolved downstream. The dedup pipeline handles this, but extraction cost is doubled.

### Worth adopting

Yes -- the density-aware chunking decision (`should_chunk()`) is a good idea. Most systems chunk unconditionally by token count. The insight that low-density prose loses context when chunked while high-density lists benefit from it is worth implementing. The JSON-structure-preserving chunking is also solid.

---

## 3. Entity Resolution

### Method

Graphiti uses a **three-tier entity resolution pipeline** in `graphiti_core/utils/maintenance/node_operations.py::resolve_extracted_nodes()` and `graphiti_core/utils/maintenance/dedup_helpers.py`:

**Tier 1: Semantic candidate retrieval.**
For each extracted node, embed its name and run cosine similarity search against existing graph nodes. Returns up to `NODE_DEDUP_CANDIDATE_LIMIT` (15) candidates with a minimum cosine score of `NODE_DEDUP_COSINE_MIN_SCORE` (0.6).

**Tier 2: Deterministic resolution** (`_resolve_with_similarity()`):
1. **Exact normalized-name match.** `_normalize_string_exact()` lowercases and collapses whitespace. If exactly one candidate matches, resolve immediately. If multiple candidates share the same normalized name, escalate to LLM (ambiguous).
2. **Entropy gate.** Short or low-entropy names (below `_NAME_ENTROPY_THRESHOLD` = 1.5 or fewer than 6 chars with fewer than 2 tokens) skip fuzzy matching and go to LLM. This prevents "Sam" from fuzzy-matching "Pam".
3. **MinHash/LSH fuzzy matching.** Compute 3-gram shingles, generate MinHash signatures (32 permutations), partition into bands (size 4) for LSH. If Jaccard similarity exceeds `_FUZZY_JACCARD_THRESHOLD` (0.9), merge deterministically.

**Tier 3: LLM resolution** (`_resolve_with_llm()`):
All unresolved nodes are sent to `dedupe_nodes.nodes()` prompt in a single batch. The LLM receives extracted entities with IDs and existing candidates with `candidate_id`s. Returns `NodeResolutions` with `duplicate_candidate_id` (-1 for no match).

After resolution, `_promote_resolved_node()` upgrades generic "Entity" labels when a duplicate carries a more specific type (e.g., merging "Sam" [Entity] with "Sam" [Person] keeps the Person label).

**Edge deduplication** (`resolve_extracted_edge()` in `edge_operations.py`):
1. Fast path: exact normalized-fact text match between same endpoints.
2. LLM dedup: `dedupe_edges.resolve_edge()` prompt receives existing facts (same endpoints) and invalidation candidates (broader search), returns `duplicate_facts` and `contradicted_facts` with continuous index numbering.

**File**: `graphiti_core/utils/maintenance/dedup_helpers.py`, `graphiti_core/utils/maintenance/node_operations.py` (lines 490-571), `graphiti_core/utils/maintenance/edge_operations.py` (lines 495-691)

### What works well

- **Three-tier cascade avoids unnecessary LLM calls.** Exact matches and high-confidence fuzzy matches are resolved deterministically, only escalating ambiguous cases to LLM. This saves significant cost and latency.
- **Entropy gate is clever.** Short names like "AI" or "Sam" produce unreliable shingle sets. The entropy check (`_name_entropy()`) uses Shannon entropy to detect these and routes them to LLM resolution instead of risking false positives.
- **MinHash/LSH is proper fuzzy matching.** 3-gram shingles with 32 permutations and band size 4 give good probabilistic near-duplicate detection. The 0.9 Jaccard threshold is appropriately conservative -- only near-identical names resolve deterministically.
- **Label promotion.** When a new extraction has a more specific type than the existing canonical node, the canonical node gets upgraded. This means the graph gets progressively more typed over time.
- **Edge dedup with continuous indexing across two lists.** The `resolve_edge` prompt receives both "existing facts" (same endpoints) and "invalidation candidates" (broader) with continuous indices. The LLM can mark a fact as both a duplicate AND contradicted (e.g., same relationship but updated details).
- **IS_DUPLICATE_OF edges.** When node duplicates are found, Graphiti creates explicit `IS_DUPLICATE_OF` relationship edges (checked in `filter_existing_duplicate_of_edges()`), maintaining provenance of merge decisions.

### Failure modes

- **Candidate retrieval depends on embedding quality for names.** If the embedder does not handle abbreviations well (e.g., "NYC" vs "New York City"), the candidate set may not include the correct match, and the LLM never gets a chance to see it. The 0.6 cosine threshold is a hard cutoff.
- **Batch LLM dedup for nodes sends ALL unresolved nodes in one call.** For episodes with many new entities, this can create a very large prompt. The prompt includes all existing candidates for all unresolved nodes, which can exceed token limits.
- **Edge dedup uses separate searches for duplicates and invalidation candidates.** Two `search()` calls per extracted edge -- one with `SearchFilters(edge_uuids=...)` (same endpoints) and one with `SearchFilters()` (broad). This is O(2E) search operations.
- **LLM malformed response handling is defensive but lossy.** If the LLM returns an ID outside the valid range, that node is silently treated as "no duplicate." Missing IDs are logged as warnings but left unresolved (defaulting to "new node"). This is safe but means some duplicates slip through.

### Worth adopting

Yes -- the three-tier cascade (exact -> fuzzy/LSH -> LLM) is the best entity resolution approach I have seen in an open-source graph system. The entropy gate for short names and the MinHash/LSH layer are both worth implementing. The `IS_DUPLICATE_OF` explicit edge for merge provenance is also valuable.

---

## 4. Relationships

### Method

Edges are typed with `relation_type` in SCREAMING_SNAKE_CASE (e.g., `WORKS_AT`, `LIVES_IN`, `IS_FRIENDS_WITH`). The extraction prompt derives the relation type from the relationship predicate.

**Edge types are user-extensible.** Callers can pass `edge_types: dict[str, type[BaseModel]]` and an `edge_type_map: dict[tuple[str, str], list[str]]` that maps (source_type, target_type) pairs to allowed edge type names. When an extracted edge's endpoints match a signature, the LLM is prompted to use the corresponding type name. Edge types with Pydantic models also get custom attribute extraction via a separate LLM call.

**Edge structure** (`EntityEdge` in `edges.py`):
- `name` -- relation type string
- `fact` -- natural language description of the relationship
- `fact_embedding` -- vector embedding of the fact text
- `episodes` -- list of episode UUIDs that reference this edge
- `valid_at`, `invalid_at` -- temporal validity bounds
- `expired_at` -- when the edge was superseded in the graph
- `reference_time` -- timestamp from the source episode
- `attributes` -- extensible dict for custom edge type properties
- `source_node_uuid`, `target_node_uuid` -- endpoint references

**Edge types in the graph:**
- `RELATES_TO` -- entity-to-entity edges (the main knowledge edges)
- `MENTIONS` -- episode-to-entity edges
- `HAS_MEMBER` -- community-to-entity edges
- `HAS_EPISODE` -- saga-to-episode edges
- `NEXT_EPISODE` -- episode-to-episode ordering edges

### What works well

- **Fact-based edges.** Each edge stores a natural language `fact` string, not just a relation type. This means "Alice works at Acme Corp as a senior engineer" is preserved as the fact, with `WORKS_AT` as the typed relation. Retrieval can match on semantic similarity of the fact text.
- **Episode tracking on edges.** The `episodes` list tracks which episodes mention each edge. This enables episode-mentions-based reranking and provenance tracking. A frequently-mentioned fact is treated as more important.
- **Signature-constrained edge types.** The `edge_type_map` prevents type-invalid edges (e.g., `WORKS_AT` between two Location entities). This is enforced before LLM extraction.
- **Custom attributes via Pydantic models.** Edge types can define structured attributes (e.g., a `WORKS_AT` edge might have `role`, `start_date` fields). These are extracted in a separate LLM call after edge resolution.

### Failure modes

- **Relation type is LLM-generated free text.** Despite the SCREAMING_SNAKE_CASE instruction, there is no enforcement or normalization. The LLM might produce `WORKS_FOR` vs `WORKS_AT` for the same relationship, creating semantically duplicate edge types. The edge dedup prompt catches semantic duplicates, but the type namespace grows unboundedly.
- **Directed edges lose bidirectional information.** `RELATES_TO` edges are directed (source -> target). Querying "what does Entity B relate to?" requires querying both directions. The `get_by_node_uuid` query uses undirected matching (`-[e:RELATES_TO]-` without arrow), but this is inconsistent with the directed save semantics.
- **No edge type ontology.** Unlike entity types which have a structured type system with IDs and descriptions, edge types are ad-hoc unless the caller provides `edge_types`. Most users will get free-text relation types with no consistency.

### Worth adopting

Yes -- fact-based edges with natural language descriptions are superior to bare relation types for retrieval. The episode-tracking on edges (knowing which episodes support a fact) is excellent for provenance. The signature-constrained edge type map is a clean way to enforce type-valid relationships.

---

## 5. Storage

### Method

Graphiti supports four graph backends via a driver abstraction:
- **Neo4j** (primary, most complete)
- **FalkorDB**
- **KuzuDB** (embedded, notable for our interest)
- **Neptune** (AWS)

**Node types stored:**
- `Entity` -- knowledge entities with `name`, `name_embedding`, `summary`, `attributes`, `labels`
- `Episodic` -- raw episode content with `valid_at`, `source`, `source_description`, `entity_edges`
- `Community` -- cluster summaries with `name`, `name_embedding`, `summary`
- `Saga` -- episode sequence containers with `summary`, `first_episode_uuid`, `last_episode_uuid`

**Indices** (from `graph_queries.py`):
- **Range indices:** UUID, group_id, name, created_at on Entity; UUID, group_id, created_at, valid_at on Episodic; UUID, group_id, name on Saga; UUID, group_id, name, created_at, expired_at, valid_at, invalid_at on RELATES_TO edges.
- **Fulltext indices:** `node_name_and_summary` on Entity (name + summary + group_id), `community_name` on Community, `episode_content` on Episodic, `edge_name_and_fact` on RELATES_TO (name + fact + group_id).
- **Vector indices:** name_embedding on Entity nodes, fact_embedding on RELATES_TO edges, name_embedding on Community nodes. Used via cosine similarity functions.

**Multi-tenancy:** The `group_id` field partitions the graph. All queries filter by group_id. In Neo4j, group_id can also map to a separate database.

**KuzuDB workaround:** Kuzu does not support rich edge properties, so Graphiti models `RELATES_TO` edges as intermediate nodes (`RelatesToNode_`) with two `RELATES_TO` edges connecting source -> intermediate -> target. This is visible throughout the codebase as Kuzu-specific query branches.

### What works well

- **Comprehensive indexing.** Every temporal field on edges (valid_at, invalid_at, expired_at, created_at) has its own index, enabling efficient temporal queries.
- **Fulltext + vector hybrid.** Both BM25 (fulltext) and cosine similarity indices exist on the same data, enabling the hybrid search modes.
- **Group-id partitioning.** Multi-tenancy is built into every query, not bolted on. The group_id is indexed and filtered at the database level.
- **Provider abstraction.** The driver pattern cleanly separates query generation from execution. Adding a new backend means implementing the operations interface.

### Failure modes

- **KuzuDB intermediate-node pattern is fragile.** Modeling edges as nodes creates 2x the relationships and requires special-case queries throughout the codebase. Every query that touches RELATES_TO has a Kuzu-specific branch. This is maintenance debt.
- **No vector index creation in graph_queries.py.** The fulltext and range indices are explicitly created, but vector indices (for name_embedding and fact_embedding) are not created in the index setup code. This suggests they rely on the graph DB's built-in vector support or external configuration.
- **Entity attributes are stored as flat properties (Neo4j) or JSON string (Kuzu).** In Neo4j, `entity_data.update(self.attributes or {})` flattens attributes into top-level properties. This means attribute keys could collide with reserved keys (uuid, name, etc.) -- the code pops these in `get_entity_node_from_record()` but this is fragile.

### Worth adopting

The comprehensive temporal indexing strategy is worth adopting. Having separate indices on valid_at, invalid_at, expired_at, and created_at enables efficient time-scoped queries. The group_id partitioning pattern is also clean. The KuzuDB intermediate-node pattern is a cautionary tale -- if we use Kuzu (which we do via Cognee), we should expect this kind of workaround.

---

## 6. Retrieval

### Method

Graphiti's search system is highly configurable via `SearchConfig` objects that compose multiple search methods with different reranking strategies.

**Search methods (per entity type):**
- **BM25 fulltext search** -- via database fulltext indices
- **Cosine similarity** -- vector search on embeddings
- **BFS (breadth-first search)** -- graph traversal from origin nodes, with configurable max depth

**Reranking strategies:**
- **RRF (Reciprocal Rank Fusion)** -- merges ranked lists from multiple search methods
- **MMR (Maximal Marginal Relevance)** -- diversity-aware reranking using embeddings
- **Cross-encoder** -- neural reranking using a cross-encoder model
- **Node distance** -- reranks edges by graph distance from a center node
- **Episode mentions** -- reranks by how many episodes reference each edge

**Search scopes:** Edges, nodes, episodes, and communities can each be searched independently or together, each with their own search method and reranker configuration.

**Pre-built recipes** (in `search_config_recipes.py`): `COMBINED_HYBRID_SEARCH_RRF`, `EDGE_HYBRID_SEARCH_NODE_DISTANCE`, `COMBINED_HYBRID_SEARCH_CROSS_ENCODER`, etc.

**File**: `graphiti_core/search/search.py`, `graphiti_core/search/search_utils.py`, `graphiti_core/search/search_config_recipes.py`

### What works well

- **Composable search configuration.** The `SearchConfig` pattern lets callers combine any search methods with any reranker, per entity type. This is far more flexible than a fixed "hybrid search" implementation.
- **BFS from search results.** When BFS is enabled without explicit origin nodes, the system first runs BM25/vector search, then uses the result nodes as BFS origins. This combines semantic relevance with graph-structural proximity.
- **RRF implementation is standard and robust.** Multiple search result lists are merged using reciprocal rank fusion, which is well-understood and parameter-light.
- **Cross-encoder reranking.** The option to use a neural cross-encoder for final reranking is powerful for precision-critical use cases.
- **Candidate over-fetch.** All search methods fetch `2 * limit` candidates before reranking, ensuring good recall.
- **Graph-aware reranking.** Node distance reranker can prioritize facts that are structurally close to a "center node" (e.g., the current user entity), combining semantic and structural relevance.

### Failure modes

- **Episode search is fulltext-only.** Episodes only support BM25 search (no vector similarity). This means semantic episode retrieval is not available -- only keyword matching.
- **BFS depth is limited.** `MAX_SEARCH_DEPTH = 3` is hard-coded in search_utils. For sparse graphs, 3 hops may not reach relevant connected entities.
- **No temporal filtering in search.** The search system does not filter by temporal validity. Expired edges (with `expired_at` set) appear in search results alongside current facts. The caller must filter post-hoc.
- **Cross-encoder is applied to raw fact strings / node names.** There is no rich context about the edge's endpoints or temporal status in the reranking input. The cross-encoder sees "Alice works at Acme Corp" but not that this was invalidated in 2025.

### Worth adopting

Yes -- the composable `SearchConfig` pattern with pluggable search methods and rerankers is the most mature search architecture in any open-source graph system I have analyzed. The BFS-from-search-results technique is particularly clever. The pre-built recipe pattern (named configurations for common use cases) is also a good API design.

---

## 7. Community Detection

### Method

Graphiti implements **label propagation** for community detection in `graphiti_core/utils/maintenance/community_operations.py`.

**Algorithm** (`label_propagation()`):
1. Each node starts as its own community.
2. For each node, count the community labels of its neighbors, weighted by edge count.
3. Each node adopts the plurality community of its neighbors.
4. Ties are broken by taking the larger community.
5. Repeat until convergence (no changes).

**Community building** (`build_community()`):
1. Run label propagation to get clusters.
2. For each cluster, hierarchically summarize member entity summaries using pairwise LLM calls (tournament-style: summarize pairs, then summarize the summaries).
3. Generate a short description/name for the community from the final summary.
4. Create `CommunityNode` with summary and `HAS_MEMBER` edges to member entities.

**Incremental updates** (`update_community()`):
When a new entity is added, find its community by checking:
1. Is it already in a community? Use that one.
2. Find the mode community of its graph neighbors.
3. Merge the new entity's summary with the community summary via pairwise summarization.

### What works well

- **Label propagation is lightweight.** No external library dependency, runs in-process, handles weighted edges (edge count as weight).
- **Incremental community updates.** New entities do not require rebuilding all communities. The mode-of-neighbors heuristic places new entities into existing communities.
- **Hierarchical summarization for community names.** Tournament-style pairwise summary generation produces balanced community descriptions.

### Failure modes

- **Label propagation is non-deterministic for ties.** The algorithm breaks ties by taking `max(community_candidate, curr_community)`, which depends on the arbitrary integer assignment of initial community IDs. Different node orderings can produce different community structures.
- **No community splitting.** If a community grows too large, there is no mechanism to split it. The summary just gets more generic.
- **Pairwise summarization is O(N log N) LLM calls.** Building a community with 32 members requires ~31 summarization calls. This is expensive.
- **Community membership is single.** An entity belongs to exactly one community (the mode of its neighbors). Overlapping communities are not supported.

### Worth adopting

The incremental community update pattern (mode-of-neighbors for placement, pairwise merge for summary update) is worth studying. The label propagation algorithm itself is standard but the integration with LLM summarization is clean. The main risk is LLM cost for large communities.

---

## 8. Temporal Model

### Method

This is Graphiti's signature feature. The temporal model uses multiple timestamps on `EntityEdge`:

- **`valid_at`** -- When the fact became true (event time). Set by LLM during edge extraction, resolved relative to `REFERENCE_TIME`.
- **`invalid_at`** -- When the fact stopped being true (event time). Set by LLM during extraction (if the text indicates an end), OR set by the system during contradiction resolution.
- **`expired_at`** -- When the edge was superseded in the graph (system time). Set by `resolve_edge_contradictions()` when a newer fact contradicts an older one.
- **`created_at`** -- When the edge was first ingested (system time).
- **`reference_time`** -- The timestamp of the episode that produced this edge (preserved for provenance).

On `EpisodicNode`:
- **`valid_at`** -- When the original document was created (event time, passed by caller as `reference_time`).
- **`created_at`** -- When the episode was ingested (system time).

**Contradiction resolution** (`resolve_edge_contradictions()` in `edge_operations.py`):

When a new edge contradicts an existing edge:
1. If the existing edge's `invalid_at` is before the new edge's `valid_at` -- no conflict (the old fact ended before the new one began).
2. If the new edge's `invalid_at` is before the existing edge's `valid_at` -- no conflict (the new fact ended before the old one began).
3. If the existing edge's `valid_at` is before the new edge's `valid_at` -- the new edge invalidates the old one: set `old_edge.invalid_at = new_edge.valid_at` and `old_edge.expired_at = now`.

**LLM-driven contradiction detection** (`resolve_extracted_edge()` in `edge_operations.py`):

The `resolve_edge` prompt receives existing facts and invalidation candidates. The LLM returns:
- `duplicate_facts` -- indices of existing facts that are identical
- `contradicted_facts` -- indices of facts (from either list) that conflict with the new fact

After LLM detection, the system applies temporal logic to determine if the contradiction is actually a temporal supersession. A newer fact (higher `valid_at`) sets `invalid_at` on the older fact.

**Reverse invalidation**: If an invalidation candidate has a LATER `valid_at` than the resolved edge, the resolved edge itself gets `invalid_at` set to the candidate's `valid_at`. This handles out-of-order ingestion -- if you ingest an old fact after a newer one, the old fact is immediately marked as superseded.

### What works well

- **Bitemporal model is principled.** The separation of event time (`valid_at`/`invalid_at`) from system time (`created_at`/`expired_at`) is textbook bitemporal design. This enables both "what was true at time T?" queries and "what did the system know at time T?" queries.
- **Out-of-order ingestion handling.** The reverse invalidation logic (lines 670-683 of edge_operations.py) correctly handles the case where facts are not ingested chronologically. A fact about "Alice worked at Acme (2020-2023)" ingested after "Alice works at NewCo (2024-present)" will still be correctly resolved.
- **LLM + temporal logic hybrid.** The LLM detects semantic contradiction, then the system applies temporal ordering rules. Neither alone would be sufficient -- LLMs are bad at temporal reasoning, and rule-based systems cannot detect semantic equivalence.
- **`reference_time` on edges preserves ingestion context.** Even after temporal resolution, each edge retains the timestamp of its source episode, enabling audit trails.

### Failure modes

- **Temporal resolution depends on LLM-extracted dates.** If the LLM fails to extract `valid_at` or `invalid_at` (returns null), the temporal logic has no data to work with. Null temporal fields disable contradiction resolution -- the `resolve_edge_contradictions()` function skips edges where either `valid_at` is None.
- **No temporal scoping in search.** Search results include both active and expired edges. The caller must filter by `expired_at IS NULL` or check `valid_at`/`invalid_at` ranges. This is a significant gap for "what is currently true?" queries.
- **ISO 8601 parsing is best-effort.** The edge extraction handles `fromisoformat()` with a fallback replace of 'Z' to '+00:00'. Malformed dates are logged and ignored (lines 211-225 of edge_operations.py), meaning temporal metadata is silently lost.
- **Contradiction resolution only applies to semantically related edges.** The system uses embedding search to find invalidation candidates, but if the embedding model does not recognize semantic contradiction (e.g., "Alice lives in NYC" vs "Alice moved to LA"), the contradiction may not be detected.

### Worth adopting

The bitemporal model is Graphiti's strongest contribution and is absolutely worth adopting. The three-timestamp design (`valid_at`, `invalid_at`, `expired_at`) plus `reference_time` gives complete temporal provenance. The LLM + temporal logic hybrid for contradiction resolution is the right approach -- semantic detection followed by temporal ordering rules.

---

## 9. Cross-Reference

### Method

Entities connect across episodes through multiple mechanisms:

1. **Entity deduplication.** When the same entity appears in different episodes, the resolution pipeline merges them into a single node. All episodes then link to the same canonical entity via `MENTIONS` edges.

2. **Episode list on edges.** Each `EntityEdge` has an `episodes: list[str]` field. When a duplicate fact is found, the new episode's UUID is appended to the existing edge's episodes list (line 553 and 611 of edge_operations.py). This creates a many-to-many relationship between edges and episodes.

3. **Episodic edges (`MENTIONS`).** Every entity extracted from an episode gets a `MENTIONS` edge from the episode to the entity. These are created by `build_episodic_edges()`.

4. **Sagas.** Episodes can be grouped into sagas via `HAS_EPISODE` edges. Consecutive episodes in a saga are linked by `NEXT_EPISODE` edges. This creates explicit sequential ordering.

5. **Communities.** Community nodes aggregate entities from multiple episodes via `HAS_MEMBER` edges, providing thematic cross-referencing.

6. **Entity summaries.** Node summaries are incrementally updated across episodes. When an entity is mentioned in a new episode, its summary is re-generated incorporating new information. This creates an implicit temporal cross-reference through the evolving summary.

### What works well

- **Multiple cross-referencing layers.** Entity dedup, edge episode tracking, episodic edges, sagas, and communities provide five distinct mechanisms for connecting information across documents.
- **Saga-based ordering.** The `NEXT_EPISODE` edges maintain explicit ordering, unlike systems that rely only on timestamps. This is important for conversation threads.
- **Episode accumulation on edges.** The `episodes` list on edges enables episode-mentions reranking -- facts supported by many episodes are ranked higher.
- **Incremental summary updates.** Entity summaries are updated with each new episode, creating a living, evolving description that synthesizes information across all sources.

### Failure modes

- **Entity dedup is the single point of cross-reference failure.** If two mentions of the same entity are not deduplicated (e.g., "Dr. Smith" vs "John Smith"), all downstream cross-references are lost. The entities exist as separate nodes with separate episodic edges, separate edges, and potentially separate communities.
- **No explicit provenance for summary updates.** When an entity summary is updated, the previous summary is overwritten. There is no history of summary versions or which episodes contributed which facts to the summary.
- **Saga scope is caller-defined.** The system does not automatically detect which episodes belong together. The caller must explicitly provide a saga name. If different callers use different saga names for related content, cross-referencing breaks.

### Worth adopting

The episode tracking on edges (`episodes` list) is a simple but powerful pattern worth adopting. It enables both provenance tracking and frequency-based relevance. The saga concept (explicit episode sequencing) is useful for conversation-style ingestion.

---

## 10. Standout Techniques

### What Graphiti does better than anything else

**1. Temporal knowledge management.**
No other open-source graph system implements bitemporal validity with automated contradiction resolution at this level. The combination of LLM-detected contradictions with rule-based temporal ordering is a unique contribution. This is not just "add a timestamp" -- it is a principled system for facts that evolve over time.

**2. Three-tier entity resolution cascade.**
The exact -> MinHash/LSH -> LLM pipeline for entity deduplication is more sophisticated than any competitor. The entropy gate for short names, the LSH probabilistic matching, and the fallback to LLM with structured output create a system that is both efficient (avoiding unnecessary LLM calls) and accurate (using LLM judgment for ambiguous cases).

**3. Fact-based edges with episode provenance.**
Storing natural language facts as edge properties (rather than just relation types) AND tracking which episodes support each fact is a design that other systems should copy. This enables both semantic search over relationships and frequency-based relevance scoring.

**4. Composable search architecture.**
The `SearchConfig` pattern with pluggable search methods (BM25, vector, BFS) and rerankers (RRF, MMR, cross-encoder, node distance, episode mentions) is the most flexible retrieval system in any open-source knowledge graph. Pre-built recipes make it accessible while keeping full configurability.

**5. Extraction prompt engineering.**
The extraction prompts are the most carefully engineered I have seen. The extensive negative examples, the anti-pattern lists (never extract pronouns, abstract concepts, bare kinship terms), and the positive examples with reasoning create prompts that produce higher-quality extractions than simpler approaches.

**6. Density-aware chunking.**
The insight that not all large documents need chunking -- only entity-dense ones -- is unique. Most systems chunk unconditionally by token count.

### What Graphiti does NOT do well

**1. No temporal filtering in search.** Despite the sophisticated temporal model, search results include expired and invalid edges. This is a major gap.

**2. No joint entity-edge extraction.** The pipeline approach (entities first, then edges) creates a coupling where edge extraction depends on exact entity name strings from the previous step.

**3. Relation type normalization.** Edge types are free-text SCREAMING_SNAKE_CASE with no enforcement, leading to type proliferation.

**4. Episode search is keyword-only.** No vector similarity for episode retrieval, limiting semantic search over source documents.

---

## Summary for Learning Engine

Graphiti's strongest ideas for potential adoption:

| Technique | Priority | Complexity |
|---|---|---|
| Bitemporal edge model (valid_at/invalid_at/expired_at) | High | Medium |
| Three-tier entity resolution (exact -> LSH -> LLM) | High | High |
| Fact-based edges with episode provenance | High | Low |
| Composable search with pluggable rerankers | Medium | High |
| Density-aware chunking decisions | Medium | Low |
| Negative-example extraction prompting | Medium | Low |
| Episode accumulation on edges | Low | Low |
| Label propagation communities | Low | Medium |
