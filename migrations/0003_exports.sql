CREATE TABLE export_jobs (
    id uuid PRIMARY KEY,
    owner_principal_id uuid NOT NULL REFERENCES principals(id),
    actor jsonb NOT NULL,
    source_id uuid,
    property_id uuid,
    request jsonb NOT NULL,
    configuration_revision bigint NOT NULL,
    status text NOT NULL CHECK (status IN ('queued','running','complete','partial','failed','cancelled','expired')),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    started_at timestamptz,
    lease_until timestamptz,
    manifest jsonb,
    error text,
    CHECK ((source_id IS NULL) <> (property_id IS NULL))
);
CREATE INDEX export_jobs_queue ON export_jobs(created_at) WHERE status='queued';
CREATE INDEX export_jobs_expiry ON export_jobs(expires_at) WHERE status<>'expired';
CREATE INDEX export_jobs_property ON export_jobs(property_id);
CREATE INDEX export_jobs_source ON export_jobs(source_id);
