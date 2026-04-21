---
name: review
description: Review changed code against engineering principles. Checks SOLID, async correctness, hardening, architecture boundaries, and system design. Use before merging non-trivial changes.
user_invocable: true
metadata:
  version: 1.0.0
---

# Review: Principles and Architecture

Review changed code against engineering principles and the project's conventions. This is not a code quality or efficiency review — `/simplify` handles that. This skill checks whether the change is **sound**, **robust**, and **respects the system's boundaries**.

## Phase 1: Identify Changes

Run `git diff` (or `git diff HEAD` for staged changes) to see what changed.

## Phase 2: Launch Review Agents in Parallel

Use the Agent tool to launch all five agents concurrently. Pass each agent the full diff and instruct it to read CLAUDE.md and relevant source files before forming judgments.

### Agent 1: SOLID Principles

Check each change against SOLID:

1. **Single Responsibility**: Does each struct, function, or module do exactly one thing? Flag functions that handle multiple concerns.
2. **Open/Closed**: Can this change be extended without modifying what was just written? Flag hardcoded behavior that should be trait-based.
3. **Liskov Substitution**: If traits are involved, can every implementor be used interchangeably? Flag partial trait impls.
4. **Interface Segregation**: Are callers forced to depend on things they don't use? Flag God-traits.
5. **Dependency Inversion**: Do high-level modules depend on low-level details? Flag functions that construct HTTP clients or DB connections directly instead of receiving them as parameters.

### Agent 2: Async Correctness (Rust/Tokio)

1. **Blocking in async context**: Flag `.await`-adjacent code that does CPU-heavy work, synchronous file I/O, or calls `std::thread::sleep`. Use `tokio::task::spawn_blocking` or `tokio::time::sleep`.
2. **Connection/resource starvation**: Flag holding resources across `.await` points that involve unrelated async work.
3. **Cancellation safety**: Flag `.await` calls inside loops that would leave state inconsistent if the future is dropped.
4. **Concurrent mutation**: Flag shared mutable state accessed without proper synchronization (Mutex, RwLock, atomics).

### Agent 3: Hardening and Reliability

1. **Graceful degradation**: If an external service fails (Cognee, Perplexity, Semantic Scholar), does the system degrade gracefully or crash? Flag `?` propagation that returns errors to the user for non-critical failures.
2. **Retry and idempotency**: Operations that can be retried must be idempotent. Flag writes that would duplicate data on retry.
3. **Input validation at system boundaries**: Validate external input (API responses, user input, tool results). Don't add defensive checks between internal modules.
4. **Timeout and resource bounds**: External API calls must have timeouts. Flag unbounded retries or operations that could hang.
5. **Panic surface**: No `.unwrap()`, `.expect()`, `panic!()` on runtime code paths (per `/rust-safety`). Flag `as` casts on numeric types.

### Agent 4: Architecture Boundaries

Delegate to `/architecture-review` if the change touches:
- Module boundaries (new crate, moved modules)
- New integrations (new external service)
- System structure (new Docker service, changed MCP tools)

Focus on passes 1 (boundaries), 3 (interfaces), and 4 (coupling).

### Agent 5: System Design

Delegate to `/system-design-review` if the change touches:
- Error handling or retry logic (Pass 5: failure semantics)
- Queues, buffers, or pipelines (Pass 8: backpressure)
- Data persistence or the intake log (Pass 2: durability)
- External service interactions (Pass 1: consistency)

### Agent 6: LLM Resource Design

Delegate to `/llm-resource-design` if the change touches:
- Agent conversation loops or message history management
- API client code (HTTP calls to metered APIs)
- Tool result handling or output processing
- Retry logic, rate limit handling, or error handling for external APIs
- Context compression, summarization, or truncation logic
- Model selection, routing, or token budget configuration

Focus on Group A (quality) if context/tools changed, Group B (cost) if API/retry changed, both if unclear.

### Agent 7: Portability and Reproducibility

Check whether the project can be cloned and run on a fresh machine with only Docker and `just` installed.

1. **Docker self-containment**: Can `docker compose up -d` start the entire system? Are there host dependencies (systemd services, host-installed binaries, host Ollama) that break this?
2. **Environment assumptions**: Do any scripts, recipes, or code assume a specific host OS, shell, user, or directory structure? Flag hardcoded paths, usernames, or host-specific config.
3. **Volume and data**: Are bind-mount paths relative (portable) or absolute (breaks on other machines)? Does `.env.example` document every required variable?
4. **Build reproducibility**: Do Dockerfiles pin base images or use `latest`? Are Cargo.lock files committed? Can both crates build in Docker without host tools?
5. **First-run experience**: Does `just setup` work from a clean clone? Does it create volumes, pull images, run migrations, and pull models without manual steps?
6. **Platform compatibility**: Do bind mounts use `:cached` for macOS? Do healthchecks use tools available in the container images?

## Phase 3: Report

Wait for all agents. Aggregate findings into three categories:

### Violations
Changes that break a principle or boundary. Must be fixed before merging. Each cites:
- File and line
- Principle violated
- Why it matters

### Concerns
Weaknesses that don't break anything today but weaken the architecture.

### Observations
Non-actionable context — areas accumulating complexity, fragile boundaries.

Present each finding with file, line, and principle. Do not fix anything — this is a review, not a refactor.
