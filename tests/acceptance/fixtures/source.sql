-- Entirely synthetic external source, deliberately distinct from engine storage.
CREATE ROLE synthetic_reader LOGIN;
CREATE SCHEMA game;
CREATE SCHEMA private;
CREATE TABLE game.activity (
  event_id text PRIMARY KEY,
  player_id text NOT NULL,
  world_id text NOT NULL,
  occurred_at timestamptz NOT NULL,
  event_class text NOT NULL,
  applied boolean NOT NULL,
  logical_ms bigint,
  amount numeric(30, 10),
  nullable_value text
);
INSERT INTO game.activity VALUES
 ('event-001','player-701','world-a','2026-04-01T00:00:00Z','human_command',true,9007199254740993,12345678901234567890.1234567890,NULL),
 ('event-002','player-701','world-a','2026-04-02T00:00:00Z','scheduled_effect',true,9007199254740994,0,'');
CREATE TABLE private.credentials (value text NOT NULL);
INSERT INTO private.credentials VALUES ('synthetic-private-marker');
REVOKE ALL ON DATABASE synthetic FROM PUBLIC;
GRANT CONNECT ON DATABASE synthetic TO synthetic_reader;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA game TO synthetic_reader;
GRANT SELECT ON game.activity TO synthetic_reader;
ALTER ROLE synthetic_reader SET default_transaction_read_only = on;
ALTER ROLE synthetic_reader SET statement_timeout = '500ms';
ALTER ROLE synthetic_reader SET lock_timeout = '100ms';
ALTER ROLE synthetic_reader SET search_path = game;
-- Password provisioned by stack.py over stdin after initialization; none is committed.
