# Shared request admission

The Rust service admits at most **64 concurrent application requests** by default, shared across all router clones, HTTP administration, ingestion, MCP and extension routes. Set `ANALYTICS_MAX_IN_FLIGHT_REQUESTS` to an integer in `1..4096` before `serve` to choose another cap. Invalid/zero/out-of-range values fail startup. Configure it against the deployment's memory, database pool and export workload budget; raising this cap does not increase those underlying capacities.

The gate uses `Semaphore::try_acquire_owned`. When full, requests do not join a semaphore wait queue, authenticate, acquire a database connection or parse a JSON body. They receive HTTP **503**, `Retry-After: 1`, and:

```json
{"error":{"code":"overloaded","message":"Service request capacity is full; retry with bounded backoff"}}
```

HTTP and MCP use this same transport-level overload response. Clients should apply bounded backoff with jitter and preserve event IDs on ingestion retries. Per-module ingestion/source/export quotas and request-size limits remain in force; this common limit is an additional bound. `GET /healthz` and `HEAD /healthz` are exempt because liveness does no database/authentication/module work. `/readyz` is admitted normally because it acquires a database connection.

A request owns its permit while its handler runs, with an independently enforced absolute deadline. This includes JSON/body reads, authentication, database-pool waits and handler execution. A client that sends `Content-Length: 1` and never sends that byte cannot occupy a permit forever. Expiry cancels the handler future, releases its slot and returns HTTP **408** with `error.code: "request_timeout"`. HTTP/1.x timeout responses include `Connection: close` so unread bodies cannot hold their keep-alive connection for another request. Overload responses do not force closure of a fully received request; the transport can reuse that connection when its body is already cheaply drainable.

Once a handler returns response headers, its permit and body producer move into a streaming lease. A separate watchdog enforces an absolute response-stream deadline. It drops **both the underlying producer/body and the permit** on expiry, even if the transport never polls the response body because its client stops reading. The watchdog wakes any pending poll; the next body poll returns an explicit timeout error, never a successful empty EOF. Normal completion, stream errors and client/body drops cancel the watchdog and release immediately. There is at most one active stream watchdog per admitted response, with no pending-admission queue. Frames are forwarded without collecting the response into memory.

| Environment setting | Default | Accepted range | Start of deadline |
| --- | --- | --- | --- |
| `ANALYTICS_REQUEST_TIMEOUT_MS` | 30000 (30 seconds) | 1–600000 ms | Successful admission, before body extraction |
| `ANALYTICS_STREAM_TIMEOUT_MS` | 300000 (5 minutes) | 1–3600000 ms | Response headers returned by the handler |

Deadlines are absolute and cannot be extended by trickling bytes. Zero, malformed and excessive values fail startup. Choose the request timeout to cover legitimate synchronous operations; long analytical work should use the bounded export job lifecycle. Choose the stream deadline for the permitted artifact size and expected transfer rate. Interrupted downloads can retry the authenticated artifact URL while the completed export is still retained; a download timeout does **not** delete or mutate that durable export. Clients must verify expected byte length and checksum and never treat a truncated transfer as complete.

The bound covers application handlers and active response bodies. It is not a limit on accepted TCP sockets, incomplete HTTP headers, kernel buffers, or the final bytes already handed to the HTTP transport. Connection/header timeouts and deployment proxy limits remain separate. Background jobs outlive their enqueue requests and retain their module-level job/concurrency budgets; this middleware does not replace those budgets. The gate does not promise FIFO fairness and does not queue overload work for later delivery.

## Composition API

Existing `analytics_server::router(state, extensions, oidc)` callers keep the default cap **and finite deadlines** without DTO changes. `router_with_admission(..., max_in_flight)` changes only the cap, retaining those deadlines. Fully configured compositions call:

```rust
analytics_server::router_with_admission_limits(
    state, extensions, oidc,
    analytics_server::AdmissionLimits {
        max_in_flight,
        request_timeout: std::time::Duration::from_millis(request_timeout_ms),
        stream_timeout: std::time::Duration::from_millis(stream_timeout_ms),
    },
)?
```

The result is `ApiResult<axum::Router>` and validates the cap and both deadlines. Preserve the full ingestion/query extension vector when integrating the binary change. The middleware surrounds the complete merged router, so direct MCP extension calls cannot bypass it. Security-response headers remain outside the gate and apply to overload responses as well.

## Focused validation

Commands:

```sh
cargo test --locked -p analytics-server --lib admission -- --nocapture
cargo clippy --locked -p analytics-server --all-targets -- -D warnings
cargo fmt --all --check
```

Ten focused tests passed on 2026-10-21 with Rust 1.99.0:

