# Analytics Engine

A headless analytics service with PostgreSQL storage, a small framework-neutral browser package, authenticated direct MCP, and structured files for analysis outside the service. All bundled data, identities and source fixtures are synthetic. MIT licensed.

The product requirements and implementation plan are in [the specification](docs/SPECIFICATION.md) and [the plan](docs/plans/2026-10-21-analytics-engine.md). The controlled local installation and browser-to-MCP analysis pipeline are implemented and tested. Service performance gates pass; final clean-head acceptance and public CI verification are in progress; nothing has been deployed or published. [Acceptance evidence](docs/acceptance.md) identifies the checks that have actually run and the remaining integration work.

## Development

Requirements: Docker with Compose v2, Rust 1.99, Node 24, Python 3.9+, OpenSSL and uv 0.9.5 (the locked official MCP test uses Python 3.13). The fixture image supplies its own Python/OpenSSL. Source builds use the committed Cargo/npm lockfiles.

```sh
npm ci
npm --prefix examples/producers ci --ignore-scripts
npx playwright install --with-deps chromium
make fixtures       # controlled OIDC/vendor fixtures and artifact client tests
make integration    # disposable PostgreSQL, restore checks and Rust workspace tests
make acceptance     # full synthetic journey, auth, restore, packaging and performance gates
```

`stack.py` generates protected credentials, random Compose project names and loopback-only ports. Normal test completion, failure and interruption clean up only that run's containers and volumes. No production connection or external account is required. See [operations](docs/operations.md) for retained local stacks, cleanup and restore boundaries.

Browser-to-producer-to-MCP-to-report acceptance runs against [the versioned contracts](docs/contracts.md). The live journey includes a real OIDC client, transactional signup/outage recovery, authenticated exports and external analysis. See the acceptance record for exact tested boundaries and pending checks. [Benchmark methodology](benchmarks/README.md) distinguishes proposed budgets from measurements.
