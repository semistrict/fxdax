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

    def test_each_parts_daemons_cpu_for_each_guest_mib_is_measured_against_its_budget(self):
        """TASK-382: the CPU a part's daemons spent for each MiB their guests wrote, all its
        daemons' together, is a measurement, and past the part's budget a failure: main's soak
        daemon spent 1.55 times the pin's, and nothing said so."""
        written = {"raw": 1000 * MIB, "copied": 0, "framed": 0, "own": 0, "log": 0}
        ms = 1_000_000
        within = qualify_budgets.CPU_BUDGETS["sandboxes"] * 1000 * ms
        self.part("sandboxes",
                  daemon("host-a-stats.txt", checks=[("memory", True, "ok")],
                         figures={"max_anon": MIB, "cpu_ns": within // 2, "written": written,
                                  "io_write_bytes": 1000 * MIB}),
                  daemon("host-b-stats.txt", checks=[("memory", True, "ok")],
                         figures={"max_anon": MIB, "cpu_ns": within, "written": written,
                                  "io_write_bytes": 1000 * MIB}))
        past = (qualify_budgets.CPU_BUDGETS["soak"] + 1) * 1000 * ms
        self.part("soak",
                  daemon("extentd-stats.txt", checks=[("memory", True, "ok")],
                         figures={"max_anon": MIB, "cpu_ns": past, "written": written,
                                  "io_write_bytes": 1000 * MIB}))
        status, said, report = self.summary("sandboxes", "soak")
        cpu = {entry["workload"]: entry["values"] for entry in report["results"]
               if entry["workload"].endswith(".cpu_per_guest_mib")}
        sandboxes = qualify_budgets.CPU_BUDGETS["sandboxes"] * 0.75
        soak = qualify_budgets.CPU_BUDGETS["soak"] + 1
        self.assertEqual(cpu, {"budgets.sandboxes.cpu_per_guest_mib": [sandboxes],
                               "budgets.soak.cpu_per_guest_mib": [soak]})
        self.assertEqual((status, report["failures"]), (1, {"budgets.cpu": 1}))
        self.assertEqual(said, f"budgets: soak: cpu: its daemons spent {soak:.1f} ms of CPU for "
                               f"each MiB their guests wrote, budget "
                               f"{qualify_budgets.CPU_BUDGETS['soak']} ms\n")

    def test_a_part_with_no_cpu_budget_is_refused(self):
        with self.assertRaises(SystemExit) as refused:
            qualify_budgets.main([str(self.results), "soak", "nightly"])
        self.assertEqual(str(refused.exception),
                         "qualify-budgets.py: no CPU budget for nightly: CPU_BUDGETS")

    def test_a_budget_passed_and_a_daemon_that_says_none_fail(self):
        """A pin's daemon from before TASK-330.2 says nothing its budgets weigh: no check passes it,
        and the summary says so rather than nothing."""
        self.part("sandboxes", daemon("fxfs-guest-sandboxes/host-a-stats.txt"),
                  daemon("extentd-stats.txt", checks=[("memory", False, "anonymous memory 4 GiB")]))
        status, said, report = self.summary("sandboxes", "soak")
        self.assertEqual(status, 1)
        self.assertEqual(report["failures"], {"budgets.unreported": 1, "budgets.memory": 1,
                                              "budgets.not-run": 1})
        self.assertEqual(said, "budgets: sandboxes/fxfs-guest-sandboxes/host-a-stats.txt: says "
                               "nothing its budgets weigh\n"
                               "budgets: sandboxes/extentd-stats.txt: memory: anonymous memory "
                               "4 GiB\n"
                               "budgets: soak: not run: no budgets.json, its run ended before "
                               "its budgets\n")


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


