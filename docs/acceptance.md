# Acceptance evidence

Status: infrastructure checkpoint. A01–A14 are all required; none is signed off by this document yet. No production deployment, real collection, or six-month live result is claimed.

## Commands actually run

On 2026-10-21, on an ARM64 development host with Docker 29.7.2, Rust 1.99.0, Node 24.18.0 and Python 3.9.6:

| Command | Actual result |
| --- | --- |
| `python3 -m unittest discover -s tests/acceptance -p test_fixtures.py -v` | 4 passed: real HTTP fixtures, signed OIDC authorization-code/PKCE, replay rejection, vendor gaps/pagination/errors |
| `python3 -m unittest discover -s tests/acceptance -p test_client.py -v` | 4 passed: authenticated streaming retrieval, checksum/size/rows, no overwrite, origin/redirect/path boundaries, protected credentials |
| `python3 -m unittest discover -s tests/acceptance -p test_build_context.py -v` | 2 passed: actual Docker build context excludes manufactured private/local handoff files and includes only intended build inputs |
| `python3 tests/acceptance/stack.py run -- python3 -m unittest discover -s tests/acceptance -p test_stack.py -v` | 3 passed against PostgreSQL 17.7: narrow reader, write/private-schema/timeout denial, lossless values, real backup/restore, reachable fixture; resources removed |

These commands were run on the foundation contract checkpoint `3b752ee2d24819444dc7195c47c90fb44410eb3b` plus the integration working changes. The integration commit is reported in the handoff; the machine-readable runner records exact HEAD and dirty state for subsequent runs. Infrastructure tests do not claim Rust adapter, human sign-in, browser or end-to-end application behavior.

## Required integration matrix

| ID | Required remaining evidence |
| --- | --- |
| A01 | Empty install → migrations → bootstrap → browser/backend → MCP identity and first export |
| A02 | Real-browser initial/SPA/delegated automatic capture, once-only listeners and sanitation |
| A03 | Tagged cross-property signup/enrollment/action; cancel/retry/expired/invalid/untagged/later/login cases |
| A04 | Different issuer/account/player IDs, multiple worlds, logout/shared device, unmapped/deleted identities |
| A05 | Transactional rollback/outbox recovery/dedup/conflicts/late commits and scheduled-action exclusion |
| A06 | All three enforced profiles, opt-out/withdrawal/deletion and no browser privilege escalation |
| A07 | Engine adapter with restricted real PostgreSQL, forbidden SQL/functions/scopes, timeouts and cancellation |
| A08 | Engine-created durable manifests and real MCP reconnect/download; checksums/types/nulls/order/expiry/failure/revocation |
| A09 | Four remotely exported datasets joined by the external report CLI with reproducible source references |
| A10 | Exact mature/immature D1/D7/D30/D180 boundaries, exclusions, unknown/expired history and zero denominator |
| A11 | Measured browser/ingestion/query/export normal/burst/failure budgets, service RSS and database impact |
| A12 | Tested contracts/install/source/privacy/restore/release documentation and exact revision |
| A13 | Rust adapters query controlled Prometheus/Sentry HTTP; scopes, redaction, rate-limit/pagination/unavailable/gaps; local time/release correlation |
| A14 | Real human OIDC and unattended MCP clients; atomic repeated/concurrent bootstrap, parity, scopes, rotation/revocation/recovery and audit |

`make acceptance` fails when required integrated components or the real journey test are absent. `run.py` writes commands, return codes, elapsed time, revision and missing prerequisites to ignored `artifacts/acceptance/commands.json`. No skipped or missing case is promoted to product acceptance. Proposed benchmark budgets live in `benchmarks/budgets.json`; no measured service numbers exist at this checkpoint.
