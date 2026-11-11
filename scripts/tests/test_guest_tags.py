# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of the suites the scripts select the guest tests' cases by (`runner list --tag`,
guest/tests/test_cases/src/lib.rs's tags): each a script names is a suite, and each suite is one a
script runs, but the manual one, which none does (TASK-160.29)."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
CASES = ROOT / "guest" / "tests" / "test_cases" / "src" / "lib.rs"
# What runs the guest tests: the scripts, and the workflows that run them.
RUNNERS = [*(ROOT / "scripts").glob("*.sh"), *(ROOT / "scripts").glob("*.py"),
           *(ROOT / "scripts" / "lib").glob("*.sh"), *(ROOT / ".github" / "workflows").glob("*.yml")]


def tags():
    """The suites lib.rs's `tags!` declares, by name."""
    declared = re.search(r"^tags! \{\n(.*?)^\}", CASES.read_text(), re.MULTILINE | re.DOTALL)
    return set(re.findall(r'^    \w+ = "([a-z0-9-]+)",$', declared.group(1), re.MULTILINE))


def named():
    """The suites the scripts and workflows name, `--tag NAME`, but for one a script makes of its
    own names, as soak-workloads-host.sh's `soak-$group`, which test_soak_workloads.py checks."""
    return {name for path in RUNNERS
            for name in re.findall(r"--tag ([a-z0-9-]+)(?![A-Za-z0-9$-])", path.read_text())}


class GuestTags(unittest.TestCase):
    def test_lib_rs_declares_the_suites(self):
        self.assertGreater(len(tags()), 10)
        self.assertIn("e2e", tags())

    def test_every_suite_named_is_one(self):
        self.assertEqual(named() - tags(), set())

    def test_every_suite_runs_but_the_manual_one(self):
        groups = ["crash", "checks", "sanitized", "replicas", "mixed"]
        soaked = {f"soak-{group}" for group in groups}
        self.assertEqual(tags() - named() - soaked, {"manual"})


if __name__ == "__main__":
    unittest.main()
