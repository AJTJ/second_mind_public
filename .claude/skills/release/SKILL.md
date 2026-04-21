---
name: release
description: Cut a release. Bumps semver in Cargo.toml files, tags, pushes, and triggers the public mirror. Use when shipping a version.
user_invocable: true
argument-hint: "<patch|minor|major> [--dry-run]"
---

# Release

Cut a versioned release of the second-mind monorepo.

## Versioning

Both crates share a version. When one changes, both bump together. This keeps the API contract between intake-engine and graph-engine aligned.

The version lives in two Cargo.toml files:
- `graph-engine/Cargo.toml`
- `intake-engine/Cargo.toml`

Follow semver strictly:
- **patch** (0.1.0 → 0.1.1) — bug fixes, test additions, doc updates, dependency bumps. No behavior change.
- **minor** (0.1.0 → 0.2.0) — new features, new API endpoints, new storage backends, new CLI commands. Backward compatible.
- **major** (0.1.0 → 1.0.0) — breaking API changes, schema migrations that aren't backward compatible, storage format changes.

## Procedure

### 1. Pre-flight

```bash
just fmt
just check
just clippy
just test
```

All must pass. Do not release with warnings from clippy.

If integration tests are available (TEST_DATABASE_URL set), run those too:
```bash
cargo test --test integration
cargo test --test neo4j_integration
cargo test --test end_to_end
```

### 2. Determine bump type

Review commits since last tag:

```bash
git log $(git describe --tags --abbrev=0 2>/dev/null || echo HEAD~10)..HEAD --oneline
```

Apply the commit types:
- All `fix:` → patch
- Any `feat:` → minor
- Any `!` (breaking) → major
- `test:`, `docs:`, `chore:`, `refactor:` alone → patch

The highest bump wins.

### 3. Bump versions

Update BOTH Cargo.toml files to the new version:

```
graph-engine/Cargo.toml    version = "X.Y.Z"
intake-engine/Cargo.toml   version = "X.Y.Z"
```

### 4. Public mirror audit

Invoke `/public-mirror` via the Skill tool. The release must not expose secrets, personal workflow, or AI attribution in the public repo. If the audit fails, fix the issues before proceeding.

### 5. Commit and tag

```bash
git add graph-engine/Cargo.toml intake-engine/Cargo.toml
git commit -m "release: vX.Y.Z"
git tag vX.Y.Z
git push && git push origin vX.Y.Z
```

The push triggers the public mirror workflow, which updates the public repo.

### 6. Verify

- Check that the GitHub Actions mirror workflow succeeded
- Check the public repo has the new code
- Check the tag appears on GitHub

## Dry run

With `--dry-run`, do steps 1-2 only. Report what the version would be and what commits drive the bump. Do not modify files or create tags.

## Rules

- Never skip pre-flight checks.
- Never tag without committing the version bump first.
- Never push a tag that doesn't match the Cargo.toml versions.
- The version in both Cargo.toml files must always match.
- No AI attribution in the release commit.
