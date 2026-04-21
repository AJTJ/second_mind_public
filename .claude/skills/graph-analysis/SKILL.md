---
name: graph-analysis
description: Analyze a knowledge graph system (like Cognee) for failure modes in entity extraction, relationship creation, and retrieval. Identifies where connections get lost or never form. Use when evaluating graph backends or designing extraction pipelines.
user_invocable: true
argument-hint: "<system-or-codebase-to-analyze>"
---

# Graph Analysis: Connection Failure Modes

Analyze a knowledge graph system to find where it fails to create, store, or retrieve meaningful connections between entities. The goal is to produce actionable findings that inform the design of a better system.

## Methodology

Work through these layers in order. Each layer feeds the next.

### 1. Extraction Analysis

How does the system turn raw text into entities and relationships?

Investigate:
- What prompt or model drives extraction? Read it.
- What entity types does it produce? Are they too generic ("thing", "concept") or too specific?
- Does it extract relationships explicitly, or only entities? Many systems extract nodes but not edges — the graph has dots but no lines.
- Does it preserve numeric data, dates, and proper nouns as discrete entities, or collapse them into summaries?
- Does the extraction prompt vary by input type, or is it one-size-fits-all?

Test by tracing a specific document through the pipeline. What goes in, what entities come out, what's lost?

### 2. Chunking Analysis

How does the system split documents before extraction?

Investigate:
- What's the chunk size? Too large = entities from different topics get co-located. Too small = relationships that span paragraphs are severed.
- Does chunking respect document structure (headings, paragraphs, code blocks)?
- Is there chunk overlap? Without overlap, entities at chunk boundaries get split.
- Can the user configure chunk strategy, or is it fixed?

### 3. Entity Resolution

How does the system handle the same concept appearing in different documents?

Investigate:
- If Document A mentions "copper" and Document B mentions "Cu" or "copper metal", do they become the same entity or three different ones?
- Is there deduplication? Fuzzy matching? Embedding similarity?
- What happens when the same entity appears with different properties across documents — last-write-wins, merge, or duplicate nodes?
- Does the system track entity provenance (which document introduced it)?

This is where most graph systems silently fail. Without entity resolution, the graph fragments into disconnected clusters per document.

### 4. Relationship Quality

How meaningful are the edges in the graph?

Investigate:
- Are relationships typed (supplies, contradicts, requires) or generic (related_to)?
- Are relationships directional? "A requires B" is different from "B requires A".
- Does the system extract implicit relationships (co-occurrence in the same chunk) or only explicit ones (stated in the text)?
- How many relationships per entity on average? Too few = disconnected graph. Too many generic ones = noise.

### 5. Storage and Isolation

How does the system organize entities in storage?

Investigate:
- Are entities namespaced by dataset/channel? If so, is the isolation physical (separate tables/databases) or logical (tags on shared tables)?
- Can entities exist in multiple namespaces?
- Can queries traverse across namespaces?
- What happens to shared entities when one namespace is deleted?

### 6. Retrieval Analysis

How does the system find relevant entities at query time?

Investigate:
- What search modes exist? (graph traversal, vector similarity, keyword, hybrid)
- How does graph traversal work? Fixed depth? Filtered by relationship type?
- Does vector search operate on chunks, entities, or both?
- Can you combine graph traversal with vector similarity in one query?
- What causes empty results? Missing entities, wrong search mode, namespace isolation, or poor embeddings?

Test by running known queries against known-ingested data. Track where results are correct, partial, or empty.

### 7. Cross-Reference Capability

This is the core question: can the system connect information across independently ingested documents?

Investigate:
- Ingest two related documents separately. Query for a connection between them.
- Does the graph contain an edge connecting entities from different documents?
- If not, is the connection findable via entity name overlap? Via embedding similarity?
- What is the actual mechanism that connects information across documents — explicit edges, shared entity nodes, or just search-time keyword matching?

## Output

Produce a structured report. For each layer, report BOTH what the system does well and where it fails. Identifying strengths is as important as identifying weaknesses — we want to learn from what works, not just catalog problems.

