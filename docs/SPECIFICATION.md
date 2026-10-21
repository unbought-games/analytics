# Unbought Analytics Engine

High-level requirements, technical direction, and implementation handoff

**Date:** October 21, 2026
**Status:** Implementation requirements
**Authority:** Product requirements. All examples and acceptance data are synthetic. Deployment-specific configuration belongs outside this repository.

## 1. What we are building

Build an open-source analytics engine that helps operators understand traffic, people, campaigns, and player outcomes across configured web properties. Anyone can deploy, instrument, and query it using their own agent.

The primary interface is for agents. An analytical agent retrieves evidence, computes metrics, merges datasets, investigates interesting changes, and explains findings to the operator. We need accessible data and repeatable analysis before dashboards.

The engine has a Rust server, a lightweight browser library distributed through npm, HTTP ingestion for browser and backend events, and MCP exposed directly by the Rust server. MCP queries produce structured export files that the agent can combine outside the service. The engine also exposes configured external data sources, initially PostgreSQL, so the agent can investigate each game's authoritative history without forcing gameplay into a universal analytics schema.

The objective is the product loop:

**More players → useful feedback and evidence → better games → more players who return.**

Traffic and registrations are useful intermediate signals. Meaningful play and retention establish whether those visits are turning into a game people value. Non-pay-to-win does not mean avoiding detailed product measurement.

## 2. Settled decisions and implementation freedom

### Settled requirements

- Rust server; TypeScript/JavaScript browser component published through npm.
- PostgreSQL is the selected application database. Operators may use an existing PostgreSQL deployment with workload isolation.
- Headless first release, including administration; no built-in human web console.
- Open source from the start, independently deployable without operator-specific services.
- Fast and lightweight; performance is a first-class acceptance criterion.
- Shared web analytics inside and outside games, across configured properties.
- Broad interaction history by default, within the configured collection policy.
- Automatic campaign/UTM capture and continuity through eligible journeys.
- Simple string campaign/experiment identifiers; no campaign-management product required.
- Easy server-side event integration for authoritative outcomes.
- Progressive collection settings selected by the operator, with enforceable visitor-choice hooks.
- Direct MCP access to discovery, queries, and exports; no separate npm MCP service required.
- Authentication for humans and agents, with full functional parity under the same role/resource permissions. Administration must be available through MCP as well as any human-facing interface.
- Structured files are the primary analytical output. Cross-system joins and number crunching can happen in the agent's workspace.
- Configurable access to external game data; game-specific history remains owned by each game.
- Read-only queries and structured exports from configured Prometheus and Sentry sources; observability collection/storage remains external.
- No required human analytics dashboard or built-in LLM.

### Selected database and remaining implementation choices

Use a single Rust service with PostgreSQL for the engine's own events, attribution,
identity mappings, configuration, principals/permissions, credential verifiers,
saved queries, export metadata and audit records. PostgreSQL is a settled choice,
not a datastore selection task for the implementer. Large JSONL exports may live
in configured file/object storage with their metadata in PostgreSQL.

Use a dedicated logical database and least-privilege roles. Keep analytics writes separate from external application databases. External connectors require restricted read-only credentials and bounded workloads. Provisioning production databases is a deployment task.

Framework, exact repository layout, license, final package names, MCP library, and code generator remain implementation choices. Choose maintained dependencies and record material choices. Select an OSI-approved license before public release. Do not make ordinary engineering choices into repeated approval questions.

Candidate browser package: `@unbought/analytics`. Package availability and publishing access must be checked at release time.

## 3. Analytics responsibilities and external observability

| Responsibility | Authoritative data | Typical question |
| --- | --- | --- |
| General web/user analytics | Visits, navigation, interactions, source/campaign context, declared conversions, permitted identity links | Which campaign brought people who signed up? |
| Game-specific analysis | Memberships, commands, outcomes, progression, economy, combat, historical rules | What did those players do, and were they still playing six months later? |

The engine connects web/user analytics with game-owned evidence while preserving their different meanings and authority. It is not a new game engine.

Observability is a separate system, expected to use tools such as Prometheus and Sentry. This engine must be able to query those configured systems and export their results so the agent can investigate whether service failures explain product behavior. It does not collect/store general operational telemetry, implement metric scraping or error tracking, manage alerts, or deploy those systems. Its responsibility is a read-only source adapter and the common export contract.

A UI click is an observed intention. A committed game command is an authoritative outcome. A scheduled completion is a game event, not a fresh human action. Preserve these distinctions in queries and exports.

## 4. Primary end-to-end story: company transparency campaign

This is the acceptance story, not approval to launch a public campaign or disclose finances.

