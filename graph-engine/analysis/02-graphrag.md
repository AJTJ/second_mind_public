# GraphRAG Analysis

Source: `~/references/graph-systems/graphrag/` (Microsoft GraphRAG, monorepo with packages/)

Evaluated: 2026-04-13

---

## 1. Extraction

**Method:** LLM-based joint entity+relationship extraction in a single prompt call per text chunk. The extraction prompt (`packages/graphrag/graphrag/prompts/index/extract_graph.py`) asks the LLM to:

1. Identify all entities matching a configurable type list (e.g., `ORGANIZATION,PERSON,GEO`)
2. Identify all (source, target) relationship pairs among extracted entities
3. Return both in a single structured text block using `<|>` tuple delimiters and `##` record delimiters

The `GraphExtractor` class (`index/operations/extract_graph/graph_extractor.py`) parses the LLM output with regex, splitting on `##` then `<|>`. Entity names are uppercased and cleaned. Relationship weights are parsed as floats (defaulting to 1.0 on failure).

**Gleaning (self-reflection pass):** Configurable via `max_gleanings`. After the initial extraction, the extractor sends `CONTINUE_PROMPT` ("MANY entities and relationships were missed...") followed by `LOOP_PROMPT` ("Answer Y if there are still entities..."). This loops up to `max_gleanings` times, accumulating results. The model can exit early by answering "N". Each gleaning round appends to the same conversation context.

**Entity types:** Fully configurable via `config.extract_graph.entity_types`. Defaults are typically `ORGANIZATION, PERSON, GEO, EVENT` but the user sets them. There is no ontology enforcement -- the LLM can return any type string it wants, and the code just uppercases it.

**NLP fast path:** An alternative `extract_graph_nlp` workflow uses noun-phrase extraction (no LLM calls) to build a co-occurrence graph. Entities are all typed `NOUN PHRASE` with empty descriptions. This is the "Fast" indexing method -- much cheaper but no semantic relationships.

**Covariates/Claims:** A separate `extract_claims` prompt extracts structured claims with subject, object, claim type, status (TRUE/FALSE/SUSPECTED), date ranges (ISO-8601), and source quotes. This is a distinct pipeline step, not joint with entity extraction.

**What works well:**
- Joint entity+relationship extraction in one prompt avoids the alignment problem of separate passes
- Gleaning is a practical technique for recall improvement -- the "MANY entities were missed" framing exploits LLM tendency to comply with pressure
- Custom delimiters (`<|>`, `##`) are more robust than JSON parsing for streaming LLM output
- Orphan relationship filtering (`filter_orphan_relationships` in `utils.py`) catches hallucinated entity names in relationships that have no corresponding entity row
- Entity names are uppercased at parse time, providing basic normalization

**Failure modes:**
- Regex parsing is brittle -- if the LLM deviates from the exact format (extra parens, missing delimiters), records are silently dropped
- No validation that entity types match the requested types -- the LLM can invent types freely
- Relationship weights are LLM-hallucinated integers (1-10 scale in examples) with no calibration -- "strength 9" from one chunk is not comparable to "strength 9" from another
- Entity descriptions are single strings from one extraction call, not accumulated -- the summarization step handles merging later but it's an extra LLM call per entity
- Gleaning adds 2 LLM calls per round (continue + loop check) which gets expensive. No evaluation of whether gleaning actually finds new entities vs. repeating existing ones

**Worth adopting:** YES -- the gleaning pattern specifically. The `CONTINUE_PROMPT`/`LOOP_PROMPT` cycle is cheap to implement and addresses the known problem of LLMs stopping extraction early. The orphan relationship filter is also a good defensive practice.

---

## 2. Chunking

**Method:** Two chunker implementations in `packages/graphrag-chunking/`:

1. **TokenChunker** (default): Fixed-size token-based chunking with configurable overlap. Encodes the full text to tokens, then slides a window of `size` tokens with `overlap` token overlap. Default: **1200 tokens, 100 overlap**.

2. **SentenceChunker**: Splits on sentence boundaries using NLTK's `sent_tokenize`. No overlap -- each sentence is its own chunk.

