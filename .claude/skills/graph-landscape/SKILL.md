---
name: graph-landscape
description: Track the knowledge graph ecosystem. Pull latest from reference systems, detect new projects, analyze changes, compare to our graph-engine. Use when you want to stay current on what others are building.
user_invocable: true
argument-hint: "[--discover to search for new projects]"
---

# Graph Landscape: Ecosystem Tracker

Monitor the open-source knowledge graph construction space. Pull updates from tracked systems, find new ones, analyze what changed, compare to our implementation.

## Reference directory

All tracked systems live in `~/references/graph-systems/`. Each is a full git clone.

## Registry

The registry file tracks what we monitor:

```
~/references/graph-systems/REGISTRY.md
```

Format:
```markdown
# Graph Systems Registry

Last updated: YYYY-MM-DD

| System | Repo | Last pulled | Last commit | Stars | Notes |
|---|---|---|---|---|---|
| GraphRAG | microsoft/graphrag | 2026-04-14 | abc1234 | 32K | Community detection |
| Graphiti | getzep/graphiti | 2026-04-14 | def5678 | 25K | Temporal validity |
| ... | ... | ... | ... | ... | ... |
```

## Procedure

### Phase 1: Discover new projects (if `--discover` flag or first run)

Search the web for:
- "knowledge graph construction" site:github.com, sorted by stars, last year
- "graphrag" OR "graph rag" site:github.com, new repos
- "entity extraction knowledge graph" open source 2025 2026

For each candidate:
1. Must be open source with actual code (not just a paper)
2. Must handle the full or partial pipeline (extraction, storage, retrieval)
3. Must have >500 stars or be from a known research lab
4. Must not already be in the registry

For each new project found:
1. Clone to `~/references/graph-systems/`
2. Add to REGISTRY.md
3. Run a quick analysis (read README, identify key technique, note what's novel)

### Phase 2: Pull latest from all tracked systems

For each system in the registry:

```bash
cd ~/references/graph-systems/<name>
git fetch origin
git log --oneline HEAD..origin/main | head -20  # see what's new
git log HEAD..origin/main --format="%H %s" > /tmp/<name>-new-commits.txt
git pull
```

Record:
- Previous commit hash (before pull)
- New commit hash (after pull)
- Number of new commits
- Summary of changes (from commit messages)

### Phase 3: Analyze changes

For each system with new commits since last pull:

1. Read the new commit messages for themes
2. Check for changes in key files:
   - Extraction prompts or logic
   - Entity resolution / deduplication
   - Search / retrieval
   - Community detection
   - Temporal handling
   - New features or major refactors
3. If significant changes found, read the actual diff for the relevant files
4. If a system has major changes (new module, new algorithm, architectural shift), invoke `/graph-analysis <system-name>` via the Skill tool to run a full deep-dive. This updates the existing report at `graph-engine/analysis/<NN>-<name>.md`.

Produce a per-system changelog entry.

### Phase 4: Compare to our graph-engine

Read our current implementation:
- `graph-engine/FEATURES.md` (what we've adopted)
- `graph-engine/src/` (current code)

For each significant change in tracked systems:
- Does it improve on something we already have?
- Does it add something we're missing?
- Does it validate or invalidate a design choice we made?

### Phase 5: Write report

Save to `graph-engine/analysis/LANDSCAPE_<date>.md`:

```markdown
# Graph Landscape Update — YYYY-MM-DD

## New Projects Discovered
[List with repo, stars, key technique, relevance to us]

## Updates to Tracked Systems

### GraphRAG
- Commits since last pull: N
- Key changes: [summary]
- Relevant to us: [yes/no — what specifically]

### Graphiti
...

## Comparison to Our Implementation
- Techniques we should adopt: [list with priority]
- Techniques that validate our approach: [list]
- Techniques we do better: [list]

## Registry Changes
- Added: [new systems]
- Updated: [pulled systems with commit hashes]
```

Also update REGISTRY.md with new dates and commit hashes.

## Rules

- Always pull before analyzing. Stale clones produce stale analysis.
- Only recommend adopting techniques that address a measured gap, not hypothetical improvements.
- When a tracked system adopts something we already have, note it as validation.
- When we do something no tracked system does, note it as differentiation.
- Include commit hashes for traceability. "GraphRAG added X" is useless without a commit reference.
- Don't clone repos into the main project directory. Always use `~/references/graph-systems/`.
- Keep REGISTRY.md as the single source of truth for what we track.

## First run

If REGISTRY.md doesn't exist, create it from the systems already cloned:

```bash
ls ~/references/graph-systems/
```

Populate the registry from what's there, then run the full procedure.
