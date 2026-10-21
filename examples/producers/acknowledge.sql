WITH marked AS (
 UPDATE synthetic_commands SET analytics_delivered_at=clock_timestamp()
 WHERE command_id=$1 RETURNING command_id
)
DELETE FROM analytics_outbox USING marked WHERE event_id=marked.command_id