Configuration in `ChunkingConfig`:
- `type`: `tokens` (default) or `sentences`
- `size`: chunk size in tokens (default 1200)
- `overlap`: overlap in tokens (default 100)
- `encoding_model`: tokenizer to use
- `prepend_metadata`: list of metadata field names from the source document to prepend to each chunk (e.g., document title)

**What works well:**
- Token-based chunking is deterministic and reproducible
- The metadata prepend feature is clever -- prepending document title/source to each chunk gives the LLM extraction context about what document a chunk came from
- 1200 tokens default is reasonable for extraction quality (small enough to focus, large enough for context)
- Configurable overlap prevents entity mentions from being split across chunk boundaries

**Failure modes:**
- No semantic awareness -- chunks can split mid-paragraph, mid-sentence, or mid-entity-description
- SentenceChunker has no size limit, so a single long sentence becomes one chunk (can be very small or very large)
- TokenChunker overlap is fixed, not adaptive -- the same 100-token overlap regardless of whether the boundary falls in meaningful text
- No special handling for structured content (tables, lists, headers) -- everything is flat text
- The chunking happens before extraction, so extraction quality depends heavily on chunk boundaries. An entity described across two chunks may get extracted twice with partial descriptions

**Worth adopting:** PARTIALLY -- the metadata prepend pattern is worth stealing. The fixed token chunking itself is bog-standard. For a knowledge graph system, semantic chunking (paragraph/section-aware) would be better.

---

## 3. Entity Resolution

**Method:** GraphRAG handles entity resolution through two mechanisms:

1. **Name-based merging at extraction time** (`_merge_entities` in `extract_graph.py`): After all chunks are processed, entities are grouped by `(title, type)`. Descriptions are collected into a list, text_unit_ids are aggregated, and frequency is counted. This is a simple exact-match deduplication on uppercased names.

2. **Description summarization** (`summarize_descriptions/`): The `SummarizeExtractor` takes the list of descriptions for each entity and calls the LLM to produce a single coherent summary. The prompt asks to "resolve contradictions" and "include information from all descriptions." It handles token limits by batching descriptions -- if descriptions exceed `max_input_tokens`, it summarizes in rounds, feeding the intermediate summary back in.

There is **no** embedding-based entity resolution, no fuzzy name matching, no clustering of similar entities. "MICROSOFT" and "MICROSOFT CORP" would be treated as separate entities. "Microsoft" and "MICROSOFT" are merged because names are uppercased.

**What works well:**
- The summarization step is elegant -- it doesn't just concatenate descriptions but asks the LLM to resolve contradictions and synthesize
- The iterative summarization with token budgeting handles entities that appear in many chunks
- Uppercasing provides basic normalization
- Single-description entities skip the LLM call entirely (optimization in `SummarizeExtractor.__call__`)

**Failure modes:**
- No fuzzy matching at all -- abbreviations, aliases, typos, and name variations create duplicate entities
- No embedding-based similarity to detect that "UN" and "United Nations" are the same entity
- No coreference resolution -- "he", "the company", "the organization" are never linked back
- The groupby on `(title, type)` means "APPLE" as ORGANIZATION and "APPLE" as PRODUCT are separate entities even if they're the same thing mentioned in different contexts
- No human-in-the-loop or confidence scoring for entity merges
- The summarization cost scales linearly with the number of unique entities -- every entity with 2+ descriptions gets an LLM call

**Worth adopting:** YES -- the iterative description summarization pattern. The approach of collecting all descriptions, then having the LLM synthesize a coherent summary while resolving contradictions is much better than just taking the first or longest description. The token-budgeted batching is a practical detail worth copying.

---

## 4. Relationships

**Method:** Relationships are extracted alongside entities in a single prompt. Each relationship has:
- `source`: entity name (uppercased)
- `target`: entity name (uppercased)
- `description`: free-text explanation of why the entities are related
- `weight`: LLM-assigned numeric strength score
- `source_id`: text unit ID (provenance)

After extraction, relationships are merged by `(source, target)` pair: descriptions are collected into a list, text_unit_ids aggregated, weights summed. The description list is later summarized by the LLM (same as entities).

