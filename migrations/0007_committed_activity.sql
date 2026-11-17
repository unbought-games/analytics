SET LOCAL lock_timeout='2s';
-- Compact authoritative activity survives short detailed-event retention.
-- No arbitrary properties, state, coordinates, request bodies or outcomes.
CREATE TABLE ingest_activity (
 property_id uuid NOT NULL, event_id uuid NOT NULL,
 world text NOT NULL, epoch text NOT NULL, actor text NOT NULL, action text NOT NULL,
 at_ms bigint NOT NULL, occurred_at timestamptz NOT NULL,
 subject_namespace text NOT NULL, subject_id text NOT NULL,
 policy_version integer NOT NULL,
 received_at timestamptz NOT NULL DEFAULT clock_timestamp(), expires_at timestamptz NOT NULL,
 PRIMARY KEY(property_id,event_id)
);
CREATE INDEX ingest_activity_scope ON ingest_activity(property_id,world,epoch,actor,at_ms);
CREATE INDEX ingest_activity_subject ON ingest_activity(property_id,subject_namespace,subject_id);
CREATE INDEX ingest_activity_occurrence ON ingest_activity(property_id,occurred_at,event_id);
CREATE INDEX ingest_activity_expiry ON ingest_activity(property_id,expires_at);
-- A shortened policy also selects received_at before original expires_at.
CREATE INDEX ingest_activity_received ON ingest_activity(property_id,received_at);
CREATE FUNCTION retain_committed_activity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.payload->>'name'='committed_activity' THEN
  INSERT INTO ingest_activity(property_id,event_id,world,epoch,actor,action,at_ms,occurred_at,subject_namespace,subject_id,policy_version,expires_at)
  VALUES(NEW.property_id,NEW.event_id,NEW.payload#>>'{properties,world}',NEW.payload#>>'{properties,epoch}',
   NEW.payload#>>'{properties,actor}',NEW.payload#>>'{properties,action}',(NEW.payload->>'logical_time')::bigint,
   NEW.occurred_at,NEW.subject_namespace,NEW.subject_id,NEW.policy_version,
   NEW.received_at+make_interval(days=>(SELECT (value#>>'{policy,identity_retention_days}')::integer FROM admin_objects WHERE kind='properties' AND id=NEW.property_id::text)));
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER ingest_activity_capture AFTER INSERT ON ingest_events FOR EACH ROW EXECUTE FUNCTION retain_committed_activity();

-- Aggregate coverage survives removal without retaining subject/event identity.
CREATE TABLE ingest_activity_gaps (
 property_id uuid NOT NULL, world text NOT NULL, epoch text NOT NULL, day bigint NOT NULL, reason text NOT NULL, events bigint NOT NULL,
 PRIMARY KEY(property_id,world,epoch,day,reason)
);
CREATE FUNCTION record_activity_removal() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 INSERT INTO ingest_activity_gaps VALUES(OLD.property_id,OLD.world,OLD.epoch,OLD.at_ms/86400000,'retention_or_deletion',1)
 ON CONFLICT(property_id,world,epoch,day,reason) DO UPDATE SET events=ingest_activity_gaps.events+1;
 RETURN OLD;
END $$;
CREATE TRIGGER ingest_activity_removed AFTER DELETE ON ingest_activity FOR EACH ROW EXECUTE FUNCTION record_activity_removal();
