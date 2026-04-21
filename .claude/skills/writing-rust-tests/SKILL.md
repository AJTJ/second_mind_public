---
name: writing-rust-tests
description: Use when writing or reviewing Rust tests in this codebase. Covers test conventions, integration test patterns, and test structure.
metadata:
  version: 1.0.0
---

# Writing Rust Tests

Conventions and patterns for tests in this project.

## Running Tests

```bash
just test                                              # unit tests (no external deps)
TEST_DATABASE_URL="..." cargo test --test integration  # integration tests (needs Postgres)
```

## Test Location

Two categories:

**Unit tests** — `#[cfg(test)] mod tests` inside source files. Pure functions only: types, chunker, resolver normalization, extractor parsing, cache key generation. No external dependencies.

**Integration tests** — `tests/integration.rs`. Requires a real Postgres with pgvector. Tests skip gracefully when `TEST_DATABASE_URL` is not set.

```
graph-engine/
  src/
    types.rs          # unit tests: serialization, enums
    chunker.rs        # unit tests: splitting behavior
    resolver.rs       # unit tests: normalize_name, singularize
    extractor.rs      # unit tests: parse_extraction, mock behavior
    cache.rs          # unit tests: cache_key consistency
    embedder.rs       # unit tests: construction
  tests/
    integration.rs    # 53 tests against real Postgres
```

## Integration Test Setup

Tests use `TEST_DATABASE_URL` and skip when it's absent:

```rust
async fn setup() -> Option<PgPool> {
    let url = match std::env::var("TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => return None,
    };
    let pool = PgPool::connect(&url).await.ok()?;
    schema::initialize(&pool).await.ok()?;
    Some(pool)
}

macro_rules! require_db {
    ($pool:expr) => { match $pool { Some(p) => p, None => return } };
}
```

Each test creates its own data with unique IDs (ULID-based) to avoid cross-test interference. Tests run in parallel. No `clean_all` between tests.

Community detection tests use `#[serial]` from the `serial_test` crate because `detect_communities` is a global operation.

## Test Helpers

```rust
fn uid() -> String { ulid::Ulid::new().to_string() }

async fn create_test_channel(pool: &PgPool, name: &str) -> i32 {
    schema::ensure_channel(pool, name).await.unwrap()
}

async fn create_test_document(pool: &PgPool, channel_id: i32) -> String { ... }
async fn create_test_entity(pool: &PgPool, name: &str) -> String { ... }
```

Keep helpers at the top of integration.rs. Each creates one record with proper FK chains.

## What to Test

Priority order:

1. **Pipeline end-to-end** — add_document creates chunks, integrate extracts entities, search returns results
2. **Data integrity** — FK constraints, deduplication, idempotency
3. **Temporal logic** — contradiction detection (novel/identical/contradictory), supersession
4. **Channel isolation** — search respects channel filters, traversal respects boundaries
5. **Error resilience** — extraction failure doesn't abort integrate, embedding failure doesn't abort add

Do NOT test implementation details. Test observable behavior through pipeline functions or database state.

## Assertions

```rust
assert_eq!(result.entities_created, 3);
assert!(channels.contains(&"research".to_string()), "entity should be in research channel");
assert!(result.is_ok(), "should not fail: {}", result.unwrap_err());
```

Include context in assertions. `assert!(x)` with no message is useless when it fails.

## Naming

Test names describe the behavior:

- Good: `test_integrate_skips_already_processed_chunks`
- Good: `test_contradiction_identical_returns_existing_id`
- Bad: `test_insert_works`
- Bad: `test_query_returns_data`

## No AI Attribution

Never add `Co-Authored-By: Claude` or `Generated with Claude Code` to test files, commit messages, or comments.