Relationships are **generic and untyped** -- there are no predicate labels like "WORKS_FOR" or "LOCATED_IN". The description field carries the semantic meaning. They are **directional** in the data model (source/target) but the graph is treated as **undirected** for community detection (the `cluster_graph.py` normalizes direction by sorting source/target).

Relationship **pruning** (`prune_graph.py`) supports:
- Minimum edge weight percentile filtering (default: 40th percentile)
- Ego node removal (highest degree node)
- Degree-based node removal
- Frequency-based node removal
- Largest connected component extraction

**What works well:**
- Free-text descriptions on edges are more expressive than fixed predicates -- they capture nuance that "WORKS_FOR" would lose
- Weight summing across chunks naturally upweights frequently co-mentioned entity pairs
- The pruning operations provide meaningful control over graph density
- Orphan relationship filtering catches edges to non-existent entities

**Failure modes:**
- No relationship types means you can't query "all employment relationships" or filter by predicate
- Directional semantics are lost when the graph is made undirected for Leiden clustering
- LLM-assigned weights are unreliable and uncalibrated -- summing them across chunks amplifies noise
- Duplicate relationships between the same pair in different directions (A->B and B->A) are not merged if the extraction produces both (the merge is on exact (source, target) pair)
- No temporal attributes on relationships -- "worked at Company X from 2010-2015" is just a description string
- The description summarization for relationships is identical to entities, but relationship descriptions need different treatment (preserving directionality, temporal context)

**Worth adopting:** YES -- the free-text edge descriptions are the right call for knowledge graphs built from unstructured text. Fixed predicates lose too much information. The weight-summing across chunks is a simple but effective signal for relationship importance.

---

## 5. Storage

**Method:** GraphRAG stores all data as **Parquet files** via a pluggable `TableProvider` abstraction. The storage layer (`packages/graphrag-storage/`) supports:
- File system storage
- Azure Blob Storage
- Azure Cosmos DB
- In-memory storage

Tables are written as individual Parquet files (`entities.parquet`, `relationships.parquet`, `communities.parquet`, `community_reports.parquet`, `text_units.parquet`, etc.). The `ParquetTableProvider` reads/writes via pandas DataFrames.

There is also a `CsvTableProvider` as an alternative. Both support streaming row operations via a `Table` abstraction (read rows async, write rows one at a time).

The graph itself is **not stored in a graph database**. It's a set of flat tables. NetworkX is used only transiently during community detection. GraphML snapshots can be exported optionally.

**Isolation:** No multi-tenant or dataset-level isolation in the storage layer. Each indexing run produces one set of output tables. Different datasets would need separate output directories.

**What works well:**
- Parquet is efficient, columnar, and self-describing -- good for analytical queries on entities/relationships
- The pluggable storage abstraction means you can run locally or in cloud without code changes
- Streaming row operations via the `Table` abstraction avoid materializing entire tables in memory
- GraphML export provides interoperability with graph visualization tools

**Failure modes:**
- No graph database means no efficient graph traversal queries (e.g., "find all paths between A and B")
- No incremental updates to individual entities -- the entire table must be rewritten (though update workflows exist)
- No ACID transactions -- concurrent writes could corrupt tables
- Parquet files must be fully read into memory for queries (no predicate pushdown in the current implementation)
- No dataset isolation -- all data goes to one output location

**Worth adopting:** PARTIALLY -- Parquet as the storage format is pragmatic for batch pipelines. The `Table` streaming abstraction is nice. But for a live knowledge graph system, you need a real graph store (which is what Cognee does with KuzuDB).

---

## 6. Retrieval

GraphRAG has **four search modes**:

### Local Search
**Method:** Entity-centric retrieval. Maps the query to entities via embedding similarity (`map_query_to_entities`), then builds a context window from:
- Community reports (25% of token budget) -- reports for communities the matched entities belong to
- Entity/relationship tables (25% of token budget) -- descriptions and connections
- Source text units (50% of token budget) -- original text chunks where entities appeared

The context is formatted as data tables and passed to an LLM with the query.

### Global Search (Map-Reduce)
**Method:** Community-report-based retrieval. Operates in two phases:
1. **Map:** Each community report is sent to the LLM with the query. The LLM returns JSON `{"points": [{"description": "...", "score": 0-100}]}`. Runs in parallel across all report batches.
2. **Reduce:** Points with score > 0 are sorted by score descending, packed into a token budget (default 8000), and sent to a final LLM call that synthesizes the answer.

