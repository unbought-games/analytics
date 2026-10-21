# Reproducible campaign measurement

The external Python 3.9+ standard-library CLI reads locally downloaded JSONL exports and performs joins outside the service. All checked-in identities, source schemas, clocks and activity are manufactured. No service, database connection, package installation or network access is needed for this example.

```sh
python3 tests/fixtures/cohorts/generate.py
examples/analysis/analyze \
  --manifest tests/fixtures/cohorts/manufactured/manifest.json \
  --cutoff 2026-07-01T01:00:00Z \
  --output artifacts/cohort-report
python3 -m unittest discover -s examples/analysis -p 'test_*.py' -v
```

Outputs are `campaign-report.json` and `campaign-report.txt`. Their content is deterministic for identical bytes and cutoff: no local absolute paths, current clock, random IDs or runtime duration enter the report. Reports retain the bundle checksum, exact input manifests and their checksums, query definitions, source read boundaries, coverage, policy versions and dataset checksums. These reports inherit the source access/retention rules; query parameters and provenance may be private even though the metrics are aggregates. Remove downloaded exports and derived reports when applicable retention/deletion requires it. The local CLI does not enforce server revocation or remote expiry, and does not fetch URLs. Server authentication and artifact retrieval belong to the exporting client.

## Export binding and integration

The transport adapter in `examples/analysis/manifest_adapter.py` consumes the exact `ExportManifest` and `DatasetManifest` v1 DTO field names from the foundation checkpoint. It does not redefine the server's export format. Four independent source queries retain separate manifests, coverage and source read times. A small local bundle maps them to the four analytical roles:

```json
{
  "format_version": "cohort-bundle/1",
  "inputs": [
    {"name":"acquisition","manifest_file":"acquisition.manifest.json","dataset_name":"acquisition"},
    {"name":"identity_map","manifest_file":"identity_map.manifest.json","dataset_name":"identity_map"},
    {"name":"memberships","manifest_file":"memberships.manifest.json","dataset_name":"memberships"},
    {"name":"activity","manifest_file":"activity.manifest.json","dataset_name":"activity"}
  ]
}
```

A binding may select a differently named dataset, such as `result`, without changing its analytical role. `--manifest` accepts this local bundle. Export manifests must have `format_version: "1"` and `query.definition_version: "cohort-input/1"`. Use source SQL/projections to produce the following rows; this is a versioned example query-result contract, not a required game schema. The synthetic SQL names in fixtures describe manufactured sources, not existing application tables.

Only locally downloaded UTF-8 JSONL files are supported. The adapter checks SHA-256, bytes, row counts, required row fields and versions. Files must remain inside the selected directory, including after symlink resolution; URLs are never followed. Each input file is bounded to 64 MiB; aggregate/filter at the source before using this intentionally small in-memory example. Missing datasets, integrity failures, malformed JSON, failed/cancelled/expired server states and unsupported schema versions fail with exit code 2. Explicit partial exports remain analyzable with unavailable rates. No omitted coverage field defaults to complete. Continuation or sampling prevents a complete-history claim.

### Canonical query-result rows: `cohort-input/1`

All IDs are strings. Wall times must use explicit UTC `Z` or `+00:00`. Optional values are JSON null, not zero. Unknown campaign attribution stays `campaign: null`; it is not direct traffic.

| Dataset | Required fields and semantics |
| --- | --- |
| `acquisition` | `event_id`, `kind` (`visit`, `signup`, `revisit`), nullable `campaign`, `occurred_at`, boolean `authoritative`, `property`, nullable `visit_id`. A visit additionally has `visitor_namespace`, nullable `visitor_id`, boolean `linkage_permitted`. Signup/revisit rows have `issuer`, `subject`. Authoritative signups have `attribution_rule` and, for tagged acquisition, `touch_id`. |
| `identity_map` | `issuer`, `subject`, `application`, nullable `local_account_id`, `status` (`verified`, `unmapped`, `deleted`). One current mapping per issuer/subject/application; several applications are allowed. Verified maps require a local account ID. |
| `memberships` | `application`, `local_account_id`, `world`, `player_id`, `enrolled_at`, `history` object. One unique player membership per application/world/player. Membership inventory includes historical/deleted world memberships needed to establish first play; do not substitute only currently active memberships. |
| `activity` | `event_id`, `application`, `world`, `player_id`, `occurred_at`, `recorded_at`, boolean `authoritative`, `actor`, `outcome`, `action`, `logical_ms` (nonnegative decimal integer string). Additional original facts such as `rules_version` are preserved in inputs. |

Membership `history` is `{ "start": "UTC", "end": "UTC", "status": "complete|partial|unavailable", "sample_fraction": "1", "reason": null }`. The nested sampling fraction accepts a JSON number or lossless decimal string; PostgreSQL exports use strings even inside JSON. Decimal comparisons do not round a fraction near one up to complete coverage. It describes the retained activity interval for that player, not their lifespan. Use partial/unavailable to represent gaps inside the interval. Retention expiry moves `start` forward; lost future history moves `end` back. Complete means every required meaningful-action class was available, not merely that a query finished successfully.

