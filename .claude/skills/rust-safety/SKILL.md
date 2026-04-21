---
name: rust-safety
description: Use when writing Rust code. Covers error handling conventions, checked arithmetic, and no-panic discipline.
metadata:
  version: 1.0.0
---

# Writing Rust Code

Safety and correctness rules for all Rust code.

## Error Handling

Use `anyhow::Result` for application-level code (binaries, CLI tools, agents). Use `thiserror` for library code where callers need to match on error variants.

Keep error context useful — include entity IDs, operation names, not just "failed".

```rust
// Good: context for debugging
anyhow::bail!("Cognee add failed for dataset '{dataset}': {e}");

// Bad: no context
anyhow::bail!("request failed");
```

### Avoid

- Raw `String` error types.
- Catching errors silently — if something can fail, propagate or log it.

## Arithmetic Safety

Never open room for underflow, overflow, or truncation.

### Pass known values instead of reconstructing them

```rust
// Bad: underflows if total == 0
fn record_progress(total: u32, done: u32) {
    let remaining = total - done;
}

// Good: caller passes the value it already knows
fn record_progress(remaining: u32, done: u32) { .. }
```

### When arithmetic is unavoidable

Use `checked_*` and return a typed error — never `.expect()` the result:

```rust
let elapsed = end_time
    .checked_sub(start_time)
    .ok_or_else(|| anyhow::anyhow!("timestamp underflow"))?;
```

### `as` casts

Avoid `as` for numeric conversions. Use `TryFrom`/`TryInto` with error handling, or `usize::from()` for infallible widening. `as` silently truncates.

## No Panics in Runtime

No `.unwrap()`, `.expect()`, `panic!()`, or `unreachable!()` on any production code path. Return typed `Result` errors instead. Error messages should tell the operator what to fix.

**Exceptions:** test code, `unreachable!()` after exhaustive matches the compiler can't prove (with a comment), and compile-time assertions.

## Unsafe Code

This project should have no `unsafe` code. If it becomes necessary, every `unsafe` block must have a `// SAFETY:` comment explaining why the invariants hold.