1. We define an experiment such as `open-finances-2026-11` and tag links with standard UTM parameters and, optionally, an explicit experiment identifier.
2. Someone visits an illustrative page such as `https://company.example/open`. `/open`, `/finances`, and `/analytics` are examples, not selected routes or existing features.
3. The browser library automatically records the landing, allowed UTM values, source context, and subsequent navigation/interactions.
4. The visitor explores the company, opens a game landing page, and starts signup through the identity application.
5. Eligible attribution context survives the cross-property/authentication journey. The authoritative signup event is associated with the selected campaign and account.
6. That account enrolls in the game application or another game. A verified identity mapping connects the acquisition record to the game's player ID.
7. The player performs meaningful actions. The agent can later retrieve the campaign cohort and its game activity/progression from the relevant sources.
8. The agent exports these datasets, joins them locally, and reports signup conversion, activation, and mature one-month/six-month return, including counts and data limitations.

Questions this must support:

- How many observed visits reached the campaign page, company landing page, and game landing pages?
- How many observed visitors continued to another property, started signup, completed signup, enrolled, and meaningfully played?
- Which accounts were acquired through this campaign? Which existing accounts merely revisited through it?
- How many acquired players returned after a day, week, month, or six months?
- What progression, action patterns, or losses preceded return or departure?
- Which parts of the funnel are unknown because collection, identity linkage, or history was unavailable?

The financial-transparency page may eventually be a beautiful public dashboard. Building that page and publishing financial data are separate work; the analytics engine measures its effect.

## 5. Browser instrumentation requirements

### Automatic collection

Provide a framework-neutral npm package and a straightforward way to use it on static sites. An operator configures endpoint, property/application ID, public ingestion credential if used, and collection profile. Framework adapters are conveniences, not requirements for using the core package.

Capture initial page views, SPA navigation, and meaningful interactions with links, buttons, tabs, menus, dialogs, and forms. Record form submission/validation categories without field contents. Prevent duplicate events from repeated initialization, framework lifecycle behavior, or multiple listeners.

Use stable semantic control names where available, preferably through a small documented attribute/component convention. Provide a bounded fallback such as element category and sanitized destination. Do not collect arbitrary DOM text or dynamic selectors containing personal information. Coverage must work without hand-writing an event for every button; explicit events enrich that coverage.

Automatically parse standard `utm_source`, `utm_medium`, `utm_campaign`, `utm_term`, and `utm_content`, plus the configured experiment key. Capture sanitized landing/referrer context. Allow normal bounded campaign strings without requiring an administrator to preregister each campaign. Validate field names, lengths and encoding; strip unapproved URL query values, fragments and sensitive content before transmission.

Track foreground/visibility and session boundaries only under an explicit definition and collection profile. Open tabs, timer ticks and background polling do not demonstrate human play. Any collection loss or sampling must be measurable and visible in coverage metadata.

### Public capabilities

The package needs initialization, explicit event tracking, permitted identity association/reset, collection-choice updates, flush/shutdown, and instrumentation exclusions. Exact function names are implementation choices. Initialization must be idempotent; logout/account switching must prevent later activity being assigned to the previous account.

Use delegated listeners and bounded asynchronous delivery. Avoid heavy DOM traversal, snapshots, replay, per-click network calls, unbounded offline queues, and persistent retry loops. Browser delivery is best effort; never claim guaranteed receipt on tab closure. Retrying an event must reuse its event ID.

## 6. Ingestion and original event facts

Provide a versioned HTTP ingestion contract suitable for browsers and arbitrary server languages, including batch requests. Publish an OpenAPI schema or equivalent machine-readable HTTP contract; generate types where useful.

The event envelope must represent:

| Field group | Required semantics |
| --- | --- |
| Identity | Stable event ID; installation/property/application scope; optional permitted visit/visitor/subject identifiers and their namespaces |
| Event | Name, schema version, browser/server producer, semantic category, typed allowlisted properties |
| Time | Occurrence timestamp, receipt timestamp, and a distinction between client, server wall-clock, and game logical time |
| Context | Environment, application release, relevant page/control, optional request/command correlation |
| Attribution | Campaign/touch reference and rule version where applicable |
| Collection | Effective collection profile/policy version and relevant coverage/sampling indicators |

Property schemas may differ by event and product. Preserve types, units and schema versions. Do not send full game journals, unrestricted request/response bodies, passwords, tokens, or secrets into the generic event store.

Separate public browser ingestion from authenticated server ingestion. A public property token is not proof of a human visit or a trusted signup. Browser traffic cannot assert a reserved authoritative outcome, impersonate another property's server, or assign arbitrary accounts. Query/admin credentials must never be shipped to the browser. Add scoped credentials, request-size limits, quotas/rate controls and documented bot/test traffic handling.

