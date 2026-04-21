# Second Mind — task runner
# Requires: mise (tool manager), just (this file), lefthook (git hooks)

set dotenv-load := false

# --- Tooling ---

# Verify mise is available
[private]
require-mise:
    @which mise > /dev/null 2>&1 || (echo "mise not found. Install: curl https://mise.run | sh" && exit 1)

# Install all dependencies and hooks
install: require-mise
    mise install
    lefthook install
    @echo "Tools installed. Run 'just doctor' to verify."

# Check all required tools are present
doctor:
    @echo "Checking tools..."
    @which mise > /dev/null 2>&1 && echo "  ✓ mise" || echo "  ✗ mise"
    @which just > /dev/null 2>&1 && echo "  ✓ just" || echo "  ✗ just"
    @which lefthook > /dev/null 2>&1 && echo "  ✓ lefthook" || echo "  ✗ lefthook"
    @which gitleaks > /dev/null 2>&1 && echo "  ✓ gitleaks" || echo "  ✗ gitleaks"
    @which bd > /dev/null 2>&1 && echo "  ✓ beads (bd)" || echo "  ✗ beads (bd)"
    @which cargo > /dev/null 2>&1 && echo "  ✓ cargo" || echo "  ✗ cargo"
    @echo "Done."

# --- Build (Rust) ---

# Format all Rust code
fmt:
    cd intake-engine && cargo fmt

# Check compilation
check:
    cd intake-engine && cargo check

# Lint with clippy
clippy:
    cd intake-engine && cargo clippy -- -D warnings

# Run tests
test:
    cd intake-engine && cargo test

# Release build
build:
    cd intake-engine && cargo build --release

# --- Learning Engine ---

ie_bin := justfile_directory() / "intake-engine/target/release/intake-engine"
ie_cmd := "docker exec intake-engine intake-engine"
compose := "cd docker && docker compose"

# Start everything
up:
    #!/usr/bin/env bash
    set -euo pipefail
    {{ compose }} up -d
    echo "Waiting for Neo4j..."
    for i in $(seq 1 60); do
        docker exec sm-neo4j neo4j status 2>&1 | grep -q "running" && break
        sleep 1
    done
    echo "Waiting for Postgres..."
    for i in $(seq 1 30); do
        docker exec sm-postgres pg_isready -U cognee > /dev/null 2>&1 && break
        sleep 1
    done
    echo "Waiting for graph-engine..."
    for i in $(seq 1 45); do
        docker exec graph-engine curl -sf http://localhost:8000/health > /dev/null 2>&1 && break
        sleep 1
    done
    docker exec graph-engine curl -sf http://localhost:8000/health > /dev/null 2>&1 || { echo "graph-engine failed to start"; exit 1; }
    echo "Waiting for intake-engine..."
    for i in $(seq 1 30); do
        docker inspect -f '{{ '{{' }}.State.Running{{ '}}' }}' intake-engine 2>/dev/null | grep -q "true" && break
        sleep 1
    done
    docker inspect -f '{{ '{{' }}.State.Running{{ '}}' }}' intake-engine 2>/dev/null | grep -q "true" || { echo "intake-engine failed to start"; exit 1; }
    just status

# Stop everything
down:
    {{ compose }} down
    @echo "Stopped."

# Show service status
status:
    #!/usr/bin/env bash
    echo "--- Services ---"
    for svc in intake-engine graph-engine sm-neo4j sm-postgres sm-ollama; do
        running=$(docker inspect -f '{{ '{{' }}.State.Running{{ '}}' }}' "$svc" 2>/dev/null)
        name=$(echo "$svc" | sed 's/^sm-//')
        if [ "$running" = "true" ]; then
            echo "  $name: up"
        else
            echo "  $name: down"
        fi
    done

# Rebuild and restart (after code changes)
restart:
    {{ compose }} up -d --build intake-engine graph-engine

