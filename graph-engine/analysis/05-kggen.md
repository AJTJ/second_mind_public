# KGGen Analysis

Source: [stair-lab/kg-gen](https://github.com/stair-lab/kg-gen) v0.4.0
Evaluated: 2026-04-13

KGGen is a Python library for extracting knowledge graphs from text using LLMs. It uses DSPy for structured LLM interactions and provides a two-phase deduplication pipeline (deterministic + LLM-based) as its core differentiator.

---

## 1. Extraction

### Method

Two-step pipeline architecture: entities first, then relations. Not joint extraction.

**Step 1 — Entity extraction** (`steps/_1_get_entities.py`):
Uses DSPy `Predict` with a `TextEntities` signature. The prompt is minimal — the DSPy signature docstring says:

> "Extract key entities from the source text. Extracted entities are subjects or objects. This is for an extraction task, please be THOROUGH and accurate to the reference text."

When `no_dspy=True`, it uses a richer prompt from `prompts/entities.txt` via LiteLLM with OpenAI structured outputs (JSON schema enforcement). This prompt defines 8 entity categories (People, Organizations, Locations, Dates, Events, Creative Works, Concepts, Products) and 8 extraction guidelines including normalization rules, relationship-potential filtering, and selectivity.

**Step 2 — Relation extraction** (`steps/_2_get_relations.py`):
Takes the entity list as input and extracts (subject, predicate, object) triples. Subjects and objects must come from the entity list. Uses DSPy `Predict` with a signature that says:

> "Extract subject-predicate-object triples from the source text. Subject and object must be from entities list."

When `no_dspy=True`, uses `prompts/relations.txt` — a much more detailed prompt that includes a 5-step analysis process (map entities to text, identify relationships systematically, draft triples, check for isolated entities, finalize). Dynamically creates a Pydantic model with `Literal` types constraining subject/object to the entity list, enforced via OpenAI structured outputs.

**Fallback path**: If entity-constrained extraction fails, it falls back to unconstrained extraction followed by a `FixedRelations` chain-of-thought step that attempts to map free-text subjects/objects back to valid entities.

### What works well

- The `no_dspy=True` path with external prompts is well-designed. The relations prompt's 5-step analysis process (especially step 4, checking for isolated entities) is a concrete technique worth studying.
- Dynamic Literal types for subject/object constrain the LLM to valid entities via structured outputs. This is clever — it makes hallucinated entities structurally impossible when using OpenAI's API.
- The fallback path with `FixedRelations` + `ChainOfThought` gracefully degrades when strict typing fails.
- `parse_relations_response()` has a two-tier parser: strict Pydantic validation first, then raw JSON with entity filtering. Robust against partial LLM failures.

### Failure modes

- The DSPy path (default) uses very thin prompts — just signature docstrings. The good prompts in `prompts/entities.txt` and `prompts/relations.txt` are only used when `no_dspy=True`. Most users will get the weaker extraction.
- Pipeline architecture means entity extraction errors cascade. If an entity is missed in step 1, no relation involving it can be extracted in step 2.
- `_filter_entities()` strips entities containing quote characters (`"`) — a workaround for an OpenAI API limitation. This silently drops entities with quotes in their names.
- No entity typing. Entities are bare strings with no category labels. The entity prompt mentions 8 categories but the output is just a flat list of strings.
- No confidence scores on any extraction.
- Conversation mode has a separate `ConversationEntities` signature but the same thin DSPy prompt with no conversation-specific extraction guidelines.

### Worth adopting

**Yes** — the Literal-type constrained relation extraction via structured outputs, and the fallback chain-of-thought repair pattern. The `prompts/relations.txt` analysis process (especially the isolated entity check) is a good prompt engineering technique. The pipeline architecture itself (entities then relations) is standard but the constraint enforcement is well done.

---

## 2. Chunking

### Method

`utils/chunk_text.py` — NLTK sentence tokenization with character-based chunk size limits. Default 500 characters.

Algorithm:
1. Split text into sentences via `nltk.sent_tokenize()`
2. Accumulate sentences into chunks until character limit
3. If a single sentence exceeds the limit, fall back to word-based splitting

Chunks are processed in parallel via `ThreadPoolExecutor`. Each chunk produces independent entity and relation sets that are unioned together via `set.update()`.

Auto-chunking: if no `chunk_size` is specified and a context length error occurs, the system automatically retries with 16384-character chunks.

### What works well

- Sentence boundary respect prevents mid-sentence splits.
- Fallback to word-level splitting for oversized sentences.
- Auto-chunking on context length errors is user-friendly.
- Parallel chunk processing via ThreadPoolExecutor.

### Failure modes

- No overlap between chunks. Entities and relationships that span chunk boundaries are lost. An entity mentioned in chunk N and related to something in chunk N+1 produces two isolated mentions.
- Character-based sizing, not token-based. A 500-character chunk could be 50 tokens or 200 depending on content. No alignment with model context windows.
- The default 500-character chunk size is very small — roughly 100-125 tokens. This severely limits the context available for extraction.
- Entity sets from different chunks are merged by string equality only. "Barack Obama" from chunk 1 and "Obama" from chunk 2 remain separate until deduplication runs later.
- No semantic awareness in chunking — splits by character count, not by topic or paragraph boundaries.

### Worth adopting

**No** — this is a basic chunker. The auto-retry on context length errors is a nice UX touch but the chunking itself is minimal.

---

## 3. Entity Resolution (Deduplication)

### Method

This is KGGen's primary contribution. Three-tier system in `steps/_3_deduplicate.py`:

**Tier 1 — SEMHASH** (`utils/deduplicate.py`):
Deterministic, no LLM calls. Pipeline:
1. Unicode normalization (NFKC)
2. Singularization via `inflect` library (token-by-token)
3. Semantic hashing via `semhash` library (character n-gram fingerprinting)
4. Self-deduplication at 0.95 similarity threshold

Maintains `original_map` (original -> normalized) and `items_map` (normalized -> original) for bidirectional mapping. Applies to both entities and edges independently.

**Tier 2 — LM_BASED** (`utils/llm_deduplicate.py`):
LLM-powered clustering. Algorithm:

1. **Embed** all entities and edges using a SentenceTransformer model
2. **Build BM25 indices** for keyword matching
3. **KMeans cluster** embeddings into groups of ~128 items each
4. **Within each cluster**, iterate:
   - Pop an item from the cluster
   - Find top-16 relevant items using rank fusion (0.5 * BM25 + 0.5 * embedding cosine similarity)
   - Ask the LLM: "Find duplicate entities for this item and suggest a best alias"
   - Remove identified duplicates from the cluster
   - Continue until cluster is exhausted
5. **Remap** all relations through the resolved entity/edge clusters

The LLM deduplication prompt (`Deduplicate` signature):
> "Find duplicate entities for the item and an alias that best represents the duplicates. Duplicates are those that are the same in meaning, such as with variation in tense, plural form, stem form, case, abbreviation, shorthand. Return an empty list if there are none."

**Tier 3 — FULL** (default):
Runs SEMHASH first to reduce the easy cases, then LM_BASED on the remaining graph.

Cluster processing is parallelized with `ThreadPoolExecutor(max_workers=64)`.

### What works well

- The tiered approach is smart. SEMHASH handles plural/case/normalization cheaply, then LM_BASED handles semantic equivalence. Running SEMHASH first reduces the work for the expensive LLM step.
- Rank fusion retrieval (BM25 + embedding) for candidate selection within clusters is a good hybrid approach. BM25 catches lexical overlap ("USA" / "U.S.A."), embeddings catch semantic similarity ("CEO" / "Chief Executive Officer").
- KMeans pre-clustering bounds the search space. Without it, every item would need to be compared against every other item via LLM calls. The 128-item cluster size is a reasonable balance.
- The LLM picks the canonical alias, not just which items are duplicates. This means the representative name can be the most natural form.
- Relations are remapped through the cluster mappings after deduplication, maintaining graph integrity.
- Entity metadata is preserved and merged through deduplication.
- Both entities AND edges are deduplicated. Edge deduplication is often overlooked — "is_part_of" vs "part of" vs "belongs to" are collapsed.

### Failure modes

- **Cross-cluster misses**: If KMeans places "Barack Obama" in cluster A and "Obama" in cluster B, they will never be compared. The 128-item cluster size makes this more likely for large graphs.
- **Order dependence**: The `while cluster: item = cluster.pop()` iteration means the order items are processed affects which duplicates are found. If "Obama" is processed first and matched with "Barack Obama", great. If "President Obama" is processed first and doesn't match either (because neither is in the top-16 relevant), the duplicate is missed.
- **One-pass only**: Each item is processed exactly once. If the LLM misses a duplicate on the first pass, there's no second chance. No iterative refinement.
- **No transitivity enforcement**: If A is marked as duplicate of B, and B is marked as duplicate of C in a different cluster, A and C are not connected. The cluster isolation prevents transitive closure.
- **LLM alias hallucination**: The `alias` output field lets the LLM choose "the best name to represent the duplicates, ideally from the set." The "ideally" qualifier means the LLM can invent a new name not in the original data.
- **context parameter is TODO**: `context` is accepted by `deduplicate()` but has a `# TODO: implement context` comment. The test for context-dependent disambiguation (`test_clustering_with_context`) passes because LLM outputs happen to be reasonable, but context is not actually forwarded to the LLM prompt.
- **Expensive**: Each entity in the graph requires an LLM call. For a 10K-entity graph with 128-item clusters, that's ~10K LLM calls for entities alone, plus another set for edges.
- **No persistence of resolution decisions**: Cluster mappings are computed fresh each time. There's no way to learn from past resolutions or apply corrections.

### Worth adopting

**Yes, selectively** — the tiered SEMHASH-then-LLM approach is the right architecture. The rank fusion retrieval for candidate selection is a solid technique. The specific implementation has real limitations (cross-cluster misses, no transitivity, order dependence) that would need fixing for production use. The singularization + Unicode normalization + semantic hashing pipeline for the cheap first pass is worth replicating.

---

## 4. Relationships

### Method

Triples are `(subject: str, predicate: str, object: str)` — all strings, stored as tuples in a set. Edges (predicates) are tracked as a separate `set[str]` on the Graph model.

The `Graph` model in `models.py`:
```python
entities: set[str]
edges: set[str]          # unique predicate strings
relations: set[Tuple[str, str, str]]  # (subject, predicate, object)
```

Predicates are free-form strings. The relations prompt suggests specific predicates ("founded_by", "located_in", "works_for") but does not constrain them. The LiteLLM path uses Literal types only for subject/object, not predicate.

### What works well

- Free-form predicates capture nuance that a fixed schema would miss.
- Edge deduplication treats predicates as first-class citizens, normalizing "likes"/"like"/"liking" through the same pipeline as entities.
- The `Graph.from_file()` method auto-repairs: if a relation references an entity not in the entities set, it adds it.

### Failure modes

- No relationship typing or categorization. "founded_by" and "created" might refer to the same semantic relationship but are treated as completely different edges.
- No relationship attributes — no confidence, no source text reference, no temporal qualifier.
- Relations are stored in a set, which means duplicate triples are silently dropped (which is actually fine) but also means identical (s, p, o) from different sources/chunks cannot be tracked separately.
- Predicate explosion: without constraints, the LLM generates many near-synonymous predicates that the edge deduplication must clean up after the fact.
- Directionality guidance is in the prompt but not enforced. Nothing prevents (Steve Jobs, founded_by, Apple) from being emitted.

### Worth adopting

**Partially** — the triple format is standard and works. Edge deduplication as a first-class operation is worth copying. But the lack of relationship attributes (source, confidence, temporal) limits usefulness for a production knowledge graph.

---

## 5. Storage

### Method

KGGen is extraction-first. No embedded database.

Storage options:
- **JSON export**: `export_graph()` serializes to JSON with entities, relations, edges, and cluster mappings.
- **Neo4j integration**: `utils/neo4j_integration.py` provides `Neo4jUploader` that creates `:Entity` nodes with `name` property and typed relationships (predicate becomes the relationship type, uppercased with underscores).
- **MCP server** (`mcp/server.py`): Persists to a JSON file (`kg_memory.json`). Loads on startup, saves after each `add_memories` call.

### What works well

- Clean separation of concerns. KGGen extracts, you choose where to store.
- Neo4j integration handles the Cypher correctly, using MERGE to avoid duplicates.
- Graph model is Pydantic-based, so serialization is straightforward.

### Failure modes

- The MCP server's persistence is a flat JSON file rewritten on every operation. No concurrent access safety, no atomic writes, no backup.
- Neo4j uploader creates relationships one at a time in a loop (`for subject, predicate, obj in graph.relations`), not batched. Extremely slow for large graphs.
- No built-in vector storage. If you want embedding-based retrieval, you must manage that externally or use the in-memory `retrieve()` method.
- The JSON export loses set ordering (converted to lists) and tuple structure (relations become nested arrays).

### Worth adopting

**No** — the storage is minimal and not the point of this library. The Neo4j integration is a useful reference for Cypher patterns but nothing to build on.

---

## 6. Retrieval

### Method

Basic embedding-based retrieval in `KGGen.retrieve()`:

1. Encode all nodes and relation types via SentenceTransformer (`generate_embeddings()`)
2. For a query, compute cosine similarity against all node embeddings
3. Take top-k nodes
4. For each top node, walk the graph to depth 2 (both incoming and outgoing edges) collecting natural-language triple strings ("X rel Y.")
5. Return the union of all context strings

The MCP server's `retrieve_relevant_memories()` is even simpler — substring matching (`query_lower in e.lower()`).

### What works well

- Graph walk from retrieved nodes provides structured context, not just isolated matches.
- Both incoming and outgoing edges are traversed, so the retrieval is direction-agnostic.
- The depth parameter controls context scope.

### Failure modes

- **No index**: `retrieve_relevant_nodes()` computes cosine similarity against every node in a Python loop. O(n) per query with no approximate nearest neighbor search.
- Embeddings are computed fresh each time unless the caller caches them.
- Context is a flat set of strings with no ranking or relevance scoring of individual triples.
- The MCP retrieval is substring matching — completely ignores the embedding-based retrieval that exists in the core library. This looks like an oversight.
- No hybrid retrieval (BM25 + embedding) for queries, even though the deduplication code already implements rank fusion.
- `retrieve_context()` uses recursive graph walking with no cycle detection. `context` is a set so results are deduplicated, but the function could traverse the same paths repeatedly in cyclic graphs.

### Worth adopting

**No** — the retrieval is a proof of concept. The graph-walk-from-seed-nodes pattern is standard. The implementation lacks indexing, caching, and ranking.

---

## 7. Community Detection

### Method

None. KMeans clustering in the deduplication pipeline groups similar items for efficiency, not for community detection. The `entity_clusters` on the Graph model represent deduplication groups (which original names map to which canonical name), not semantic communities.

The visualization code (`visualize_kg.py`) computes connected components for display purposes but this is not exposed as a feature.

### Worth adopting

**No** — nothing here.

---

## 8. Temporal

### Method

None. No timestamps on entities, relations, or extraction events. The entity prompt mentions "Dates and Time Periods" as an entity category, so temporal entities can be extracted (e.g., "2024" as an entity node), but there is no temporal modeling of when facts are true or when they were extracted.

### Worth adopting

**No** — nothing here.

---

## 9. Cross-Reference

### Method

The `aggregate()` method on KGGen merges multiple Graph objects:

```python
def aggregate(self, graphs: list[Graph]) -> Graph:
    all_entities = set()
    all_relations = set()
    all_edges = set()
    for graph in graphs:
        all_entities.update(graph.entities)
        all_relations.update(graph.relations)
        all_edges.update(graph.edges)
    return Graph(entities=all_entities, relations=all_relations, ...)
```

The MCP server uses this: each `add_memories` call generates a new graph and aggregates it with the existing memory graph.

Cross-reference between extraction runs happens only if `deduplicate()` is called after aggregation. Without deduplication, "Barack Obama" from run 1 and "Obama" from run 2 remain separate nodes.

The `entity_metadata` field (a `dict[str, set[str]]`) allows tracking provenance — which source texts an entity came from — but this is only set by external callers, not by the extraction pipeline itself.

### What works well

- Set-based aggregation naturally deduplicates exact string matches.
- The deduplication pipeline, when applied after aggregation, handles cross-run entity resolution.
- `entity_metadata` merging through deduplication preserves provenance.

### Failure modes

- The MCP server's `add_memories` aggregates but does NOT deduplicate afterward. Duplicates accumulate with every call.
- No incremental deduplication. To resolve entities across runs, you must re-deduplicate the entire graph.
- No source tracking is built into the extraction pipeline. You'd need to manually populate `entity_metadata`.

### Worth adopting

**Partially** — the aggregate-then-deduplicate pattern is the right idea. But the lack of incremental resolution and the MCP server's missing deduplication call show this isn't production-ready.

---

## 10. Standout Techniques

### 1. Tiered Deduplication (the signature feature)

The SEMHASH -> LLM pipeline is the best idea in this codebase. Cheap deterministic normalization handles the easy 80% (plurals, case, Unicode), then expensive LLM calls handle semantic equivalence. This avoids wasting LLM tokens on "cats" vs "cat" while still catching "CEO" vs "Chief Executive Officer."

**Concrete numbers from the test suite**: The semhash step at 0.95 threshold catches plurals (cat/cats), case variations (Person/person/PERSON), but correctly preserves true synonyms (happy/joyful, CEO/Chief Executive Officer) for the LLM step.

### 2. Rank Fusion for Candidate Retrieval

The `get_relevant_items()` method combines BM25 (lexical) and embedding similarity (semantic) with equal 0.5/0.5 weighting. This is used within deduplication clusters to find candidate duplicates. BM25 catches abbreviation overlaps that embeddings miss; embeddings catch semantic equivalence that BM25 misses.

### 3. Literal-Type Constrained Extraction

The dynamic Pydantic model creation in `_create_relations_model()` that constrains subject/object to a `Literal[tuple(entities)]` type, enforced through OpenAI structured outputs, is a practical technique for ensuring relation extraction stays grounded in the entity list. The graceful fallback through `parse_relations_response()` handles cases where the LLM can't satisfy the constraints.

### 4. Edge Deduplication as First-Class Operation

Most knowledge graph systems focus on entity resolution and ignore predicate normalization. KGGen runs the full deduplication pipeline on edges too, collapsing "likes"/"like"/"liking" and "manages"/"oversees"/"supervises" into canonical forms. This significantly reduces predicate sprawl.

---

## Summary Assessment

| Area | Quality | Worth adopting |
|---|---|---|
| Extraction | Good (especially no_dspy path) | Yes — constrained extraction + fallback |
| Chunking | Basic | No |
| Entity Resolution | Strong architecture, implementation gaps | Yes — tiered SEMHASH+LLM, rank fusion |
| Relationships | Standard triples, no attributes | Partially — edge dedup pattern |
| Storage | Minimal, not the focus | No |
| Retrieval | Proof of concept | No |
| Community Detection | None | No |
| Temporal | None | No |
| Cross-Reference | Aggregate + deduplicate pattern | Partially |

**Bottom line**: KGGen is an extraction and deduplication library, not a full knowledge graph system. Its deduplication pipeline (SEMHASH normalization -> KMeans clustering -> rank fusion candidate selection -> LLM-based resolution) is its genuine contribution and the part most worth studying. The extraction is competent but the best prompts are behind a non-default flag. Everything else (storage, retrieval, temporal, community) is either minimal or absent.

For the learning engine's graph-engine: the tiered deduplication architecture and edge normalization patterns are directly applicable. The extraction prompts (especially the relations prompt with isolated-entity checking) are worth adapting. The cross-cluster miss problem and lack of transitive resolution are the main gaps to solve if adopting this approach.
