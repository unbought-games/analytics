# Server acquisition and named route verification

The browser SDK supports clean-link fallback labels and server-owned initial
views. Route labels retain declared parameter names; concrete ordinary paths and
unmatched paths remain available. Invite/credential parameters stay named in page
and referrer fields. The collector accepts route and authentication-method context
and atomically attributes a server signup at its original carried source time.

Local validation (2026-11-12, changes on top of e99bf88 plus integrated CI #6):
- Browser suite: 35 passed (including opt-out, source retention, no duplicate initial view, named grouping and unmatched paths).
- PostgreSQL ingestion suite: 17 passed (30.02 s), including server-only acquisition with intermediate pages delivered first, immutable acquisition, retries, deletion, policy and rollback.
- Focused named-route check after preserving trailing slashes: passed.
- TypeScript check/build, generated contract check and Rust Clippy for core/ingest: passed.

Two browser assertions initially exposed invite codes in referrers after changing
unknown-path behavior. The production referrer normalizer now uses route patterns;
assertions were preserved and passed. No test was deleted or weakened. Setup and
runner wait were not separately measured. Integration uses disposable PostgreSQL;
there was no production access, deployment or registry publication.

The application producers must deploy after this additive collector contract.
See ../server-attribution.md for the source-time and route field semantics.

Review follow-up: missing browser referrers now stay null instead of resolving to
the current page. Unrepresentable URLs leave page/route null while preserving
navigation and interaction observations. Two focused browser regressions passed;
no assertion was weakened. A temporary variable-name collision was caught by the
build before test execution and corrected. The collector accepts nullable context
fields by its existing wire contract.
