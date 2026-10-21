-- Explicit producer-owned history retention; never report missing history as zero.
WITH doomed AS MATERIALIZED (
 SELECT command_id,analytics_delivered_at,applied_at,journal_sequence
 FROM synthetic_commands WHERE applied_at<$1
 ORDER BY journal_sequence LIMIT 1000 FOR UPDATE SKIP LOCKED
), gap AS (
 INSERT INTO producer_coverage_gaps(reason,event_count,history_start,history_end,first_sequence,last_sequence)
 SELECT 'source_retention_without_ack',count(*),min(applied_at),max(applied_at),min(journal_sequence),max(journal_sequence)
 FROM doomed WHERE analytics_delivered_at IS NULL HAVING count(*)>0
 RETURNING event_count
), removed AS (
 DELETE FROM synthetic_commands USING doomed
 WHERE synthetic_commands.command_id=doomed.command_id RETURNING synthetic_commands.command_id
)
SELECT (SELECT count(*) FROM removed) AS removed,
       coalesce((SELECT event_count FROM gap),0) AS unavailable_events
