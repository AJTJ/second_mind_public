---
name: writing-pr-descriptions
description: Use when writing or updating a pull request description. Covers structure, what level of detail belongs where, diagram conventions, and how to guide reviewers through a large diff.
metadata:
  version: 1.0.0
---

# Writing PR Descriptions

A good PR description answers three questions: what changed, why, and how should I review this? Everything else is optional.

## Structure

Not every section applies to every PR. A one-commit bug fix doesn't need architecture diagrams. Use what fits.

### Summary (always)

Two to three sentences. What did you do and why. Don't repeat the commit list — synthesize.

"Adds per-subscriber email tracking to the messaging worker, replacing the single `emails_sent` boolean on item requests. Required for safe retry — without this, retrying a partially failed request would double-send to successful subscribers."

Not: "This PR introduces a comprehensive email dispatch tracking abstraction, establishes per-subscriber delivery boundaries, implements granular status tracking with configurable retry windows, and adds migration support for the new join table."

### What changed (for anything beyond trivial)

For refactors and architectural changes, use a comparison table that maps the **same concerns** before and after. This lets the reviewer see what replaced what in one scan.

| Concern | Before | After | Why |
|---------|--------|-------|-----|
| Email tracking | Single boolean on request | Per-subscriber dispatch table | Safe retry without double-send |
| Failure mode | Whole request marked FAILED | Individual subscriber failures | Partial success is recoverable |

Don't describe the "before" and "after" as separate blocks with different content. The whole point of the table is that each row is a direct comparison.

### Diagrams (when the component relationships aren't obvious)

Use Mermaid. GitHub renders Mermaid natively in PR descriptions.

Good uses for diagrams:

- Component ownership / dependency graph
- Sequence diagrams for runtime flows (request → worker → sheets + email)
- Shutdown or initialization ordering

Bad uses:

- Restating what's already clear from the code
- Diagrams with 15+ nodes (nobody will read them)

Keep diagrams focused on one concept. Two small diagrams beat one sprawling one — unless the two concepts share components and have a cause-and-effect relationship. In that case, one diagram with phase highlighting shows the connection that splitting would hide.

Diagrams are illustrations in a story, not standalone artifacts. A sentence or two before each diagram should set up what the reader is about to see and why it matters.

### How to review (for PRs with 5+ files or 3+ commits)

List components in dependency order as a table. Each row: what the component is, what it does, where it lives. No coaching ("read this first"), no implementation details they'll see in the diff. Just the map.

| # | Component | What it does | Files |
|---|-----------|-------------|-------|
| 1 | Migration | Adds `item_request_email_dispatches` table | `migrations/...` |
| 2 | Domain model | Dispatch struct + Diesel mappings | `domain/messaging/models.rs` |
| 3 | Service layer | Create/update dispatch records | `domain/messaging/services/` |
| 4 | Worker integration | Wire dispatch tracking into process loop | `domain/messaging/messaging_worker.rs` |

### Design decisions (when you made non-obvious choices)

Only include decisions where a reasonable reviewer might ask "why not X?" Keep each one to a sentence or two. A table works well.

| Decision | Rationale |
|----------|-----------|
| Join table vs JSON column | Need to query by status across requests for retry. JSON would require full-table scan. |
| Pending as default status | Worker sets sent/failed after attempt. Avoids race between insert and send. |

Don't explain things that are standard practice or obvious from context.

### Metrics, config, and test plan (when applicable)

A test plan is a checklist of what's been verified and what still needs manual testing. Use checkboxes.

## What doesn't belong in the description

- Internal implementation details (struct field listings, query shapes). The diff shows this. The description should explain intent and trade-offs.
- Exhaustive file-by-file changelogs. The diff view already has this. Group by concept instead.
- Prose that restates what the code does. "This function iterates over the dispatches and checks status" is noise. "Retry skips already-sent subscribers so partial failures are recoverable" is context the diff can't show.
- Local-only tracker IDs or agent scratch context. Do not include Beads IDs, local environment notes, or personal tooling context in PR descriptions unless the user explicitly asks for that audience crossover.
- **Bot signatures or attribution lines.** Never add "Generated with Claude Code", "Co-Authored-By: Claude", or any AI tool branding to PR descriptions, PR bodies, or issue comments. The work speaks for itself.

## Checklist before publishing

- [ ] Can a reviewer who hasn't seen the code understand what this PR does from the summary alone?
- [ ] Does the "what changed" table (or equivalent) map the same concerns before and after?
- [ ] Are diagrams focused on one concept each, using Mermaid?
- [ ] Is there a review guide for PRs with 5+ files?
- [ ] Did you cut any detail that's better seen in the diff?
- [ ] Read it once out loud. Does it sound like a person wrote it?
