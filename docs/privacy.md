# Collection and identity controls

Collection is checked again by the ingestion service against the current stored
property policy. Client settings can reduce collection; they cannot elevate it.
Unknown policies, stale policy versions and unknown event schemas are rejected.
The service never trusts a public property credential to assert an account,
signup, committed command, or scheduled game effect.

Minimal collection permits approved observations without visit, visitor or
subject identifiers. Journey collection permits visit-scoped observations;
it cannot supply cross-visit person counts. Identified collection permits
verified pseudonymous linkage, subject to the separate linkage and collection
choice settings. Cross-property continuity requires an additional explicit
allowlist and both properties' permission. Detailed properties, event retention,
and acquisition/identity retention remain independent controls.

No profile enables field values, arbitrary DOM text, replay, fingerprinting,
keystrokes, raw request bodies or sensitive property keys. Scalar property types
must match an administrator-registered event schema. URL fields must already be
sanitized and cannot contain credentials, query strings or fragments. The
browser must sanitize paths appropriate to its application (for example replacing
an email in a route with a semantic route name); the server cannot infer whether
an arbitrary path is personal data. Administrators must configure safe routes.

Withdrawal blocks future data associated with the relevant browser/visit or
verified subject. Reset ends an association; subsequent anonymous activity must
start a fresh visit/visitor identity. A browser cannot claim a new account by
resetting. Deletion removes events, original touches, acquisition, mappings and
handoffs in its authorized scope and retains a suppression tombstone so delayed
retries cannot recreate that history. Operators must apply the corresponding
lifecycle to previously downloaded exports and independently owned game data.

Policy versions and receipt times are retained with accepted events. Historical
links are only made where the policy and verified evidence permit them; no
anonymous cross-device continuity is inferred. Acquisition retention can exceed
interaction retention, but never exempts identities from deletion. D180 analysis
is unavailable where retention or linkage settings do not preserve its evidence.

Operators choose the processing purpose, applicable choices, notices and
retention. Pseudonymous, cookieless or server-side collection is not automatically
anonymous or exempt from applicable law. These presets do not promise that a
banner is unnecessary.
