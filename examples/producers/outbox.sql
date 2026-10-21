-- Synthetic producer database only, separate from the analytics database.
-- Store the command and its complete analytics envelope in ONE transaction.
CREATE TABLE IF NOT EXISTS synthetic_commands (
  command_id uuid PRIMARY KEY,
  player_id uuid NOT NULL,
  applied_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE IF NOT EXISTS analytics_outbox (
  event_id uuid PRIMARY KEY,
  payload jsonb NOT NULL,
  captured_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  delivered_at timestamptz,
  attempts integer NOT NULL DEFAULT 0,
  lease_until timestamptz,
  last_error_code text,
  quarantined boolean NOT NULL DEFAULT false,
  CHECK (octet_length(payload::text) <= 65536)
);
CREATE INDEX IF NOT EXISTS analytics_outbox_pending
  ON analytics_outbox(available_at) WHERE delivered_at IS NULL;
-- A dispatcher leases a bounded batch with FOR UPDATE SKIP LOCKED, commits,
-- then sends it. A crash after HTTP acceptance before local marking is safe:
-- the same event ID and immutable payload are retried after lease expiry.
-- Unaccepted authoritative facts are never evicted to free capacity. Admission
-- must reject new commands before committing if the reserved outbox is full.
