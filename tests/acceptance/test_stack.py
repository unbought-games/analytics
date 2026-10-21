"""Tests run only with stack.py run: no fallback to an operator's database."""
import json
import os
from pathlib import Path
import subprocess
import unittest
from urllib.request import urlopen
from stack import compose, load_state


class StackTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.state = load_state(os.environ["ANALYTICS_STACK_STATE"])

    def sql(self, sql, role="synthetic_reader", database="synthetic"):
        return compose(self.state, "exec", "-T", "sources", "psql", "-X", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-U", role, "-d", database, input=sql, text=True, capture_output=True).stdout.strip()

    def test_external_reader_is_restricted_and_lossless(self):
        self.assertEqual(self.sql("SELECT logical_ms::text FROM game.activity ORDER BY event_id LIMIT 1;"), "9007199254740993")
        self.assertEqual(self.sql("SELECT amount::text FROM game.activity ORDER BY event_id LIMIT 1;"), "12345678901234567890.1234567890")
        for statement in ["INSERT INTO game.activity(event_id) VALUES ('forbidden');", "SELECT * FROM private.credentials;", "SELECT pg_sleep(2);", "CREATE TABLE public.forbidden (id int);"]:
            with self.assertRaises(subprocess.CalledProcessError):
                self.sql(statement)
        self.assertEqual(self.sql("SELECT count(*) FROM game.activity;"), "2")

    def test_source_backup_restore_preserves_values(self):
        backup = compose(self.state, "exec", "-T", "sources", "pg_dump", "-U", "fixture_migration", "-d", "synthetic", "-Fc", capture_output=True).stdout
        self.sql("CREATE DATABASE restore_check;", "fixture_migration", "postgres")
        try:
            compose(self.state, "exec", "-T", "sources", "pg_restore", "-U", "fixture_migration", "-d", "restore_check", "--exit-on-error", input=backup, capture_output=True)
            self.assertEqual(self.sql("SELECT row_to_json(t)::text FROM game.activity t ORDER BY event_id;"), self.sql("SELECT row_to_json(t)::text FROM game.activity t ORDER BY event_id;", database="restore_check"))
            with self.assertRaises(subprocess.CalledProcessError):
                self.sql("SELECT * FROM private.credentials;", database="restore_check")
        finally:
            self.sql("DROP DATABASE restore_check;", "fixture_migration", "postgres")

    def test_cohort_source_retains_types_and_duplicate_facts(self):
        from cohort_source import FIXTURES, ROLES, projection
        for role in ROLES:
            rows = [json.loads(line) for line in (FIXTURES / (role + ".jsonl")).read_text().splitlines()]
            actual = self.sql("SELECT row_to_json(projected)::text FROM (" + projection(role) + ") projected;")
            actual = [json.loads(line) for line in actual.splitlines()]
            self.assertEqual(len(actual), len(rows))
            for original, restored in zip(rows, actual):
                self.assertEqual(original, {key: restored[key] for key in original})
        history = self.sql("SELECT history->>'sample_fraction' FROM cohort.synthetic_memberships ORDER BY fixture_ordinal LIMIT 1;")
        self.assertEqual(history, "1")

    def test_fixture_http_is_reachable(self):
        with urlopen(os.environ["TEST_FIXTURE_URL"] + "/healthz", timeout=5) as response:
            self.assertTrue(json.load(response)["synthetic"])


if __name__ == "__main__":
    unittest.main()
