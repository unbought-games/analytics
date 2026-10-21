-- Revision is a durable generation token, never reset by deletion/recreation.
-- Sequence values are intentionally nontransactional: rollbacks may leave gaps.
CREATE SEQUENCE admin_object_revision_seq AS bigint;
SELECT setval('admin_object_revision_seq', GREATEST(COALESCE(MAX(revision), 0) + 1, 1), false) FROM admin_objects;
ALTER TABLE admin_objects ALTER COLUMN revision SET DEFAULT 0;
CREATE FUNCTION assign_admin_object_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.revision := nextval('admin_object_revision_seq');
    RETURN NEW;
END;
$$;
CREATE TRIGGER admin_object_revision BEFORE INSERT OR UPDATE ON admin_objects
FOR EACH ROW EXECUTE FUNCTION assign_admin_object_revision();
