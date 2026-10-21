# Analytics engineering

Read `docs/SPECIFICATION.md` and `docs/plans/2026-10-21-analytics-engine.md`.

- This public repository is a generic product. No references to the operator’s other properties, private context, real identities, credentials, production data or deployment configuration. All committed fixtures and examples are synthetic.
- Rust service, PostgreSQL storage, small framework-neutral TypeScript browser package, direct authenticated Rust MCP, headless administration.
- Preserve all A01–A14 requirements. Do not claim success from stubs or reduce required functionality to a demonstration.
- Every change belongs to its assigned worktree. Do not modify another chat’s checkout. Coordinate shared contracts through the orchestrating chat.
- Use Conventional Commit commits and PR titles. Verify titles before creating/updating PRs.
- Keep secrets out of arguments, logs and committed files. Bootstrap secrets use protected ignored files.
- Test meaningful behavior with real disposable PostgreSQL and controlled external API fixtures. No production queries, deployment or package publication.
- Record exact commands, outcomes and limitations. Leave clean commits and report their hashes. Do not merge or force-push branches.
