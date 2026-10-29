# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/dax-gate.sh, the gate a change to the guest's DAX code passes (TASK-119.8)."""

import re
import subprocess
import unittest

from script import SCRIPTS

GATE = SCRIPTS / "dax-gate.sh"
TEST_GCE = SCRIPTS / "test-gce.sh"


class DaxGate(unittest.TestCase):
    def test_suites_are_test_cases_test_gce_takes(self):
        """The suites the gate runs on its first host, as it hands them to test-gce.sh, are names
        test-gce.sh takes: a newline among them was refused, and the suites never ran."""
        suites = subprocess.run(
            ["bash", "-c", 'eval "$(sed -n "/^suites=(/,/)$/p" "$1")"; printf %s "${suites[*]}"',
             "bash", GATE], check=True, capture_output=True, text=True).stdout
        self.assertIn("fxfs-guest-checks-dax-delay", suites.split(" "))
        pattern = re.search(r"\[\[ \$\{FXDAX_E2E_TEST:-\} =~ (.+?) \]\] \|\|", TEST_GCE.read_text())
        ran = subprocess.run(["bash", "-c", f'[[ $1 =~ {pattern.group(1)} ]]', "bash", suites])
        self.assertEqual(ran.returncode, 0, f"test-gce.sh refuses {suites!r}")


if __name__ == "__main__":
    unittest.main()