# Start Cognee for comparison (optional, separate from default stack)
cognee-up:
    {{ compose }} --profile cognee up -d cognee-backend cognee-postgres

# Pull the embedding model (required on first run)
pull-model:
    docker/init-ollama.sh

# View logs
logs *args="--tail 50":
    {{ compose }} logs {{ args }}

# Ingest a file through the intake engine (logged + replayable)
ingest file dataset="research" prompt="research":
    #!/usr/bin/env bash
    filename=$(basename "{{ file }}")
    cp "{{ file }}" data/"$filename"
    {{ ie_cmd }} ingest /app/data/"$filename" --dataset {{ dataset }} --prompt {{ prompt }}

# Search the knowledge base (pretty printed)
search query dataset="":
    #!/usr/bin/env bash
    if [ -n "{{ dataset }}" ]; then
        result=$({{ ie_cmd }} search "{{ query }}" --dataset "{{ dataset }}" 2>&1)
    else
        result=$({{ ie_cmd }} search "{{ query }}" 2>&1)
    fi
    echo "$result" | python3 -c "
    import sys, json
    try:
        data = json.load(sys.stdin)
        for ds in data:
            name = ds.get('dataset_name', '?')
            results = ds.get('search_result', [])
            if not results:
                print(f'  [{name}] No results.')
                continue
            for r in results:
                text = r.get('text', '')
                # Render the markdown text cleanly
                print(f'\033[1;34m[{name}]\033[0m')
                print(text[:2000])
                if len(text) > 2000:
                    print(f'  ... ({len(text)} chars total)')
                print()
    except json.JSONDecodeError:
        print(sys.stdin.read())
    "

# Keyword search (full-text, no embeddings, fast)
search-keyword query:
    {{ ie_cmd }} search "{{ query }}" -t KEYWORD

# Search the knowledge base (raw JSON)
search-raw query dataset="":
    #!/usr/bin/env bash
    if [ -n "{{ dataset }}" ]; then
        {{ ie_cmd }} search "{{ query }}" --dataset "{{ dataset }}"
    else
        {{ ie_cmd }} search "{{ query }}"
    fi

# Show intake log (what's been ingested)
log limit="20":
    {{ ie_cmd }} log --limit {{ limit }}

# Replay all logged entries (or a specific one)
replay entry_id="" prompt="":
    #!/usr/bin/env bash
    args=""
    if [ -n "{{ entry_id }}" ]; then args="{{ entry_id }}"; fi
    if [ -n "{{ prompt }}" ]; then args="$args --prompt {{ prompt }}"; fi
    {{ ie_cmd }} replay $args

# List available processing prompts
prompts:
    {{ ie_cmd }} prompts

# Compact the intake log (deduplicate in-place)
compact:
    {{ ie_cmd }} compact

# [DEPRECATED] Explorer binary — use /explore skill in Claude Code instead.
# The skill runs the same research methodology without a separate API bill.
# Keeping the recipe for backward compatibility with existing review data.
explore *args:
    @echo "DEPRECATED: Use the /explore skill in Claude Code instead."
    @echo "The explorer binary has been replaced by the /explore skill."
    @echo "To run: type '/explore <your research directive>' in Claude Code."
    @exit 1

# Wipe graph engine data and reset intake log
wipe:
    {{ ie_cmd }} wipe

