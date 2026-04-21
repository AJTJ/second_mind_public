# Second Mind

Rust knowledge graph engine with LLM-driven entity extraction, bi-temporal relationship tracking, and hybrid retrieval.

## Architecture

```
Claude Code (MCP tools, /explore skill)
              |
              | MCP (port 8001)
              |
         Intake Engine (Rust, Docker)
         logs + stores + forwards
              |
         Graph Engine (Rust, port 8000, Docker)
         chunking + extraction + resolution + search
              |
        ┌─────┴─────┐
    Neo4j         Postgres + pgvector
    entities      embeddings
    relationships LLM cache
    traversal     metadata
```

All services run in Docker Compose. Prompts and data are bind-mounted from the host.

## Usage

The primary interface is conversation. Use MCP tools to interact with the knowledge graph.

Ingesting:
> "Ingest this paper through the research channel."
> "Store this ETF fact sheet as investment data."

Searching:
> "What do I know about copper supply chains?"
> "Search the investment channel for Canadian-accessible ETFs."

Replaying with a different lens:
> "Replay that document through the investment channel instead."

Research:
> "/explore copper demand dynamics for AI infrastructure"

The MCP tools behind these:
- `ingest(sources, datasets, prompt_name)` — store, log, forward to graph engine
- `search(query, datasets?, search_type?, top_k?)` — query the knowledge graph
- `replay(entry_id?, prompt_override?)` — re-process with original or different prompt
- `log(dataset?, prompt?, limit?)` — view ingestion history

CLI equivalents: `just ingest`, `just search`, `just replay`, `just log`.

## Two Modes

**Simple mode** — vector search only. Fast, cheap, proven. Chunks are embedded and searchable via similarity. No Claude API cost beyond embedding. Good for reference material, quick storage, high-volume ingestion.

> "Store this quickly, I just need to find it later."

```
just ingest-simple paper.pdf
```

**Full mode** — graph extraction. Slower, costs API calls, builds entity-relationship graph. Enables multi-hop traversal, contradiction detection, temporal tracking. Worth it for core research you'll query relationally.

> "Ingest this through the investment channel."

```
just ingest paper.pdf investment
```

Use simple mode by default. Upgrade to full mode when you need relational queries across documents. You can always integrate later:

```
just integrate research
```

## Channels

`channels/` contains `.md` files. Each file is a processing prompt — the filename is the channel name. The prompt tells the LLM what to extract during ingestion.

Create a new channel by adding a `.md` file. Use it immediately:
> "Ingest this through the worldview channel."

Don't mix channels during a single ingestion. Label results by channel when returning from multiple.

## Access Rules

All access goes through the intake engine via MCP tools or `just` recipes. Do not call the graph engine's REST API directly. Do not call Docker containers directly.
- **Delete**: `just delete-dataset <name>` (CLI only, not MCP)
- **Integrate**: `just integrate <dataset>`

The intake log is the source of truth. Direct calls create ghost data the log doesn't know about.

## Search Types

| Type | Use when |
|---|---|
| `KEYWORD` | Fast name-based search, no embeddings |
| `GRAPH_COMPLETION` | Relational queries, "how does X relate to Y" |
| `CHUNKS` | Specific facts, data points, raw text |
| `SIMILARITY` | Related concepts, fuzzy entity matching |
| `SUMMARIES` | Broad overview, "what do I know about X" |

When searching and getting empty results, try a different search type before concluding the knowledge doesn't exist.

## Environment Variables

See `docker/.env.example` for all config. Key variables:

| Variable | Default | Purpose |
|---|---|---|
| `GRAPH_BACKEND` | `neo4j` (in compose) | Graph storage backend (`neo4j` or `postgres`) |
| `NEO4J_PASSWORD` | `secondmind` | Neo4j auth password |
| `DATABASE_URL` | (set in compose) | Postgres connection for vectors/cache |
| `EMBEDDING_MODEL` | `qwen3-embedding:4b` | Ollama embedding model |
| `ANTHROPIC_API_KEY` | (required) | Claude API key for entity extraction |
| `MAX_GLEANINGS` | `1` | Gleaning passes per chunk (0 to disable, reduces API cost) |
| `EMBEDDING_MAX_TOKENS` | `32000` | Max document size in estimated tokens |

## Operations

```bash
just setup            # First-run: wipe, start services, pull model
just up               # Start all services
just down             # Stop everything
just status           # Health check
just restart          # Rebuild + restart containers
just logs             # View container logs
just wipe             # Reset all data
just integrate <dataset>    # Run entity extraction on a dataset
just compact          # Deduplicate intake log
just delete-dataset <name>  # Delete a dataset
```

Research is done via the `/explore` skill in Claude Code.

## Build & Quality

```bash
just fmt              # Format
just check            # Compilation check
just clippy           # Lint
just test             # Unit tests
just build            # Release build
```

Pre-commit hooks: rustfmt check, gitleaks, cargo check.
Pre-push hooks: clippy (-D warnings), tests.

## Versioning

Both crates share a semver version. The version in `graph-engine/Cargo.toml` and `intake-engine/Cargo.toml` must always match.

Use `/release <patch|minor|major>` to cut a release. It runs pre-flight checks, bumps versions, commits, tags, and pushes.

## Just Recipe Enforcement

Use `just` recipes for ALL operations. Do not call binaries, docker, or tools directly. The justfile is the canonical interface.
