# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of the qualification before a pin (TASK-330.4): scripts/qualify-budgets.py's summary of
the daemons' budgets, scripts/bench-ab.py's report of what moved and what failed between a pin
and a candidate, and what scripts/qualify.sh and scripts/test-e2e.sh's FXDAX_E2E_HARNESS refuse
before they run anything."""

import contextlib
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

import script
from script import SCRIPTS

qualify_budgets = script.load("qualify-budgets")
bench_ab = script.load("bench-ab")

MIB = 1 << 20


def daemon(name, *, checks=(), figures=None):
    return {"daemon": name, "checks": [{"budget": budget, "passed": passed, "says": says}
                                       for budget, passed, says in checks],
            "figures": figures or {}}


class Budgets(unittest.TestCase):
    def setUp(self):
        self.results = Path(tempfile.mkdtemp())

    def part(self, name, *daemons):
        (self.results / name).mkdir()
        (self.results / name / "budgets.json").write_text(json.dumps(list(daemons)))

    def summary(self, *parts):
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            status = qualify_budgets.main([str(self.results), *parts])
        report = json.loads((self.results / "bench-budgets.json").read_text())
        return status, said.getvalue(), report

    def test_each_parts_daemons_figures_are_measurements(self):
        written = {"raw": 100 * MIB, "copied": 0, "framed": 0, "own": 0, "log": 0}
        self.part("sandboxes",
                  daemon("host-a-stats.txt", checks=[("memory", True, "ok")],
                         figures={"max_anon": 300 * MIB, "written": written,
                                  "io_write_bytes": 150 * MIB}),
                  daemon("host-b-stats.txt", checks=[("memory", True, "ok")],
                         figures={"max_anon": 500 * MIB, "written": written,
                                  "io_write_bytes": 250 * MIB}))
        status, said, report = self.summary("sandboxes")
        self.assertEqual((status, said), (0, ""))
        self.assertEqual(report["results"], [
            {"workload": "budgets.sandboxes.max_anon", "unit": "MiB", "higher_is_better": False,
             "values": [500.0]},
            {"workload": "budgets.sandboxes.io_per_guest_byte", "unit": "x",
             "higher_is_better": False, "values": [2.0]},
        ])
        self.assertEqual(report["failures"], {})

    def test_a_budget_passed_and_a_daemon_that_says_none_fail(self):
        """A pin's daemon from before TASK-330.2 says nothing its budgets weigh: no check passes it,
        and the summary says so rather than nothing."""
        self.part("sandboxes", daemon("fxfs-guest-sandboxes/host-a-stats.txt"),
                  daemon("extentd-stats.txt", checks=[("memory", False, "anonymous memory 4 GiB")]))
        status, said, report = self.summary("sandboxes", "soak")
        self.assertEqual(status, 1)
        self.assertEqual(report["failures"], {"budgets.unreported": 1, "budgets.memory": 1,
                                              "budgets.missing": 1})
        self.assertEqual(said, "budgets: sandboxes/fxfs-guest-sandboxes/host-a-stats.txt: says "
                               "nothing its budgets weigh\n"
                               "budgets: sandboxes/extentd-stats.txt: memory: anonymous memory "
                               "4 GiB\n"
                               "budgets: soak: no budgets.json: its run ended before its budgets\n")


def side(results, *, failures=None, failed=(), **workloads):
    """A round's directory: one case's results, each workload (higher_is_better, values), and its
    failures."""
    directory = Path(tempfile.mkdtemp(prefix="r"))
    entries = [{"workload": name, "unit": "s", "higher_is_better": higher, "values": values}
               for name, (higher, values) in workloads.items()]
    report = {"results": entries, "failures": failures or {}, "failed": list(failed)}
    (directory / f"bench-{results}.json").write_text(json.dumps(report))
    return directory


class Report(unittest.TestCase):
    """What an A/B of two pins' qualifications says: each metric, those that moved, and each side's
    failures."""

    def test_a_metric_one_side_alone_measured_is_listed(self):
        a = [side("q", vms=(False, [100]))]
        b = [side("q", vms=(False, [90]), per_guest_byte=(False, [1.1]))]
        self.assertEqual(bench_ab.table("pin", a, "main", b).splitlines()[2:], [
            "| vms | s | 100 (100–100) | 90 (90–90) | 0.90× | no |",
            "| per_guest_byte | s | - | 1.1 (1.1–1.1) | - | - |",
        ])

    def test_those_whose_ranges_are_apart_moved(self):
        a = [side("q", vms=(False, [100, 110]), slowest=(False, [3000, 9000]))]
        b = [side("q", vms=(False, [105, 120]), slowest=(False, [300, 400]))]
        self.assertEqual(bench_ab.moved(a, b), ["slowest"])

    def test_each_sides_failures_by_kind_and_round(self):
        a = [side("q", failures={"framings": 1, "writes": 1},
                  failed=["framings: host-a framed 4 segments slower than 1000 ms",
                          "writes: the stores wrote 1.85 times"]),
             side("q", failures={"writes": 1}, failed=["writes: the stores wrote 1.91 times"])]
        b = [side("q"), side("q")]
        lines = bench_ab.failure_table("pin", a, "main", b).splitlines()
        self.assertEqual(lines[:4], ["| Failed | pin | main |", "|---|---|---|",
                                     "| framings | 1 of 2 | 0 of 2 |",
                                     "| writes | 2 of 2 | 0 of 2 |"])
        self.assertEqual(lines[5], f"- {a[0].name}: framings: host-a framed 4 segments slower "
                                   "than 1000 ms")
        self.assertEqual(bench_ab.failure_table("pin", b, "main", b), "")


def bash(command, env=None):
    done = subprocess.run(["/bin/bash", "-c", command], capture_output=True, text=True,
                          env={"PATH": "/usr/bin:/bin", **(env or {})})
    return done.returncode, done.stdout, done.stderr


class Refusals(unittest.TestCase):
    """What the qualification's scripts refuse before they run anything, on any host."""

    def test_a_harness_goes_with_prebuilt_artifacts(self):
        results = tempfile.mkdtemp()
        status, _, err = bash(f"{SCRIPTS / 'test-e2e.sh'} fxfs-guest",
                              {"FXDAX_E2E_RESULTS": results, "FXDAX_E2E_HARNESS": "/h"})
        self.assertEqual((status, err), (2, "FXDAX_E2E_HARNESS goes with FXDAX_E2E_PREBUILT, "
                                            "whose artifacts it drives\n"))

    def test_a_harness_of_no_artifacts_is_refused(self):
        prebuilt, harness = tempfile.mkdtemp(), tempfile.mkdtemp()
        (Path(prebuilt) / "artifacts.txt").write_text("seccomp real\n")
        status, _, err = bash(f"{SCRIPTS / 'test-e2e.sh'} fxfs-guest",
                              {"FXDAX_E2E_RESULTS": tempfile.mkdtemp(),
                               "FXDAX_E2E_PREBUILT": prebuilt, "FXDAX_E2E_HARNESS": harness})
        self.assertEqual((status, err), (2, f"{harness} holds no artifacts (FXDAX_E2E_BUILD_ONLY "
                                            "makes them)\n"))

    def test_qualify_takes_a_sandboxes_mode_and_a_soaks_seconds(self):
        results = tempfile.mkdtemp()
        status, _, err = bash(f"{SCRIPTS / 'qualify.sh'} {results}",
                              {"FXDAX_QUALIFY_SANDBOXES": "nightly"})
        self.assertEqual((status, err), (2, "FXDAX_QUALIFY_SANDBOXES is short or long\n"))
        status, _, err = bash(f"{SCRIPTS / 'qualify.sh'} {results}",
                              {"FXDAX_QUALIFY_SOAK_SECS": "30m"})
        self.assertEqual((status, err), (2, "FXDAX_QUALIFY_SOAK_SECS is a count of seconds\n"))

    def test_qualify_refuses_an_earlier_runs_results(self):
        results = tempfile.mkdtemp()
        (Path(results) / "bench-budgets.json").write_text("{}")
        status, _, err = bash(f"{SCRIPTS / 'qualify.sh'} {results}")
        self.assertEqual((status, err), (2, f"{results} holds an earlier run's results: name a "
                                            "new directory.\n"))

    def test_the_ab_takes_the_fs_bench_or_the_qualification(self):
        status, _, err = bash(f"{SCRIPTS / 'bench-ab-host.sh'}", {"FXDAX_AB_SUITE": "soak"})
        self.assertEqual((status, err), (2, "FXDAX_AB_SUITE is fs or qualify\n"))


if __name__ == "__main__":
    unittest.main()
