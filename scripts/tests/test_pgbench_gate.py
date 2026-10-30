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

THRESHOLDS = {"decay": 0.9, "dax": 0.95, "static-dax": 0.0, "density": 0.9}


def log(*vms):
    """A case's log of `vms`, each its runs' tps, as a run keeps it: its console (log.txt), where
    pgbench says each run's tps, and its guest's output (stdout.txt), where the guest says how many
    runs each VM made."""
    console = ["pgbench: at 1.15 s: initdb -D /mnt/fxfs/pgdata -A trust"]
    stdout = ["OK base"]
    for runs in vms:
        for tps in runs:
            console += ["latency average = 3.840 ms", "initial connection time = 4.1 ms",
                        f"tps = {tps} (without initial connection time)"]
        stdout += ["OK load 5000000", f"OK runs {len(runs)}", "OK checked"]
    return "\n".join(console) + "\n", "\n".join(stdout) + "\n"


class PgbenchGate(unittest.TestCase):
    def results(self, cases, rounds=False):
        """A results directory of `cases`, each case's log by its name: in e2e/CASE/CASE, or, if
        `rounds`, each of a list of logs in e2e/CASE/round-K/CASE."""
        root = Path(tempfile.mkdtemp())
        for case, logged in cases.items():
            for at, (console, stdout) in enumerate(logged if rounds else [logged], 1):
                place = root / "e2e" / case / (f"round-{at}" if rounds else case)
                if rounds:
                    place = place / case
                place.mkdir(parents=True)
                (place / "log.txt").write_text(console)
                (place / "stdout.txt").write_text(stdout)
        return root

    def test_runs_are_split_by_vm(self):
        console, stdout = log([1, 2, 3], [4, 5, 6])
        self.assertEqual(gate.runs((console + stdout).splitlines()),
                         [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])

    def test_vms_are_told_apart_by_the_guest_s_output_beside_the_console(self):
        console, stdout = log([1, 2], [3, 4])
        place = self.results({"fxfs-guest-pgbench-dax": (console, stdout)})
        log_txt = place / "e2e" / "fxfs-guest-pgbench-dax" / "fxfs-guest-pgbench-dax" / "log.txt"
        self.assertEqual(gate.runs(gate.said(log_txt)), [[1.0, 2.0], [3.0, 4.0]])
        log_txt.with_name("stdout.txt").unlink()
        self.assertEqual(gate.runs(gate.said(log_txt)), [[1.0, 2.0, 3.0, 4.0]])

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

    def test_each_runs_latencies_are_its_guests_histograms_over_it(self):
        lines = [
            "fxfs stats: seals 0 seals_hist 0,0,0,0 device_reads 3",
            "fxfs stats: seals 10 seals_hist 0,5,4,1 device_reads 9",
            "tps = 100.0 (without initial connection time)",
            "fxfs stats: seals 10 seals_hist 0,5,4,1",
            "fxfs stats: seals 12 seals_hist 2,5,4,1",
            "tps = 90.0 (without initial connection time)",
        ]
        found, per_vm = gate.records(lines)
        self.assertEqual((found, per_vm), ([
            (100.0, {"seals": {"count": 10, "p50_us": 2.048, "p90_us": 4.096, "p99_us": None}}),
            (90.0, {"seals": {"count": 2, "p50_us": 1.024, "p90_us": 1.024, "p99_us": 1.024}}),
        ], None))
        results = self.results({"fxfs-guest-pgbench-dax": ("\n".join(lines), "OK runs 2\n")})
        self.assertEqual(gate.measure_latencies(results)["dax"][0][0][1]["seals"]["count"], 2)
        self.assertEqual(gate.measure_latencies(self.results({"fxfs-guest-pgbench": log([1])})),
                         {}, "no histograms, no latencies")

    def test_one_daemon_s_last_vm_is_gated_against_its_first(self):
        measured = gate.measure(self.results({
            "fxfs-guest-pgbench-dax": log([100, 100, 95], [90, 85, 80]),
        }))
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            {"gate": "decay", "case": "dax", "value": 0.95, "threshold": 0.9, "passed": True},
            {"gate": "density", "case": "dax", "value": 0.85, "threshold": 0.9,
             "passed": False},
        ])

    def test_dax_is_reported_against_ext4_with_dax_on_pmem_which_decays_ungated(self):
        measured = gate.measure(self.results({
            "fxfs-guest-pgbench-dax": log([900, 880, 860]),
            "pgbench-ext4-dax-pmem": log([2000, 1000, 1500]),
        }))
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            {"gate": "decay", "case": "dax", "value": 0.9556, "threshold": 0.9, "passed": True},
            {"gate": "static-dax", "case": "dax", "value": 0.5867, "threshold": 0.0,
             "passed": True},
        ])
        gated = gate.gates(measured, {**THRESHOLDS, "static-dax": 0.6})
        self.assertEqual(gated[1]["passed"], False)

    def test_density_is_of_the_first_round_of_many_vms_in_any_results(self):
        single = self.results({
            "fxfs-guest-pgbench": log([1000, 1000]),
            "fxfs-guest-pgbench-dax": log([900, 900]),
        })
        density = self.results({"fxfs-guest-pgbench-dax": log([800, 800], [700, 720], [600, 600])})
        measured = gate.measure(single, density)
        self.assertEqual(measured["dax"], [[[900.0, 900.0]],
                                           [[800.0, 800.0], [700.0, 720.0], [600.0, 600.0]]])
        self.assertEqual(gate.gates(measured, THRESHOLDS), [
            {"gate": "decay", "case": "no-dax", "value": 1.0, "threshold": 0.9, "passed": True},
            {"gate": "decay", "case": "dax", "value": 1.0, "threshold": 0.9, "passed": True},
            {"gate": "density", "case": "dax", "value": 0.75, "threshold": 0.9, "passed": False},
            {"gate": "dax", "case": "dax", "value": 0.9, "threshold": 0.95, "passed": False},
        ])

    def test_the_static_dax_threshold_is_set_from_the_environment(self):
        os.environ["FXDAX_GATE_STATIC_DAX"] = "0.7"
        try:
            self.assertEqual(gate.thresholds()["static-dax"], 0.7)
        finally:
            del os.environ["FXDAX_GATE_STATIC_DAX"]

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
