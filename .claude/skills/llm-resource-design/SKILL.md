---
name: llm-resource-design
description: Evaluate LLM agent architecture for output quality per token and cost efficiency. Covers context engineering, rate limits, cost curves, memory tiering, phase design, and failure modes. Reusable from review (PR) or standalone.
user-invocable: true
metadata:
  version: 1.0.0
---

# LLM Resource Design Review

Unified evaluation of LLM agent architecture — whether the design maximizes output quality per token spent, stays within resource limits, and degrades gracefully when limits are hit.

Grounded in: Anthropic's context engineering guidance, ACON (arxiv 2510.00615), MemGPT (arxiv 2310.08560), "Lost in the Middle" (arxiv 2307.03172), pointer-based context overflow (arxiv 2511.22729).

Core principle: **every token in the context window competes for the model's attention. Signal gets drowned by accumulation.** The goal is the smallest set of high-signal tokens that maximizes both output quality and cost efficiency.

## Inputs

Accept one of:

- **Code changes**: PR diff touching agent loops, API clients, context management, tool handlers, retry logic
- **Full agent codebase**: standalone review of all LLM-consuming code paths
- **Delegation context**: from `/review`, scoped to relevant passes

## Reference: Pricing and Limits

Before running passes, establish the pricing and rate limit context:

| Model | Input | Output |
|-------|-------|--------|
| Claude Sonnet 4 | $3/M tokens | $15/M tokens |
| Claude Opus 4 | $15/M tokens | $75/M tokens |
| Claude Haiku 3.5 | $0.80/M tokens | $4/M tokens |

Token estimation: 1 token ~ 4 characters. If API or pricing is unknown, state assumptions.

## Passes

Eight passes in two groups. Run all passes; skip individual checks with stated reason.

### Group A: Architecture Quality

These passes evaluate whether the agent's design produces the best possible *output quality*. Cost savings are a side effect — the primary concern is: does this architecture let the model do its best work?

#### Pass 1: Context quality

Is the context filled with high-signal tokens, or is signal diluted by noise?

**Evaluate:**
- What percentage of context at iteration N is *actionable* for the current step vs accumulated history? Estimate the signal-to-noise ratio: useful-facts-tokens / total-tokens.
- Are tool outputs injected raw or processed first? Compute: average tool result size vs estimated useful content. A 3K-token web page with ~150 tokens of useful facts is a 5% signal ratio.
- Per "Lost in the Middle" (arxiv 2307.03172): LLMs show 30%+ accuracy drop for information in middle positions of context. Map where key information lands at each iteration — is it in the danger zone?
- Does the system prompt remain effective as context grows? At 50K+ tokens, system prompt effectiveness drops ~15%. At what iteration does the agent's context cross this threshold?

**Check for:**
- **Raw data injection**: tool outputs dumped verbatim into context without extraction or summarization. Measure: average tool result size in tokens vs useful facts extracted.
- **Signal dilution**: each iteration adds low-signal tokens (boilerplate, navigation text, repeated instructions) that crowd out high-signal content. Estimate the cumulative noise ratio at iteration N.
- **Positional decay**: important information (key findings, system instructions) lands in the middle of a growing context where attention is weakest. Map the positions.
- **System prompt erosion**: system instructions at position 0 lose influence as context grows. Is the system prompt reinforced at phase transitions?
- **No relevance filtering**: all prior tool results stay in context even when they're irrelevant to the current phase. Does the agent carry SURVEY-phase web scrapes into SYNTHESIZE?

**What good looks like:**
- Tool results are fact-extracted before entering context (e.g., "extract 3-5 key claims from this page")
- Context organized so current-step-relevant information is at the end (recency position, strongest attention)
- Older history compressed into summaries that preserve key facts but drop raw data
- System prompt reinforced or re-injected at phase transitions

#### Pass 2: Memory architecture

Does the agent manage context like an OS manages memory, or does it treat the context window as a flat append-only buffer?

