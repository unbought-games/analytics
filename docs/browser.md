# Browser package

The browser SDK captures observations with zero runtime dependencies. Build output
includes ESM, CommonJS, TypeScript declarations and a standalone `Analytics`
global for static sites. It is not a server producer and cannot claim a signup,
enrollment or committed action. Browser counts do not establish human play.

## Build and integrate

From the repository root:

```sh
npm ci
npm run build
npm run check
npx playwright install chromium
npm run test:browser
npm run test:browser:service
npm run benchmark:browser
```

The service regression starts its own loopback-only disposable `postgres:17`
container, runs the actual Rust ingestion harness and removes the container on
completion. Docker and Cargo are required. `INGEST_TEST_DATABASE_URL` may instead
name an explicitly disposable test database; the Rust harness creates an isolated
schema. Never point this command at a production database. Linux CI can install
Chromium with `npx playwright install --with-deps chromium`.

Use the built workspace package locally; publishing and public package-name
availability are release steps. No npm publication has been performed.

```ts
import {init, browserSchemas} from '@independent-analytics/browser';

const analytics = init({
  endpoint: 'https://analytics.example',
  propertyId: '11111111-1111-4111-8111-111111111111',
  publicToken: 'replace-with-issued-public-property-token',
  policyVersion: 1,
  profile: 'journey',
  choice: {enabled: true},
  routes: ['/', '/guide', '/articles/:slug']
});
await analytics.ready;
```

First configure the property's allowed origins/policy and issue a public
property token through authenticated administration. Register the nine version-1
schemas returned by `browserSchemas(propertyId)` using `admin_execute` (`put`,
`resource_kind: "schemas"`, `resource_id: "<property>:<name>:1"`, `value: schema`).
`examples/static-site/schemas.json` is the same synthetic schema array. The SDK
uses `X-Property-Token`; never supply an admin, agent or server-ingestion credential.

`init` retrieves `/v1/ingest/policy?property_id=...` before installing capture.
It applies the current server profile ceiling, policy version, cross-property
permission, attribution window, handoff audiences and preference-signal policy.
Failure is closed: `ready` rejects and collection does not start. Explicit
`track` calls before readiness return false and increment `dropped.notReady`.
Automatic interactions during policy loading are not reconstructed; local
`coverageStartedAt` records the coverage start. The initial URL is held only in
memory until policy resolution, so a SPA redirect during startup preserves the
original permitted landing and current navigation. On configuration-fetch failure,
call `shutdown`, resolve the configuration/network problem and initialize again.
There is no endless configuration retry loop.

The server rechecks current policy on every ingest. A running client does not poll
for policy changes; restart it with the operator's new configuration when policy
changes. The service rejects a stale policy version rather than storing disallowed
facts. Changing visitor choice is immediate through `setCollectionChoice`.

`init` is idempotent per normalized endpoint/property, including repeated script
loads and framework remounts. A repeated call returns the existing client; use its
choice API to change visitor state. Call it from a browser lifecycle, not during
server rendering. `shutdown` removes listeners and history wrappers and permits a
fresh initialization. Different properties can coexist without duplicate capture
within one property. All dependencies are build/test dependencies only.

For a static site, copy `dist/analytics.global.js` to the site's own asset directory
and call `Analytics.init(...)` after loading it. The source example can be opened
with `python3 -m http.server 4173 --bind 127.0.0.1` from the repository root at
`/examples/static-site/`. Replace its synthetic meta configuration and register its
origin/schemas. The example begins with collection disabled and never submits form
contents or claims an account exists.

## Automatic collection and sanitization

| Event | Meaning and safe properties |
| --- | --- |
| `page_view` | Initial view, changed History URL, popstate or hash navigation; no arbitrary page title |
| `control_activate` | Links, buttons, semantic ARIA controls, tabs, menus, disclosure summaries; `control_kind`, sanitized `destination` |
| `form_submit` | Submission observation, not authoritative success; `control_kind`, `method` |
| `form_validation` | One event per form/validation task; `result` is required/format/range/length/custom/invalid/multiple |
| `disclosure_change` | Native details open/close; `control_kind`, `open` |
| `dialog_change`, `dialog_close` | Native dialog lifecycle; `control_kind`, optional `open` |
| `visibility_change`, `page_restore` | Opt-in document visibility or back-forward cache restoration; never a play/activity claim |

