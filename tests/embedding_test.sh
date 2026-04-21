#!/usr/bin/env bash
# Integration test — verifies the intake engine → Cognee pipeline works end-to-end.
# Ingests via CLI, cognifies, searches, cleans up. All through the intake engine.
#
# Uses isolated DATA_DIR so test data never touches the production log.
# Dataset name enforced to start with "test_" as a safety guard.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
IE_BIN="${IE_BIN:-$PROJECT_DIR/intake-engine/target/release/intake-engine}"
DATASET="test_integration_$(date +%s)"

# Safety: dataset name must start with test_
[[ "$DATASET" == test_* ]] || { echo "ABORT: dataset '$DATASET' missing test_ prefix"; exit 1; }

# Isolated test environment — production log/library untouched
TEST_DIR=$(mktemp -d)
export DATA_DIR="$TEST_DIR/data"
export PROMPTS_DIR="$PROJECT_DIR/channels"

cleanup() {
    "$IE_BIN" delete-dataset "$DATASET" --force 2>/dev/null || true
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# Pre-flight
[ -x "$IE_BIN" ] || { echo "ERROR: binary not found at $IE_BIN — run 'just build'"; exit 1; }

passed=0
failed=0

check() {
    local name="$1"; shift
    echo -n "  $name ... "
    if "$@"; then echo "PASS"; passed=$((passed + 1))
    else echo "FAIL"; failed=$((failed + 1)); fi
}

echo "=== Integration Test (dataset: $DATASET) ==="

# --- Ingest two docs ---
DOC_A="$TEST_DIR/rust.txt"
DOC_B="$TEST_DIR/graphs.txt"
echo "Rust's ownership model eliminates data races at compile time. The borrow checker ensures references are always valid." > "$DOC_A"
echo "Knowledge graphs represent information as entities and relationships, capturing semantic connections between concepts." > "$DOC_B"

check "Ingest doc A" "$IE_BIN" ingest "$DOC_A" --dataset "$DATASET" --prompt research --no-cognify
check "Ingest doc B" "$IE_BIN" ingest "$DOC_B" --dataset "$DATASET" --prompt research --no-cognify

# --- Cognify ---
check "Cognify" "$IE_BIN" cognify "$DATASET" --prompt research
sleep 2

# --- Search ---
check "Search finds rust doc" bash -c "$IE_BIN search 'Rust ownership borrow checker' --dataset $DATASET 2>/dev/null | grep -qi 'ownership\|borrow'"
check "Search finds graph doc" bash -c "$IE_BIN search 'knowledge graph relationships' --dataset $DATASET 2>/dev/null | grep -qi 'graph\|relationship\|entities'"

# --- Cleanup ---
check "Delete test dataset" "$IE_BIN" delete-dataset "$DATASET" --force

# --- Isolation check ---
PROD_LOG="$PROJECT_DIR/data/intake.jsonl"
check "Production log untouched" bash -c "! grep -q '$DATASET' '$PROD_LOG' 2>/dev/null"

echo ""
echo "=== $passed passed, $failed failed ==="
[ "$failed" -eq 0 ]