```
## System: [name]

### Extraction
- Method: [how entities are extracted]
- What works well: [specific techniques that produce good results, with evidence]
- Failure modes: [what gets lost, with evidence]
- Worth adopting: [yes/no — what specifically]

### Chunking
- Strategy: [how documents are split]
- What works well: [specific strengths]
- Failure modes: [where connections break]
- Worth adopting: [yes/no — what specifically]

### Entity Resolution
- Method: [how duplicates are handled]
- What works well: [specific strengths]
- Failure modes: [where the graph fragments]
- Worth adopting: [yes/no — what specifically]

### Relationships
- Quality: [typed/generic, directional, density]
- What works well: [specific strengths]
- Failure modes: [where edges are missing or meaningless]
- Worth adopting: [yes/no — what specifically]

### Storage
- Isolation model: [physical/logical, namespace behavior]
- What works well: [specific strengths]
- Failure modes: [where isolation prevents useful connections]
- Worth adopting: [yes/no — what specifically]

### Retrieval
- Modes: [what search types exist]
- What works well: [specific strengths — which queries produce good results]
- Failure modes: [what causes empty or irrelevant results]
- Worth adopting: [yes/no — what specifically]

### Community / Hierarchy
- Method: [how the system organizes entities into groups or levels]
- What works well: [specific strengths]
- Failure modes: [where organization breaks down]
- Worth adopting: [yes/no — what specifically]

### Temporal Handling
- Method: [how time-sensitive data is handled]
- What works well: [specific strengths]
- Failure modes: [where temporal information is lost]
- Worth adopting: [yes/no — what specifically]

### Cross-Reference
- Mechanism: [how cross-document connections actually work]
- What works well: [specific strengths]
- Failure modes: [where connections fail to form]
- Worth adopting: [yes/no — what specifically]

### Standout Techniques
- [Numbered list: techniques this system does better than any other surveyed system. Be specific — name the function, module, or algorithm.]

### Recommendations
- [Numbered list of specific improvements if building on this system, ordered by impact]
```

## Research Baseline

Use these findings to calibrate your analysis. They represent the current state of the art — what's known about where knowledge graph construction fails and why.

### Extraction Error Rates

- LLMs hallucinate entities on null inputs — they over-confidently label text as entities when none exist. A self-verification pass reduces this. (Wang et al., "GPT-NER," NAACL 2025)
- Zero-shot LLM entity extraction achieves as low as 33.5% precision on domain-specific datasets. Two-thirds of extracted entities can be wrong. (GPT-4o on Mongolian Medical Entity Relationship dataset, PMC11986385, 2024)
- Across domains, entity extraction accuracy ranges 60-85%. That's 15-40% noise. Post-extraction filtering is required infrastructure. (PremAI GraphRAG Implementation Guide, 2026)
- LLMs underperform supervised models on nested NER ("Bank of America" containing "America"). Output format (JSON vs tagged text) materially changes extraction quality. (Chen et al., EMNLP 2024)

### Entity Resolution

- Without coreference resolution, node duplication increases by 28%. Without structured prompts, noise increases by 73%. GraphRAG baseline has 30.5% duplication rate. (CORE-KG, arXiv:2510.26512, 2025)
- KGGen's iterative LLM-based entity clustering achieves 66% accuracy vs GraphRAG's 48% vs OpenIE's 30%. The deduplication step is where the most quality is left on the table. (Mo et al., NeurIPS 2025)
- GPT-4 outperforms fine-tuned models by 40-68% F1 on entity matching with unseen entity types. For heterogeneous personal knowledge systems, LLM-based resolution is more robust than fine-tuned models. (Peeters & Bizer, EDBT 2025)

### Relationship Extraction

- Entity extraction achieves 92-95% F1 on standard benchmarks. Relationship extraction sits at 60-65% F1. Your graph will have substantially more accurate nodes than edges. (CoNLL2003, DocRED benchmarks)
- The "extract entities then predict relations" pipeline amplifies errors. For N entities, O(N²) entity pairs must be evaluated, most of which are noise. Joint extraction outperforms pipeline approaches. (arXiv:2511.08143, 2025)
- Even sophisticated multi-agent approaches (9 collaborative agents) achieve 83% correctness. ~17% of extracted triples are still wrong. Error is structural, not incidental. (KARMA, NeurIPS 2025 Spotlight)

### Chunking

- Adaptive chunking improves retrieval F1 from 0.24 to 0.64 (167% improvement) vs fixed-size chunking. Fixed-size is the worst option. (PMC12649634, 2025)
- Every chunk boundary is a potential information loss point. Entities and relationships spanning boundaries are silently dropped. Overlap helps but does not fully solve this. (SLIDE, arXiv:2503.17952, 2025)
- Smaller chunks (64-128 tokens) optimize for fact retrieval. Larger chunks (512-1024) optimize for contextual understanding. No universal optimal size exists. (arXiv:2505.21700, 2025)

