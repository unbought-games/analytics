# Analytics Engine

A headless analytics service with PostgreSQL storage, a small framework-neutral browser package, authenticated direct MCP, and structured files for analysis outside the service. All bundled data, identities and source fixtures are synthetic. MIT licensed.

The product requirements and implementation plan are in [the specification](docs/SPECIFICATION.md) and [the plan](docs/plans/2026-10-21-analytics-engine.md). The controlled local installation and browser-to-MCP analysis pipeline passed a clean 22-command acceptance run, including measured performance gates. The subsequent same-origin configuration fix passed focused same-origin and cross-origin checks. [Acceptance evidence](docs/acceptance.md) records the tested source boundaries and limitations; [public CI runs and artifacts](https://github.com/unbought-games/analytics/actions/workflows/ci.yml) provide the result for each public revision. These checks do not deploy or publish anything.

## Deploy

The production layout is one Rust container, your PostgreSQL database, an HTTPS
reverse proxy, and a persistent volume for export files. Administration is through
MCP/API; there is no web dashboard. No provider-specific services are required.

Follow the [self-hosting guide](docs/deployment.md) to configure the database,
start the service, create the first admin credential, connect an MCP client, and
verify collection and exports. It also covers optional OIDC/source connections,
scheduled retention cleanup, backups, upgrades and recovery. Migrations run
automatically using the service's schema-owning database account.

Use `compose.deploy.yaml` for deployment. The default `compose.yaml` is a disposable
test stack. Local acceptance results are not evidence of a live production install.

## Development

Requirements: Docker with Compose v2, Rust 1.99, Node 24, Python 3.9+, OpenSSL and uv 0.9.5 (the locked official MCP test uses Python 3.13). The fixture image supplies its own Python/OpenSSL. Source builds use the committed Cargo/npm lockfiles.

```sh
npm ci
npm --prefix examples/producers ci --ignore-scripts
npx playwright install --with-deps chromium
make fixtures       # controlled OIDC/vendor fixtures and artifact client tests
make analysis       # fixtures plus deterministic cohort/retention analysis
make test-ci        # validation selection, including Git moves/deletions
make integration    # disposable PostgreSQL, restore checks and Rust workspace tests
make acceptance     # full synthetic journey, auth, restore, packaging and performance gates
```

`stack.py` generates protected credentials, random Compose project names and loopback-only ports. Normal test completion, failure and interruption clean up only that run's containers and volumes. No production connection or external account is required. See [operations](docs/operations.md) for retained local stacks, cleanup and restore boundaries.

Browser-to-producer-to-MCP-to-report acceptance runs against [the versioned contracts](docs/contracts.md). The live journey includes a real OIDC client, transactional signup/outage recovery, authenticated exports and external analysis. See the acceptance record for exact tested boundaries, and the CI artifact for the public revision being evaluated. [Benchmark methodology](benchmarks/README.md) distinguishes proposed budgets from measurements.

## Continuous integration

PRs select validation from their complete changed-path set:

| Changes | Validation |
| --- | --- |
| Only root README/AGENTS/CHANGELOG or Markdown under `docs/` | Fixture/client tests and tracked-secret scan; no Rust, npm or Docker setup |
| Only analytical examples, optionally with documentation | Above plus cohort/retention tests |
| Ingestion, query adapters, browser SDK or producer/static-site examples | Build, static/contract checks and real PostgreSQL/Rust/producer/browser integration, plus HTTP/MCP/OIDC/export/restore checks |
| Shared core/auth/server, migrations, contracts, dependency manifests/lockfiles, packaging, CI/test harnesses, benchmarks or unclassified paths | Full acceptance including fresh installation, cross-property journey, restore, containers and performance |

Mixed changes use the strongest tier. Missing/empty comparisons fail closed to
full acceptance; deleted and renamed paths retain their original risk. Every
main push, version-tag push and manual workflow run uses full acceptance. Image
publication still requires successful full acceptance on a push. No scheduled
validation is added. The existing `acceptance` check name remains stable; inspect
its selection step and command artifact for the suite that actually ran.

npm download caches cover both lockfiles. Rust caches include compiler and
dependency identity, exclude toolchain binaries, and are saved only by successful
main pushes. PRs restore those caches without publishing shared Rust entries.
Existing per-ref cancellation is preserved, including superseded PR runs and main
publication. The first run for a new cache key is cold.