Listeners are delegated. Native keyboard activations emit the click once; a
custom accessible control should dispatch its ordinary activation click. Open
shadow roots are handled through composed event paths for composed events.
Non-composed events inside closed/shadow roots require explicit host integration;
there is no DOM traversal, mutation observer or replay snapshot. Unchanged
`replaceState` URLs do not create views. A hash change creates a navigation
observation, but the fragment itself is never sent. The complete event path is
limited to 64 entries; deeper controls are excluded.

Use a static, non-personal `data-analytics="start_signup"` semantic name. It is sent
as `context.control`; no DOM text, element ID, CSS path, label, field name or field
value is copied. Fallbacks use element category and sanitized destination. Exclude
any subtree with `data-analytics-ignore`; contenteditable subtrees are excluded.
No key/input listeners, arbitrary error messages or validation messages are read.

`routes` is an explicit allowlist. A template `/articles/:slug` yields
`/articles/:redacted`. An unknown path becomes `/[redacted]`. URLs retain origin
and this approved route only; credentials, every query string and all fragments
are removed. Referrers keep only origin and a redacted path. Do not list personal
static routes or use personal data in semantic attributes. HTTP/HTTPS links are
eligible; other schemes are omitted. The collector requires HTTPS, except for
loopback development.

Campaign capture separately parses the five standard UTM keys and
`experimentKey` (default `experiment`). Values are bounded to 128 characters,
normalized to NFC, and must occur once. Invalid encoding, controls, markup,
email-like values and obvious credential-like strings are discarded. Approved
campaign text remains data, never HTML or executable instructions. Normal bounded
campaign strings need no preregistration. `environment` (default `default`) and
optional `release` are bounded explicit configuration.

Custom observations require a named allowlisted schema in both SDK configuration
and service administration:

```ts
const analytics = init({
  // ... endpoint, propertyId, publicToken, policyVersion, profile ...
  endpoint: 'https://analytics.example',
  propertyId: '11111111-1111-4111-8111-111111111111',
  publicToken: 'replace-with-issued-public-property-token',
  policyVersion: 1,
  profile: 'minimal',
  choice: {enabled: true, detailedProperties: true},
  schemas: {signup_started: {variant: {type: 'string', values: ['a', 'b']}}}
});
await analytics.ready;
analytics.track('signup_started', {variant: 'a'});
```

Only declared finite numbers, safe integers, booleans and bounded strings are
copied, with at most 20 declared fields visited. Strings can have an enum and
maximum length, capped at 256. Secret/contact/field-value key names are excluded
even if declared. `detailedProperties` defaults to false and is separate from
profile. Basic broad interaction categories remain available. Do not put sensitive
values into otherwise innocuous declared keys. `track` returns whether the event
entered the queue, not whether the service received it. Built-in event names and
reserved authoritative outcomes cannot be asserted through `track`.

## Profiles, choice and sessions

| Effective profile | Browser state and analytical limits |
| --- | --- |
| Minimal | No visit/visitor/touch identifiers or browser persistence. Broad permitted events and a landing campaign are possible; no unique-person, cross-visit or longitudinal claims. |
| Journey | Random visit UUID and eligible touches in sessionStorage, scoped to endpoint origin/property. No durable visitor. Cannot establish cross-device or cross-visit people. |
| Identified | Visit plus random pseudonymous visitor UUID. Visitor persists locally for the configured window, up to 30 days; optional `persistentAttribution` retains eligible touches across visits. Account binding still requires trusted server evidence. |

Visit timeout defaults to 30 minutes, configurable from one minute to one day.
Only foreground navigation/control/form activity updates the inactivity boundary.
There is no periodic activity event; custom tracks, background polling and visibility
changes do not extend a visit. The next foreground activity after timeout starts a
new visit; journey mode discards old visit touches. Identified mode can retain
eligible permitted attribution independently. Storage writes are bounded to state
changes and at most once per five seconds of normal interactions. Blocked storage
falls back to memory and sets `storageUnavailable`; persistence cannot be claimed.
No cookies, fingerprinting or cross-device inference are used.

`setCollectionChoice({enabled: false})` stops collection, aborts requests that are
still cancellable, purges pending observations/validation and clears both storage
areas. It sends best-effort server withdrawal for the old identifiers. Re-enabling
creates fresh identifiers and a new observed view. A lower requested profile
cannot be escalated above the initialized server ceiling. Cross-property linking
requires explicit visitor opt-in and server permission. GPC and DNT are honored by
default; server-required GPC cannot be overridden by browser configuration.