### Graph Completeness

- Automated KG construction captures ~60% of what human experts would include. The other 40% is missed. This is the current ceiling, not a bug in any tool. (BioKGrapher, PMC11536026, 2024)
- Best-in-class extractors (KGGen) miss one-third of known facts. GraphRAG misses over half. A single ingestion pass will not capture everything. Re-processing with different prompts is necessary for coverage. (KGGen, NeurIPS 2025)

### Retrieval Failures

- Seven distinct failure points in RAG: missing content, missed top-ranked docs, lost in consolidation, not extracted from context, wrong format, wrong specificity, incomplete. Failure points 1-3 are retrieval failures — the answer exists but isn't surfaced. (Barnett et al., IEEE/ACM CAIN 2024)
- KG-RAG models rely on memorized knowledge rather than performing symbolic reasoning over graph structure. Textual entity labels improve performance more than graph structure. The graph's value is in connections text search cannot find, not in replacing text search. (BRINK benchmark, arXiv:2508.08344, 2025)

### Cross-Document Linking

- Cross-document coreference resolution achieves 65-79% F1. For every 100 entities that should be linked across documents, 21-35 are missed. These missed links are the primary cause of graph fragmentation. (arXiv:2504.05767, 2025)
- Performance drops to 64% F1 on challenging subsets (different terminology, implicit references). The long tail of entity linking failures is where graphs fragment. (arXiv:2409.15113, 2024)

### Graph Fragmentation

- LLM-constructed knowledge graphs "often suffer from fragmentation — resulting in disconnected subgraphs that limit inferential coherence." Fragmentation is the expected outcome of document-by-document construction, not an edge case. (ReGraphRAG, EMNLP 2025 Findings)
- Pipeline fragmentation causes cumulative error propagation. A 15% entity error rate × 30% relation error rate × 25% resolution miss rate compounds to far worse final quality than any single stage suggests. (arXiv:2510.20345, 2025)
- Post-processing (deduplication, consistency checking, gap filling) is required infrastructure for any automatically constructed knowledge graph. (Same survey)

## Rules

- Read the actual code, not just documentation. Documentation describes intent. Code describes reality.
- Trace real data through the pipeline when possible. Abstract analysis misses implementation bugs.
- Distinguish between "the system can't do this" and "the system does this poorly." The fix is different.
- Be specific. "Search is bad" is useless. "GRAPH_COMPLETION returns empty when the query entity doesn't exactly match an extracted entity name" is actionable.
- Use the research baseline above to calibrate expectations. If you find extraction accuracy below 60%, that's below the known floor. If entity resolution is absent entirely, that's a known ~30% duplication problem.
- Separate systemic issues (chunking strategy, pipeline design) from incidental ones (a bug in one function). Systemic issues require architectural changes. Incidental ones need patches.

## Effective Organization Patterns

Use these findings to evaluate whether a system organizes knowledge for retrieval, not just storage.

### Source Text Preservation

- Graphs storing only extracted entities score 15-20% on retrieval. Adding source text chunks as node properties jumps accuracy to 90%. The graph provides navigation; the text provides answers. (arXiv:2511.05991, 2025)
- Fact-view (extracted triple) and context-view (source text) provide complementary retrieval signals. Store both. (KDD 2022, Multi-View Clustering for Open KB Canonicalization)

### Hierarchical Community Structure

- Community detection (Leiden algorithm) partitions graphs into semantically coherent groups at multiple levels. Intermediate levels (not root, not leaf) achieve strongest retrieval. Root-level summaries use 9-43x fewer tokens with only 15-20% comprehensiveness loss. (Microsoft GraphRAG, arXiv:2404.16130, 2024)
- Community detection as a pre-retrieval index focuses search on relevant subgraphs, improving both latency and quality. (CommunityKG-RAG, arXiv:2408.08535, 2025)
- Hierarchical KGs match flat KGs on structured sensemaking but significantly improve interpretability and navigation for open-ended exploration. (ACM CHIIR 2016)

### Temporal Organization

