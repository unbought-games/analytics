"""Exercise risk selection and actual Git diff behavior, including moves/deletes."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from ci_scope import changed_paths, scope_for_event, select_suite


class SelectionTests(unittest.TestCase):
    def test_documentation_avoids_native_stack(self):
        self.assertEqual(select_suite(["README.md", "docs/deployment.md"]), "fixtures")

    def test_analysis_gets_cohort_tests(self):
        self.assertEqual(select_suite(["docs/measurement.md", "examples/analysis/metrics.py"]), "analysis")

    def test_each_component_keeps_service_integration(self):
        for path in (
            "packages/browser/src/index.ts", "crates/analytics-ingest/src/privacy.rs",
            "crates/analytics-query/src/lib.rs", "examples/producers/outbox.ts",
            "examples/static-site/site.js",
        ):
            with self.subTest(path=path):
                self.assertEqual(select_suite([path]), "integration")

    def test_high_risk_and_unknown_paths_require_acceptance(self):
        for path in (
            "crates/analytics-core/src/auth.rs", "crates/analytics-server/src/main.rs",
            "migrations/0008.sql", "contracts/openapi.json", "Cargo.lock",
            "package-lock.json", "Dockerfile", ".dockerignore", "compose.deploy.yaml",
            ".github/workflows/ci.yml", "tests/acceptance/run.py", "benchmarks/service.py",
            "scripts/ci_scope.py", "docs/example.py", "new-component/main.rs",
            "packages/browser/package.json", "packages/browser/.npmignore",
            "examples/producers/package-lock.json", "crates/analytics-ingest/Cargo.toml",
        ):
            with self.subTest(path=path):
                self.assertEqual(select_suite(["README.md", path]), "acceptance")

    def test_mixed_changes_use_strongest_tier_regardless_of_order(self):
        paths = ["README.md", "examples/analysis/metrics.py", "packages/browser/src/index.ts"]
        self.assertEqual(select_suite(paths), "integration")
        self.assertEqual(select_suite(reversed(paths)), "integration")
        self.assertEqual(select_suite(paths + ["Cargo.toml"]), "acceptance")

    def test_empty_comparison_fails_closed(self):
        self.assertEqual(select_suite([]), "acceptance")

    def test_main_tags_manual_and_unknown_events_are_full(self):
        for event in ("push", "workflow_dispatch", "merge_group", ""):
            with self.subTest(event=event):
                self.assertEqual(scope_for_event(event, "unused"), "acceptance")
        self.assertEqual(scope_for_event("pull_request", ""), "acceptance")


class GitComparisonTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "--quiet")
        self.git("config", "user.email", "fixture@example.test")
        self.git("config", "user.name", "Synthetic fixture")
        self.write("crates/analytics-server/src/main.rs", "fn main() {}\n")
        self.write("README.md", "Example\n")
        self.commit()
        self.base = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, text=True, stderr=subprocess.PIPE)

    def write(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)

    def commit(self):
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "test: synthetic change")

    def test_prose_edit_selects_fixtures(self):
        self.write("README.md", "Updated example\n")
        self.commit()
        self.assertEqual(scope_for_event("pull_request", self.base, self.root), "fixtures")

    def test_deleted_server_file_requires_acceptance(self):
        (self.root / "crates/analytics-server/src/main.rs").unlink()
        self.commit()
        self.assertEqual(scope_for_event("pull_request", self.base, self.root), "acceptance")

    def test_rename_into_docs_keeps_original_risk(self):
        (self.root / "docs").mkdir()
        self.git("mv", "crates/analytics-server/src/main.rs", "docs/old.md")
        self.commit()
        self.assertIn("crates/analytics-server/src/main.rs", changed_paths(self.base, self.root))
        self.assertEqual(scope_for_event("pull_request", self.base, self.root), "acceptance")

    def test_unavailable_base_requires_acceptance(self):
        self.assertEqual(scope_for_event("pull_request", "missing-ref", self.root), "acceptance")

    def test_newlines_in_names_are_not_split_into_false_docs_paths(self):
        self.write("new\nREADME.md", "executable content")
        self.commit()
        self.assertEqual(changed_paths(self.base, self.root), ["new\nREADME.md"])
        self.assertEqual(scope_for_event("pull_request", self.base, self.root), "acceptance")


if __name__ == "__main__":
    unittest.main()
