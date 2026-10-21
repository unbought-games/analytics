# Browser observations

Framework-neutral, zero runtime dependency SDK for the independently hosted analytics engine. ESM, CommonJS and a standalone `Analytics` browser global are built from the same TypeScript source. The package uses an explicit `files` allowlist; no local server configuration is packaged.

```ts
import {init} from '@unbought/analytics';
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

Configure the property's allowed origin, collection policy and the schemas returned by `browserSchemas(propertyId)` before loading the SDK. Public tokens are not admin or server credentials. The SDK retrieves the effective policy before collection and rejects unavailable/invalid policy rather than guessing.

`init` is idempotent per endpoint/property across repeated script loads. `track`, `identify`, `reset`, `setCollectionChoice`, `flush`, `shutdown`, `getJourneyContext`, `createHandoff`, `redeemHandoff` and `getCoverage` are documented in `docs/browser.md` in the source repository. `ready` exposes initialization failures.

URLs contain only approved static routes; dynamic template segments become `:redacted`. Form values, arbitrary text, query strings and fragments are never collected. Use static `data-analytics="control_name"` on controls and `data-analytics-ignore` on excluded subtrees. Set `choice.detailedProperties` explicitly to permit custom schema fields.

Identity and cross-property journeys require configured, authenticated application/server integrations. Never pass an arbitrary account ID to `identify`; it accepts an expiring, journey-bound proof issued by the trusted server flow. Call `reset` before account changes and before obtaining a fresh proof. Opt-out clears queued data and local identities. Server-side deletion remains an authenticated operation.

Delivery is bounded best effort, including on page closure. Inspect `getCoverage()` for queue saturation, permanent rejections, exhausted retries and control-request failures. No replay, fingerprinting, DOM snapshots, input listeners, offline disk queues or runtime dependencies are included.

Requires modern browsers with Fetch, Web Crypto, AbortController, History and ES2022. The verified browser is Chromium; other engines need release-time verification. HTTP is supported only on loopback for local testing. No package has been published by this repository.
