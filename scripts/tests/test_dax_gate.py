# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/dax-gate.sh, the gate a change to the guest's DAX code passes (TASK-119.8)."""

import re
import subprocess
import unittest

from script import SCRIPTS

GATE = SCRIPTS / "dax-gate.sh"
TEST_HOST = SCRIPTS / "test-host.sh"


class DaxGate(unittest.TestCase):
    def test_suites_are_ones_test_host_takes(self):
        """What the gate hands test-host.sh to run, its suites by tag, test-host.sh takes: a newline
        among the cases it named once was refused, and the suites never ran."""
        given = re.findall(r'FXDAX_E2E_TEST=("[^"]*"|\S+)', GATE.read_text())
        self.assertEqual([selection.strip('"') for selection in given],
                         ["--tag dax-gate", "fxfs-guest-pgbench-dax-forked"])
        pattern = re.search(r"\[\[ \$\{FXDAX_E2E_TEST:-\} =~ (.+?) \]\] \|\|", TEST_HOST.read_text())
        for selection in given:
            ran = subprocess.run(["bash", "-c", f'[[ $1 =~ {pattern.group(1)} ]]', "bash",
                                  selection.strip('"')])
            self.assertEqual(ran.returncode, 0, f"test-host.sh refuses {selection!r}")


if __name__ == "__main__":
    unittest.main()
