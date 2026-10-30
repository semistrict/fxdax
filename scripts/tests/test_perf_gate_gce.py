# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/perf-gate-gce.sh, the sandbox team's performance gate on a GCE host: run in a
copy of the scripts whose test-gce.sh is a fake, which says how it was run and writes the pgbench
logs the gate reads."""

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import script

# The fake test-gce.sh: records its arguments and the variables the gate sets, and writes each case
# it is to run a log of a VM a run of tps 100, every VM after the first one a run of tps 80, as
# FXDAX_PGBENCH_VMS has it; or fails if FAKE_FAIL names its results.
FAKE_TEST_GCE = r"""#!/usr/bin/env bash
set -euo pipefail
results=$3
echo "$* E2E_TEST=$FXDAX_E2E_TEST VMS=${FXDAX_PGBENCH_VMS:-} ROUNDS=$FXDAX_E2E_ROUNDS \
WORKSPACE=$FXDAX_WORKSPACE_TESTS KERNEL=$FXDAX_GUEST_KERNEL_BUILD" >> "$CALLS"
for case in $FXDAX_E2E_TEST; do
    mkdir -p "$results/e2e/$case/$case"
    for ((vm = 0; vm < ${FXDAX_PGBENCH_VMS:-1}; vm++)); do
        tps=$((vm == 0 ? 100 : 80))
        printf 'tps = %s (without initial connection time)\nOK runs 1\n' "$tps"
    done > "$results/e2e/$case/$case/log.txt"
done
[[ ${FAKE_FAIL:-} != "$(basename "$results")" ]]
"""


class PerfGateGce(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        scripts = self.root / "scripts"
        scripts.mkdir()
        for name in ["perf-gate-gce.sh", "pgbench-gate.py"]:
            shutil.copy2(script.SCRIPTS / name, scripts / name)
        fake = scripts / "test-gce.sh"
        fake.write_text(FAKE_TEST_GCE)
        fake.chmod(0o755)
        self.calls = self.root / "calls.txt"
        self.results = self.root / "results"

    def gate(self, *args, **environment):
        return subprocess.run(
            [self.root / "scripts" / "perf-gate-gce.sh", *args, "fxdax-test-r1", self.results],
            env={"PATH": "/usr/bin:/bin", "CALLS": str(self.calls), **environment},
            capture_output=True, text=True)

    def test_the_cases_then_the_density_run_then_the_gate(self):
        gated = self.gate()
        self.assertEqual(self.calls.read_text().splitlines(), [
            f"run fxdax-test-r1 {self.results}/single E2E_TEST=fxfs-guest-pgbench "
            "fxfs-guest-pgbench-dax pgbench-ext4-dax-pmem VMS= ROUNDS=1 WORKSPACE=0 KERNEL=release",
            f"run fxdax-test-r1 {self.results}/density E2E_TEST=fxfs-guest-pgbench-dax VMS=3 "
            "ROUNDS=1 WORKSPACE=0 KERNEL=release",
        ])
        # The density run's last VM at 80 tps against its first's 100.
        self.assertEqual((gated.returncode, gated.stderr),
                         (1, "failed: density of dax: 0.8 under 0.9\n"))
        report = json.loads((self.results / "gate.json").read_text())
        self.assertEqual(report["host"], "nested")
        self.assertEqual(report["tps"]["dax"], [[[100.0]], [[100.0], [80.0], [80.0]]])
        self.assertEqual(report["tps"]["ext4-dax-pmem"], [[[100.0]]])
        self.assertEqual([(each["gate"], each["case"], each["passed"])
                          for each in report["gates"]], [
            ("decay", "no-dax", True),
            ("decay", "dax", True),
            ("density", "dax", False),
            ("dax", "dax", True),
            ("static-dax", "dax", True),
        ])

    def test_a_bare_metal_host_with_the_gates_passing(self):
        gated = self.gate("--bare-metal", FXDAX_GATE_DENSITY="0.8", FXDAX_PERF_GATE_VMS="2",
                          FXDAX_GUEST_KERNEL_BUILD="debug")
        self.assertEqual((gated.returncode, gated.stderr), (0, ""))
        self.assertIn("VMS=2 ROUNDS=1 WORKSPACE=0 KERNEL=debug",
                      self.calls.read_text().splitlines()[1])
        self.assertEqual(json.loads((self.results / "gate.json").read_text())["host"], "bare-metal")

    def test_a_failed_run_fails_it_after_the_gate(self):
        gated = self.gate(FXDAX_GATE_DENSITY="0.8", FAKE_FAIL="single")
        self.assertEqual(gated.returncode, 1)
        self.assertEqual(len(self.calls.read_text().splitlines()), 2)
        self.assertTrue((self.results / "gate.json").exists())

    def test_its_usage(self):
        usage = subprocess.run([script.SCRIPTS / "perf-gate-gce.sh", "fxdax-test-r1"],
                               capture_output=True, text=True)
        self.assertEqual(usage.returncode, 2)
        self.assertRegex(usage.stderr, r"Usage: .*perf-gate-gce.sh \[--bare-metal\] INSTANCE "
                                       r"RESULTS\n$")
