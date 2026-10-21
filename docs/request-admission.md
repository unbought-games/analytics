# Shared request admission

The Rust service admits at most **64 concurrent application requests** by default, shared across all router clones, HTTP administration, ingestion, MCP and extension routes. Set `ANALYTICS_MAX_IN_FLIGHT_REQUESTS` to an integer in `1..4096` before `serve` to choose another cap. Invalid/zero/out-of-range values fail startup. Configure it against the deployment's memory, database pool and export workload budget; raising this cap does not increase those underlying capacities.

The gate uses `Semaphore::try_acquire_owned`. When full, requests do not join a semaphore wait queue, authenticate, acquire a database connection or parse a JSON body. They receive HTTP **503**, `Retry-After: 1`, and:

```json
{"error":{"code":"overloaded","message":"Service request capacity is full; retry with bounded backoff"}}
```

HTTP and MCP use this same transport-level overload response. Clients should apply bounded backoff with jitter and preserve event IDs on ingestion retries. Per-module ingestion/source/export quotas and request-size limits remain in force; this common limit is an additional bound. `GET /healthz` and `HEAD /healthz` are exempt because liveness does no database/authentication/module work. `/readyz` is admitted normally because it acquires a database connection.

A request owns its permit while its handler runs. Once a handler returns response headers, the permit moves into a wrapper around the response `Body`. Streaming downloads therefore keep capacity occupied until the body reaches its end, reports an error, or is dropped. Dropping a cancelled handler future also releases its permit. Body size hints and frames are forwarded without collecting the response into memory. Finite empty responses release immediately. A slow consumer cannot cause the service to admit unlimited additional streaming producers.

The bound covers application handlers and active response bodies. It is not a limit on accepted TCP sockets, incomplete HTTP headers, kernel buffers, or the final bytes already handed to the HTTP transport. Connection/header timeouts and deployment proxy limits remain separate. Background jobs outlive their enqueue requests and retain their module-level job/concurrency budgets; this middleware does not replace those budgets. The gate does not promise FIFO fairness and does not queue overload work for later delivery.

## Composition API

Existing `analytics_server::router(state, extensions, oidc)` callers keep the default cap and require no DTO changes. Configured compositions call:

```rust
analytics_server::router_with_admission(state, extensions, oidc, max_in_flight)?
```

The result is `ApiResult<axum::Router>` and validates the cap. Preserve the full ingestion/query extension vector when integrating the binary change. The middleware surrounds the complete merged router, so direct MCP extension calls cannot bypass it. Security-response headers remain outside the gate and apply to overload responses as well.

## Focused validation

Commands:

```sh
cargo test --locked -p analytics-server --lib admission -- --nocapture
cargo clippy --locked -p analytics-server --all-targets -- -D warnings
cargo fmt --all --check
```

Five focused tests passed on 2026-10-21 with Rust 1.99.0:

1. A held handler saturates the shared gate; 60 MCP/admin/readiness requests receive structured 503 responses, each within a 250 ms test deadline and collectively within one second. Invalid JSON is rejected by admission before parsing. Liveness stays available. Cancelling the handler restores capacity.
2. A response that has returned headers and one streamed chunk continues occupying its permit. End-of-stream releases it. Dropping an unread body also releases it.
3. A streaming-body error releases capacity even while the errored body remains allocated.
4. The actual production router factory merges a controlled streaming extension and proves it shares capacity with MCP, administration and readiness. Dropping the stream restores the ordinary unauthenticated MCP 401/resource-metadata challenge.
5. Zero and excessive caps are rejected; the documented default is accepted.

These tests exercise Axum routing, futures and response-body lifetimes directly. The composition test intentionally uses an unconnected lazy database pool to demonstrate that admission rejects before database/authentication work; it makes no database correctness claim. Existing PostgreSQL security evidence remains separate. This is a deterministic overload/recovery test, not a throughput benchmark or a claim about network-level connection limits. Full composed workload measurements belong to the integration benchmark suite.
