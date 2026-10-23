# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/bench.sh, which runs the benchmark's cases (scripts/test-e2e.sh), then compares
what they measured (scripts/bench-report.py): here in a tree of its own, bench.sh and
bench-report.py as they are, and in place of test-e2e.sh, cases that write the measurements given
and end with the status given."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from script import SCRIPTS

PGBENCH = {
    "case": "bench-fs-ext4-dax-pmem",
    "backend": "ext4-dax-pmem",
    "host_os": "linux",
    "vmm": "firecracker",
    "iterations": 1,
    "results": [{"workload": "pgbench-tps", "unit": "tps", "higher_is_better": True,
                 "values": [1000.0]}],
}
# test-e2e.sh, whose cases write $FAKE_MEASURED's measurements and end with $FAKE_STATUS.
CASES = """#!/bin/bash
cp "$FAKE_MEASURED"/*.json "$FXDAX_BENCH_RESULTS" 2> /dev/null
exit "$FAKE_STATUS"
"""


class Bench(unittest.TestCase):
    def bench(self, measured, status, earlier=None):
        """Runs bench.sh, its cases measuring `measured` (each case's report) and ending with
        `status`, in a results directory that holds `earlier`'s files, by name: its status, output
        and errors, by line."""
        self.tree = tree = Path(tempfile.mkdtemp())
        for name, text in (earlier or {}).items():
            (tree / "results" / name).parent.mkdir(parents=True, exist_ok=True)
            (tree / "results" / name).write_text(text)
        (tree / "scripts").mkdir()
        for name in ["bench.sh", "bench-report.py"]:
            (tree / "scripts" / name).symlink_to(SCRIPTS / name)
        (tree / "scripts" / "test-e2e.sh").write_text(CASES)
        (tree / "scripts" / "test-e2e.sh").chmod(0o755)
        (tree / "measured").mkdir()
        for report in measured:
            (tree / "measured" / f"{report['case']}.json").write_text(json.dumps(report))
        done = subprocess.run(
            ["/bin/bash", tree / "scripts" / "bench.sh", "1"],
            env={
                # This python3 first, for bench-report.py.
                "PATH": f"{Path(sys.executable).parent}:/usr/bin:/bin",
                "FXDAX_BENCH_RESULTS": str(tree / "results"),
                "FAKE_MEASURED": str(tree / "measured"),
                "FAKE_STATUS": str(status),
            },
            capture_output=True,
            text=True,
        )
        return done.returncode, done.stdout, done.stderr.splitlines()

    def test_a_failed_case_leaves_the_others_compared(self):
        status, report, errors = self.bench([PGBENCH], 1)
        self.assertEqual(
            (status, report.split("\n\n")[:2]),
            (
                1,
                [
                    "Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 1.",
                    "| Workload | Unit | ext4-dax-pmem |\n|---|---|---|\n"
                    "| pgbench-tps | tps | 1,000 (1,000–1,000) |",
                ],
            ),
        )
        self.assertEqual(errors[-1:], [f"Results: {self.tree / 'results'}"])

    def test_cases_that_all_failed_fail_with_their_status(self):
        status, report, errors = self.bench([], 2)
        self.assertEqual((status, report), (2, ""))
        self.assertEqual(errors[-1:], [f"{self.tree / 'results'}: no results"])

    def test_an_earlier_runs_results_are_not_reported_as_this_ones(self):
        # A case writes its measurements only when it passes: those of one that failed in this run
        # would be the earlier run's.
        for earlier in [{"bench-fs-ext4-dax-pmem.json": json.dumps(PGBENCH)}, {"logs/x.log": ""}]:
            with self.subTest(earlier):
                status, report, errors = self.bench([], 1, earlier)
                self.assertEqual(
                    (status, report, errors),
                    (
                        2,
                        "",
                        [
                            f"{self.tree / 'results'} holds an earlier run's results: name a new "
                            "directory in FXDAX_BENCH_RESULTS."
                        ],
                    ),
                )

    def test_a_directory_of_what_the_host_records_is_new(self):
        # bench-linux-host.sh records the host there, and has bench.sh's output and errors written
        # there, before bench.sh runs.
        made = {"host.txt": "Linux\n", "comparison.md": "", "bench.log": ""}
        status, report, _ = self.bench([PGBENCH], 0, made)
        self.assertEqual(
            (status, report.split("\n\n")[0]),
            (0, "Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 1."),
        )

    def test_cases_that_passed_are_compared(self):
        status, report, _ = self.bench([PGBENCH], 0)
        self.assertEqual(
            (status, report.split("\n\n")[0]),
            (0, "Host: linux. VMM: firecracker. Iterations: ext4-dax-pmem 1."),
        )


if __name__ == "__main__":
    unittest.main()