For accepted batches, define the acknowledgement and durability boundary, per-event rejection behavior, and retry semantics. Deduplicate on a scoped stable event ID; conflicting payloads with the same ID must not silently overwrite one another. Handle late and out-of-order delivery without losing events at a time/cursor boundary. Report rejected/dropped/backlogged data.

Authoritative server outcomes should be captured transactionally or through a recoverable outbox/journal projection. Application success must not depend on a synchronous external analytics call. A generic SDK cannot make an arbitrary post-commit callback durable; document the producer's responsibility. Bounded queues, recovery and retention must prevent analytics failure from exhausting the game host.

## 7. Attribution and identity

### Campaign facts and rules

Campaigns/experiments are strings with optional metadata. A string is sufficient to start recording and querying; campaign CRUD, budgets and creative-management screens are unnecessary. Use an explicit experiment ID when provided and preserve UTM values separately. A `utm_campaign` alone must still be queryable as a campaign.

Store original touch facts separately from the attributed conversion. Define exactly which touch supplies acquisition attribution. A proposed v1 rule is **first eligible tagged touch within a configurable 30-day pre-signup window**, subject to the active collection policy. Preserve the last eligible tagged touch as separate evidence if available. Record the rule/version and the selected touch ID so future analyses can compare models without rewriting original facts. The implementer may choose a different default only by documenting the behavior and updating its tests.

Once an authoritative signup is associated with a campaign, normal browsing and later campaigns must not overwrite that acquisition relationship. Later campaign interactions are additional touches; existing-account logins are not new signups. A deliberately corrected attribution requires an auditable correction, not silent mutation. Unknown/missing attribution remains unknown, not automatically direct traffic.

The pre-signup attribution window is different from post-signup cohort retention. Keep the minimal permitted acquisition/identity facts long enough to support six-month analysis even if detailed browser interactions have shorter retention. "Always know where they came from" means durable acquisition provenance within the configured retention/deletion policy, not an exemption from deletion or indefinite storage of every click.

### Cross-property journey

Support the eligible path from `company.example` through the identity application to `game.example` and future configured properties. Do not assume one cookie can span unrelated domains. Use explicit, allowlisted handoffs with bounded lifetime, audience binding and verified redemption, integrated with the existing auth state where appropriate. An opaque reference must not become an exposed account identifier, authentication token or open redirect.

Preserve eligible campaign context through ordinary navigation, auth redirects, cancellation/retry and delayed signup within the selected window. Exact persistence depends on collection policy. Untagged browsing must not overwrite a still-valid tagged touch. Expired context must not leak into unrelated later signups. Invalid/untrusted campaign strings are not executable instructions.

### Verified identity

Keep anonymous/browser identities distinct from authenticated account identities. Resolve account-to-player links through the verified authentication/enrollment path. In the synthetic acceptance installation, identity-provider subjects and application-local account UUIDs differ; issuer/subject mapping and memberships connect them. Never join those UUIDs directly or use email as the universal key.

Support several players/world memberships per account and several games per account without double-counting people. Expose namespaces and mapping coverage in exports. Identity associations from an untrusted browser are not authoritative. Do not probabilistically merge people using IP addresses or browser fingerprints.

Only link prior history when the configured collection basis and actual evidence permit it. Do not invent anonymous cross-device continuity. Logout/shared-device boundaries, deleted identities, and unknown mappings must be explicit.

## 8. Progressive collection and privacy controls

Support operator-selected presets with explicit behavior. Proposed v1 profiles:

| Profile | Identity/history behavior | Expected limitations |
| --- | --- | --- |
| Minimal | Broad permitted interaction events without durable person identity | Cannot claim unique people across visits or person-level longitudinal retention |
| Journey | Defined visit-scoped event sequences | Cross-visit attribution and return may remain unavailable |
| Identified | Verified pseudonymous account linkage and history across visits | Requires operator-configured purpose, retention, disclosures and applicable choices |

Cross-property linking, detailed property collection and retention are separate controls. Presets must not silently enable session replay, fingerprinting, keystrokes or raw field capture. Basic interaction breadth and identity depth are different settings.

The full campaign-to-six-month-player story requires a configuration that permits the necessary attribution and identity linkage. Minimal mode must state its limitations instead of producing misleading conversion/retention claims. Operators must be able to see which questions their selected settings can answer.

Enforce the effective policy in the browser and ingestion service. Provide integrations for consent/opt-out state, withdrawal, applicable preference signals and deletion. Do not collect prohibited fields and merely hide them from queries later. A browser-supplied permission flag alone is not trusted server authorization. Keep unrelated operational records and their retention purposes distinct from optional browser analytics.

The operator is responsible for selecting lawful processing and appropriate notices/choices; the product supplies enforceable settings and documentation. No preset is marketed as universally "no banner required." Cookieless, pseudonymous and server-side do not automatically mean exempt or anonymous. Deployment-specific policy review belongs to the operator.

