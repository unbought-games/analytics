"""Ask Docker itself to verify deny rules with manufactured private-file canaries."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BuildContextTests(unittest.TestCase):
    def exported_context(self, ignore, files):
        with tempfile.TemporaryDirectory(prefix="analytics-context-") as directory:
            base = Path(directory)
            context = base / "context"
            context.mkdir()
            (context / ".dockerignore").write_text(ignore.read_text())
            for name in files:
                target = context / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("manufactured-build-boundary-marker\n")
            recipe = base / "Dockerfile"
            recipe.write_text("FROM scratch\nCOPY . /context/\n")
            output = base / "output"
            subprocess.run(["docker", "buildx", "build", "--file", str(recipe), "--output", "type=local,dest=" + str(output), str(context)], check=True, capture_output=True)
            return {str(path.relative_to(output / "context")) for path in (output / "context").rglob("*") if path.is_file()}

    def test_root_context_excludes_private_and_non_build_files(self):
        allowed = {"Cargo.toml", "Cargo.lock", "crates/example/src/lib.rs", "migrations/0001_example.sql", "examples/producers/outbox.rs"}
        denied = {"UNBOUGHT_ANALYTICS_ENGINE.md", ".env", "local/password", "secrets/bootstrap.json", "artifacts/export.jsonl", "README.md", "crates/example/.env.local", "crates/example/signing.key", "crates/example/secrets/token", "crates/example/target/generated", "crates/example/__pycache__/cache.pyc"}
        self.assertEqual(self.exported_context(ROOT / ".dockerignore", allowed | denied), allowed)

    def test_fixture_context_contains_only_runtime_source(self):
        allowed = {"server.py", "Dockerfile"}
        denied = {"source.sql", "__pycache__/server.pyc", "signing.key", "local/credential", ".env"}
        self.assertEqual(self.exported_context(ROOT / "tests/acceptance/fixtures/.dockerignore", allowed | denied), allowed)


if __name__ == "__main__":
    unittest.main()