**Dynamic community selection** (optional): Instead of scanning all reports, walks the community hierarchy top-down. Rates each community report's relevance via LLM, only descends into children of relevant communities. Prunes irrelevant subtrees.

### DRIFT Search (Decompose, Retrieve, Infer, Follow-up, Transform)
**Method:** Iterative refinement search combining global and local. Steps:
1. **Primer:** Uses `PrimerQueryProcessor` to expand the query via HyDE (hypothetical document embedding) using a random community report as template. Embeds the expansion and finds top-k similar community reports by cosine similarity.
2. **Decompose:** Sends top-k reports to LLM to get intermediate answers + follow-up queries + confidence scores.
3. **Iterative refinement:** For `n_depth` iterations, takes the highest-ranked incomplete follow-up queries and runs them through `LocalSearch`. Each round produces new intermediate answers and more follow-ups.
4. **Reduce:** All intermediate answers are combined into a final response.

### Basic Search
**Method:** Standard vector RAG -- embeds the query, retrieves similar text chunks, passes them as context to LLM. No graph structure used.

**What works well:**
- The local search token budget allocation (community/entity/text proportions) is a thoughtful design that balances abstraction levels
- Global search's map-reduce is the right pattern for corpus-wide questions -- it's the only way to answer "what are the main themes" without retrieving every document
- DRIFT's iterative refinement is sophisticated -- it can drill down from high-level community summaries to specific entities
- Dynamic community selection avoids scanning all reports for global queries
- All search modes support streaming responses

**Failure modes:**
- **Empty results are common in global search** -- if all map responses score 0, you get a canned "I do not know" response. This happens when community reports don't mention the query topic at all.
- Local search depends entirely on `map_query_to_entities` finding the right entities. If the query doesn't match entity description embeddings, you get irrelevant context.
- DRIFT is extremely expensive -- primer (1 LLM call for HyDE + 1 embedding), decomposition (k LLM calls), then n_depth * k_followups local searches (each with its own LLM call). For default settings, easily 10-20+ LLM calls per query.
- Global search costs scale linearly with the number of communities -- every community report gets a map LLM call unless dynamic selection is enabled.
- No hybrid search -- you pick one mode. There's no automatic routing.
- Basic search ignores the graph entirely -- it's just vanilla RAG included for baseline comparison.
- The `map_query_to_entities` function uses entity description embeddings, not entity names. If an entity has a poor description, it won't match relevant queries.

**Worth adopting:** YES -- the global search map-reduce pattern and the dynamic community selection. These solve the "how do I answer questions about the whole corpus" problem that pure vector search can't handle. DRIFT is interesting but the cost may be prohibitive for personal use.

---

## 7. Community Detection

**Method:** Hierarchical Leiden clustering via `graspologic_native` (Rust implementation). The pipeline:

1. Edge normalization: sort source/target to make undirected, deduplicate
2. Optional LCC extraction (restrict to largest connected component)
3. Run `hierarchical_leiden` with parameters:
   - `max_cluster_size`: maximum community size (configurable, drives hierarchy depth)
   - `resolution`: 1.0 (fixed)
   - `randomness`: 0.001 (near-deterministic)
   - `use_modularity`: True
   - `iterations`: 1
4. Produces a list of `HierarchicalCluster` objects with `(node, cluster, level, parent_cluster, is_final_cluster)`
5. Communities are organized into levels with parent-child relationships

**Community report generation** (`prompts/index/community_report.py`): For each community, the system:
1. Collects all entities and relationships in the community
2. Formats them as CSV-like tables (entity: id, title, description; relationship: id, source, target, description)
3. Sends to LLM with a detailed prompt requesting: TITLE, SUMMARY, IMPACT SEVERITY RATING (0-10), RATING EXPLANATION, and DETAILED FINDINGS (5-10 key insights)
4. Output is structured JSON with grounding rules (data references like `[Data: Entities (5, 7)]`)

**Hierarchy levels:** Determined automatically by `max_cluster_size`. Smaller max = more levels. Each level produces its own set of communities. Entities can belong to communities at every level. Parent-child relationships are tracked.

