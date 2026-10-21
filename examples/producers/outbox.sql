-- Synthetic producer-owned authoritative command journal, separate from analytics.
-- Application commits these business facts without consulting delivery capacity.
CREATE TABLE IF NOT EXISTS synthetic_commands (
  journal_sequence bigserial UNIQUE NOT NULL,
  command_id uuid PRIMARY KEY,
  player_id uuid NOT NULL,
  applied_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  event_payload jsonb NOT NULL,
  analytics_delivered_at timestamptz,
  analytics_quarantined boolean NOT NULL DEFAULT false,
  CHECK (octet_length(event_payload::text) <= 65536)
);
CREATE INDEX IF NOT EXISTS synthetic_commands_projection
  ON synthetic_commands(journal_sequence)
  WHERE analytics_delivered_at IS NULL AND NOT analytics_quarantined;
CREATE TABLE IF NOT EXISTS analytics_outbox (
  event_id uuid PRIMARY KEY REFERENCES synthetic_commands(command_id) ON DELETE CASCADE,
  payload jsonb NOT NULL,
  captured_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  attempts integer NOT NULL DEFAULT 0,
  lease_until timestamptz,
  last_error_code text,
  CHECK (octet_length(payload::text) <= 65536)
);
CREATE INDEX IF NOT EXISTS analytics_outbox_pending ON analytics_outbox(available_at,event_id);
CREATE TABLE IF NOT EXISTS producer_coverage_gaps (
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  reason text NOT NULL,
  event_count bigint NOT NULL,
  history_start timestamptz NOT NULL,
  history_end timestamptz NOT NULL,
  first_sequence bigint NOT NULL,
  last_sequence bigint NOT NULL
);
CREATE OR REPLACE VIEW producer_delivery_coverage AS
SELECT
 (SELECT count(*) FROM synthetic_commands WHERE analytics_delivered_at IS NULL AND NOT analytics_quarantined) AS pending_journal,
 (SELECT count(*) FROM analytics_outbox) AS projected_queue,
 (SELECT count(*) FROM synthetic_commands WHERE analytics_quarantined) AS quarantined,
 (SELECT coalesce(sum(event_count),0) FROM producer_coverage_gaps) AS unconfirmed_pruned_events,
 (SELECT min(applied_at) FROM synthetic_commands) AS retained_history_start,
 (SELECT max(recorded_at) FROM producer_coverage_gaps) AS last_coverage_gap;
-- Projection/delivery is reconstructible from the retained journal. There is no
-- advancing cursor: transactions committing out of sequence cannot be skipped.
-- Journal retention is the application's independent policy. Before purging an
-- unacknowledged record, retention.sql atomically records an explicit coverage gap.