Server manifest `coverage` remains the exact parent-level DTO: `complete`, nullable `history_start/history_end`, `limitations`, nullable `sample_rate`, `policy_versions`. All four exports must establish coverage for the relevant interval; complete=true without interval bounds or sample_rate=1 cannot establish availability. If a source cannot prove membership inventory or mapping completeness, declare that coverage incomplete. A per-player complete interval never overrides incomplete parent coverage.

Join `(issuer, subject)` → verified `(application, local_account_id)` → `(application, world, player_id)`. Never join subject UUID directly to local UUID. World/player IDs may repeat across applications; subjects may repeat across issuers. Several memberships count as one acquired account. Acquisition events deduplicate by `(property, event_id)`, so the same event UUID on two properties remains two facts. Exact duplicate delivery is ignored; conflicting scoped IDs and ambiguous account ownership fail. Explicit deleted or unmapped identities are excluded from player metrics. Do not supply unverified browser identity assertions as verified mappings.

### Fact and clock rules

The exporter supplies immutable authoritative signup acquisition, including selected touch/rule provenance. The analysis does not reattribute using later browsing. `revisit` is a verified existing-account login/touch, not a signup; it never enters acquisition denominators. Browser visits and observed browser identities are counted separately from accounts and memberships. A browser identity may represent multiple people or devices and is never labeled a person.

The report is an occurrence-time restatement: only events with `occurred_at < cutoff` count. Source reads can occur later, so late-arriving original facts may appear in a reproduced report with a new manifest. This is not a claim about what was known at cutoff. `recorded_at`, source reads and checksums preserve that distinction. Game logical milliseconds are lossless strings and never used as elapsed wall time. No anonymous cross-device or shared-device identity is inferred; visit-to-signup links must be explicit, permitted and scoped by property.

## Cross-property ingestion exports

The engine's acquisition projection is signup evidence from the signup property. Its original `property` and `visit_id` must stay unchanged: an authentication visit is not the original campaign landing. A selected `touch_id` alone is not permission to join arbitrary browser identities.

To analyze actual ingestion exports, add separately authorized source bindings to the local bundle:

```json
{"name":"acquisition_visits","manifest_file":"origin-visits/manifest.json","dataset_name":"visits"}
{"name":"acquisition_touches","manifest_file":"origin-touches/manifest.json","dataset_name":"touches"}
```

These are additional elements of `inputs`, not separate top-level JSON documents. Each optional role may occur several times for different properties. All manifests remain exact server v1 manifests with `query.definition_version: "cohort-input/1"`; downloads keep their original file names in separate directories. The service authorizes every property export independently. The client never expands a single property's authorization through the local bundle.

- `acquisition_visits` rows use the ordinary acquisition `kind: "visit"` schema. They represent the canonical earliest retained navigation for a `(property, visit_id)`, including original campaign, timestamp, visitor namespace/ID and explicit `linkage_permitted`. A tagged touch later in a visit does not replace its landing campaign or manufacture another visit.
- `acquisition_touches` rows require `touch_id` (globally unique engine touch key), `property`, nullable `visit_id`, nullable `campaign`, `occurred_at`, boolean `linkage_permitted`. Original `event_id` and `campaign_facts` may also be present. An event UUID is property-scoped; it is not the global touch key.

`normalize.py` (`attributed-visit/1`) resolves only `signup.touch_id → touch.property/visit_id → canonical visit`. It requires explicit permitted linkage on both touch and visit, matching selected-touch/acquisition campaigns, and `visit time <= touch time <= signup time`. Conflicting global touch IDs, conflicting canonical visits, and contradictory facts fail closed. Missing/deleted/unknown touch or visit evidence remains unresolved. No identity, history, timestamps or property scope are guessed. Touches do not enter the visit count.

For a resolved signup, normalization adds `attributed_visit_property`, `attributed_visit_id`, `attributed_visit_verified: true` to an in-memory copy. It preserves the original signup `property`, `visit_id`, selected `touch_id` and attribution rule. A trusted query projection may supply the same three fields directly; true requires authoritative signup, selected touch/rule and nonnull origin scope. The optional normalizer rejects contradictions with that evidence. Every original manifest and checksum remains in report provenance; `provenance.normalization` records its version and matched/unresolved counts. Source files are never rewritten. Conclusive acquisition coverage is the intersection of all participating exports, never the strongest individual source.

For a newly observed real journey, `observed_converted_visits` shows successfully linked conversions even before the 30-day cohort matures. This is an observed count, not a conclusive rate: `signup_conversion.rate` remains null while immature or historically incomplete. It lets integration prove a real browser → ingest → export → analysis conversion without advancing the clock or claiming nonexistent historical coverage.

## Metric definitions: `campaign-cohort/2`

Every metric contains cohort size, numerator, eligible/mature denominator, exclusions and nullable rate; the report carries the definition and cutoff, and the exact source coverage is preserved in provenance. Exclusion counts partition the cohort with the denominator. An unavailable denominator produces null, never 0%. A zero numerator with a positive denominator is a measured 0%.

