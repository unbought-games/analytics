#!/usr/bin/env bash
set -euo pipefail
name="analytics-query-test-$(date +%s)-$$"
cleanup() { docker rm -f "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT
# Trust is confined to this disposable, loopback-bound fixture container.
docker run --name "$name" --rm -d -e POSTGRES_HOST_AUTH_METHOD=trust -p 127.0.0.1::5432 postgres:17 >/dev/null
for _ in $(seq 1 60); do
  if docker exec "$name" pg_isready -U postgres >/dev/null 2>&1; then break; fi
  sleep 0.2
done
port=$(docker port "$name" 5432 | sed 's/.*://')
export QUERY_TEST_DATABASE_URL="postgresql://postgres@127.0.0.1:${port}/postgres"
cargo test -p analytics-query -- --nocapture
