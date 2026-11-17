# Compact committed activity

An authoritative producer can send `committed_activity` v1 using the existing
server ingestion API. Register an authoritative `human_action` schema with exactly
four required string properties (max_length 128): `world`, `epoch`, `actor`, `action`.
The envelope requires an identified subject, consent, sample_rate 1, dropped_events
0, game clock and nonnegative decimal `logical_time` equal to occurred_at in
milliseconds. It forbids campaign/visit/visitor context. Browser ingestion cannot
assert it. Existing authentication, property authorization, policy validation,
withdrawal/suppression, quotas, receipt deduplication and conflict detection apply.
The producer owns the definition of a committed human action and must capture it
transactionally with application success. Automatic effects must not be sent as
new human actions. No full command, game state or arbitrary properties are retained.

Ingestion atomically saves a typed `ingest_activity` fact before acknowledgement.
The table stores property/event identity, world/epoch/actor/action, logical time,
subject, policy version and retention metadata. Detailed events follow existing
event retention; compact activity follows the existing identity retention policy.
This preserves exact counts, arbitrary cutoff windows and first-observed elapsed
return cohorts beyond detailed-event expiry. It does not promise permanent or
true-first history. Both active expiry and explicit retention/deletion can shorten
coverage. Subject deletion removes compact facts even after detailed events expire;
existing subject/event suppression prevents retry resurrection.

The property-scoped analytics `activity` dataset exports event_id, world, epoch,
actor, action, at_ms, subject_namespace, subject_id and policy_version. Filters,
cutoffs, ordering, budgets and lossless integer encoding use the ordinary export
contract. The separate `ingest_activity_gaps` table records aggregate counts by
property/world/epoch/UTC day when activity is removed. It contains no event or
subject identity. A restricted SQL source should combine these with currently
expired rows awaiting cleanup. Coverage gaps must accompany reports; current
retained rows alone never establish complete history.

Delivery uses stable event IDs and immutable envelopes. Retries after a lost ack
return duplicate, conflicting payloads return conflict. Producers retain required
unacknowledged/quarantined facts and report source gaps. Backfills use those same
IDs and policy snapshots; current consent is not proof of historical permission.
A policy-version change may quarantine old source envelopes until adjudicated.
The existing deduplication/late-event horizon still applies; producers must not
silently treat an outside-horizon rejection as successful delivery.

Migration 0007 is additive. Deploy it and the ingestion implementation before
registering/activating a producer. Rollback leaves additive schema and retained
facts intact. Do not remove the migration from a database that has applied it.
No policy duration, deployment, external source, production data or retention job
is changed by this repository change.

Validation uses manufactured data: actual PostgreSQL ingestion checks authoritative
admission, duplicate/conflict behavior, consent refusal, event expiry and subject
deletion; query tests check property, cutoff and expiry isolation. No live historical
reconciliation has been performed.
