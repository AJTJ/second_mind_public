---
name: system-design-review
description: Evaluate fundamental system design decisions — consistency models, durability guarantees, latency budgets, failure semantics, ordering, backpressure, and clock assumptions. Reusable from review (PR) or standalone.
user_invocable: true
metadata:
  version: 1.0.0
---

# System Design Review

Structured evaluation of a system's fundamental design decisions. Where `architecture-review` evaluates how a system is decomposed (components, boundaries, coupling), this skill evaluates whether the design decisions that determine correctness, performance, and reliability are sound.

This is the skill for questions like: "Will this lose data?" "What happens when Google Sheets is down?" "Is the worker's retry logic correct?" "What happens when the producer is faster than the consumer?"

Reusable standalone, or delegated to by `review`.

## Inputs

Same as `architecture-review`:

- Code changes (PR diff, branch range, file list)
- System description (verbal or diagrammatic)
- Full codebase (standalone review)
- Delegation context from `review`

## Passes

Run all eight passes. Skip individual checks when they have no signal, but do not skip entire passes without stating why.

### Pass 1: Consistency model

What consistency guarantees does the system provide, and are they appropriate?

**Evaluate:**

- Is the consistency model stated or implicit?
- At each data boundary, what does the consumer assume about freshness, ordering, and completeness?
- Do those assumptions match what the producer guarantees?

**Check for:**

- Read-your-own-writes violations (write data, read it back through a different path, get stale data)
- Phantom consistency (appears consistent under normal load, diverges under pressure)
- Mixed consistency levels without explicit boundaries


### Pass 2: Durability and data loss

What happens to data in flight when something crashes, restarts, or overflows?

**Evaluate:**

- For each data path, classify the delivery guarantee: at-most-once, at-least-once, or exactly-once
- Is the chosen guarantee appropriate for the data's value?
- What's the blast radius of data loss?
- Is data loss detectable?

**Check for:**

- Unbounded buffers that will exhaust memory
- In-memory state with no persistence that's expensive to rebuild
- "Fire and forget" writes to external systems with no ack or retry
- Implicit exactly-once assumptions


### Pass 3: Latency budget

Does the system have latency targets, and do the components fit within them?

**Evaluate:**

- Is there an end-to-end latency target?
- Are the per-stage budgets based on measurements or assumptions?
- What happens when a stage exceeds its budget?

**Check for:**

- Sequential external calls that compound latency (Cognito → DB → S3 → SES in one request)
- Batching intervals that dominate latency
- Network round-trips in critical paths
- CloudFront → App Runner proxy adding latency vs direct


### Pass 4: Capacity and resource sizing

Will this system fit within its resource envelope at production scale?

**Evaluate:**

- Are resource requirements stated?
- What happens when a resource limit is hit?

**Back-of-envelope checks:**

- **DB connections**: 15 max pool. Worker uses connections. How many concurrent requests before pool exhaustion?
- **App Runner**: Single instance. What's the concurrent request limit? When does it need to scale?
- **Google Sheets API**: Rate limits (100 requests/100 seconds per user). How many plants before hitting limits?
- **SES**: Sending limits per second/day. How many requests before throttled?
- **S3**: Effectively unlimited, but presigned URL generation has latency

### Pass 5: Failure semantics

What does each component promise when things go wrong?

**Evaluate:**

- For each component, what are the failure modes?
- For each failure mode, what's the stated or implicit behavior?
- Is the failure behavior appropriate?

**Check for:**

- Retry without backoff (retry storm)
- Retry without idempotency check
- Catch-all error handling that swallows meaningful errors
- Missing circuit breakers on external dependencies
- Cascading failure paths
- Partial failure ambiguity ("did the write succeed or not?")


### Pass 6: Ordering guarantees

Does the system preserve the ordering that correctness requires?

**Evaluate:**

- At each stage, is input ordering preserved in the output?
- If ordering is not preserved, does the consumer tolerate out-of-order delivery?

**Check for:**

- Parallel processing that destroys ordering
- Retry that inserts events out of order
- Ordering assumptions in display logic


### Pass 7: Clock and time

Are the system's assumptions about time valid?

**Evaluate:**

- Which clock source is used?
- Are timestamps generated at the point of observation?
- Are timezone assumptions consistent?

**Check for:**

- Wall-clock timestamps compared across different systems
- Timestamps taken at the wrong point
- Timezone confusion (UTC in DB, local time in UI, mixed in Google Sheets)


### Pass 8: Backpressure and flow control

When a downstream component is slower than upstream, what happens?

**Evaluate:**

- At each producer-consumer boundary, what happens when the consumer can't keep up?
- Is backpressure explicit or implicit?
- Is there a pressure relief valve?

**Check for:**

- Unbounded queues between components
- Head-of-line blocking
- Backpressure that propagates to end users


## Scoring rubric

Same as `architecture-review`:

- `Likelihood` (1-3): probability this gap causes problems
- `Impact` (1-3): blast radius if hit
- `Detectability` (1-3): how likely caught before production

Severity mapping:

- `P0`: design flaw that will cause data loss, correctness failure, or production impact under normal operation
- `P1`: flaw likely to cause problems under production load, during failure recovery, or at scale
- `P2`: weakness that will cause operational friction, tech debt, or incorrect assumptions
- `P3`: quality concern or unlikely-but-worth-noting risk

## Output format

### 1. Design summary

2-3 sentences: what the system does, overall assessment, and the single most important finding.

### 2. Design decisions map

A table of the key design decisions identified, their current state (explicit/implicit, validated/assumed), and the pass that evaluates them.

### 3. Findings

Ordered by severity. For each:

```
[P0|P1|P2|P3] <title>

Pass: <which pass>
Decision: <which design decision is affected>

Evidence: what the design says or assumes
Gap: what's missing, incorrect, or unvalidated
Risk: what happens if unaddressed (under what conditions)
Fix direction: minimal change that resolves this
```

### 4. Open questions

Decisions that could change findings or system viability.

### 5. Residual risks

Risks inherent to the chosen design, even if all findings are addressed.

## Caller integration

- **From `review` (PR)**: Focus on passes relevant to the changed code. If error handling changed, run Pass 5. If a buffer or queue changed, run Pass 4 and Pass 8. Don't run all eight passes for a one-file change.
- **Standalone**: Full output as described above.

## Constraints

- Evaluate design decisions, not implementation quality.
- Don't demand NASA-grade rigor for an early-stage product. Match analysis depth to blast radius.
- Give credit for explicit decisions. "We accept eventual consistency for Sheets sync" is valid, not a finding.
- Flag implicit decisions. If the system silently provides at-most-once delivery without stating it, call that out.
