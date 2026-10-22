# Controlled acceptance evidence

This repository is tested with manufactured identities, events and external sources. No production database, real player export, deployment, campaign launch, package publication or image publication is part of these checks. A completed synthetic suite does not mean a deployment is collecting live data.

## Reproduce

Prerequisites: Docker/Compose v2, Rust 1.99.0, Node 24, Python 3.9+ and `uv` 0.9.5. The official MCP client uses its locked Python 3.13 environment. CI runs Python 3.13 and installs Chromium.

```sh
npm ci
npm --prefix examples/producers ci --ignore-scripts
npx playwright install --with-deps chromium
make build check
make acceptance
```

`make acceptance` generates fresh protected credentials and randomly named disposable Compose resources. It executes 23 commands: fixture/client/database/build-boundary checks, the full Rust workspace, the independent review regression explicitly including its default-ignored case, real TypeScript producer recovery, analytical boundaries, real Chromium unit/service checks, composed HTTP/MCP/OIDC/vendor/export/restore tests, the live cross-property journey, independent fresh CLI and release-container installations, contract and package checks, and the native release benchmark. The benchmark has separate 5,000-event normal and 70,000-event burst workloads plus a 100,000-row export and a real-socket lost-acknowledgment recovery probe. Run `make benchmark` separately to reproduce those measurements.

The runner writes exact commands, exit codes, elapsed times, revision, dirty status and source-tree hashes to ignored `artifacts/acceptance/commands.json`. The benchmark writes `artifacts/benchmarks/service.json`; CI uploads only those two explicitly named files. No absent prerequisite, skipped-only suite, failed command, unresolved transport batch or failed performance gate is counted as acceptance. `acceptance_complete` describes this controlled environment only.

## Requirement-to-evidence mapping

| ID | Executable evidence |
| --- | --- |
| A01 Independent setup | `test_container.py` builds the release image, starts with automatic migrations, bootstraps with protected handoff, serves with the same schema-owning account, and initializes a real remote MCP client. `test_startup.py` covers concurrent empty-database starts, upgrades, restarts and migration failure. `cli_smoke.py` independently starts from an empty database. |
| A02 Capture | 28 Chromium behaviors plus `test:browser:service` and `browser_journey.mjs` verify initial/SPA/delegated capture once, same-batch tagged landing references, sanitized URL/query/fragment/control data and reset. |
| A03 Campaign continuity | Three actual origins traverse company → game → identity → game using audience-bound handoffs. Signup selects the actual retained touch; the report joins separately authorized origin visit/touch exports. Rust database suites cover cancel/retry/expiry/invalid audience/untagged/later campaign/login boundaries. |
| A04 Identity | Real OIDC authorization code/PKCE and signature-checked callback bind a subject to a distinct application account and player. Actual exported mappings join enrollment/action evidence. Rust and analysis suites cover multiple worlds/issuers, logout, unknown/deleted identities and scoped UUID reuse. |
| A05 Authoritative delivery | Live browser signup commits its application fact and native journal while analytics is stopped; rollback emits neither, dispatcher recovery deduplicates. Rust and TypeScript producer tests recover 10,025 facts after queue saturation/loss without blocking further application commits; late commits, quarantine and retention gaps are explicit. |
| A06 Profiles and privacy | Database tests enforce all profiles, schema allowlists, choice/GPC/withdrawal/reset/deletion, retention and anti-escalation. Chromium tests verify effective policy and queue purging. Unknown or immature linkage remains unavailable in reports. |
| A07 Source queries | Actual restricted PostgreSQL roles and parser/column/function boundaries, cancellation and budgets are tested in the query suites. Composed MCP discovers/configures and exports manufactured source tables through protected secret references. |
| A08 Files | Real MCP export jobs, independent reconnect/download clients and the official SDK verify bytes, SHA-256, types, nulls and lossless numbers. Query and independent-review suites cover partial/failure/cancel/expiry/revocation, generation invalidation and cleanup beyond 1,000 terminal jobs. |
| A09 External analysis | The real journey downloads six genuine server manifests/datasets and invokes the external CLI. It observes one verified conversion, one enrolled account and one first-observed player. Separately, eight genuine PostgreSQL exports test complete versus unknown source history. |
| A10 Cohorts | 43 analysis tests exercise exact D1/D7/D30/D180 action and maturity boundaries, multiple memberships, missing history, deletion, exclusions and zero denominators. Manufactured dates establish D180 logic; the live journey's immature conversion/retention rates stay null. |
| A11 Performance | Committed browser compressed-byte/interaction/volume/queue/heap measurements; unchanged release normal/burst latency gates, explicit refusal/unknown accounting, database reconciliation, sampled RSS and 100,000-row export measurements. Initial failed latency and transport-unknown runs are retained. See `benchmarks/README.md`. |
| A12 Handoff | Combined OpenAPI generation/check resolves all references and preserves browser wire types; actual npm package inventory allows only 16 intended files. Operations cover protected setup, runtime permissions, source access, privacy/retention, tested engine restore and separate release steps. |
| A13 External observability | Actual Rust adapters query controlled Prometheus instant/range/empty and Sentry paginated issue/event/detail APIs. Typed downloaded files preserve gaps/special values and release/time correlation; sensitive fields, unauthorized project paths, rate limits and unavailable results are explicitly checked. No vendor mutation is introduced. |
| A14 Auth and parity | Real human OIDC and unattended agent access share grants. The official `mcp==2.3.0` client checks discovery, handshake, administration, scoped denial, same-session/reconnect revocation and artifact checksums. CLI/database suites prove atomic bootstrap, rotation and recovery. The application account owns its schema for automatic migrations but cannot create cluster roles or databases. Shared admission/deadlines are tested against real slow TCP clients. |