Policy changes affect future collection. Make their effective date/version visible to analysis, honor withdrawal prospectively and deletion as required, and never manufacture historical identity links that were not collected or permitted.

## 9. External sources and game-specific evidence

The Rust service must support operator-configured data sources. The first useful connector is PostgreSQL for the identity application and the game application; the core model must permit future source kinds without hard-coding the game application into generic analytics.

Include read-only Prometheus and Sentry adapters as specified below. They query operator-provided services; installing or instrumenting those services is a separate infrastructure task.

A source definition includes stable ID, kind, environment, allowed schemas/views, credentials referenced through the deployment's secret mechanism, query budgets, and documentation of timestamp/identity/history semantics. Sources are configured by an administrator; ordinary MCP queries cannot supply arbitrary connection strings or probe arbitrary network addresses.

Through MCP, an agent must be able to discover source schemas, inspect documented column types, execute flexible read-only analytical queries, and export results. Do not restrict it to a fixed menu of prewritten reports: the point is to ask new questions. Enforce database privileges and execution limits; a naive check that SQL starts with SELECT is not a sufficient read-only boundary. Do not expose credentials, security tables, unsafe functions or write/admin capabilities through analysis access.

Use narrow database roles, source/column allowlists where needed, read-only transactions, query cancellation, statement/lock budgets, bounded concurrent queries and result/export limits. Use workload isolation or replicas as justified by actual load. The agent should aggregate near the source before exporting detailed evidence. Full production dumps are not the normal analysis mechanism.

Game facts can remain in the game database. Export query results and identity mappings through the same file-result contract; cross-source joins can happen in the agent workspace. Do not build distributed SQL, replicate every game table, or create a universal economy model as a prerequisite.

### Game history requirements

For a player, entity, world and interval, the game should retain enough evidence to understand actions, outcomes and relevant causes. At minimum, audit:

- Enrollment and lifecycle timestamps.
- Commands attempted/admitted/applied/rejected, with deduplication IDs and reasons.
- Scheduled effects and actions by other players affecting this player.
- Progression milestones, resource changes and combat/loss evidence where relevant.
- World/epoch, rules version, event ordering and history completeness.

A chronological player-command export is a required starting capability, but it is not proof every economy question is answerable. Passive production, historical state, deleted entities and external attacks may require additional facts or reconstruction. Prefer existing journals/records; do not add snapshots every tick or derived counters to the authoritative game state merely for analytics.

Classify each target question as **answerable now**, **needs a query/access improvement**, or **needs a new original fact**. Record exact missing facts and the date instrumentation starts. Never backfill unsupported history from UUIDs, current state or guessed timestamps.

### External observability query adapters

- **Prometheus:** support bounded instant and range queries using PromQL, plus authorized metric/label discovery. Preserve query expression, evaluation time or start/end/step, labels, units where known, result types and source warnings. Limit query duration, returned series and samples. Preserve gaps and special values through documented lossless JSON encoding; missing samples are not zero.
- **Sentry:** support querying issues/error events by configured organization/project, interval and supported filters, and retrieving selected event details for investigation. Preserve issue/event identifiers, timestamps, release/environment context and pagination. Issue counts are not event counts or affected-player counts. Use only required read scopes, redact sensitive context and never resolve/delete issues or change project settings through this adapter.
- **Shared behavior:** configure endpoints, source scopes and secret references administratively. Respect rate limits, cancellation, output budgets and source retention. Record unsupported operations, unavailable sources and partial results explicitly; return no-data only after a successful query proves it. Do not proxy arbitrary URLs or expose vendor credentials.
- **Output:** use the JSON-manifest/JSONL export contract in Section 10. Include vendor query parameters, source read time, metric resolution or issue/event semantics, coverage and continuation state. Correlate locally by time, application, environment, release and explicit request IDs where available. Do not invent player-level joins from aggregate metrics.

This is query/export integration only. Scrapers, tracing SDKs, error capture, time-series storage, alert routing and observability dashboards remain outside the engine's product scope. Provider API references: [Prometheus queries](https://prometheus.io/docs/prometheus/latest/querying/basics/), [Sentry issues](https://docs.sentry.io/api/events/list-an-organizations-issues/), [Sentry event detail](https://docs.sentry.io/api/events/retrieve-an-issue-event/).

## 10. Agent interface and structured files

### Authentication, authorization and human/agent parity

Humans and agents are first-class authenticated principals. Separate identity
from authority: an agent can receive the same owner/admin capabilities as a
human, while either may also be restricted to analysis or specific properties.
There must be no human-only product operation or mandatory UI step for routine
administration. Capability parity is not automatic elevation of every agent.

