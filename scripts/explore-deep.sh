#!/usr/bin/env bash
# Chained research loop — each iteration researches the gaps from the previous.
# Usage: explore-deep.sh "directive" [iterations] [dataset] [prompt]
set -euo pipefail

DIRECTIVE="$1"
ITERATIONS="${2:-3}"
DATASET="${3:-research}"
PROMPT="${4:-research}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
EXPLORER="$PROJECT_DIR/explorer/target/release/explorer"
IE_BIN="${INTAKE_ENGINE_BIN:-$PROJECT_DIR/intake-engine/target/release/intake-engine}"

# Export so the explorer's check_knowledge tool can find the intake engine
export INTAKE_ENGINE_BIN="$IE_BIN"

current_directive="$DIRECTIVE"

for i in $(seq 1 "$ITERATIONS"); do
    echo ""
    echo "=== Iteration $i/$ITERATIONS ==="
    echo "Directive: ${current_directive:0:120}..."
    echo ""

    # Run explorer
    cd "$PROJECT_DIR"
    "$EXPLORER" run --async "$current_directive" 2>&1

    # Find the latest review file
    latest=$(ls -t "$PROJECT_DIR/data/reviews/"*.json 2>/dev/null | head -1)
    if [ -z "$latest" ]; then
        echo "ERROR: No review file produced"
        exit 1
    fi
    echo "Review: $(basename "$latest")"

    # Mark as approved (status only — don't use `explorer approve` which double-ingests)
    python3 -c "
import json
with open('$latest') as f: d = json.load(f)
d['status'] = 'approved'
with open('$latest', 'w') as f: json.dump(d, f, indent=2)
"

    # Ingest into knowledge graph (single path — no double ingestion)
    "$IE_BIN" ingest "$latest" --dataset "$DATASET" --prompt "$PROMPT" 2>&1

    # Extract gaps for next iteration
    if [ "$i" -lt "$ITERATIONS" ]; then
        gaps=$(python3 -c "
import json, sys
d = json.load(open('$latest'))
fj = d.get('findings_json') or {}
gaps = fj.get('gaps', [])
follow_ups = fj.get('suggested_follow_ups', [])
items = gaps + follow_ups
if not items:
    print('Expand on the findings so far with more specific data, numbers, and sources.')
else:
    print('Research these gaps, building on existing knowledge: ' + '; '.join(items[:5]))
")
        echo ""
        echo "Gaps → next directive: ${gaps:0:150}..."
        current_directive="$gaps"

        # Cooldown between iterations — rate limit recovery + cognify completion
        echo "Waiting 90s for rate limit recovery and cognify completion..."
        sleep 90
    fi
done

echo ""
echo "=== Deep research complete: $ITERATIONS iterations ==="