**What works well:**
- Hierarchical communities provide multi-resolution views of the knowledge graph -- you can answer questions at different granularity levels
- The community report format is well-structured (title, summary, findings with data references) -- it's essentially pre-computed answers about graph neighborhoods
- `graspologic_native` (Rust) makes Leiden fast even on large graphs
- The community report prompt enforces grounding -- claims must reference specific entity/relationship IDs
- Impact severity ratings provide a built-in importance signal for community selection
- The `max_cluster_size` parameter gives users control over granularity

**Failure modes:**
- **Cost is the biggest issue.** Every community gets an LLM call for report generation. A graph with 1000 communities = 1000 LLM calls just for reports, plus the extraction and summarization costs.
- Fixed resolution (1.0) and randomness (0.001) mean the clustering is not tunable for different graph structures -- dense vs. sparse graphs need different parameters
- Only 1 iteration of Leiden -- multiple iterations could improve partition quality
- The "fast" pipeline uses `create_community_reports_text` which skips LLM-based reports entirely, suggesting the cost is acknowledged as prohibitive
- Community reports become stale when the graph is updated -- the update pipeline must regenerate reports for affected communities
- The report prompt is biased toward investigative/compliance framing ("legal compliance, technical capabilities, reputation, noteworthy claims") -- it doesn't adapt to domain
- Communities with very few entities produce thin, low-value reports

**Worth adopting:** YES -- this is GraphRAG's signature contribution and the most valuable technique in the entire system. The combination of hierarchical Leiden + LLM-generated community summaries creates a pre-computed answer index that enables corpus-wide queries. The specific implementation of community reports with structured JSON, grounding rules, and impact ratings is well-designed. The cost is real but the capability is unique.

---

## 8. Temporal Handling

**Method:** Minimal. The `CommunityReport` data model has an optional `period` field. The `create_communities` workflow sets `period` to the current UTC date (`datetime.now(timezone.utc).date().isoformat()`). This is a snapshot timestamp, not temporal reasoning.

The `extract_claims` prompt extracts `claim_date` as ISO-8601 date ranges (`start_date`, `end_date`), which is the most temporal awareness in the system.

Entity and relationship data models have an `attributes` dict that could hold temporal data, but nothing populates it.

The update workflows (`update_entities_relationships`, `update_communities`, etc.) handle incremental additions but don't version or timestamp individual entities/relationships.

**What works well:**
- Claims extraction captures temporal ranges (start/end dates)
- The period field on communities enables basic temporal partitioning

