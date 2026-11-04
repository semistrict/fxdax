# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/perf-gate-gce.sh, the sandbox team's performance gate, and copy-on-write DAX's,
on a GCE host: run in a copy of the scripts whose test-gce.sh is a fake, which says how it was run
and writes the logs the gate reads."""

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import script

# The fake test-gce.sh: records its arguments and the variables the gate sets, and writes each case
# it is to run a log as the case's would be, or fails if FAKE_FAIL names its results. A pgbench
# case's: a VM a run of tps 100, every VM after the first one a run of tps 80, as
# FXDAX_PGBENCH_VMS has it: the tps on the console (log.txt), the runs each VM made in the guest's
# output (stdout.txt); a density case's, the host's memory too, before the first VM and after each
# VM's runs, each VM costing it 1000 MiB without DAX, 300 with it. A bigmap case's: its results,
# cold reads at 100 MiB/s without DAX, 400 with it, its guest's cache grown by a quarter of its
# file without DAX, and the host's memory at each step, its 200 MiB of writes costing the host
# 200 MiB.
FAKE_TEST_GCE = r"""#!/usr/bin/env bash
set -euo pipefail
results=$3
echo "$* E2E_TEST=$FXDAX_E2E_TEST VMS=${FXDAX_PGBENCH_VMS:-} RUNS=${FXDAX_PGBENCH_RUNS:-} \
ROUNDS=$FXDAX_E2E_ROUNDS WORKSPACE=$FXDAX_WORKSPACE_TESTS KERNEL=$FXDAX_GUEST_KERNEL_BUILD" \
    >> "$CALLS"
mib=1048576
for case in $FXDAX_E2E_TEST; do
    place=$results/e2e/$case/$case
    log=$place/log.txt
    mkdir -p "$place"
    : > "$log"
    if [[ $case == *bigmap* ]]; then
        if [[ $case == *dax* ]]; then read=400 cached=0; else read=100 cached=1024; fi
        echo "bigmap: file_bytes $((4096 * mib)).000" >> "$log"
        echo "bigmap: read_cold_mib_s $read.000" >> "$log"
        echo "bigmap: write_pages 51200.000" >> "$log"
        used=1000 guest=0 vm=0
        for phase in base mounted read-cold read-warm written synced end; do
            case $phase in
                mounted) vm=1 ;;
                read-cold) used=$((used + 100)) guest=$cached ;;
                written) used=$((used + 200)) ;;
            esac
            echo "memory: vm=$vm phase=$phase host_used=$((used * mib))" \
                "guest_cached=$((guest * mib))" >> "$log"
        done
        printf 'OK base\nOK checked\n' > "$place/stdout.txt"
        continue
    fi
    if [[ $case == *dax* ]]; then per=300; else per=1000; fi
    if [[ $case == *density* ]]; then
        echo "memory: vm=0 phase=base host_used=$((1000 * mib))" >> "$log"
    fi
    echo 'OK base' > "$place/stdout.txt"
    for ((vm = 0; vm < ${FXDAX_PGBENCH_VMS:-1}; vm++)); do
        tps=$((vm == 0 ? 100 : 80))
        echo "tps = $tps (without initial connection time)" >> "$log"
        if [[ $case == *density* ]]; then
            echo "memory: vm=$((vm + 1)) phase=ran host_used=$(((1000 + (vm + 1) * per) * mib))" \
                "guest_total=$((1024 * mib)) guest_available=$((512 * mib))" >> "$log"
        fi
        printf 'OK load 5000000\nOK runs 1\nOK checked\n' >> "$place/stdout.txt"
    done
done
[[ ${FAKE_FAIL:-} != "$(basename "$results")" ]]
"""

# The cases of the single run, and of the density run.
SINGLE = ("fxfs-guest-pgbench fxfs-guest-pgbench-dax pgbench-ext4-dax-pmem "
          "fxfs-guest-pgbench-forked fxfs-guest-pgbench-dax-forked "
          "fxfs-guest-bigmap fxfs-guest-bigmap-dax")
DENSITY = "fxfs-guest-pgbench-density fxfs-guest-pgbench-dax-density"


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
            f"run fxdax-test-r1 {self.results}/single E2E_TEST={SINGLE} VMS= RUNS= "
            "ROUNDS=1 WORKSPACE=0 KERNEL=release",
            f"run fxdax-test-r1 {self.results}/density E2E_TEST={DENSITY} VMS=3 "
            "RUNS=2 ROUNDS=1 WORKSPACE=0 KERNEL=release",
        ])
        # Each density run's last VM at 80 tps against its first's 100.
        self.assertEqual((gated.returncode, gated.stderr), (1, "".join(
            f"failed: density of {case}: 0.8 under 0.9\n"
            for case in ["no-dax-density", "dax-density"])))
        report = json.loads((self.results / "gate.json").read_text())
        self.assertEqual(report["host"], "nested")
        self.assertEqual(report["tps"]["dax"], [[[100.0]]])
        self.assertEqual(report["tps"]["dax-density"], [[[100.0], [80.0], [80.0]]])
        self.assertEqual(report["tps"]["ext4-dax-pmem"], [[[100.0]]])
        self.assertEqual(report["memory"]["no-dax-density"]["per_added_vm_mib"], 1000.0)
        self.assertEqual(report["memory"]["dax-density"]["per_added_vm_mib"], 300.0)
        self.assertEqual(report["bigmap"]["bigmap-no-dax"]["guest_cache_share"], 0.25)
        self.assertEqual(report["bigmap"]["bigmap-dax"]["written_memory_per_byte"], 1.0)
        self.assertEqual([(each["gate"], each["case"], each["passed"])
                          for each in report["gates"]], [
            ("decay", "no-dax", True),
            ("decay", "dax", True),
            ("decay", "no-dax-forked", True),
            ("decay", "dax-forked", True),
            ("decay", "no-dax-density", True),
            ("density", "no-dax-density", False),
            ("decay", "dax-density", True),
            ("density", "dax-density", False),
            ("dax", "dax", True),
            ("forked-dax", "dax-forked", True),
            ("static-dax", "dax", True),
            ("density-memory", "dax-density", True),
            ("density-memory-mib", "dax-density", True),
            ("bigmap-read", "bigmap-dax", True),
            ("bigmap-guest-cache", "bigmap-dax", True),
            ("bigmap-written-memory", "bigmap-dax", True),
        ])

    def test_a_bare_metal_host_with_the_gates_passing(self):
        gated = self.gate("--bare-metal", FXDAX_GATE_DENSITY="0.8", FXDAX_PERF_GATE_VMS="2",
                          FXDAX_PERF_GATE_DENSITY_RUNS="3", FXDAX_GUEST_KERNEL_BUILD="debug")
        self.assertEqual((gated.returncode, gated.stderr), (0, ""))
        self.assertIn("VMS=2 RUNS=3 ROUNDS=1 WORKSPACE=0 KERNEL=debug",
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
