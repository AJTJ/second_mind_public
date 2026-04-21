#!/bin/bash
# Wait for Ollama to be ready, then pull the embedding model.
# Run after `docker compose up -d`.

set -euo pipefail

echo "Waiting for Ollama..."
for i in $(seq 1 60); do
    if docker exec sm-ollama ollama list >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

echo "Pulling embedding model (qwen3-embedding:4b, ~2.5GB on first run)..."
docker exec sm-ollama ollama pull qwen3-embedding:4b

echo "Model ready."