- Bi-temporal modeling (event time + ingestion time) with validity intervals is the state of the art. Old information is invalidated but not deleted. Scored 94.8% on deep memory retrieval. (Zep/Graphiti, arXiv:2501.13956, 2025)
- Three-tier architecture: episodes (raw inputs with timestamps), semantic entities (extracted), communities (clustered summaries). Bidirectional indices between layers. (Same Zep paper)
- Decay policies that down-weight old information improve time-sensitive retrieval. (arXiv:2403.04782, 2025)

### Graph Density and Predicate Vocabulary

- Real-world KGs converge on dozens to hundreds of reusable predicates. A core set of 20-50 relationship types with an extension mechanism balances consistency with coverage. (Dillinger, "Nature of KG Predicates")
- Sparse, meaningful connections outperform dense, noisy ones. Graph sparsity increases memory capacity quadratically up to a critical threshold. (arXiv:2411.14480, 2025)
- 5-15 high-quality relationships per entity is the sweet spot. Too many links trigger the fan effect — activation divides among connections, weakening each. (Collins & Loftus, spreading activation, 1975)

### Multi-Perspective Organization

- Multi-view query rewriting (decomposing queries by domain perspective) nearly triples retrieval accuracy. Cross-perspective results are complementary, not redundant. (MVRAG, arXiv:2404.12879, 2024)
- Channel-based organization aligns with levels-of-processing theory: deeper semantic processing (interpreting meaning through a specific lens) creates more retrievable memories than surface-level storage. (Craik & Lockhart, 1972)

### Contradictions and Novelty

- Schema-violating information (contradictions) forms stronger, more distinct memories. A "contradicts" relationship type should be first-class. (van Kesteren et al., Trends in Neuroscience, 2012)
- Concept maps with cross-links outperform hierarchical outlines for recall of relationships between ideas. Cross-domain links matter more than within-topic hierarchy. (Novak & Canas, IHMC)

### Retrieval Architecture

- Dual-channel retrieval (vector similarity + graph traversal) outperforms either alone. (KG-RAG, Nature Scientific Reports, 2025)
- Retrieved subgraph size should be proportional to query complexity. Over-retrieving degrades performance. (SubgraphRAG, ICLR 2025)
- 8K token context windows outperformed 16K/32K/64K in benchmarks. Smaller context forces more focused retrieval. (GraphRAG, arXiv:2404.16130, 2024)
- Self-reflection during extraction nearly doubles entity detection without introducing noise. (GraphRAG, same paper)

### Ontology Design

- For personal knowledge, flexible discovery-oriented schemas outperform rigid predefined ontologies. Let entity types emerge. Invest in metadata (provenance, temporal context, confidence). (Balog & Kenter, "Personal Knowledge Graphs," ACM 2019)
- Fixed schemas improve retrieval precision; dynamic schemas improve coverage. Optimal is a core vocabulary with extension mechanism. (Yang, "Fixed vs Dynamic Schema")

### Bidirectional Pipeline Feedback

No existing system implements backward communication between pipeline stages. All treat the pipeline as strictly forward (chunk → extract → resolve → store → retrieve). Three feedback patterns are worth investigating:

- **Extraction-aware-of-graph.** Feed existing entity names into the extraction prompt so the LLM produces canonical names from the start, reducing downstream resolution burden. No published research — this is a novel integration pattern identified during comparative analysis of 6 systems (see graph-engine/analysis/SYNTHESIS.md).
- **Resolution-aware-of-communities.** Use community membership as a signal during entity resolution. "Cu" appearing in a text about mining is almost certainly "copper" if "copper" already exists in a materials community. No published research on this specific feedback path.
- **Retrieval-aware-of-extraction-quality.** Weight search results by extraction confidence — entities confirmed across many documents rank higher than single-source entities. Conceptually related to TF-IDF and citation counting but applied to graph entities.

These add coupling between stages that are currently independent. Whether the quality improvement justifies the complexity is an open question.

### Quality Gates

- No existing system (Cognee, GraphRAG, Graphiti, LightRAG, KGGen, nano-graphrag) implements filtering between extraction and storage. Every system faithfully stores whatever the LLM extracts, including noise. (Comparative analysis, 2026)
- In production, Cognee's graph was 62.5% contaminated by noise entities extracted from failed web scrapes ingested as research data. The noise was structurally valid (real entity names like "technical difficulties") but semantically garbage. (Second Mind production data, 24 queries analyzed)
- Mechanical validity checks (minimum source length, error pattern blocklist, entity name length bounds) catch the worst cases without heuristic judgment. Fancier filtering (meaningfulness scoring) is optional and adds LLM cost.