class NotRun(unittest.TestCase):
    """TASK-330.4: a part of the qualification that left no result didn't run, a failed check, not
    a metric left out: its A/B ran hours with the sandboxes' part started on neither side, and its
    tables just lacked their rows."""

    def test_a_round_lacking_another_rounds_result_did_not_run_it(self):
        rounds = [side("fxfs-guest-sandboxes", vms=(False, [100])) for _ in range(3)]
        for directory in rounds:
            (directory / "bench-budgets.json").write_text('{"results": []}')
        (rounds[1] / "bench-fxfs-guest-sandboxes.json").unlink()
        self.assertEqual(bench_ab.not_run(rounds),
                         [f"{rounds[1].name}: no bench-fxfs-guest-sandboxes.json"])
        empty = Path(tempfile.mkdtemp(prefix="r"))
        self.assertEqual(bench_ab.not_run([rounds[0], empty]), [
            f"{empty.name}: no result at all", f"{empty.name}: no bench-budgets.json",
            f"{empty.name}: no bench-fxfs-guest-sandboxes.json"])

    def test_a_part_that_says_it_did_not_run_did_not(self):
        a = [side("fxfs-guest-sandboxes", failures={"sandboxes.not-run": 1},
                  failed=["sandboxes: not run: it left no result"])]
        b = [side("fxfs-guest-sandboxes", vms=(False, [90]))]
        self.assertEqual(bench_ab.not_run(a + b), [f"{a[0].name}: sandboxes not run"])
        self.assertEqual(bench_ab.not_run(b), [])

    def test_the_comparison_fails_saying_so_first(self):
        a = [side("fxfs-guest-sandboxes", failures={"sandboxes.not-run": 1})]
        b = [side("fxfs-guest-sandboxes", vms=(False, [90]))]
        done = subprocess.run([str(SCRIPTS / "bench-ab.py"), "pin", *map(str, a), "--", "main",
                               *map(str, b)], capture_output=True, text=True)
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertEqual(done.stdout.splitlines()[:3], [
            "## NOT RUN: this comparison is incomplete", "", f"- {a[0].name}: sandboxes not run"])
        self.assertEqual(done.stderr, f"NOT RUN: {a[0].name}: sandboxes not run\n")

    def test_qualify_fails_a_part_that_left_no_result_as_not_run(self):
        """Its sandboxes refused at once (no ext4 disk, or no findmnt): exit 3, and a result in
        the part's place whose one check, sandboxes.not-run, failed."""
        results = Path(tempfile.mkdtemp())
        disk = "/dev/shm" if Path("/dev/shm").is_dir() else tempfile.mkdtemp()
        status, _, err = bash(f"{SCRIPTS / 'qualify.sh'} {results}",
                              {"FXDAX_QUALIFY_SOAK_SECS": "0", "FXDAX_SANDBOX_DISK": disk})
        self.assertEqual(status, 3, err)
        self.assertIn(f"qualify: NOT RUN: sandboxes left no result (bench-fxfs-guest-sandboxes.json): "
                      f"see its log, {results}/sandboxes\n", err)
        report = json.loads((results / "bench-fxfs-guest-sandboxes.json").read_text())
        self.assertEqual(report["failures"], {"sandboxes.not-run": 1})
        self.assertEqual(json.loads((results / "bench-budgets.json").read_text())["failures"],
                         {"budgets.not-run": 1})
        self.assertEqual(bench_ab.not_run([results]), [f"{results.name}: budgets not run",
                                                       f"{results.name}: sandboxes not run"])
        self.assertTrue(err.endswith(f"qualify: exit 3, results in {results}\n"), err)


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

    def test_the_daemons_sockets_are_short_whatever_the_results_path(self):
        """TASK-330.4: a host run's results nested deeper passed a socket path's 107 bytes, and
        test-e2e.sh's daemon exited at once, the sandboxes' run with it, on both sides of an A/B.
        Its sockets are in a directory of their own under /tmp: their paths' length is fixed."""
        text = (SCRIPTS / "test-e2e.sh").read_text()
        self.assertIn('sockets=$(mktemp -d /tmp/fxdax-e2e-sockets.XXXXXX)', text)
        self.assertIn('socket="$sockets/extentd.sock"\ncontrol="$sockets/extentd.control"', text)
        longest = len("/tmp/fxdax-e2e-sockets.XXXXXX/extentd.control")
        self.assertLess(longest, 107)
        self.assertNotIn('"$results/extentd.sock"', text)

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