# First-run setup: wipe volumes, start services, run migrations, pull embedding model
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Wiping existing volumes for clean start..."
    {{ compose }} down -v 2>/dev/null || true
    echo "Starting databases and Ollama..."
    {{ compose }} up -d postgres neo4j ollama
    echo "Waiting for Postgres..."
    for i in $(seq 1 30); do
        docker exec sm-postgres pg_isready -U cognee > /dev/null 2>&1 && break
        sleep 1
    done
    echo "Waiting for Neo4j..."
    for i in $(seq 1 60); do
        docker exec sm-neo4j neo4j status 2>&1 | grep -q "running" && break
        sleep 1
    done
    echo "Pulling embedding model (~2.5GB on first run)..."
    just pull-model
    echo "Starting graph engine and intake engine..."
    {{ compose }} up -d graph-engine intake-engine
    echo "Waiting for graph engine..."
    for i in $(seq 1 60); do
        docker exec graph-engine curl -sf http://localhost:8000/health > /dev/null 2>&1 && break
        sleep 1
    done
    docker exec graph-engine curl -sf http://localhost:8000/health > /dev/null 2>&1 || { echo "Graph engine failed to start."; exit 1; }
    echo "Waiting for intake engine..."
    for i in $(seq 1 15); do
        docker inspect -f '{{ '{{' }}.State.Running{{ '}}' }}' intake-engine 2>/dev/null | grep -q "true" && break
        sleep 1
    done
    echo "Ready."
    just status

# Stop Cognee comparison stack
cognee-down:
    {{ compose }} --profile cognee down

# Run integration tests (embedding model verification)
test-integration: build
    ./tests/embedding_test.sh

# Run integrate on a dataset
integrate dataset prompt="research":
    {{ ie_cmd }} integrate {{ dataset }} --prompt {{ prompt }}

# Compare search results between two dataset versions
compare query dataset_a dataset_b:
    {{ ie_cmd }} compare "{{ query }}" {{ dataset_a }} {{ dataset_b }}

# View search history
search-history limit="20":
    {{ ie_cmd }} search-history --limit {{ limit }}

# Delete a specific dataset from Cognee (logged)
delete-dataset dataset:
    {{ ie_cmd }} delete-dataset {{ dataset }}

# --- Worktree Management ---

# Create a worktree in .wt/ for isolated work
worktree name branch="":
    #!/usr/bin/env bash
    set -euo pipefail
    branch="{{ branch }}"
    if [ -z "$branch" ]; then
        branch="{{ name }}"
    fi
    git worktree add ".wt/{{ name }}" -b "$branch" 2>/dev/null || \
        git worktree add ".wt/{{ name }}" "$branch"
    mise trust ".wt/{{ name }}" 2>/dev/null || true
    echo "Worktree created: .wt/{{ name }} (branch: $branch)"
    echo "cd .wt/{{ name }}"

# Remove a worktree
worktree-rm name *flags="":
    git worktree remove ".wt/{{ name }}" {{ flags }}
    @echo "Worktree removed: .wt/{{ name }}"

# --- Beads (Task Tracking) ---

# Run bd with clean env (prevents env var leaks between sessions)
bd *args:
    #!/usr/bin/env bash
    unset BEADS_AGENT BEADS_SESSION BEADS_CONTEXT
    export LEFTHOOK=0
    bd {{ args }}

# Configure custom beads statuses
beads-configure:
    just bd config set-custom-statuses awaiting_review

# Find unblocked work
beads-ready:
    just bd ready

# Show all open work
beads-status:
    just bd status

# Lint the beads database
beads-lint:
    just bd lint

# Find potential duplicates
beads-dups:
    just bd find-duplicates --threshold 0.4

# Show stale beads
beads-stale:
    just bd stale

# Where am I? Current bead context
beads-where:
    just bd where

# Quick capture
beads-todo *args:
    just bd todo {{ args }}

# Handoff comments
beads-comments *args:
    just bd comments {{ args }}

# Store durable knowledge
beads-remember *args:
    just bd remember {{ args }}

# Recall stored knowledge
beads-recall *args:
    just bd recall {{ args }}

# Show bead history
beads-history *args:
    just bd history {{ args }}

# Install beads git hooks
beads-hooks:
    just bd hooks install --chain

# Ingest without integration (fast, cheap — vector search only, no graph extraction)
ingest-simple file dataset="research":
    #!/usr/bin/env bash
    filename=$(basename "{{ file }}")
    cp "{{ file }}" data/"$filename"
    {{ ie_cmd }} ingest /app/data/"$filename" --dataset {{ dataset }} --prompt {{ dataset }} --no-integrate
