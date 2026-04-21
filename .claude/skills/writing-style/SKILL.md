---
name: writing-style
description: Use when writing any prose that other people will read — commit bodies, issue writeups, code comments, or documentation. Enforces directness and clarity.
metadata:
  version: 1.0.0
---

# Writing Style

Get to the point. Every sentence should deliver information, not justify its own existence. Be descriptive when the subject warrants it, but don't narrate your reasoning process or explain where an idea came from unless the origin is the point.

## Voice

Write like you're talking to a colleague who already has context. Direct, specific, no ceremony. Sentence fragments are fine. Technical depth is welcome. Conversational scaffolding is not.

Good: "Retry logic was swallowing timeouts. Now it propagates them."

Good: "The worker spawns at startup and polls every 5s. Stuck requests get reclaimed after 5 minutes."

Bad: "This change introduces improved error handling semantics that ensure timeout conditions are properly propagated through the retry mechanism rather than being silently consumed."

Bad: "After considering several approaches, we decided to use a polling interval because it provides a natural backpressure mechanism that aligns well with our concurrency model."

## What to cut

- Conversational justification: "The reason we do X is..." — just state X and its effect.
- Meta-commentary: "It's worth noting", "as mentioned above", "to summarize". The reader can follow the text.
- Preamble: "In order to improve reliability, we..." — start with what changed.
- Restating what you just said in different words.
- Previewing what you're about to say.

## Common pitfalls

- Don't overuse em dashes. One or two per document is fine. Prefer commas, periods, or parentheses. If you reach for an em dash, consider starting a new sentence instead.
- Don't default to triplets. "X, Y, and Z" in every clause gets repetitive. Sometimes two items or four is the right number.
- Don't oversell. Say what the thing does, not how important it is. Drop "foundational", "game-changer", "at the heart of".
- Don't lean on formulaic transitions. "Furthermore", "Moreover", "Additionally" — if the next sentence follows logically, it doesn't need a signpost.
- Don't stack adjectives. "A lightweight, configurable, extensible framework" — pick the one that matters.
- Don't default to `**Bold Title:** explanation` bullet lists. A plain sentence or a table often works better.

## Structural variety

Vary paragraph length and format. Some paragraphs are one sentence. Mix prose, code snippets, and tables naturally rather than following a rigid template.

## README-specific rules

READMEs talk to busy people who want to know what the thing does, not how you built it.

- Lead with what the system does in one sentence. Not what it is.
- Show, don't describe. A diagram or code block beats a paragraph.
- Cut every sentence that starts with "This" referring to the project itself.
- No "features" lists. Features are things marketers write. Show the system working.
- If a section has more prose than code, it's probably too long.
- The reader decides if it's interesting. Your job is to be clear, not persuasive.
