# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/soak-workloads-gce.sh, which soaks the guest suites on GCE hosts."""

import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SOAK = ROOT / "scripts" / "soak-workloads-gce.sh"
CASES = ROOT / "guest" / "tests" / "test_cases" / "src" / "lib.rs"


class SoakWorkloads(unittest.TestCase):
    def test_every_case_it_names_is_a_test_case(self):
        cases = set(re.findall(r'"(fxfs-guest[a-z-]*)"', CASES.read_text()))
        named = set(re.findall(r"\bfxfs-guest[a-z-]*", SOAK.read_text()))
        self.assertGreater(len(named), 10)
        self.assertEqual(named - cases, set())

    def test_a_group_unknown_makes_no_host(self):
        with tempfile.TemporaryDirectory() as results:
            run = subprocess.run(
                [SOAK, results, "crash", "lunch"], capture_output=True, text=True
            )
        self.assertEqual((run.returncode, run.stderr), (2, "no such group: lunch\n"))


if __name__ == "__main__":
    unittest.main()