Support identifiable human access, delegated agent access on behalf of a human
where appropriate, and independently identifiable service/agent accounts for
unattended operation. Use a standards-based human sign-in integration (OIDC is
the preferred proposal, with operator-selected issuer) and documented scoped,
revocable credentials for automation. Do not require a particular identity provider.
Provide a secure headless initial-owner/bootstrap and recovery
path without default public credentials or a permanently open setup endpoint.
Exact authentication components are implementation choices; both human and
non-interactive agent workflows must be demonstrable.

Remote MCP authentication must interoperate with the selected protocol version's
authorization specification, including discovery and token validation. Document
the machine-client flow and how supported clients obtain/use credentials. Do
not conflate an OIDC identity token, an ingestion key and a query/admin access
token. Validate issuer/audience/expiry as applicable and enforce authorization
on every operation and artifact retrieval. Reference: [MCP authorization](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/draft/basic/authorization/index.mdx);
pin the supported released protocol version during implementation.

Use one authorization layer for HTTP, MCP, CLI and any eventual UI. Define roles
or scopes covering ingestion, analytical queries/exports, property/source/policy
administration, credential/principal management and data retention/deletion.
Permissions can be constrained to installations, properties, sources and exports.
Both humans and agents with equivalent grants receive equivalent capabilities.
Existing read-only PostgreSQL/Prometheus/Sentry adapters remain read-only for
everyone; parity does not add vendor mutation or game-write capabilities.

Every product administrative operation must have an authenticated programmatic
interface and MCP coverage: configure properties and source connections, set
collection/retention policies, manage authorized users/agents and grants,
issue/rotate/revoke permitted credentials, inspect operational status, and perform
authorized export/data deletion. Prevent privilege escalation beyond the caller's
grant-management authority. Secure bootstrap/host-secret provisioning may use a
documented CLI; these must also be automatable by an appropriately authorized
operator agent. No human-browser-only approval flow is introduced by this spec.

Maintain an audit trail of sensitive administration with principal, delegated
identity when applicable, action, target, timestamp and outcome. Do not log
credential values or expose existing source secrets. Credential creation/rotation
may deliver a secret once through an explicitly authorized secure response;
ordinary queries and exports never include those secrets. Support expiry,
rotation and revocation; test that revoked access cannot obtain protected data,
including through queued exports or artifact download references. If a future
UI uses cookie sessions, protect its state-changing requests and logout lifecycle.

### Headless bootstrap and first-agent configuration

Provide a documented operator CLI in the Rust binary, illustratively
`analytics bootstrap`, for the first installation. It performs the database writes
needed to establish administration; normal setup must not require hand-written
SQL, manual password/token hashes, or editing authentication tables.

The installation guide must cover this complete sequence:

1. Configure the analytics database connection and run versioned migrations using
   the documented deployment credentials. Separate migration/provisioning access
   from runtime permissions; no database superuser credential enters MCP.
2. Run bootstrap locally or in the deployment container using authorized host
   access. Create the named initial owner principal and a separately identifiable
   first agent/service principal, granting the agent the selected owner/admin
   role. This establishes who owns the installation without requiring an OIDC
   browser login, email delivery or a web UI before the first agent can operate.
3. Issue the agent's initial scoped access credential with documented expiry and
   revocation semantics. Generate a high-entropy secret, retain only its verifier
   when using opaque tokens, and deliver the raw value once through a protected
   file/secret handoff. Do not put it in command arguments, ordinary logs, source
   control or example client configuration committed to a repository. If using a
   credential exchange, document how that credential obtains the MCP access token.
4. Configure the agent's MCP client with the server URL and a secret reference in
   the selected client's supported authentication mechanism. Include a working
   example using placeholders and a live identity/capability check so the agent
   can verify its principal and granted authority without exposing the secret.
5. Through MCP, configure properties, collection policies, source connections,
   additional humans/agents and their grants, then perform a test query/export.
   No switch to a browser or SQL console is required for routine configuration.
6. Document credential rotation, revocation and owner recovery. Bind any later
   human sign-in identity explicitly to the existing owner; do not silently
   attach external identities based only on a matching email address.

Bootstrap must be atomic and safe under concurrent execution. Once an owner is
initialized, rerunning the bootstrap command must not create another owner,
replace credentials, reset permissions or disclose an existing secret. Return a
clear already-initialized result and direct the operator to normal authorized
credential management. Provide a distinct, documented local recovery command for
an operator with deployment access if all administrative credentials are lost;
record the recovery and make revocation choices explicit. Recovery is not a
permanently open HTTP backdoor. Both bootstrap and recovery must be automatable
by an agent that has the same authorized deployment access as a human operator.

### Human interface scope