## Recorded results and revision boundaries

- The first full functional `make acceptance` run completed all then-current **20 commands successfully**, including Rust ingestion's 11 database suites (79.81 s), explicit independent review, TypeScript recovery, 43 analysis tests, 27 Chromium tests, seven composed-service tests (17.978 s), actual journal-based journey, fresh CLI setup, release container installation (88.469 s), contract drift and package checks. It was marked dirty while acceptance fixtures/evidence were strengthened; it is not final clean-head evidence.
- The runtime/restore test includes nonempty accepted event, immutable acquisition and verified identity-map rows; row hashes and migration/principal/grant/credential/audit/config/export counts match after restore. Existing files still pass checksum verification, revoked credentials remain denied, duplicate bootstrap fails, and new exports/reports succeed.
- Linux CI exposed a dynamic-port bug when Compose recreated the fixture dependency. `6f8d25c` fixes the startup order: dependencies finalize first, the advertised port is then read, and engine startup uses `--no-deps`. The deterministic forced-recreation test passed locally (66.341 s), asserting stable container identity/port through startup and actual host readiness/MCP. Public CI verifies Linux behavior.
- The final ten admission tests passed in the clean run (1.03 s), including real headers-only requests, nonreading/unpolled response streams, bounded overload connection reuse and uncertain-acknowledgment behavior. Request and stream deadlines reclaim capacity without relying on a client to consume bytes.
- The unchanged optimized workload passed on clean `4b1bee3` with the authoritative public lockfile. Normal p95/p99 were **30.47/40.91 ms** against **250/1,000 ms** limits; all 5,000 normal events persisted. Burst explicitly accounted for 13,100 acknowledgments and 56,900 HTTP refusals. A separate real-socket uncertain-ACK probe reconciled/retried exact stable IDs. Export and RSS gates passed. Full raw results, prior failures and source-tree hashes are committed under `benchmarks/results/`.

The final clean pre-followup run of `make check && make acceptance` passed all **22 commands** on `ae9cd0e33b4d4f2e82d9d328d072b633427a8e8e`, with `dirty: false` and `acceptance_complete: true`. Its measured normal p95/p99 were 83.09/154.12 ms, still below unchanged 250/1,000 ms limits. The final Rust source tree was `f39fae257d0b9bf1728a2448158b5e3beffdf6a9` and Cargo lock blob `4c65e45b33023f60b514d81c853e6246d90bce0e`, matching the public integration branch at that point.

