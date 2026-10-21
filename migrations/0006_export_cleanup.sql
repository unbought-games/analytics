-- Durable cleanup progress prevents old terminal jobs starving later artifacts.
ALTER TABLE export_jobs ADD COLUMN cleaned_at timestamptz;
CREATE INDEX export_jobs_cleanup ON export_jobs(created_at,id)
 WHERE status IN ('expired','cancelled','failed') AND cleaned_at IS NULL;
