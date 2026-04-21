---
name: architecture-review
description: Evaluate system architecture — component boundaries, data flow, interface contracts, coupling, operational topology, and evolution paths. Reusable from review (PR) or standalone.
user_invocable: true
metadata:
  version: 1.0.0
---

# Architecture Review

Structured evaluation of system architecture. This skill is the reusable architectural analysis core — it can be invoked standalone or delegated to by `review` when a change touches module boundaries, new integrations, or system structure.

The focus is system-level: component boundaries, data flow, interface contracts, coupling, deployment topology, and operational readiness. Code-level concerns (SOLID on individual types, async correctness) belong in the `review` skill — this skill evaluates the architecture those types live inside.

## Inputs

Accept one of:

- **Code changes**: PR diff, branch range, or file list
- **System description**: verbal or diagrammatic description from the user
- **Full codebase**: standalone review of the entire system
- **Delegation context**: when called from `review`, the caller provides the relevant architectural context

## Passes

Run all six passes. Skip individual checks when they have no signal, but do not skip entire passes without stating why.

### Pass 1: Component boundary analysis

Are the system's components well-bounded, with clear responsibilities and minimal overlap?

**Evaluate:**

- Does each component have a single, stated reason to exist?
- Are there components that do two unrelated things?
- Are there responsibilities split across components that should be co-located?
- Is the granularity appropriate?

**Check for:**

- God components (one component that everything depends on)
- Orphan components (exist but nothing uses them)
- Responsibility overlap (two components that both "own" the same concern)
- Missing components (a responsibility that no component claims)


### Pass 2: Data flow analysis

Can you trace data from input to output through named components and named intermediate forms?

**Evaluate:**

- Is the data flow described end-to-end, or are there gaps?
- At each boundary, is the data format specified?
- Are transformations explicit (what changes, what's preserved)?

**Check for:**

- Unnamed intermediate forms ("the data is processed and passed on")
- Format mismatches at boundaries (producer emits X, consumer expects Y)
- Implicit ordering dependencies ("A must run before B" without enforcement)
- Data that crosses trust boundaries without validation


### Pass 3: Interface contract analysis

Are the interfaces between components precisely defined, minimal, and evolvable?

**Evaluate:**

- Is each inter-component interface defined precisely enough to implement from the spec alone?
- Are there hidden coupling channels (shared mutable state, implicit ordering)?
- Can each component be tested in isolation against the interface?
- Can the interface evolve without breaking consumers?

**Check for:**

- Interfaces defined by implementation rather than contract
- Bidirectional dependencies (A calls B and B calls A)
- Temporal coupling ("call init() before process()" without type-state enforcement)
- Leaky abstractions (consumer must understand provider internals)


### Pass 4: Coupling and dependency analysis

Does the dependency graph support independent development, testing, and deployment?

**Evaluate:**

- Draw the dependency graph. Is it a DAG?
- Do dependencies point toward stability?
- Are there dependency shortcuts?

**Check for:**

- Dependency cycles
- Unstable components that many others depend on
- Shared database as an implicit coupling channel (multiple components writing to the same tables without coordination)
- `shared/` types creating hidden coupling between domains

### Pass 5: Operational architecture

Can this system be deployed, monitored, debugged, restarted, and rolled back?

**Evaluate:**

- **Deployment**: Can frontend and backend be deployed independently? What happens during partial deployment?
- **Monitoring**: Does each component emit health signals? Are there end-to-end health checks?
- **Debugging**: Can you trace a request through the system? Are there correlation IDs?
- **Restart**: What state is lost on restart? (message worker position, JWKS cache, in-flight requests)
- **Rollback**: Can you revert to the previous version? Are migrations reversible?
- **Failure modes**: What happens when each dependency fails? (RDS, Cognito, S3, SES, Google Sheets, fck-nat)

**Check for:**

- Components that can't be restarted independently
- Missing health checks or monitoring blind spots
- Shared fate (App Runner crash kills both API and message worker)
- State that can't be reconstructed after a crash


### Pass 6: Evolution and extensibility

Can the architecture accommodate likely future changes without rework of stable components?

**Evaluate:**

- What are the most likely directions of change? (new integrations, multi-tenant, mobile app, real-time updates)
- For each likely change, which components need modification?
- Are extension points in the right places?

**Check for:**

- Hardcoded assumptions about number/type of organizations, plants, or roles
- Extension points that no one has validated with a second use case
- Over-generalization adding complexity now
- Missing extension points where planned work explicitly needs them


## Scoring rubric

- `Likelihood` (1-3): probability this gap causes problems
- `Impact` (1-3): blast radius if hit
- `Detectability` (1-3): how likely caught before production

Severity mapping:

- `P0`: architectural flaw that will cause a correctness or safety failure
- `P1`: flaw likely to block implementation, cause a production incident, or force rework
- `P2`: weakness that will cause friction, tech debt, or operational pain
- `P3`: quality concern or unlikely-but-worth-noting risk

## Output format

### 1. Architecture summary

2-3 sentences: what the architecture is, overall assessment, and the single most important finding.

### 2. Component map

A brief listing of the components identified and their responsibilities. Use a Mermaid diagram if the relationships are non-trivial.

### 3. Findings

Ordered by severity. For each:

```
[P0|P1|P2|P3] <title>

Pass: <which pass>
Component(s): <which components are involved>

Evidence: what the design/code says or does
Gap: what's missing or wrong
Risk: what happens if unaddressed
Fix direction: minimal change that resolves this
```

### 4. Open questions

Decisions that could change findings or the architecture's viability.

### 5. Residual risks

Risks inherent to the chosen architecture, even if all findings are addressed.

## Caller integration

- **From `review` (PR)**: Focus on passes 1, 3, and 4 (boundaries, interfaces, coupling) since `review` already covers operational concerns. Return findings for merge into the review output.
- **Standalone**: Full output as described above.

## Constraints

- Evaluate the architecture, not the prose or code style.
- Don't demand architecture astronautics. This is a small team, early-stage product. Simple is better.
- Give credit for explicit simplicity. Fewer components with clear boundaries beats many components with unclear ones.
- When checking referenced code, verify it exists and roughly matches the description. Don't do a full code review.
