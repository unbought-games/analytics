WITH marked AS (
 UPDATE synthetic_commands SET analytics_quarantined=true
 WHERE command_id=$1 AND analytics_delivered_at IS NULL RETURNING command_id
)
DELETE FROM analytics_outbox USING marked WHERE event_id=marked.command_id
