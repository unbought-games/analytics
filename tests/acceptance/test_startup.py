"""Automatic migrations against real disposable PostgreSQL, without a migrate step."""
import hashlib
from pathlib import Path
import unittest
from concurrent.futures import ThreadPoolExecutor
from service import Service
from stack import ROOT, compose


class StartupTests(unittest.TestCase):
    def sql(self, service, statement):
        return compose(service.state, "exec", "-T", "postgres", "psql", "-X", "-A", "-t",
                       "-U", "analytics_runtime", "-d", service.database, "-v", "ON_ERROR_STOP=1",
                       input=statement, text=True, capture_output=True).stdout.strip()

    def test_empty_database_concurrent_start_restart_and_env_connection(self):
        with Service(name="startup-first", database="startup_first") as first, Service(name="startup-second") as second:
            # Both processes start against the same empty database using DATABASE_URL.
            dsn = Path(first.env.pop("ANALYTICS_DATABASE_URL_FILE")).read_text()
            first.env["DATABASE_URL"] = dsn
            second.env.pop("ANALYTICS_DATABASE_URL_FILE")
            second.env["DATABASE_URL"] = dsn
            with ThreadPoolExecutor(max_workers=2) as pool:
                futures = [pool.submit(service.start) for service in (first, second)]
                for future in futures:
                    future.result()
            count = self.sql(first, "SELECT count(*) FROM _sqlx_migrations WHERE success;")
            self.assertEqual(int(count), len(list((ROOT / "migrations").glob("*.sql"))))
            first.migrate_bootstrap()
            first.client.initialize()
            owner = first.client.tool("identity")["principal_id"]
            second.stop()
            first.stop()
            first.start()
            self.assertEqual(first.client.tool("identity")["principal_id"], owner)
            self.assertEqual(self.sql(first, "SELECT count(*) FROM _sqlx_migrations WHERE success;"), count)

    def test_upgrade_applies_pending_migrations(self):
        with Service(name="startup-upgrade", database="startup_upgrade") as service:
            # Manufacture an actual prior schema using the first committed migration.
            migration = sorted((ROOT / "migrations").glob("*.sql"))[0]
            version, description = migration.stem.split("_", 1)
            checksum = hashlib.sha384(migration.read_bytes()).hexdigest()
            self.sql(service, "CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TIMESTAMPTZ NOT NULL DEFAULT now(), success BOOLEAN NOT NULL, checksum BYTEA NOT NULL, execution_time BIGINT NOT NULL);\n" + migration.read_text() + f"\nINSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES ({int(version)},'{description.replace('_', ' ')}',true,decode('{checksum}','hex'),0);")
            service.start()
            self.assertEqual(int(self.sql(service, "SELECT count(*) FROM _sqlx_migrations WHERE success;")), len(list((ROOT / "migrations").glob("*.sql"))))
            service.migrate_bootstrap()
            service.client.initialize()
            self.assertTrue(service.client.tool("status")["ready"])

    def test_failed_migration_never_serves_or_bootstraps(self):
        with Service(name="startup-failure", database="startup_failure") as service:
            self.sql(service, "CREATE TABLE principals (incompatible integer);")
            result = service.command("serve", "--bind", service.address, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"migration_failed", result.stderr)
            self.assertNotIn(Path(service.env["ANALYTICS_DATABASE_URL_FILE"]).read_bytes(), result.stderr)
            output = service.directory / "failed-bootstrap.json"
            result = service.command("bootstrap", "--owner", "Owner", "--agent", "Agent", "--output", str(output), check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())
            self.assertEqual(self.sql(service, "SELECT count(*) FROM _sqlx_migrations;"), "0")


if __name__ == "__main__":
    unittest.main()
