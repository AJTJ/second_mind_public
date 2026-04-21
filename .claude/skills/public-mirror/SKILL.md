---
name: public-mirror
description: Audit what would be exposed in the public repo. Security scan, workflow separation, dead link check, build verification. Called by /align and /release.
user_invocable: true
argument-hint: "[--fix to update .public-exclude]"
---

# Public Mirror Audit

Verify that the public mirror contains only what it should. No secrets, no personal workflow, no dead links, no AI attribution. The public repo must build and make sense without private files.

## When to use

- Before every release (`/release` calls this)
- During every alignment check (`/align` calls this)
- Manually with `/public-mirror` when changing what's tracked or excluded

## What gets mirrored

The mirror workflow (`.github/workflows/mirror-public.yml`) clones the repo, deletes everything in `.public-exclude`, creates an orphan commit, and force-pushes to the public repo. The public repo has no history — one commit, always.

## Procedure

### 1. Security scan

Search all tracked files (excluding paths in `.public-exclude`) for:

```
Patterns:
  sk-ant-          Anthropic API keys
  ghp_             GitHub PATs
  password=        hardcoded passwords (not in .env.example placeholder context)
  secret=          hardcoded secrets
  @gmail.com       personal email
  @learningengine  personal email
  172.17.0.        Docker bridge IPs (reveals deployment topology)
  127.0.0.1        localhost references in non-Docker config
```

For each match, determine: is this in a file that would be public (not in `.public-exclude`)? If yes, flag it.

Exceptions (not flagged):
- `password` as a function parameter name in Rust code
- `password` in `.env.example` with placeholder values like `cognee`
- `127.0.0.1` in docker-compose.yml port bindings (standard practice)

### 2. Workflow separation

Verify `.public-exclude` covers all private files. Check each category:

**Must be excluded:**

| Path | Why |
|---|---|
| `CLAUDE.md` | Personal AI workflow |
| `AGENTS.md` | Personal agent config |
| `UPDATES.md` | Personal update log |
| `.mcp.json` | Local MCP server config |
| `.claude/settings.json` | Runtime config |
| `.claude/settings.local.json` | Local overrides |
| `.claude/scheduled_tasks.lock` | Runtime state |
| `.claude/skills/align/` | Personal workflow skill |
| `.claude/skills/beads/` | Personal issue tracker skill |
| `.claude/skills/research-updates/` | Personal research skill |
| `.beads/` | Issue tracker data |
| `.beads-credential-key` | Issue tracker credentials |
| `data/` | Personal knowledge data |
| `channels/` | Personal extraction lenses |
| `docker/.env` | Secrets |
| `research/` | Personal research notes |
| `.public-exclude` | This exclusion list |
| `.github/` | Mirror workflow + any CI |

**Must NOT be excluded (should be public):**

| Path | Why |
|---|---|
| `README.md` | Public docs |
| `justfile`, `lefthook.yml`, `mise.toml`, `.gitignore` | Dev tooling |
| `assets/` | Logo |
| `graph-engine/` (all of `src/`, `tests/`, `migrations/`, `Cargo.*`) | Core source code |
| `graph-engine/FEATURES.md`, `PLAN.md` | Design docs |
| `graph-engine/analysis/` | Research analysis |
| `graph-engine/Dockerfile` | Build config |
| `intake-engine/` (all of `src/`, `Cargo.*`, `Dockerfile`) | Core source code |
| `channels.example/` | Generic example channels |
| `docker/docker-compose.yml`, `.env.example`, `init-ollama.sh`, `patches/` | Infrastructure |
| `architecture.md` | System design |
| `.claude/skills/` (methodology skills) | Engineering methodology |

Methodology skills that should be public: `graph-analysis`, `graph-landscape`, `explore`, `review`, `architecture-review`, `system-design-review`, `commits`, `rust-safety`, `writing-rust-tests`, `writing-style`, `llm-resource-design`, `writing-pr-descriptions`, `release`, `public-mirror`.

### 3. AI attribution check

Search all files that would be public for:

```
Co-Authored-By: Claude
Generated with Claude Code
Generated with [Claude Code]
noreply@anthropic.com
```

None of these may appear in public files. The `/commits` skill prohibits them.

### 4. Dead link check

In `README.md`, find all relative links (`[text](path)`) and verify each path exists on disk AND is not in `.public-exclude`. A link to an excluded file is a dead link in the public repo.

### 5. Build simulation

Would the public repo work?

- Do both `Cargo.toml` files reference only crates available on crates.io? (No path dependencies to excluded crates)
- Does `docker-compose.yml` reference build contexts that exist? (`../graph-engine`, `../intake-engine`)
- Does `.env.example` exist? (Users need this to configure)
- Does `channels.example/` exist and contain at least one `.md` file?
- Does `README.md` setup section reference files that exist in the public mirror?

### 6. New file check

List all files tracked by git that are NOT in `.public-exclude` and NOT in the "must be public" list above. These are unreviewed — flag each for decision (exclude or keep public).

```bash
git ls-files | while read f; do
    # check if f matches any .public-exclude pattern
    # check if f is in the known-public list
    # if neither, flag it
done
```

## Output

Report findings in categories:

```
## Public Mirror Audit

### Security: [PASS/FAIL]
[list of flagged files if any]

### Workflow separation: [PASS/FAIL]
[missing exclusions if any]

### AI attribution: [PASS/FAIL]
[matches if any]

### Dead links: [PASS/FAIL]
[broken links if any]

### Build: [PASS/FAIL]
[missing files if any]

### Unreviewed files: [list]
[files not explicitly categorized]
```

## Arguments

- `/public-mirror` — audit and report
- `/public-mirror --fix` — audit, report, and update `.public-exclude` to fix any gaps

## Rules

- Never expose secrets, personal emails, or API keys in public files.
- Never expose personal workflow configuration (CLAUDE.md, .mcp.json, beads).
- Methodology skills are public — they document engineering decisions.
- When in doubt, exclude. It's easier to make something public later than to retract it.
- The public repo must be a complete, buildable, understandable project for someone who has never seen the private repo.
