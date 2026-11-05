# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/soak-workloads-gce.sh, which soaks the guest suites on GCE hosts."""

import re
import subprocess
import tempfile
import unittest
from pathlib import Path

import test_guest_tags

ROOT = Path(__file__).resolve().parent.parent.parent
SOAK = ROOT / "scripts" / "soak-workloads-gce.sh"


class SoakWorkloads(unittest.TestCase):
    def test_every_group_soaks_a_suite(self):
        """Each group of the guest's cases soaks the suite tagged with its name (TASK-160.29)."""
        groups = re.findall(r"^ +(\w+)\) soak \1 ", SOAK.read_text(), re.MULTILINE)
        self.assertEqual(groups, ["crash", "checks", "sanitized", "replicas"])
        self.assertEqual({f"soak-{group}" for group in groups} - test_guest_tags.tags(), set())

    def test_a_group_unknown_makes_no_host(self):
        with tempfile.TemporaryDirectory() as results:
            run = subprocess.run(
                [SOAK, results, "crash", "lunch"], capture_output=True, text=True
            )
        self.assertEqual((run.returncode, run.stderr), (2, "no such group: lunch\n"))


if __name__ == "__main__":
    unittest.main()
