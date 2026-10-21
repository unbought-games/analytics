-- Manufactured data; run only in the disposable fixture database.
CREATE ROLE fixture_reader LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS;
CREATE SCHEMA reporting;
CREATE TABLE reporting.activity (id bigint PRIMARY KEY, amount numeric(40,10), occurred_at timestamptz, payload jsonb, hidden text);
INSERT INTO reporting.activity VALUES
 (9007199254740993,123456789012345678901234567890.1234567890,'2026-01-01T00:00:00Z','{"nested":9007199254740993,"missing":null}', 'never export'),
 (9007199254740994,NULL,'2026-01-02T00:00:00Z','{"nested":0}', 'private');
GRANT USAGE ON SCHEMA reporting TO fixture_reader;
GRANT SELECT(id,amount,occurred_at,payload) ON reporting.activity TO fixture_reader;
CREATE TABLE reporting.large (id bigint PRIMARY KEY, value text);
INSERT INTO reporting.large SELECT n,repeat('synthetic',16) FROM generate_series(1,200000) n;
GRANT SELECT ON reporting.large TO fixture_reader;
CREATE TABLE reporting.private (id integer);
CREATE FUNCTION reporting.unsafe() RETURNS integer LANGUAGE sql SECURITY DEFINER AS 'SELECT 42';
REVOKE EXECUTE ON FUNCTION reporting.unsafe() FROM PUBLIC;
