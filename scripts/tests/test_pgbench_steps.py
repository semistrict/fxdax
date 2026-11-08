# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/pgbench-steps.py, the pgbench cases' runs step by step (TASK-91.8), on logs as
test_fxfs_guest_pgbench.rs writes them: a case's console and its daemon's samples."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import script

steps = script.load("pgbench-steps")

LOAD = [
    "pgbench: at 2.20 s: pgbench -h /run/postgresql -i -q -s 50 bench",
    "done in 13.45 s (drop tables 0.00 s, create tables 0.01 s, client-side generate 9.44 s, "
    "vacuum 0.43 s, primary keys 3.57 s).",
    "pgbench: 13.5s, exit status: 0",
    "guest cpu: cpu  100 0 100 200 0 0 0 0 0 0",
]


def run(at, cpu_after, tps, latency):
    """A run's lines, started at the guest's uptime `at`, the guest's CPU ticks `cpu_after` after
    it: as the case logs them, the run's output after its CPU."""
    return [
        f"pgbench: at {at} s: pgbench -h /run/postgresql -c 4 -j 2 -T 60 bench",
        "starting vacuum...end.",
        "pgbench: 60.1s, exit status: 0",
        "fxfs stats: device_reads 1",
        f"guest cpu: cpu  {cpu_after}",
        "number of transactions actually processed: 120266",
        f"latency average = {latency} ms",
        "initial connection time = 11.688 ms",
        f"tps = {tps} (without initial connection time)",
        "pgbench: at 77.00 s: psql -h /run/postgresql -At -c 'select 1' bench",
        "pgbench: 62.8ms, exit status: 0",
    ]


def sample(t, commits, wait_ns, writes, write_ns):
    """A daemon sample at `t`, as the host writes it."""
    return [
        f"t={t} pending_upload_bytes=0 oldest_pending_ms=0",
        "object-store: get=24",
        f"daemon: maps=0 commits={commits} commit-ns=9 commit-wait-ns={wait_ns}",
        f"requests: open=2 open-ns=700 write={writes} write-ns={write_ns} commit=12 commit-ns=50",
        "kvm: exits=1",
    ]


class Steps(unittest.TestCase):
    def setUp(self):
        self.results = Path(tempfile.mkdtemp())

    def case(self, name, log, samples=None):
        directory = self.results / "logs" / name / name
        directory.mkdir(parents=True)
        (directory / "log.txt").write_text("\n".join(log) + "\n")
        if samples is not None:
            (directory / "samples.txt").write_text("\n".join(samples) + "\n")

    def test_each_run_is_reported_with_the_guests_cpu_and_the_daemons_work(self):
        # Run 1 from 16 s to 76.1 s: the samples at 16 and 78 bracket it, 62 s apart.
        self.case("fxfs-guest-pgbench-dax",
                  LOAD + run(16.0, "500 0 300 400 0 0 0 0 0 0", 2003.6, 1.996),
                  sample(14, 10, 0, 100, 0) + sample(16, 20, 1000, 200, 1000)
                  + sample(78, 82_020, 410_001_000, 207_000, 6_831_001_000))
        self.case("pgbench-ext4-blk-unsafe",
                  LOAD + run(13.5, "400 100 200 300 0 0 0 0 0 0", 3609.4, 1.11))
        found = steps.cases([self.results])
        self.assertEqual(found["fxfs-guest-pgbench-dax"], {
            "load": {"seconds": 13.45, "phases": {
                "drop tables": 0.0, "create tables": 0.01, "client-side generate": 9.44,
                "vacuum": 0.43, "primary keys": 3.57}},
            "runs": [{
                "started": 16.0, "seconds": 60.1,
                "cpu": {"user": 0.5, "system": 0.25, "idle": 0.25, "steal": 0.0},
                "latency_ms": 1.996, "tps": 2003.6,
                "daemon": {"seconds": 62, "requests": {"write": {"per_s": 3335.5, "mean_us": 33.0}},
                           "commits_per_s": 1322.6, "commit_wait_us": 5.0},
            }],
        })
        self.assertEqual(found["pgbench-ext4-blk-unsafe"]["runs"][0]["daemon"], None)
        self.assertEqual(found["pgbench-ext4-blk-unsafe"]["runs"][0]["cpu"],
                         {"user": 0.667, "system": 0.167, "idle": 0.167, "steal": 0.0})
        report = steps.markdown(found)
        self.assertIn("| fxfs-guest-pgbench-dax | 13.45 s | 2,004 |\n", report)
        self.assertIn("Load (pgbench -i): 13.45 s: drop tables 0.00 s, create tables 0.01 s, "
                      "client-side generate 9.44 s, vacuum 0.43 s, primary keys 3.57 s.\n", report)
        self.assertIn("| 1 | 2,004 | 2.00 ms | 50% | 25% | 25% | 0% | write 3,336 (33) | "
                      "1,323 (5 us) |\n", report)
        self.assertIn("| 1 | 3,609 | 1.11 ms | 67% | 17% | 17% | 0% | – | – |\n", report)

    def test_a_run_without_samples_around_it_reports_no_daemon(self):
        # The samples end before the run does: none at or after its end.
        self.case("fxfs-guest-pgbench", LOAD + run(16.0, "500 0 300 400 0 0 0 0 0 0", 2000, 2.0),
                  sample(16, 20, 1000, 200, 1000) + sample(40, 30, 2000, 300, 2000))
        found = steps.cases([self.results])
        self.assertEqual(found["fxfs-guest-pgbench"]["runs"][0]["daemon"], None)

    def test_the_command_writes_json_and_markdown(self):
        self.case("pgbench-ext4-blk-writeback",
                  LOAD + run(13.5, "300 0 200 900 0 0 0 0 0 0", 1536.2, 2.6))
        out = self.results / "steps.json"
        done = subprocess.run(
            [sys.executable, script.SCRIPTS / "pgbench-steps.py", "--json", out, self.results],
            capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(done.stdout.splitlines()[:3], [
            "| Case | Load | Run 1 tps |", "|---|---|---|",
            "| pgbench-ext4-blk-writeback | 13.45 s | 1,536 |"])
        self.assertEqual(json.loads(out.read_text())["pgbench-ext4-blk-writeback"]["runs"][0]["tps"],
                         1536.2)

    def test_no_case_fails(self):
        done = subprocess.run([sys.executable, script.SCRIPTS / "pgbench-steps.py", self.results],
                              capture_output=True, text=True)
        self.assertEqual((done.returncode, done.stderr.strip()),
                         (1, f"no pgbench case's log in {self.results}"))


if __name__ == "__main__":
    unittest.main()