The first release is explicitly **headless**, including administration. the operator
selected this to ensure MCP supports the complete workflow. Humans use
authenticated CLI/API/MCP clients and retrieve files; do not build a web console,
event inspector or dashboard in this release. Human authentication does not
itself imply a product UI; an external identity provider may have its own sign-in
screen. A human UI may be reconsidered later, with no exclusive capabilities.

### MCP capabilities

The Rust server exposes an authenticated MCP interface with these capabilities; exact tool names are implementation choices:

| Capability | Contract |
| --- | --- |
| Discover | List authorized properties, sources, event schemas, collection profiles, metric definitions and coverage |
| Query analytics | Filter/aggregate stored events, retrieve ordered sequences and campaign cohorts with explicit windows |
| Query source | Describe authorized external capabilities and execute bounded read-only PostgreSQL, Prometheus or Sentry queries |
| Export | Materialize results as structured files; return status/manifest and a usable retrieval mechanism |
| Reproduce | Reference a saved query/definition/version and its parameters; rerun with a specified cutoff |
| Administer | All product configuration, principal/credential/grant management and authorized lifecycle/deletion operations available to a human with equivalent permissions |

Keep ingestion, query and administration scopes distinct. An analytical role is read-only apart from creating private query/export artifacts for both humans and agents; agents may also hold owner/admin roles. A tool description is not authorization; enforce scope in the service. A built-in LLM, separate Node MCP service, and hosted SDK-generation account are unnecessary.

### Files are the primary result

**Every analytical query should be able to produce a durable, bounded, structured export.** MCP returns a compact description, counts/status and manifest/resource references, rather than dumping large rowsets into the model context. Small summary values may accompany the artifact.

Use UTF-8 JSON for the manifest and small structured reports; use JSON Lines (`.jsonl`) for row-oriented datasets so agents can stream them with ordinary tools. Document optional compression. CSV can be an optional convenience; Parquet is deferred until scale warrants it. Do not serialize arbitrary database values through lossy JavaScript numbers.

An export consists of a manifest plus one or more named datasets. The manifest must include:

- Export ID, format/schema version, dataset/file names, media types, byte counts and checksums.
- Column names/types, units, timestamp conventions, null semantics and join-key namespaces.
- Source/property/environment and producer provenance.
- Query text or reproducible query reference, definition version, parameters and requested time window, with sensitive parameters restricted to authorized private artifacts.
- Creation time, source read times, cutoffs/watermarks and history/freshness coverage.
- Row counts, completion status, truncation/continuation state, sampling and collection-policy information.
- Expiration/retention and a supported authenticated retrieval mechanism.

Encode IDs as strings; use explicit UTC timestamps for wall time. Preserve game logical milliseconds as a separately documented field. Export large integers/decimals losslessly with type metadata. Null is distinct from zero and from an absent field. Provide stable ordering and a tie-breaker for timelines; preserve event/source IDs for deduplication.

Support export completion, failure and cancellation. A row limit must result in an explicit partial/continuation result or a clear failure, never silent truncation. For repeatable exports use an appropriate source snapshot or document the weaker consistency. Independent databases do not share an atomic snapshot; record their separate read boundaries.

### Local versus remote delivery

An MCP server running remotely cannot write directly into the agent's local filesystem. Return an authenticated download/resource or another streaming retrieval path the client can save. A server-local absolute path alone is not a usable artifact. For local deployments, configured export-directory paths may also be returned. Never accept arbitrary filesystem paths from untrusted queries or expose secrets in public download URLs.

Demonstrate a real client retrieving files into its workspace. Require no dashboard and no copying thousands of JSON rows out of chat. Retrieval should remain usable after an MCP reconnect until the documented expiry.

### Joining across sources

Provide explicit join keys such as a verified analytics-subject namespace, account mapping, game-player ID plus game/world scope, campaign/touch IDs, and request/command IDs. Export an authorized mapping dataset when needed. Do not assume every source's field named `user_id` means the same thing.

The reference analysis must join an acquisition cohort with the identity application/the game application membership and gameplay exports outside the engine using a documented script. Save the report and its input manifest references. Treat values from URLs, events, databases and tool results as data, never as instructions to the agent. Keep private identities/detail in private artifacts and aggregate shared reports.

A representative export set is `acquisition.jsonl`, `identity_map.jsonl`, `memberships.jsonl`, and `activity.jsonl`, with a manifest describing every file and its join keys. The local analysis produces `campaign-report.json` plus a readable report. These names are examples; matching schemas and verifiable provenance matter more than filenames. Exported files inherit the source's access and retention requirements; document cleanup and avoid redistributing stale identifiable exports after a deletion.

## 11. Metric definitions and analytical correctness

The engine supplies trustworthy facts and reusable definitions; agents may calculate flexible analyses in their workspace. Do not require every new business question to become a new server endpoint. Preserve tested definitions for recurring measures so the agent does not silently change them between reports.