`reset()` clears identity, touches, queued old-identity events and storage, then
sends best-effort suppression of the old visit/visitor. Local-storage removal also
clears identified state in other open tabs of the same origin/property. Already
committed events are not retracted by aborting a request. Server deletion is the
separate authenticated deletion operation; a local reset is not a deletion claim.
`getCoverage().controlFailures` makes failed reset/withdrawal calls visible.

## Verified identity and cross-property handoffs

Use `getJourneyContext()` to obtain only the property, permitted visit/visitor and
first-touch reference for your authenticated application integration. Browser
identifiers are untrusted observations. The trusted signup/login flow verifies the
account and captures its authoritative event; it must not trust a supplied browser
subject/account identifier. See `docs/ingestion.md` for transactional producer rules.

After the event is stored, the trusted application requests `/v1/identity/proof`
under `identity:manage`, with the exact journey IDs and evidence event ID. It returns
a short-lived opaque capability to the browser. `await analytics.identify(proof)`
redeems it at the public `/v1/identity/identify` route with the property token; it
never transmits a raw account/issuer subject. Proofs bind exact property and journey.
The browser does not resolve identities or rewrite earlier events.

Call `reset()` at logout or before switching accounts, **before** asking the trusted
application to issue a proof for fresh `getJourneyContext()` identifiers. Calling
`identify` again on an already identified client clears local identity and rejects
until a fresh journey-bound proof is obtained. This prevents later observations
being assigned to the previous account. Host authentication/logout lifecycle remains
responsible for calling the boundary API, including after a full-page auth change.

Campaign facts persist only within the selected policy/window. Untagged browsing
retains a valid first touch; later tagged views add a separate touch. A tagged
`page_view` sends campaign data and no existing `touch_id`. Its `event_id` becomes
the browser's property-scoped reference alias for subsequent events/handoffs. The
service resolves aliases to its own distinct touch IDs after acceptance and rejects
missing/untrusted references. Same-batch observations are in order. A rejected or
permanently lost landing remains missing evidence rather than fabricated acquisition;
its local reference is forgotten so later recovered observations are not poisoned
with a known missing touch. A click
never resends campaign data as a fresh touch. Server rules select first/last facts
and immutable acquisition; local state is not authoritative attribution.

```ts
const analytics = init({
  endpoint: 'https://analytics.example',
  propertyId: '11111111-1111-4111-8111-111111111111',
  publicToken: 'replace-with-issued-public-property-token',
  policyVersion: 1,
  profile: 'identified',
  choice: {enabled: true, crossProperty: true, persistentAttribution: true},
  handoffOrigins: {
    'https://destination.example': '22222222-2222-4222-8222-222222222222'
  },
  // Destination-side integration: this is your authenticated application route.
  redeemBridge: async (request, signal) => {
    const response = await fetch('/authenticated-journey-redemption', {
      method: 'POST', credentials: 'same-origin', signal,
      headers: {'Content-Type': 'application/json'}, body: JSON.stringify(request)
    });
    if (!response.ok) throw new Error('Journey redemption failed');
  }
});
await analytics.ready;
const reference = await analytics.createHandoff('https://destination.example/signup');
```

`createHandoff` flushes the source queue first, then asks the service for an opaque,
expiring capability bound to the allowlisted audience property. It returns only the
reference. The application transports that reference through its existing allowlisted
auth state; the SDK does not decorate arbitrary links or create redirects. On the
destination, `redeemHandoff(reference)` invokes `redeemBridge` with the exact
`RedeemHandoffRequest`. Your authenticated backend calls the service's protected
redemption endpoint; no server credential is present in JavaScript. Apply the
application's usual CSRF/auth-state protections to its bridge. Callback resolution
is not evidence of identity and never sets browser subject fields.

Cancellation leaves eligible source campaign state intact, so retry can create a
fresh handoff. Server expiry, audience checks, idempotent same-target redemption,
immutable signup and existing-account-login semantics are service responsibilities.
Expired local touches are excluded and reset clears the previous journey. Cross-
property browser persistence is never assumed to span unrelated domains.

## Bounded delivery and coverage

Default limits are 200 retained events, 131,072 encoded event bytes, 20 events per
batch, 32,768 batch bytes, one-second delayed delivery, three attempts per event and
a five-second request timeout. In-flight events count toward both queue limits.
An individual event is capped at 8 KiB. Configurable absolute ceilings are 1,000
events/1 MiB queue, 100 events/48 KiB batch, five attempts and 30-second timeout.
One flush handles at most ten batches and 30 seconds; leftovers use the next
scheduled bounded drain. Saturation discards the new event and counts it. No
persistent offline queue or timers survive shutdown.