1. A held handler saturates the shared gate; 60 MCP/admin/readiness requests receive structured 503 responses, each within a 250 ms test deadline and collectively within one second. Invalid JSON is rejected by admission before parsing. Liveness stays available. Cancelling the handler restores capacity.
2. A response that has returned headers and one streamed chunk continues occupying its permit. End-of-stream releases it. Dropping an unread body also releases it.
3. A streaming-body error releases capacity even while the errored body remains allocated.
4. The actual production router factory merges a controlled streaming extension and proves it shares capacity with MCP, administration and readiness. Dropping the stream restores the ordinary unauthenticated MCP 401/resource-metadata challenge.
5. Zero/excessive caps and deadlines are rejected; documented defaults are accepted.
6. A response body is kept allocated and never polled. Its 50 ms watchdog drops the underlying channel receiver and releases capacity independently. Polling afterward produces a timeout error, not EOF success.
7. A real Axum server receives **64 real TCP `POST /mcp` requests** with complete headers and `Content-Length: 1`, but no body bytes. They saturate all 64 slots; ordinary MCP traffic initially receives 503. With a one-second test deadline, every stalled socket receives 408 plus connection closure without sending the missing byte. A complete MCP request then reaches normal authentication (401), proving capacity recovered. Liveness stays available.
8. A real TCP client requests an infinite controlled response with a small socket receive buffer, then never reads its headers/body and keeps the socket open. The 150 ms streaming deadline drops the producer and restores service capacity while the reader remains open. This complements the explicitly unpolled body test.

9. A real TCP A/B comparison sends eight bounded 64 KiB uploads per variant while admission is full, with the explicit overload close header added versus omitted. The controlled client continues sending before reading the early response. Both variants recorded eight upload-side transport errors, despite all eight having observable 503 headers when the test subsequently read the socket. This demonstrates possible uncertain acknowledgement independently of that header; it does not promise a particular error rate on other systems.
10. Two complete small rejected uploads successfully receive 503 on the same HTTP/1 connection. Removing unconditional overload closure preserves this cheap connection reuse; no rejected body is awaited by application code.

The ten-test run completed in 1.02 seconds on the recorded run. Deadline tests use shorter configured limits; production defaults remain 30 seconds for requests and 300 seconds for response streaming.

These tests exercise Axum routing, real loopback TCP connections, futures and response-body lifetimes. The composition test intentionally uses an unconnected lazy database pool to demonstrate that admission rejects before database/authentication work; it makes no database correctness claim. Existing PostgreSQL security evidence remains separate. This is a deterministic overload/recovery test, not a throughput benchmark or a claim about network-level connection limits. Full composed workload measurements belong to the integration benchmark suite.


## Uncertain acknowledgements and bounded retries

Fail-fast rejection attempts to send 503; it cannot guarantee that the client receives or reads that status. A client still sending a request body can see a reset or write/read error first. In the pinned Hyper 1.12.0 implementation, dropping an unread incoming body invokes `poll_drain_or_close_read`: the transport polls the body once and closes its read side if it has not already finished. Removing an explicit close header therefore does not eliminate this outcome. The service leaves this cheap transport decision in place and does not introduce a separate body-drain queue, buffer or grace-period waiter outside admission. The explicit 408 close and all admission/deadline protections remain enabled.

Account for request outcomes separately: an observed committed per-event acknowledgement, an explicit HTTP rejection, or an unknown transport outcome. A transport error is **neither proof of rejection nor proof of acceptance**. It can also occur after an accepted batch commits but before its acknowledgement arrives. Preserve the original scoped event IDs and payloads; retain authoritative outbox records until acknowledged or reconciled. Retry unknown outcomes with bounded backoff using identical IDs and payloads. `accepted` and `duplicate` acknowledgements are successful reconciliation; conflicts must fail visibly. If bounded retries cannot resolve the outcome, report it as unresolved rather than assuming absence or silently dropping the record.

Integration supplied independent evidence at root revision `4b1bee3`: a 700-request burst recorded 131 HTTP 200, 569 HTTP 503 and zero unknown outcomes, with exactly 13,100 persisted events matching acknowledged counts. A separate real TCP probe sent two synthetic event IDs, then closed without reading an acknowledgement. Its initial database snapshot found zero; same-ID retry accepted two, the final exact ID set contained two, and subsequent replay returned two duplicates. Recovery took 0.200 seconds outside measured burst latency/throughput. The older run with one generic transport error remains preserved as failed/uncertain evidence; its exception class was not captured and is not inferred from the A/B experiment. The integration benchmark report owns the raw ID-set checksum and measured workload results.