**Evaluate:**
- Is there a distinction between working memory (current step) and reference memory (prior findings)?
- Can the agent page information in and out of context? Can it query external storage on demand?
- What is the eviction policy? FIFO? Relevance-based? None?
- Can the agent inspect or edit its own context? (MemGPT's key innovation: self-directed memory editing via tool calls)

**Reference architectures** (weakest → strongest):
- **Flat history** (anti-pattern): all messages appended, all resent every call. Context grows quadratically. No eviction.
- **Sliding window**: only the last K messages kept. Simple, preserves recency, but loses early context entirely.
- **Summarize-then-drop**: after each phase or N iterations, compress history into a summary, drop raw messages. Preserves key facts, bounded growth. (ACON, arxiv 2510.00615: 26-54% token reduction, 95%+ accuracy preserved)
- **Hierarchical / MemGPT-style**: main context (working memory) + external context (archival). Agent explicitly pages data in/out via tool calls. (MemGPT, arxiv 2310.08560)
- **Pointer-based**: large tool outputs stored externally, referenced by ID in context. Agent requests full content only when needed. (arxiv 2511.22729)

**Check for:**
- **No memory tiering**: everything lives in the context window with no external storage or retrieval mechanism
- **No eviction**: old messages never leave context, even when they have zero relevance to current work
- **No summarization boundary**: the agent never synthesizes intermediate results — raw data accumulates until final synthesis
- **Missing self-management**: the agent cannot inspect or edit its own context (can't decide what to keep or discard)

#### Pass 3: Phase design

Is the workflow decomposed into phases with clear boundaries, or is it a single undifferentiated loop?

**Evaluate:**
- Are there distinct phases with different context needs? (A planning phase needs different context than a synthesis phase)
- Does context carry over between phases verbatim, or is it compressed at phase transitions?
- Is there a token budget per phase, or does early work consume the budget leaving nothing for later phases? Estimate: what fraction of total tokens is consumed by each phase?
- Can phases run with different models? Data gathering doesn't need the same reasoning depth as synthesis.
- Does each phase have a stopping condition based on information sufficiency, or just an iteration count?

**Check for:**
- **Phase bleed**: all phases share one growing context with no compression at transitions. Survey-phase web scrapes are still in raw form during synthesis.
- **Front-heavy consumption**: early exploration phases consume most of the token budget with raw data collection, leaving synthesis under-resourced. Estimate the percentage split.
- **No intermediate synthesis**: the agent collects data for N iterations then tries to synthesize everything at once from a massive, noisy context, instead of synthesizing incrementally at each phase boundary.
- **Fixed model for all phases**: using the same expensive model for data gathering (where a cheaper model or non-LLM tool would suffice) and for reasoning (where model quality matters). Calculate the cost difference.
- **No phase-gated stopping**: the agent runs a fixed number of iterations regardless of whether it has sufficient information. No detection of "I have enough data, time to synthesize."

**What good looks like:**
- Phase transitions trigger context compression (summarize findings, drop raw tool results)
- Token budgets allocated per phase (e.g., 40% survey, 30% deepen, 20% challenge, 10% synthesize)
- Cheaper models or non-LLM tools handle data gathering; expensive models handle reasoning and synthesis
- Phases have quality-aware stopping: if 3 consecutive iterations add no new facts, move to next phase

#### Pass 4: Tool output management

How does the agent handle data returned by tools? This directly determines the signal-to-noise ratio of the context.

**Evaluate:**
- What is the size distribution of tool outputs? Measure each tool's typical output in tokens.
- What is the signal-to-noise ratio per tool? Estimate: useful-facts / total-tokens for each tool type.
- Are tool outputs truncated, summarized, fact-extracted, or stored externally before entering context?
- Does the agent re-fetch tool results it already has, or cache them?
- Are tool results structured (JSON with named fields) or unstructured (prose, raw HTML)?

**Check for:**
- **Unbounded tool output**: tools that return arbitrary-length content with no truncation or processing before it enters context
- **Raw HTML/page dumps**: web scraping tools that inject full page content including navigation, ads, cookie banners, boilerplate. Typical signal ratio: 10-20%.
- **Redundant fetches**: the agent calls the same tool with similar queries multiple times, each adding duplicative content to context
- **No output schema**: tool results are free-form text rather than structured data that the agent can selectively reference
- **Context-heavy tools**: tools where output size exceeds useful information content by 5x+. Calculate the ratio for each tool.

**What good looks like:**
- Tool outputs processed by an extraction step before entering context: "extract the 3-5 key facts from this page" (can use a cheap model)
- Large outputs stored externally with a pointer/summary in context (arxiv 2511.22729)
- Tools return structured data (JSON with named fields) so the agent can reference specific parts
- Duplicate or near-duplicate tool calls detected and cached
- Each tool's contribution to context is proportional to its information value

### Group B: Cost and Resilience

These passes evaluate whether the agent's resource consumption is bounded, predictable, and survivable. The primary concern is: can this run reliably without surprise costs, crashes, or silent quality loss?

#### Pass 5: Context growth

How does context grow over time? Is it bounded?

**Evaluate:**
- Identify every data structure that accumulates across iterations (message history, buffers, logs)
- For each accumulation point, determine the growth function:
  - **Constant**: fixed-size context per call (stateless)
  - **Linear**: O(n) — context grows proportionally to iterations
  - **Quadratic**: O(n^2) — each iteration adds data AND resends all prior data
  - **Unbounded**: no maximum, grows until external limit is hit
- Does the growth pattern change after the fixes in Group A? (e.g., phase compression changes quadratic to step-function)

**Compute:**
- `tokens_at_iteration(k)` = base_tokens + k * per_iteration_tokens
- `iteration_hitting_context_limit` = (context_window - base_tokens) / per_iteration_tokens
- `iteration_hitting_rate_limit` = solve for k where single request exceeds per-minute budget
- `total_tokens_across_N_iterations` = N * base_tokens + N*(N+1)/2 * per_iteration_tokens

#### Pass 6: Rate limit exposure

When does the code hit throttling, and what happens?

**Evaluate:**
- Identify every external API call and its rate limit (tokens/min, requests/min, requests/day)
- For each call site, compute the request size at each iteration
- Account for nested/compound calls: if tool A calls API B which calls API C, sum the rate limit consumption across all levels
- Are rate limits shared across concurrent runs? (Two explorer instances hit the same org limit)

**Check for:**
- **No 429 error handling**: API calls that crash on rate limit responses instead of retrying
- **No retry with backoff**: missing exponential backoff for rate-limited responses
- **No request pacing**: burst of API calls with no delay between them
- **Nested calls compounding rate consumption**: single logical operation triggering multiple API requests (e.g., `research` tool making its own Claude call inside the main agent loop)
- **Rate limit arithmetic mismatch**: code assumes N calls/min but math shows it hits the limit at N/3

**Compute:**
- `requests_per_run` = number of API calls in a typical execution
- `tokens_per_minute_at_iteration(k)` = tokens_at_iteration(k) / estimated_minutes_per_iteration
- `safe_iterations` = max k where tokens_per_minute_at_iteration(k) <= rate_limit

#### Pass 7: Cost estimation

What does this cost per run, per pipeline, per month?

**Evaluate:**
- For each API call site, compute input cost, output cost, and nested call costs at each iteration
- Compute the cost curve: cost as a function of iterations, not just a single number
- Distinguish typical run (median iterations) from worst case (max iterations)
- If the agent is part of a pipeline (e.g., 6-phase research), compute the full pipeline cost

**Check for:**
- **No cost cap or budget limit**: code runs to completion regardless of accumulated cost
- **Cost-insensitive retry**: retry that resends the full (expensive) context each attempt
- **Worst-case cost surprise**: >3x gap between typical and worst-case cost with no safeguard
- **Hidden cost multipliers**: nested API calls, redundant reprocessing, fan-out patterns
- **No cost observability**: no logging or metrics that would reveal actual spend after a run

**Compute:**
- `cost_at_iteration(k)` = input_cost(k) + output_cost(k) + nested_costs(k)
- `total_run_cost(N)` = sum(k=1..N) cost_at_iteration(k)
- `typical_run_cost` = total_run_cost(median_iterations)
- `worst_case_cost` = total_run_cost(max_iterations)
- `pipeline_cost` = sum of all runs in a multi-phase pipeline

#### Pass 8: Failure modes and degradation

What happens when limits are hit? Does the system know when quality is declining?

**Evaluate:**
- For each limit (context window, rate limit, budget, service quota), trace what happens when exceeded
- Classify each failure mode: crash, silent truncation, retry storm, graceful degradation, or data loss
- Determine if partial results are preserved (can the user get value from an interrupted run?)
- Can the agent detect that its output quality is declining? Is there any feedback loop?
- Does the system log enough to diagnose quality issues post-hoc?

**Check for:**
- **Crash on 429**: rate limit response treated as fatal error
- **No partial result preservation**: if agent crashes at iteration 15/30, are iterations 1-14 lost?
- **Retry without state preservation**: restart from scratch instead of resume
- **Cascading failure**: rate limit on one API causes timeout on another
- **Silent degradation**: quality drops as context grows but nothing signals this to the user or system. The agent keeps producing outputs that look complete but are based on noisy, position-degraded context.
- **No context size tracking**: the agent doesn't know how full its context window is
- **No circuit breaker**: no mechanism to stop the agent when it's producing diminishing returns (e.g., iteration 20 adds nothing that iteration 10 didn't already have)
- **No quality self-assessment**: the agent never evaluates whether its context is sufficient for the current task

**What good looks like:**
- Agent tracks context usage and warns when approaching thresholds
- Intermediate outputs evaluated (even heuristically) to detect quality decline
- Circuit breaker stops execution when marginal value drops below threshold
- Logs capture per-iteration token counts and a quality proxy (new-facts-per-iteration)
- Partial results are flushed to disk incrementally, not held in memory until completion

## Scoring rubric

- `Quality impact` (1-3): how much output quality is affected
- `Resource waste` (1-3): tokens or dollars spent without contributing
- `Fixability` (1-3): effort to change (1 = config, 3 = rewrite)

Severity:
- `P0`: actively degrades quality or will crash under normal usage
- `P1`: significant quality or cost inefficiency at moderate scale
- `P2`: missed optimization, works but wasteful
- `P3`: future improvement opportunity

## Output format

### 1. Summary

Three parts, each required:
- What the agent does (1 sentence)
- Critical quality finding from Group A (1 sentence — how output quality is affected)
- Critical cost/resilience finding from Group B (1 sentence — what breaks or what it costs)

### 2. Context flow diagram

Show how data moves through context across iterations/phases — current vs recommended.

### 3. Resource map

| Call site | API | Growth | Tokens iter 1 | Tokens iter N | Rate limit | Nested |
|-----------|-----|--------|---------------|---------------|------------|--------|

### 4. Cost curve

| Iter | Input tokens | Output tokens | Nested cost | Iter cost | Cumulative |
|------|-------------|---------------|-------------|-----------|------------|

### 5. Findings

Ordered by severity:

    [P0|P1|P2|P3] <title>

    Pass: <which pass>
    Location: <file:line>
    Current behavior: <what it does>
    Impact: <quality and/or cost effect, with numbers>
    Recommended pattern: <prior art reference>
    Fix direction: <minimal change>

### 6. Budget projection

- Typical run cost: $X
- Worst-case run cost: $Y
- Runs per $50/month: N
- Cost-dominant factor: what to optimize first

### 7. Recommended architecture

Sketch the target architecture incorporating all P0/P1 fixes. Reference which prior art each recommendation draws from.

### 8. Open questions

Assumptions that could change findings; design decisions needing empirical testing.

## Caller integration

- **From `/review` (PR)**: Focus on passes relevant to changed code. Context/memory changed → Group A. API/retry/cost changed → Group B. Return findings for merge.
- **Standalone**: Full output with both groups.

## Delegation triggers from `/review`

Delegate to `/llm-resource-design` when changes touch:
- Agent conversation loops or message history management
- API client code (HTTP calls to metered APIs)
- Tool result handling or output processing
- Phase transitions or workflow structure
- Context compression, summarization, or truncation logic
- Retry logic or error handling for external API calls
- Model selection or routing logic
- Token limit, max iteration, or budget configuration

## Constraints

- Compute with numbers. "Context grows" is not a finding. "3K tokens/iteration, hitting 30K rate limit at iteration 10" is.
- Ground recommendations in published research. Cite the paper.
- Match depth to blast radius. $0.02 script ≠ unattended agent loop.
- Credit existing safeguards (truncation, retry, caps).
- Flag implicit budgets. "Developer notices their bill" is not a safeguard.