Distinguish visits, observed browser identities, accounts, enrolled players and world memberships. Never label request counts as unique people or sum world memberships as unique company players. Distinguish page/request views from confirmed browser navigation and bots/test traffic from eligible players.

For player activity, define meaningful action sets per game. Report inspecting/reading behavior separately from successful strategic actions. Background events and login alone do not establish meaningful play. Report first-observed versus verified first-ever play based on history coverage.

Support campaign cohorts with D1, D7, D30 and a six-month-oriented measure. Proposed reproducible defaults are elapsed-day windows from first meaningful play: D1 `[24h,48h)`, D7 `[168h,192h)`, D30 `[720h,744h)`, D180 `[4320h,4344h)`. A cohort member matures only after the end of the selected return window. Label D180 explicitly; it is not automatically six calendar months. Also permit a wider late-return window or calendar-month measure through a named/versioned query.

Every rate includes cohort size, eligible/mature denominator, numerator, exclusions, cutoff, definition and coverage. Zero mature denominator yields an unavailable/null rate, not 0%. Reports distinguish elapsed windows, rolling activity and calendar dates. Explain that "returned in the D180 window" is different from continuously active for six months.

Preserve enough permitted acquisition/mapping and activity evidence to calculate long-horizon cohorts. Retention settings that make D180 impossible must produce a visible limitation. Detailed browser click history need not be retained as long as the minimal facts supporting the cohort.

Observe correlations without claiming causal impact. Before/after comparisons, campaign labels and feature-use cohorts do not by themselves constitute randomized experiments. Small samples and immature cohorts are legitimate inconclusive results.

## 12. Performance, reliability and operation

Performance must be measured, not described as "lightweight" without evidence. Before finalizing architecture, record reproducible budgets and benchmarks for:

- Browser compressed/minified bytes and dependencies; initialization and per-interaction main-thread cost.
- Request/byte volume per representative session, batching behavior and maximum queue memory.
- Ingestion throughput, p50/p95/p99 acknowledgement latency and behavior under downstream failure.
- Query/export latency and memory on a representative retained dataset; streaming exports must not buffer the full result in memory.
- External database query cost, cancellation and isolation from gameplay.

Use a documented fixture dataset and hardware/configuration. Measure normal browsing and a burst scenario. Exact numeric budgets are implementation proposals to record with results, not invented product guarantees. A result that blocks the UI or gameplay requests, grows memory without bound, or silently drops authoritative facts fails the design regardless of average throughput.

Provide health/readiness, migrations, backup/restore documentation, structured diagnostic logs and minimal service metrics for operating this service. Expose ingestion backlog/loss and export failures. These are the engine's own operational signals, not an observability product; external systems own their collection and alerting.

Provide a reproducible local deployment and containerized self-hosting path. No mandatory connection to company-specific services, generator vendors, proprietary model APIs or a billing account. Credentials stay outside source control. Public documentation uses synthetic data.

## 13. SDK generation and integration scope

Generate wire types/API clients where it reduces drift. Keep the performance-sensitive browser capture runtime small and intentional. Server libraries may add batching/flush helpers, but authoritative delivery requires application-level durable capture as described above.

Rust and TypeScript integrations are the first priorities because the game application uses Rust and the identity application uses TypeScript. The HTTP contract lets other languages integrate without waiting for an official SDK. Every client accepts the operator's base URL.

A generator decision must consider local reproducibility, output licensing, bundle size, schema fidelity, timestamp/int64 handling, error types and regeneration stability. Generating a separate TypeScript MCP service is superseded by the selected Rust-server architecture.

No npm publication is performed by this specification. Prepare package metadata, build artifacts, integration examples and publish/release instructions for the appropriate release step.

## 14. The measurement and improvement loop

The analyst’s workflow should be repeatable:

1. Write a question/hypothesis and assign a campaign/experiment string if relevant.
2. Discover sources and definitions; check whether the required facts and history exist.
3. Query/export what is already available and document exact missing evidence.
4. Prepare targeted instrumentation changes in the relevant product or engine.
5. Test, review and release those changes through the applicable project process.
6. Verify live collection and complete a controlled attributable journey before relying on it.
7. Run the authorized experiment, then query/export and join results at explicit cutoffs.
8. Report findings, uncertainty and decisions; preserve failed and superseded experiments.

The system makes this loop easier; it does not autonomously authorize campaigns, spending, messages, changes to gameplay, or deployments. Adding a new event can only measure from its actual coverage start. Preserve the no-pay-to-win principle; marketing actions never grant gameplay advantages.

## 15. First-release scope and deferred work

