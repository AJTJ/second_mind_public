---
name: commits
description: Use when creating git commits. Enforces conventional commit format, atomic commit boundaries, and proper message structure.
metadata:
  version: 1.0.0
---

# Commits

Every commit in this repo follows the Conventional Commits spec and must be atomic.

## Format

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

- **Subject line:** imperative mood, lowercase after colon, no trailing period, ≤72 chars
- **Body:** separated from subject by one blank line, wrapped at 72 chars, explains **why** not how
- **Footers:** `token: value` or `token #value` format (e.g., `Closes #123`, `BREAKING CHANGE: ...`)

## Types

| Type | When to use |
|------|-------------|
| `feat` | New capability that didn't exist before |
| `fix` | Corrects a bug causing wrong behavior |
| `refactor` | Restructures code without changing behavior |
| `perf` | Measurable performance improvement |
| `test` | Add, update, or fix tests only |
| `docs` | Documentation only (README, docstrings) |
| `build` | Build system or production dependencies (Cargo.toml, Dockerfile, package.json) |
| `ci` | CI/CD configuration |
| `chore` | Maintenance that doesn't touch src or test (.gitignore, dev tooling, AI tool configs) |
| `style` | Purely cosmetic (formatting, whitespace) — no logic or structure change |
| `revert` | Full rollback of a previous commit |

### Disambiguation

- **`fix` vs `feat`:** if users experienced a bug → `fix`. If adding something preventive that was never there → `feat`.
- **`refactor` vs `fix`:** if behavior was already correct but code structure improved → `refactor`. If behavior was wrong → `fix`.
- **`refactor` vs `style`:** `style` is cosmetic only (ran a formatter). `refactor` changes internal structure (extracted a function, simplified a condition, renamed for clarity).
- **`build` vs `chore`:** `build` affects production build/deps. `chore` is dev-only admin tasks.
- **Deleting dead code:** `refactor` (structural improvement, no behavior change).
- **Partial revert to fix side effects:** `fix` not `revert` — you're fixing a bug, not rolling back a whole change.

## Scope

Scope is the **module or subsystem** affected:

- Engine: `graph-engine`, `intake-engine`, `pipeline`, `extractor`, `resolver`, `temporal`, `communities`
- Infrastructure: `docker`, `compose`, `schema`, `migrations`
- Knowledge: `prompts`, `channels`, `search`, `embedder`
- Cross-cutting: `deps`, `gitignore`, `claude`, `infra`, `skills`

**Omit scope** when the change is truly global or doesn't fit any single module.

**Do NOT** use issue numbers, vague words (`misc`, `stuff`), or overly specific names that change every commit.

## Atomic Commits

An atomic commit is the smallest meaningful, complete unit of work that:

1. Addresses **exactly one logical concern**
2. Leaves the codebase in a **working state** (compiles, tests pass)
3. Can be **reverted without side effects** beyond what the message describes
4. Can be **described with a single, focused subject line**

### The revertability test

If reverting a commit also removes other unrelated changes, it was not atomic.

### When to split

- Bug fix + unrelated formatting → two commits
- Feature + refactoring of unrelated code → two commits
- API change + its documentation → can be one commit (same concern)

### The preparatory commit pattern (gold standard)

When a refactor is needed to enable a fix or feature, do the refactor first as a separate commit that preserves behavior, then apply the fix/feature on top:

```
# Commit 1
refactor(worker): extract email sending logic from process loop

# Commit 2
feat(worker): add per-subscriber delivery tracking
```

### When one honest commit is better

When changes are **genuinely interleaved** across the same lines and separating them would create artificial broken intermediate states, one commit with a detailed body is better than fake atomicity.

### Fake atomicity (anti-pattern)

Multiple commits that each don't compile or pass tests independently, and only make sense when read together. This is worse than one larger honest commit. Atomic means each commit is a **complete, valid state** — not just small.

## Breaking Changes

Two methods, use both for maximum clarity:

```
feat(domain)!: restructure item request model

BREAKING CHANGE: ItemRequest now uses per-subscriber tracking. Update all callers.
```

- `!` goes immediately before `:`
- `BREAKING CHANGE` in footer **must** be uppercase
- Breaking changes trigger a MAJOR version bump and can be part of **any** type

## When to Use a Body

**Use a body when:**

- The change requires context about **why** it was made
- There were alternative approaches considered
- The change affects multiple areas for one logical reason
- The change has non-obvious implications

**Skip the body when:**

- `docs: fix typo in README`
- `style: run rustfmt`
- The subject line tells the complete story

## No AI Attribution

Never add `Co-Authored-By: Claude`, `Generated with Claude Code`, or any AI tool branding to commit messages, trailers, or footers. The commit stands on its own.

## Examples

```
feat(worker): add per-subscriber email delivery tracking

Tracks delivery status per subscriber instead of a single boolean on the
request. Required for safe retry — without this, retrying a partially
failed request would double-send to successful subscribers.
```

```
fix(routes): return 404 instead of 500 for missing plant
```

```
refactor(domain): consolidate duplicate fetch functions for email subscribers
```

```
chore(claude): add beads skill and update agent instructions
```

```
perf(domain): batch-load item requests to fix N+1 in fetch_items_by_plant

Single query with IN clause replaces per-item loop. Reduces DB round
trips from N+1 to 2 for plants with many items.
```

```
test(worker): add integration test for stuck request recovery
```
