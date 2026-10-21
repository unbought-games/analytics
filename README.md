# Analytics Engine

A headless analytics service with PostgreSQL storage, a small framework-neutral browser package, authenticated direct MCP, and structured files for analysis outside the service. All bundled data, identities and source fixtures are synthetic. MIT licensed.

The product requirements and implementation plan are in [the specification](docs/SPECIFICATION.md) and [the plan](docs/plans/2026-10-21-analytics-engine.md). This is an implementation checkpoint, not a completed or deployed release. [Acceptance evidence](docs/acceptance.md) identifies the checks that have actually run and the remaining integration work.

## Development

Requirements: Docker with Compose v2, Rust 1.99, Node 24, Python 3.9+ and OpenSSL. The fixture image supplies its own Python/OpenSSL. Source builds use the committed Cargo/npm lockfiles.

```sh
make fixtures       # controlled OIDC/vendor fixtures and artifact client tests
make integration    # disposable PostgreSQL, restore checks and Rust workspace tests
make acceptance     # fails if any integrated component or journey test is absent
```

`stack.py` generates protected credentials, random Compose project names and loopback-only ports. Normal test completion, failure and interruption clean up only that run's containers and volumes. No production connection or external account is required. See [operations](docs/operations.md) for retained local stacks, cleanup and restore boundaries.

Browser-to-producer-to-MCP-to-report acceptance is being assembled against [the versioned contracts](docs/contracts.md). The current fixture/reader checks do not establish complete product acceptance. [Benchmark methodology](benchmarks/README.md) distinguishes proposed budgets from measurements.
