WITH pending AS MATERIALIZED (
 SELECT event_id,payload,available_at FROM analytics_outbox
 WHERE available_at<=clock_timestamp() AND (lease_until IS NULL OR lease_until<clock_timestamp())
 ORDER BY available_at,event_id LIMIT 50 FOR UPDATE SKIP LOCKED
), budgeted AS (
 SELECT event_id,sum(octet_length(payload::text)+1) OVER(ORDER BY available_at,event_id) AS bytes
 FROM pending
)
UPDATE analytics_outbox o SET lease_until=clock_timestamp()+interval '60 seconds',attempts=LEAST(attempts+1,1000000)
 FROM budgeted p WHERE o.event_id=p.event_id AND p.bytes<=240000
 RETURNING o.event_id,o.payload,o.attempts
