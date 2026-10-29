# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/pgbench-gate.py, which gates a GCE run of pgbench on its throughput."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import script

gate = script.load("pgbench-gate")

THRESHOLDS = {"decay": 0.9, "dax": 0.95, "density": 0.9}


def log(*vms):
    """A case's log of `vms`, each its runs' tps, as the guest and pgbench print them."""
    lines = ["pgbench: at 1.15 s: initdb -D /mnt/fxfs/pgdata -A trust", "OK base"]
    for runs in vms:
        lines.append("OK load 5000000")
        for tps in runs:
            lines += ["latency average = 3.840 ms", "initial connection time = 4.1 ms",
                      f"tps = {tps} (without initial connection time)"]
        lines.append(f"OK runs {len(runs)}")
    return "\n".join(lines) + "\n"


class PgbenchGate(unittest.TestCase):
    def results(self, cases, rounds=False):
        """A results directory of `cases`, each case's log text by its name: in e2e/CASE/CASE, or,
        if `rounds`, each of a list of logs in e2e/CASE/round-K/CASE."""
        root = Path(tempfile.mkdtemp())
        for case, text in cases.items():
            for at, each in enumerate(text if rounds else [text], 1):
                place = root / "e2e" / case / (f"round-{at}" if rounds else case)
                if rounds:
                    place = place / case
                place.mkdir(parents=True)
                (place / "log.txt").write_text(each)
        return root

    def test_runs_are_split_by_vm(self):
        self.assertEqual(gate.runs(log([1, 2, 3], [4, 5, 6]).splitlines()),
                         [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])

    def test_each_rounds_runs_are_measured_in_order(self):
        results = self.results({"fxfs-guest-pgbench-dax": [log([1, 2]), log([3, 4])] * 5},
                               rounds=True)
        self.assertEqual(gate.measure(results)["dax"][::5], [[[1.0, 2.0]], [[3.0, 4.0]]])
        self.assertEqual(len(gate.measure(results)["dax"]), 10)

    def test_the_team_s_decay_and_dax_against_no_dax_fail(self):
        measured = gate.measure(self.results({
            "fxfs-guest-pgbench": log([839, 840, 845, 838, 842]),
            "fxfs-guest-pgbench-dax": log([922.3, 811.1, 795.0, 644.8, 669.8]),
        }))
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            {"gate": "decay", "case": "no-dax", "value": 1.0036, "threshold": 0.9,
             "passed": True},
            {"gate": "decay", "case": "dax", "value": 0.7262, "threshold": 0.9, "passed": False},
            {"gate": "dax", "case": "dax", "value": 0.9464, "threshold": 0.95, "passed": False},
        ])

    def test_one_daemon_s_last_vm_is_gated_against_its_first(self):
        measured = gate.measure(self.results({
            "fxfs-guest-pgbench-dax": log([100, 100, 95], [90, 85, 80]),
        }))
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            {"gate": "decay", "case": "dax", "value": 0.95, "threshold": 0.9, "passed": True},
            {"gate": "density", "case": "dax", "value": 0.85, "threshold": 0.9,
             "passed": False},
        ])

    def test_the_command_writes_json_and_fails_on_a_gate(self):
        results = self.results({"fxfs-guest-pgbench-dax": log([100, 80])})
        out = Path(tempfile.mkdtemp()) / "gate.json"
        environment = {**os.environ, "FXDAX_GATE_DECAY": "0.75"}
        passed = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-gate.py", "--json", out, results],
            env=environment, capture_output=True, text=True)
        self.assertEqual((passed.returncode, passed.stderr), (0, ""))
        self.assertEqual(json.loads(out.read_text()), {
            "host": "nested",
            "tps": {"dax": [[[100.0, 80.0]]]},
            "gates": [{"gate": "decay", "case": "dax", "value": 0.8, "threshold": 0.75,
                       "passed": True}],
        })
        failed = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-gate.py", "--host", "bare-metal", results],
            capture_output=True, text=True)
        self.assertEqual((failed.returncode, failed.stderr),
                         (1, "failed: decay of dax: 0.8 under 0.9\n"))
        self.assertEqual(json.loads(failed.stdout)["host"], "bare-metal")

    def test_a_run_without_pgbench_fails(self):
        empty = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-gate.py", tempfile.mkdtemp()],
            capture_output=True, text=True)
        self.assertEqual(empty.returncode, 1)
        self.assertRegex(empty.stderr, r"^no pgbench runs in ")