**Required first release:** independently runnable headless Rust service backed by PostgreSQL; documented first-owner/agent bootstrap; human and agent authentication with full permission-equivalent MCP administration; configurable properties and progressive profiles; browser package with broad automatic interaction/UTM capture; authenticated server ingestion; campaign/account association; the cross-property signup/enrollment example; direct MCP discovery/query/export; JSON/JSONL file delivery; read-only PostgreSQL, Prometheus and Sentry source adapters; an agent-side cross-source cohort analysis; integration and operating documentation; and meaningful validation.

**Defer:** any human web administration console or analytics dashboard, public financial dashboard, session replay/heatmaps, predictive churn/LTV, automatic marketing decisions, ad-network integrations, distributed SQL, comprehensive warehouse replication, Kafka-scale infrastructure, numerous SDK languages, and a commercial multi-tenant SaaS. Do not let these delay the complete first journey.

**Outside this product:** operating or replacing Prometheus/Sentry, general telemetry ingestion/storage, monitoring/alerting infrastructure and observability dashboards. The required adapters query existing services only; adapter tests must work against controlled fixtures without deploying a production monitoring stack.

## 16. Acceptance evidence

| ID | What must be demonstrated |
| --- | --- |
| A01 Independent setup | A clean install with synthetic properties/data, no company-specific credentials, browser instrumentation, backend event and working MCP client. |
| A02 Capture | Initial/SPA navigation and meaningful interactions automatically appear once; URLs/properties are sanitized; repeated SDK initialization is safe. |
| A03 Campaign continuity | A tagged company landing → game → the identity application signup → enrollment → first action journey produces traceable acquisition. Include cancellation, retry, expired/invalid context, untagged navigation, later campaigns and existing-account login. |
| A04 Identity | Correct mapping despite different the identity application/the game application UUIDs; multiple worlds, logout/shared device, unmapped and deleted subjects do not create false joins. |
| A05 Authoritative delivery | Rollback produces no success event; retries deduplicate; conflicts are detected; scheduled actions do not count as human play; late/out-of-order commits are retained; outage recovery works. |
| A06 Collection profiles | Each profile actually changes collected/linkable data; forbidden fields are excluded; withdrawal/opt-out/deletion work; client flags cannot escalate server authorization; reports expose unavailable linkage. |
| A07 Source queries | The agent discovers/query-exports a configured PostgreSQL source; credentials and unauthorized data stay inaccessible; writes/unsafe operations and runaway queries are prevented. |
| A08 File outputs | MCP provides a retrievable manifest plus JSON/JSONL datasets; remote retrieval works; checksums/types/order/nulls/large integers survive; partial/failure/expiry behavior is explicit. |
| A09 External analysis | A script joins campaign/account/player/activity files outside the engine and produces reproducible signup/activation/D1/D7/D30/D180 metrics with source references. |
| A10 Cohort correctness | Controlled clocks/fixtures exercise boundaries, mature/immature cohorts, exclusions, missing history and retention expiry. Six-month logic is tested with fixtures; no invented live six-month result. |
| A11 Performance | Recorded browser/ingestion/query/export measurements under documented workloads, including bounded memory, backpressure and database impact. |
| A12 Handoff | Versioned contract, examples, schema/data dictionary, documented attribution rule, privacy-profile behavior, source setup, restore/release instructions and known gaps. |
| A13 External observability queries | Prometheus instant/range and Sentry issue/event queries produce retrievable typed files against controlled API fixtures. Prove scope restrictions, rate-limit/pagination handling, missing-series and unavailable-source semantics, and local correlation by time/release. No collector, alerting system or vendor mutation is introduced. |
| A14 Human/agent auth and parity | Demonstrate empty-database migrations → atomic first-owner/agent bootstrap → protected credential handoff → MCP identity/capability check → configuration/query/export, without UI or manual SQL. Concurrent/repeated bootstrap cannot create extra owners or overwrite/reveal credentials. Test rotation, revocation and explicit local recovery, human access and unattended agent access. Agents can perform every human product operation with equivalent grants; restricted principals cannot escalate. Enforce identical resource scopes across MCP/HTTP/artifacts, exclude secrets from ordinary outputs, and verify audit attribution and denied access after revocation. |

Complete the full story in a controlled local/integration environment. Live verification follows the applicable deployment authorization and must confirm actual revision/schema/collection, not infer it from green tests. Do not make fake production players or export real player databases merely to populate examples.


## 17. Repository and release boundaries

This repository is a generic independently deployable product. Do not include references, paths, hostnames, integration configuration, schemas or private working memory from the operator’s other properties. Use manufactured datasets and synthetic producers for all committed tests, examples and benchmarks. Optional smoke tests against applications outside this repository are local setup only; no deployment or package publication is part of implementation.

Record tested revisions, acceptance evidence A01–A14, benchmark results, sample exports, and remaining deployment/publication steps. Distinguish implemented, tested, deployed and collecting real data. Commits and PR titles follow Conventional Commits.
