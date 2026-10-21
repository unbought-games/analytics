# Analytics Engine

A headless analytics service with PostgreSQL storage, a small framework-neutral browser package, authenticated direct MCP, and structured files for analysis outside the service. All bundled data, identities and source fixtures are synthetic. MIT licensed.

The product requirements and implementation plan are in [the specification](docs/SPECIFICATION.md) and [the plan](docs/plans/2026-10-21-analytics-engine.md). The controlled local installation and browser-to-MCP analysis pipeline passed a clean 22-command acceptance run, including measured performance gates. The subsequent same-origin configuration fix passed focused same-origin and cross-origin checks. [Acceptance evidence](docs/acceptance.md) records the tested source boundaries and limitations; [public CI runs and artifacts](https://github.com/unbought-games/analytics/actions/workflows/ci.yml) provide the result for each public revision. These checks do not deploy or publish anything.

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

Browser-to-producer-to-MCP-to-report acceptance runs against [the versioned contracts](docs/contracts.md). The live journey includes a real OIDC client, transactional signup/outage recovery, authenticated exports and external analysis. See the acceptance record for exact tested boundaries, and the CI artifact for the public revision being evaluated. [Benchmark methodology](benchmarks/README.md) distinguishes proposed budgets from measurements.
