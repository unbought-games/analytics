-- Dispatcher holds advisory transaction lock 935792; application writers do not.
WITH capacity AS (
 SELECT LEAST(100,GREATEST(0,10000-count(*)))::integer AS remaining FROM analytics_outbox
), pending AS (
 SELECT c.command_id,c.event_payload FROM synthetic_commands c
 WHERE c.analytics_delivered_at IS NULL AND NOT c.analytics_quarantined
 AND NOT EXISTS(SELECT 1 FROM analytics_outbox o WHERE o.event_id=c.command_id)
 ORDER BY c.journal_sequence LIMIT (SELECT remaining FROM capacity)
 FOR UPDATE OF c SKIP LOCKED
)
INSERT INTO analytics_outbox(event_id,payload)
 SELECT command_id,event_payload FROM pending ON CONFLICT DO NOTHING