- Signup conversion: observed landing-campaign visits with an explicitly linked authoritative signup (regardless of the signup’s selected acquisition campaign) in `[visit, visit + 30 days)`. One converted visit counts once. Visits mature at 30 days; linkage must be permitted and acquisition history complete throughout the interval. This measures observed visit conversion, not unique people or all acquisition. The 30-day conversion window is distinct from the service's pre-signup touch selection rule. A visit landing on campaign A and later acquiring through campaign B can count as a converted visit for A while the acquired account remains in B. Version 2 makes this distinction explicit and supports verified cross-property origin scope; version 1 required equal signup/visit property and campaign.
- Activation: acquired accounts with an authoritative human applied `construct` or `navigate` command during `[signup, signup + 7 days)`. Accounts mature at seven days. Complete mappings, memberships and action histories are required. These action names are a versioned synthetic meaningful-action set; a new game needs an explicit reviewed action definition.
- First observed play: earliest eligible action across an account's mapped players/applications. Verified first-ever play additionally requires complete historical membership inventory and activity from the earlier of signup and all enrollments through that action. It cannot be inferred from current state, UUIDs or a truncated journal.
- Retention: eligible acquired accounts anchored to verified first-ever play; any eligible action by any of their mapped players in the return window counts once. First-observed anchors without complete earlier history are excluded. Accounts without observed meaningful play remain visible as exclusions. Signup cohort size is retained so every missing account is explainable.

| Measure | Window after first meaningful play | Maturity boundary |
| --- | --- | --- |
| D1 | `[24h, 48h)` | anchor + 48h |
| D7 | `[168h, 192h)` | anchor + 192h |
| D30 | `[720h, 744h)` | anchor + 744h |
| D180 | `[4320h, 4344h)` | anchor + 4344h |

Start boundaries are included and end boundaries excluded. At a cutoff exactly equal to the end, the account is mature. One microsecond earlier is immature. D180 is a one-day window 180 elapsed days after first play; it is neither six calendar months nor evidence of continuous activity for six months. Login, reading/inspection, browser clicks, scheduled completions, background polling, rejected commands and other-player effects are excluded from meaningful play.

Exclusions use a deterministic precedence: deleted/unmapped linkage; missing first-play anchor; unverifiable anchor; immaturity; unavailable return history. Activation instead checks immaturity then its seven-day history. An observed return with incomplete history does not silently improve the eligible denominator. Unknown history is not measured churn. Campaign association does not establish causation.

## Manufactured acceptance evidence and limitations

The fixture cutoff is July 1, 2026 at 01:00 UTC, exactly 181 elapsed days after its January 1 first-play anchor. `glass-garden` has ten acquired accounts, twelve observed visits, one existing-account revisit and nine linked world memberships across seven enrolled accounts. Mature D1/D7/D30 are 1/4; D180 is 1/3 because one journal ends before that return window. Activation is 4/6; signup conversion is 9/10. Every denominator has additional explicit exclusions. A separate null-campaign cohort proves unknown attribution, and an earlier campaign retains the revisiting account's acquisition.

Fixtures include two issuers sharing the same opaque subject, differing local account UUIDs, multiple worlds and applications, duplicate deliveries, exact return boundaries, a recent immature account, no enrollment, deleted/unmapped identities, missing early history, expired late history and events that are not human play. Tests also mutate coverage and clocks, validate tampering/path handling, and reproduce CLI output bytes.

This task demonstrates external local-file analysis and cohort correctness (A09/A10). It does not claim production collection, real six-month retention, PostgreSQL adapter correctness, remote authentication/downloads, or the complete browser-to-service journey. Those require the separate service/source/integration checks. The fixtures use exact server manifest DTO fields, but the SQL projections and truthful coverage must be supplied by a real source integration.

### Recorded validation

On Python 3.9.6, the following commands succeeded in the assigned worktree:

```sh
python3 tests/fixtures/cohorts/generate.py
python3 -m unittest discover -s examples/analysis -p 'test_*.py' -v
examples/analysis/analyze --manifest tests/fixtures/cohorts/manufactured/manifest.json --cutoff 2026-07-01T01:00:00Z --output /tmp/analytics-cohort-report
git diff --cached --check
```

The original suite ran 32 tests successfully, including regeneration byte equality and two subprocess CLI runs with byte-identical JSON/text outputs. The standalone CLI exited 0 and emitted the documented counts. Whitespace validation passed. These are local synthetic-file tests; no PostgreSQL connection, external API, production query, publication or deployment was performed by this task. Integration should run the same CLI against actual authenticated query-export downloads, including recursive numeric-string encoding and incomplete-history manifests.

The scoped-acquisition and cross-property normalization followup ran the same unittest command with 43 passing tests and reran the standalone CLI successfully. Added checks cover equal event UUIDs across properties, real-export-shaped separate visit/touch inputs, original signup scope preservation, a later selected campaign distinct from the landing campaign, missing/disallowed/ambiguous origins, source-manifest integrity and provenance, and observed conversions with null immature rates. This local adapter regression suite does not substitute for integration's actual browser/ingestion export journey.
