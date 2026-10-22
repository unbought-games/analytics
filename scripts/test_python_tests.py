"""Prove missing, renamed, empty and skipped-only suites cannot pass CI."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUNNER = Path(__file__).with_name("run_python_tests.py")


class DiscoveryTests(unittest.TestCase):
    def run_fixture(self, files):
        with tempfile.TemporaryDirectory() as directory:
            for name, text in files.items():
                Path(directory, name).write_text(text)
            return subprocess.run(
                [sys.executable, str(RUNNER), directory],
                capture_output=True, text=True,
            )

    def test_missing_or_renamed_tests_fail(self):
        for files in ({}, {"renamed.py": "import unittest\n"}):
            result = self.run_fixture(files)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("No tests discovered", result.stderr)

    def test_discoverable_file_without_cases_fails(self):
        result = self.run_fixture({"test_empty.py": "import unittest\n"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("No tests discovered", result.stderr)

    def test_all_skipped_fails(self):
        result = self.run_fixture({"test_example.py": """import unittest
@unittest.skip('synthetic skip')
class Example(unittest.TestCase):
    def test_example(self):
        pass
"""})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("All tests skipped", result.stderr)

    def test_real_success_and_failure_propagate(self):
        for value, expected in ((True, 0), (False, 1)):
            result = self.run_fixture({"test_example.py": f"""import unittest
class Example(unittest.TestCase):
    def test_example(self):
        self.assertTrue({value})
"""})
            self.assertEqual(result.returncode, expected)
            self.assertIn("Ran 1 test", result.stderr)

    def test_import_failure_propagates(self):
        result = self.run_fixture({"test_example.py": "import nonexistent_synthetic_dependency\n"})
        self.assertNotEqual(result.returncode, 0)

    def test_class_setup_skip_without_running_tests_fails(self):
        result = self.run_fixture({"test_example.py": """import unittest
class Example(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise unittest.SkipTest('synthetic setup unavailable')
    def test_example(self):
        pass
"""})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("All tests skipped", result.stderr)

    def test_class_skip_does_not_hide_other_successful_coverage(self):
        result = self.run_fixture({"test_example.py": """import unittest
class Unavailable(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise unittest.SkipTest('synthetic setup unavailable')
    def test_example(self):
        pass
class Available(unittest.TestCase):
    def test_example(self):
        self.assertEqual(2 + 2, 4)
"""})
        self.assertEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