**Failure modes:**
- No temporal reasoning during extraction -- "Company X acquired Company Y in 2020" creates a relationship with no time context (it's in the description string only)
- No entity versioning -- an entity's description reflects all mentions across time, mixing past and present
- No temporal decay or relevance weighting -- old information has the same weight as new
- The community period is just a creation timestamp, not a content-based time range

**Worth adopting:** NO -- temporal handling is essentially absent. The claims extraction date range is the one useful pattern, but it's a separate pipeline from entity/relationship extraction.

---

## 9. Cross-Reference (Cross-Document Entity Linking)

**Method:** Entities are merged across documents through the extraction pipeline:

1. Each text chunk produces its own entity extractions with `source_id` tracking which chunk it came from
2. `_merge_entities` groups by `(title, type)` across all chunks from all documents, collecting `text_unit_ids` (provenance list)
3. `_merge_relationships` groups by `(source, target)` across all chunks, collecting all `text_unit_ids` and summing weights
4. Description summarization produces a single coherent description from all mentions

This means an entity mentioned in Document A and Document B automatically gets linked because the uppercase name matches. The `text_unit_ids` list provides full provenance back to the source chunks.

Community detection then connects entities from different documents into the same community if they share relationships, creating implicit cross-document connections.

**What works well:**
- Automatic cross-document linking via name matching is simple and works for well-known entities
- Full provenance tracking (text_unit_ids on every entity and relationship) enables tracing back to source documents
- Community detection creates emergent cross-document themes -- entities from different papers that share common connections end up in the same community
- Weight summing on relationships amplifies cross-document co-mentions

**Failure modes:**
- Cross-document linking depends entirely on exact name matches (after uppercasing) -- no embedding-based matching
- Domain-specific terms that appear in multiple documents with different meanings get falsely merged (e.g., "TRANSFORMER" in an electrical engineering doc vs. an ML doc)
- No document-level metadata on entities -- you can trace back via text_unit_ids but there's no first-class "this entity appears in documents X, Y, Z" field
- No cross-reference confidence scoring

**Worth adopting:** PARTIALLY -- the provenance tracking via text_unit_ids is essential and worth copying. The weight-summing as a cross-document importance signal is useful. But the lack of fuzzy entity matching means cross-document linking only works for entities with identical names.

---

## 10. Standout Techniques

### 1. Community Reports as Pre-Computed Answers (THE differentiator)
GraphRAG's core insight: instead of answering corpus-wide questions at query time by reading everything, pre-compute summaries of graph neighborhoods. Each community report is essentially a cached answer to "what's going on in this part of the knowledge graph." This enables global search (map-reduce over community reports) that would be impossible with pure vector retrieval. No other open-source system does this.

### 2. Map-Reduce Search Over Graph Summaries
The global search pattern -- fan out LLM calls to each community report, score relevance, sort by importance, pack into a reduce context, synthesize -- is a production-quality implementation of multi-hop question answering over large corpora. The score-based filtering (drop score=0) and token-budgeted packing are practical details that matter.

### 3. Dynamic Community Selection
Walking the community hierarchy top-down, using LLM calls to prune irrelevant subtrees, is an efficient alternative to scanning all reports. It's like a semantic B-tree index. The fallback logic (if no relevant communities found at level N, try all communities at level N+1) handles edge cases well.

### 4. DRIFT's Iterative Decomposition
The primer -> decompose -> local search -> follow-up cycle is the most sophisticated query strategy in any open-source graph RAG system. Using HyDE to expand queries with community report templates as structure guidance is clever. The follow-up query generation creates a breadth-first exploration of the knowledge graph guided by relevance scoring.

### 5. Gleaning for Extraction Recall
The multi-turn extraction pattern (extract -> "you missed many" -> extract more -> "any more? Y/N") is simple, effective, and widely applicable to any LLM extraction task. It exploits the LLM's tendency to comply with prompts suggesting incompleteness.

### 6. Description Summarization with Contradiction Resolution
Rather than just concatenating or picking the best entity description, having the LLM synthesize a single coherent description while resolving contradictions is the right approach. The iterative token-budgeted summarization handles entities mentioned hundreds of times.

---

## Summary: What to Adopt for the Learning Engine

| Technique | Priority | Effort | Notes |
|---|---|---|---|
| Community reports as pre-computed answers | HIGH | HIGH | Core value proposition. Requires Leiden + report generation pipeline. |
| Gleaning for extraction recall | HIGH | LOW | Just add continue/loop prompts to existing extraction. |
| Description summarization with contradiction resolution | HIGH | MEDIUM | Better entity descriptions = better retrieval. |
| Free-text edge descriptions (not fixed predicates) | HIGH | LOW | Already doing this in Cognee? Verify. |
| Provenance tracking (text_unit_ids) | HIGH | LOW | Essential for traceability. |
| Global search map-reduce | MEDIUM | HIGH | Only valuable with community reports in place. |
| Dynamic community selection | MEDIUM | MEDIUM | Optimization for global search. |
| Metadata prepend on chunks | MEDIUM | LOW | Cheap improvement to extraction context. |
| Orphan relationship filtering | LOW | LOW | Defensive, easy to add. |
| DRIFT iterative search | LOW | HIGH | Expensive, complex, marginal benefit for personal use. |

---

## Key Metrics

- **Extraction**: 1+ LLM call per chunk (+ 2 per gleaning round)
- **Summarization**: 1 LLM call per entity with 2+ descriptions, 1 per relationship with 2+ descriptions
- **Community reports**: 1 LLM call per community
- **Global search**: 1 LLM call per community report batch + 1 reduce call
- **Local search**: 1 LLM call for answer generation
- **DRIFT search**: 10-20+ LLM calls per query
- **Storage**: Parquet files (no graph database)
- **Graph library**: graspologic_native (Rust) for Leiden, NetworkX transiently
