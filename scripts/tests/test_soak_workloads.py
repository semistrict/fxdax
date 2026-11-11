# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/soak-workloads-host.sh, which soaks the guest suites on a host of one's own."""

import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import test_guest_tags

ROOT = Path(__file__).resolve().parent.parent.parent
SOAK = ROOT / "scripts" / "soak-workloads-host.sh"


class SoakWorkloads(unittest.TestCase):
    def test_every_group_soaks_a_suite(self):
        """Each group of the guest's cases soaks the suite tagged with its name (TASK-160.29)."""
        groups = re.findall(r"^ +(\w+)\) soak \1 ", SOAK.read_text(), re.MULTILINE)
        self.assertEqual(groups, ["crash", "checks", "sanitized", "replicas", "mixed"])
        self.assertEqual({f"soak-{group}" for group in groups} - test_guest_tags.tags(), set())

    def test_a_group_unknown_runs_nothing(self):
        with tempfile.TemporaryDirectory() as results:
            run = subprocess.run(
                [SOAK, results, "crash", "lunch"], capture_output=True, text=True
            )
        self.assertEqual((run.returncode, run.stderr), (2, "no such group: lunch\n"))

    def soaked(self, *groups, **environment):
        """Runs the soak of `groups` in a copy of the scripts whose test-host.sh is a fake that says
        how it was run: what each run was given."""
        root = Path(tempfile.mkdtemp())
        (root / "scripts").mkdir()
        shutil.copy2(SOAK, root / "scripts" / SOAK.name)
        fake = root / "scripts" / "test-host.sh"
        fake.write_text('#!/bin/bash\nmkdir -p "$1"\necho "$(basename "$1") $FXDAX_E2E_TEST '
                        'rounds=$FXDAX_E2E_ROUNDS kernel=$FXDAX_GUEST_KERNEL_BUILD '
                        'follow=$FXDAX_FOLLOW_SECONDS failpoints=${FXDAX_EXTENTD_FAILPOINTS:-}" '
                        '>> "$(dirname "$1")/../ran.txt"\n')
        fake.chmod(0o755)
        (root / "scripts" / "guest_results.py").write_text("#!/bin/sh\n")
        (root / "scripts" / "guest_results.py").chmod(0o755)
        run = subprocess.run([root / "scripts" / SOAK.name, root / "results", *groups],
                             capture_output=True, text=True,
                             env={"PATH": "/usr/bin:/bin", **environment})
        self.assertEqual((run.returncode, run.stderr), (0, ""))
        return (root / "ran.txt").read_text().splitlines()

    def test_each_group_one_run_after_another(self):
        self.assertEqual(self.soaked("crash", "mixed", FXDAX_SOAK_ROUNDS="2"), [
            "crash --tag soak-crash rounds=100 kernel=release follow=20 failpoints=",
            "mixed --tag soak-mixed rounds=80 kernel=release follow=0 failpoints=1",
        ])

    def test_a_cap_bounds_each_groups_rounds(self):
        self.assertEqual(self.soaked("sanitized", "replicas", FXDAX_SOAK_CAP="1"), [
            "sanitized --tag soak-sanitized rounds=1 kernel=debug follow=20 failpoints=",
            "replicas --tag soak-replicas rounds=1 kernel=release follow=600 failpoints=",
        ])


if __name__ == "__main__":
    unittest.main()