The adapter uses the generated OpenAPI envelope. Every retry reuses the exact event
ID and serialized payload. Committed `accepted`/`duplicate` ACKs remove events;
`rejected`/`conflict` and permanent HTTP errors count as rejections. Network errors,
HTTP 408/425/429/5xx and missing ACK entries retry within the attempt limit. There is
no claim that `fetch` completion means durability without the service's per-event
ACK. Fetch requests omit credentials and referrers and reject redirects.

`pagehide` and hidden visibility attempt a bounded `keepalive` flush. Tab closure is
best effort: the browser may drop even a submitted request. No `sendBeacon` receipt
claim or synchronous unload work is used. `shutdown` attempts one final bounded
flush, then discards leftovers with visible `dropped.policy` accounting.

`getCoverage()` returns produced, accepted, rejected, per-reason loss, queue and
in-flight counts/encoded bytes, high-water marks, request/body-byte volume, local
runtime ID, collection start, storage limitation, preference blocking and control
failures. These are local counters, not proof of humans or receipt on closure.
Wire `collection.dropped_events` is a delta attached once to the next successfully
queued observation and remains fixed on retry. The service counts it only on new
accepted events. If that reporting observation itself is lost, the server cannot
know the full loss; local counters are the complete runtime view. Retained bytes
measure serialized event bytes, not exact JavaScript object allocator overhead.
No sampling is applied (`sample_rate: 1`).

## Contract and release checks

`src/wire.ts` is locally generated from `contracts/openapi.json`; `npm run check`
fails on drift. Run `npm run generate:wire --workspace @independent-analytics/browser`
after a coordinated contract change. The generator emits only selected DTO types,
adds no runtime dependency, retains UUID/time strings, and never coerces decimal
receipt cursors into JavaScript numbers. Timestamp/logical-time/server producer
semantics remain owned by the Rust contract. Additional identify/config module
routes follow `docs/ingestion.md`.

Package `files` explicitly includes only `dist`, `README.md` and `LICENSE` (npm also
includes package metadata). Run `npm pack --dry-run --json --ignore-scripts
--workspace @independent-analytics/browser` to inspect the publish boundary. The
package name is a proposed generic name; verify registry availability/access and
build the distribution at release time. No publication, deployment or production
collection occurred in these checks.

## Recorded validation and limitations

The 27-test Chromium suite verifies automatic capture exactly once, repeated
bundle loading, SPA history, semantic controls/open shadow roots/keyboard activation,
form contents exclusion, asynchronous disclosures/dialogs, URL/UTM sanitation,
profile ceilings, GPC/DNT, withdrawal, shared-tab reset, identity-proof boundaries,
opaque handoff bridge handling, campaign reload/untagged/later/expiry behavior,
queue saturation/retry/timeout/partial ACK, unload attempts and storage failure.
All HTTP fixtures are controlled and synthetic.

`npm run test:browser:service` verifies actual Chromium → Rust ingestion → disposable
PostgreSQL: initial tagged event and same-batch references accepted, exact batch
replay deduplicated, forged browser subject rejected, and reset starts a fresh
untagged journey. It uses the ingestion test harness with protected operations
denied; it does not claim the full authenticated signup→enrollment→MCP story. The
assembled acceptance task owns that evidence and true credential interoperability.

`packages/browser/benchmarks/latest.json` records exact workload, platform, bytes,
latencies, request volume, queue saturation and Chromium heap observations. Budgets
are engineering proposals enforced by the script, not product guarantees. The
workload includes 25 warm initializations, five navigation transitions/50 clicks,
and a 10,000-click outage burst. The recorded M2 Pro/Chromium run was below the
12 KiB ESM-gzip, 5 ms synchronous-init-p95 and 2 ms interaction-p95 budgets. Browser
timer precision limits individual sub-millisecond measurements. The artificial
10,000-dispatch loop is reported separately; ordinary capture never runs that loop.

Only Chromium is validated here. Verify Firefox/WebKit and minimum browser versions
before promising those targets. Native non-composed shadow events and application
widget implementations need explicit integration where delegated capture cannot
observe them. Server retention/deletion, cross-property signup provenance and
longitudinal analysis require the full integration evidence, not browser test counts.

