# Acceptance evidence

Status: infrastructure checkpoint. A01–A14 are all required; none is signed off by this document yet. No production deployment, real collection, or six-month live result is claimed.

## Commands actually run

On 2026-10-21, on an ARM64 development host with Docker 29.7.2, Rust 1.99.0, Node 24.18.0 and Python 3.9.6:

| Command | Actual result |
| --- | --- |
| `python3 -m unittest discover -s tests/acceptance -p test_fixtures.py -v` | 4 passed: real HTTP fixtures, signed OIDC authorization-code/PKCE, replay rejection, vendor gaps/pagination/errors |
| `python3 -m unittest discover -s tests/acceptance -p test_client.py -v` | 4 passed: authenticated streaming retrieval, checksum/size/rows, no overwrite, origin/redirect/path boundaries, protected credentials |
| `python3 -m unittest discover -s tests/acceptance -p test_build_context.py -v` | 2 passed: actual Docker build context excludes manufactured private/local handoff files and includes only intended build inputs |
| `python3 tests/acceptance/stack.py run -- python3 -m unittest discover -s tests/acceptance -p test_stack.py -v` | 4 passed against PostgreSQL 17.7 (latest run includes Task 5 rows): typed manufactured cohort source equality, narrow reader, write/private-schema/timeout denial, lossless values, real backup/restore, reachable fixture; resources removed |

These commands were run on the foundation contract checkpoint `3b752ee2d24819444dc7195c47c90fb44410eb3b` plus the integration working changes. The integration commit is reported in the handoff; the machine-readable runner records exact HEAD and dirty state for subsequent runs. Infrastructure tests do not claim Rust adapter, human sign-in, browser or end-to-end application behavior.

The analysis checkpoint `9a42d20855ac14bd28b222ecb122193d027e7847` was imported separately. `python3 -m unittest discover -s examples/analysis -p 'test_*.py' -v` ran all 32 tests successfully here. Actual PostgreSQL fixture loading now preserves all four datasets' rows, booleans, nested history strings and duplicate deliveries. This does not yet prove the engine query/export path. The prepared `service.py` and `export_cohorts.py` helpers will exercise that path after the service modules arrive.

Additional integrated commands passed before the final full-suite run:

- `python3 tests/acceptance/stack.py run -- cargo test --locked -p analytics-server -p analytics-ingest --tests`: foundation security and all then-current ingestion tests passed with real PostgreSQL, no skipped tests.
- `python3 tests/acceptance/stack.py run -- python3 -m unittest discover -s tests/acceptance -p test_service.py -v`: five tests passed (9.686s), covering CLI/MCP, real OIDC code flow, eight actual cohort exports with complete versus unknown coverage, restricted runtime database role, and complete engine database/export restore followed by fresh exports.
- `python3 tests/acceptance/stack.py run -- python3 -m unittest discover -s tests/acceptance -p test_container.py -v`: real release image built, migrated with separate migration credentials, bootstrapped/served with runtime credentials, and passed authenticated MCP identity/tool discovery (74.014s including build). Cleanup includes the engine profile.

These are controlled synthetic results. The manufactured D180 result tests definitions and the real export pipeline; it is not a live browser retention claim. The real cross-property browser journey then passed in 10.126 seconds: three real Chromium origins, explicit audience-bound handoffs, actual OIDC code/PKCE and signature-checked callback, rolled-back signup, committed signup during analytics outage, Rust outbox recovery, enrollment/action/scheduled facts, replay deduplication and reset. Six actual MCP-generated scoped exports were downloaded and fed to the external normalizer. It reported one observed verified conversion, one enrolled account and one first-observed player; immature live conversion and D180 rates remained null. No manufactured acquisition file was substituted into this live path.

The official `mcp==2.3.0` SDK passed discovery, initialized tools, authorized administration, restricted/revoked access, and a real 12,220-byte artifact checksum/revocation check. The latest composed service suite additionally queried both controlled vendors through the actual Rust adapters (instant/range/empty, issue pagination/event detail, special values, redaction, rate-limit/unavailable failures and scope denial): seven tests passed in 24.279 seconds.

`make acceptance` now explicitly includes all module tests, the ignored-by-default independent review regression, browser unit and real-service tests, live service/official SDK/restore tests, full browser journey, separate CLI lifecycle and container installs, combined contract drift and npm package inventory checks. Release-mode performance and the final producer/admission corrections are still pending final integration; no final A01–A14 signoff yet.

## Full functional run

`make acceptance` completed all **20 commands successfully** after the journal correction, on the task's integrated history through `6b53804`. The run was marked dirty because the acceptance fixture was being strengthened while it progressed; it is functional evidence, not final clean-head release signoff. Rust ingestion ran 11 database suites, the explicit independent review regression ran despite its default ignored annotation, TypeScript recovered its 10,025-command fixture, 43 analytical tests and 27 Chromium tests passed, and the real journal-based cross-property journey, official MCP client, vendor adapters, nonempty engine restore, fresh CLI lifecycle and release container installation all passed. Contract regeneration and actual npm package inventory checks also passed.

The restore now includes nonempty accepted signup/acquisition/verified identity-map facts and compares hashes of their rows across the restored database, then verifies existing artifact checksums, revoked access and fresh exports. Original source tree hashes accompany benchmark evidence so equivalent cherry-picked public revisions can be identified independently of task commit hashes. Final public CI and the optimized A11 workload remain pending.

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
