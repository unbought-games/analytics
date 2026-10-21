-- Receipt cursor allocation is serialized by the ingestion transaction's advisory
-- lock. A returned watermark therefore never skips a concurrently uncommitted row.
CREATE TABLE ingest_events (
 receipt_cursor bigserial PRIMARY KEY,
 property_id uuid NOT NULL,
 event_id uuid NOT NULL,
 payload jsonb NOT NULL,
 fingerprint text NOT NULL,
 occurred_at timestamptz NOT NULL,
 received_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 producer text NOT NULL,
 category text NOT NULL,
 policy_version integer NOT NULL,
 profile text NOT NULL,
 visit_id text,
 visitor_id text,
 subject_namespace text,
 subject_id text,
 expires_at timestamptz NOT NULL,
 UNIQUE(property_id,event_id)
);
CREATE INDEX ingest_events_subject ON ingest_events(property_id,subject_namespace,subject_id);
CREATE INDEX ingest_events_visitor ON ingest_events(property_id,visitor_id);
CREATE INDEX ingest_events_occurrence ON ingest_events(property_id,occurred_at,event_id);
CREATE INDEX ingest_events_expiry ON ingest_events(expires_at);

-- Dedupe receipts survive short event retention but contain no original payload.
CREATE TABLE ingest_receipts (
 property_id uuid NOT NULL,event_id uuid NOT NULL,fingerprint text NOT NULL,
 receipt_cursor bigint NOT NULL,expires_at timestamptz NOT NULL,
 subject_hash text,visit_hash text,visitor_hash text,
 PRIMARY KEY(property_id,event_id)
);
CREATE INDEX ingest_receipts_subject ON ingest_receipts(property_id,subject_hash);
CREATE INDEX ingest_receipts_visitor ON ingest_receipts(property_id,visitor_hash);
CREATE TABLE ingest_touches (
 touch_id uuid PRIMARY KEY,property_id uuid NOT NULL,event_id uuid NOT NULL,
 occurred_at timestamptz NOT NULL,received_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 visit_id text,visitor_id text,campaign jsonb NOT NULL,policy_version integer NOT NULL,
 expires_at timestamptz NOT NULL,
 UNIQUE(property_id,event_id)
);
CREATE INDEX ingest_touches_identity ON ingest_touches(property_id,visitor_id,occurred_at);
CREATE TABLE ingest_touch_access (
 touch_id uuid NOT NULL REFERENCES ingest_touches(touch_id) ON DELETE CASCADE,
 property_id uuid NOT NULL,visit_id text,visitor_id text,expires_at timestamptz NOT NULL
);
CREATE INDEX ingest_touch_access_target ON ingest_touch_access(property_id,visitor_id,visit_id);
CREATE TABLE ingest_acquisitions (
 property_id uuid NOT NULL,subject_namespace text NOT NULL,subject_id text NOT NULL,
 signup_event_id uuid NOT NULL,signed_up_at timestamptz NOT NULL,
 first_touch_id uuid,last_touch_id uuid,campaign jsonb,
 rule_version text NOT NULL,policy_version integer NOT NULL,expires_at timestamptz NOT NULL,
 PRIMARY KEY(property_id,subject_namespace,subject_id)
);
CREATE TABLE ingest_identity_links (
 id uuid PRIMARY KEY,property_id uuid NOT NULL,
 subject_namespace text NOT NULL,subject_id text NOT NULL,
 target_namespace text NOT NULL,target_id text NOT NULL,
 relationship text NOT NULL,world text NOT NULL DEFAULT '',evidence_event_id uuid NOT NULL,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),expires_at timestamptz NOT NULL,
 UNIQUE(property_id,target_namespace,target_id,relationship,world)
);
CREATE TABLE ingest_bindings (
 property_id uuid NOT NULL,visit_id text NOT NULL,visitor_id text NOT NULL,
 subject_namespace text NOT NULL,subject_id text NOT NULL,evidence_event_id uuid NOT NULL,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),expires_at timestamptz NOT NULL,
 ended_at timestamptz,
 PRIMARY KEY(property_id,visit_id,visitor_id)
);
CREATE TABLE ingest_suppressions (
 property_id uuid NOT NULL,kind text NOT NULL,identifier_hash text NOT NULL,
 reason text NOT NULL,created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 PRIMARY KEY(property_id,kind,identifier_hash)
);
CREATE TABLE ingest_handoffs (
 verifier text PRIMARY KEY,property_id uuid NOT NULL,audience uuid NOT NULL,
 visit_id text,visitor_id text,touch_id uuid REFERENCES ingest_touches(touch_id) ON DELETE CASCADE,
 expires_at timestamptz NOT NULL,redeemed_at timestamptz,
 redeemed_visit_id text,redeemed_visitor_id text
);
CREATE TABLE ingest_counters (
 property_id uuid NOT NULL,code text NOT NULL,total bigint NOT NULL DEFAULT 0,
 PRIMARY KEY(property_id,code)
);
CREATE TABLE ingest_quotas (
 property_id uuid NOT NULL,bucket text NOT NULL CHECK(bucket IN ('public','server')),
 window_start timestamptz NOT NULL,events integer NOT NULL,PRIMARY KEY(property_id,bucket)
);
CREATE TABLE ingest_audit (
 id bigserial PRIMARY KEY,principal_id uuid NOT NULL,delegated_by uuid,property_id uuid NOT NULL,
 action text NOT NULL,details jsonb NOT NULL,created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE ingest_browser_tokens (
 id uuid PRIMARY KEY,property_id uuid NOT NULL,verifier text UNIQUE NOT NULL,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),expires_at timestamptz NOT NULL,
 revoked_at timestamptz
);
CREATE TABLE ingest_proofs (
 verifier text PRIMARY KEY,property_id uuid NOT NULL,visit_id text,visitor_id text,
 subject_namespace text NOT NULL,subject_id text NOT NULL,evidence_event_id uuid NOT NULL,
 expires_at timestamptz NOT NULL,redeemed_at timestamptz
);