The implementation revision tested on October 21, 2026 is `9c377bd` (foundation
contract `3b752ee`, ingestion checkpoint `b81e2bd`; local cherry-pick hashes differ).
Final commands and outcomes:

| Command | Outcome |
| --- | --- |
| `npm ci` | Reproducible install completed; 8 packages installed, audit reported 0 vulnerabilities |
| `npm run check` | Generated wire drift check and strict TypeScript check passed |
| `npm run test:browser` | 27 Chromium tests passed |
| `npm run test:browser:service` | Real PostgreSQL/Rust/Chromium contract regression passed; 4 stored events, replay dedupe, forgery rejection and reset verified |
| `npm run benchmark:browser` | All recorded engineering budgets passed |
| `npm pack --dry-run --json --ignore-scripts --workspace @independent-analytics/browser` | 16 files; only metadata, license, README and built distribution |
| `git diff --check` | No whitespace errors |

Final benchmark: ESM 26,085 minified bytes / 8,856 gzip bytes; global script 26,575 /
9,081 bytes. Synchronous initialization p95 0.3 ms; policy-ready p95 7.2 ms on local
HTTP; interaction p95 0.1 ms. The 56-event normal workload used five requests and
38,475 body bytes without loss. The 10,000-click outage burst retained at most 190
events / 130,861 encoded bytes, counted 9,811 saturation drops and 190 exhausted
observations, and drained to zero. Chromium retained-heap growth after GC was
184,396 bytes for that page workload. The fixture's database/authentication scope
and browser-engine limits above apply; these are not production measurements.

### Focused A11 heap evidence

The evidence-only followup runs with:

```sh
npm run benchmark:heap --workspace @independent-analytics/browser
```

`packages/browser/benchmarks/heap.json` stores every sample and all 20 runs: five
fresh Chromium contexts each for normal/enabled, normal/uninitialized-control,
outage/enabled and outage/uninitialized-control. It measures unchanged runtime
revision `9c377bd` on Apple M2 Pro (12 logical CPUs), macOS arm64, Node 24.18.0 and
Chromium 156.0.8078.4. The command completed successfully on October 21, 2026,
including the existing event-count/encoded-byte bounds and normal accepted-count
checks. No runtime change or new heap-size guarantee was introduced.

The harness calls CDP `HeapProfiler.collectGarbage` before the baseline, then
`Runtime.getHeapUsage` (`usedSize`). For the normal five-navigation/50-click
workload it samples before and after each segment flush. For the 10,000-click
outage it samples after every 100 clicks without forcing GC between chunks, then
forces GC with the queue retained. Finally it exhausts bounded retries and forces
GC again. The control loads the same bundle and page but never calls `init`.
Sampling and control runs are interleaved; all sample arrays stay in the Node
harness rather than being retained in the browser page.

| Workload | Highest sampled whole-page JS heap (bytes) | Sampled peak growth over baseline, range (bytes) | Retained growth after forced GC, range (bytes) |
| --- | ---: | ---: | ---: |
| Normal, SDK enabled | 1,260,324 | 406,008–406,008 | 94,052–94,052 |
| Normal, uninitialized control | 856,528 | 160,428–160,428 | 62,348–62,348 |
| Outage, SDK enabled | 5,734,388 | 4,827,848–4,880,072 | 242,760–242,952 |
| Outage, uninitialized control | 1,110,388 | 309,096–414,288 | 73,996–73,996 |

The outage queue high-water mark in this instrumented workload was 189 events and
130,732 encoded event bytes in every run. After draining and forced GC, enabled
outage runs retained 175,520–175,712 bytes over their original baselines. That
residual includes JIT, inspector, fetch and other browser bookkeeping; it is not a
claim of leaked queue entries. The encoded payload bound and these observed heap
numbers measure different things.

These are whole-page V8 JavaScript heap measurements, including SDK and fixture
objects, not isolated SDK allocation or process RSS. Native DOM/network memory is
outside this metric. Sampled maxima can miss within-chunk transient peaks, so the
reported peak is an observed lower bound, not a proven maximum. CDP observation
changes scheduling and itself creates bookkeeping; automatic GC/JIT and allocator
behavior create noise. Subtracting the control or dividing by retained events does
not establish an exact queue-memory or per-event guarantee. This focused heap run
is separate from the uninterrupted burst timing in `latest.json` and should not be
used as its throughput measurement.
