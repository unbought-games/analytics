#!/usr/bin/env python3
"""Run native unittest discovery, failing when no executable coverage remains."""
import argparse
import sys
import unittest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory")
    parser.add_argument("--pattern", default="test_*.py")
    args = parser.parse_args()
    suite = unittest.TestLoader().discover(args.directory, pattern=args.pattern)
    if suite.countTestCases() == 0:
        print("No tests discovered; refusing an empty successful suite.", file=sys.stderr)
        return 1
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if result.testsRun <= len(result.skipped):
        print("All tests skipped; no executable coverage ran.", file=sys.stderr)
        return 1
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
