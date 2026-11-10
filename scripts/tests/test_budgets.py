# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/budgets.py (TASK-330.2): each extent daemon of a run held to its budgets, its
anonymous memory, its uploads' and its disk's writes, from its stats file."""

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

import script

budgets = script.load("budgets")

MIB = 1 << 20


def stats(*, over=0, held=100 * MIB, io_written=300 * MIB, uploaded=0):
    """A daemon's stats file's lines, of a run whose guests wrote 100 MiB."""
    return "\n".join([
        f"object-store: get=1 create=2 up-segments={uploaded} up-extents=0 down=0 failed-create=0",
        "daemon: fetched-bytes=4096 written_raw_bytes=104857600 written_copied_bytes=0 "
        "written_framed_bytes=52428800 written_own_bytes=0 written_log_bytes=4096 "
        f"written_writes=400 stats_written_bytes=8192 io_write_bytes={io_written} "
        "io_cancelled_write_bytes=0",
        "requests: write=400 write-ns=1000 commit=10 commit-ns=1000",
        f"uploads: held=0 max-held={held} budget={512 * MIB}",
        f"process: cpu-ns=2000000000 anon={200 * MIB} max-anon={300 * MIB} shmem=0 max-shmem=0 "
        f"anon-budget={1200 * MIB} over-budget={over} max-over-budget={over * MIB}",
    ]) + "\n"


class Budgets(unittest.TestCase):
    def setUp(self):
        self.results = Path(tempfile.mkdtemp())

    def run_budgets(self, *args):
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            status = budgets.main([*map(str, args), str(self.results)])
        return status, said.getvalue(), json.loads((self.results / "budgets.json").read_text())

    def test_daemons_within_their_budgets_pass(self):
        """A daemon whose memory, uploads and disk are within their budgets passes, and what it
        wrote for each guest byte is said."""
        (self.results / "extentd-stats.txt").write_text(stats(io_written=151 * MIB))
        (self.results / "extentd.log").write_text("x" * 1000)
        status, said, report = self.run_budgets()
        self.assertEqual(status, 0, said)
        self.assertEqual([check["budget"] for check in report[0]["checks"]],
                         ["memory", "uploads", "disk"])
        self.assertIn("extentd-stats.txt: written for each guest byte: raw 1.0 copied 0.0 "
                      "framed 0.5 own 0.0 log 0.0 io 1.51", said)
        self.assertEqual(report[0]["figures"]["cpu_ns_per_request"], 2000000000 // 410)

    def test_a_sample_past_the_memory_budget_fails(self):
        """TASK-317's growth: a sample past the daemon's anonymous memory budget fails the run."""
        (self.results / "extentd-stats.txt").write_text(stats(over=3, io_written=151 * MIB))
        status, said, _ = self.run_budgets()
        self.assertEqual(status, 1)
        self.assertIn("extentd-stats.txt: memory: PAST ITS BUDGET: anonymous memory 300.0 MiB at "
                      "most, budget 1200.0 MiB; 3 samples past it, by 3.0 MiB at most", said)

    def test_uploads_past_their_budget_fail(self):
        (self.results / "extentd-stats.txt").write_text(stats(held=600 * MIB, io_written=0))
        status, said, _ = self.run_budgets()
        self.assertEqual(status, 1)
        self.assertIn("uploads: PAST ITS BUDGET: uploads held 600.0 MiB at most", said)

    def test_writes_the_daemon_does_not_count_fail(self):
        """A daemon whose kernel wrote more than it says it did, its log, stats file, cache and
        pages allowed, wrote something twice, uncounted: its bucket's objects allowed too, those
        uploaded and its upload memory's worth in flight, unless its bucket is in memory."""
        case = self.results / "fxfs-guest-x"
        case.mkdir()
        (case / "extentd-stats.txt").write_text(stats(io_written=800 * MIB, uploaded=150 * MIB))
        status, said, _ = self.run_budgets()
        self.assertEqual(status, 0, said)
        self.assertIn("bucket 662.0 MiB", said)
        status, said, _ = self.run_budgets("--bucket-in-memory", case / "extentd-stats.txt")
        self.assertEqual(status, 1)
        self.assertIn("fxfs-guest-x/extentd-stats.txt: disk: PAST ITS BUDGET: the kernel wrote "
                      "800.0 MiB for it, it says 151.6 MiB at most: stores 150.0 MiB, pages 1.6 "
                      "MiB, stats 0.0 MiB, log 0.0 MiB, bucket 0.0 MiB, cache 0.0 MiB", said)


if __name__ == "__main__":
    unittest.main()