A subsequent focused same-origin check found that browsers omit Origin from a same-origin GET configuration request. The narrow `5706de6` server / `c66f124` SDK followup uses a bodyless POST configuration alias with the same Origin/token validation and no writes. The browser-owner suite now contains 28 passing behaviors. Integration reran `npm run test:browser:service`: both the actual Rust/PostgreSQL cross-origin and same-origin transparent-proxy paths passed, each storing four observations and checking deduplication, forgery rejection and reset. Actual requests carried POST, the page Origin, no body and no Referer. Combined contract generation still resolves all references across 39 paths, `npm run check` confirms unchanged wire types, and package inventory remains 16 allowlisted files. A direct build-byte check measured the updated ESM at 26,099 raw / 8,862 gzip bytes; CPU/heap measurements were not rerun for this HTTP-method-only change. The existing authenticated cross-property journey also passed as the focused integration regression (10.425 s); final public CI runs the full latest suite.

Task commit hashes can differ from public cherry-picks or squash history. Match recorded crate/migration/lock/browser/producer/harness tree hashes to identify equivalent source. The clean 22-command result above and focused same-origin followup are separate revision boundaries. For any public commit or pull-request head, select its run in [public CI](https://github.com/unbought-games/analytics/actions/workflows/ci.yml) and inspect the `acceptance-command-evidence` artifact: `commands.json` records the exact revision, source hashes and command outcomes, and `service.json` records that run's measured performance gates. This provides current-head evidence without treating an earlier local result as proof for later changes.

## Practical limits

Browser delivery remains best effort. An HTTP overload response itself can be lost; absent acknowledgment is unknown acceptance, requiring stable-ID retry and reconciliation. The benchmark never folds recovery into measured throughput or relabels an unknown as a refusal. Source histories have independent snapshots, and incomplete coverage never becomes a measured zero. RSS sampling may miss shorter peaks and recorded throughput is hardware/workload-specific. Downloaded artifacts and reports inherit deletion/retention obligations; local files do not disappear when remote access is revoked. Deployment and publication remain separately authorized work.

## Automatic startup migration validation — 2026-10-21

The deployment now uses one schema-owning application account. `serve` and
`bootstrap` apply pending migrations automatically; the separate migration
container and runtime grant step have been removed. `DATABASE_URL` is supported,
with protected-file configuration retaining precedence. The standalone example
is `compose.deploy.yaml`; the operator steps are in `docs/deployment.md`.

Focused validation commands, run from the repository root:

| Command | Outcome |
| --- | --- |
| `cargo build --locked -p analytics-server` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | Passed |
| `python3 scripts/generate-contract.py --check` | Passed; 39 paths, all references resolved |
| `python3 tests/acceptance/stack.py run -- python3 -m unittest discover -s tests/acceptance -p test_startup.py -v` | 3 passed: concurrent fresh starts, restart/identity persistence, pending upgrades, migration failure rollback and refusal to serve/bootstrap; direct environment connection exercised |
| `python3 tests/acceptance/stack.py run -- python3 -m unittest discover -s tests/acceptance -p test_service.py -v` | 7 passed: administration, OIDC, official MCP client, exports, controlled vendor sources, schema privileges and restore |
| `python3 tests/acceptance/stack.py run -- python3 crates/analytics-server/tests/cli_smoke.py` | Passed: bootstrap on empty database without explicit migration, repeat rejection, MCP identity, rotation, revocation and recovery |
| `python3 tests/acceptance/stack.py run -- python3 -m unittest discover -s tests/acceptance -p test_container.py -v` | Initial container test passed; standalone deployment test added afterward and exercised separately below |
| `python3 tests/acceptance/stack.py run -- python3 -m unittest discover -s tests/acceptance -p test_container.py -k standalone -v` | Passed: actual deployment Compose file, default UID 65532, automatic migrations, protected bootstrap copy, MCP identity, container recreation and repeated-bootstrap rejection |
| `python3 tests/acceptance/secret_check.py` | Passed; zero findings |
| `git diff --check` | Passed |

The tracked-secret check initially flagged the documentation password placeholder;
it was changed to an explicit `<password>` placeholder and the check passed.

All database runs used disposable PostgreSQL 17.7 and manufactured credentials;
containers and volumes were removed after testing. External APIs used controlled
fixtures. The standalone deployment test overrides only the test network and
loopback port. The complete A01–A14 acceptance suite, browser suite and performance
benchmarks were not rerun for this startup change. No production deployment,
production query or package publication was performed.
