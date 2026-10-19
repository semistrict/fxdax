# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/bench-report.py: python3 -m unittest discover scripts/tests (scripts/test.sh)."""

import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "bench-report.py"
spec = importlib.util.spec_from_file_location("bench_report", SCRIPT)
bench_report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench_report)

STATS = """object-store: get=1 get-range=95 head=0 list=7 create=110 update=2 overwrite=0 \
delete=1 up-segments=127401984 up-extents=0 up-checkpoints=20480 up-moves=0 up-other=1024 down=46080
"""


def report(directory):
    out = io.StringIO()
    argv = sys.argv
    sys.argv = ["bench-report.py", str(directory)]
    try:
        with contextlib.redirect_stdout(out):
            bench_report.main()
    finally:
        sys.argv = argv
    return out.getvalue()


class ObjectStorage(unittest.TestCase):
    def results(self, stats):
        directory = Path(tempfile.mkdtemp())
        case = {
            "case": "bench-fs-fxfs-guest-dax",
            "backend": "fxfs-guest-dax",
            "host_os": "linux",
            "iterations": 1,
            "results": [
                {"workload": "pgbench-tps", "unit": "tps", "higher_is_better": True, "values": [1376.0]}
            ],
        }
        (directory / "bench-fs-fxfs-guest-dax.json").write_text(json.dumps(case))
        (directory / "logs").mkdir()
        if stats is not None:
            (directory / "logs" / "extentd-stats.txt").write_text(stats)
        return directory

    def test_the_stats_kept_are_reported_with_their_cost(self):
        section = report(self.results(STATS)).split("Object storage:", 1)[1]
        self.assertEqual(
            section,
            """ the extent daemon's requests to its bucket, for the cases that kept their
stores there, at GCS Standard single-region prices (class A $0.005 per 1,000, class B
$0.0004 per 1,000, deletes free).

| Request | Count | Class | Cost |
|---|---|---|---|
| get | 1 | B | $0.000000 |
| get-range | 95 | B | $0.000038 |
| head | 0 | B | $0.000000 |
| list | 7 | A | $0.000035 |
| create | 110 | A | $0.000550 |
| update | 2 | A | $0.000010 |
| overwrite | 0 | A | $0.000000 |
| delete | 1 | free | $0.000000 |
| all | 216 | | $0.000633 |

Uploaded 121.5 MiB (segments 121.5 MiB, extents 0 B, checkpoints 20.0 KiB, moves 0 B, other 1.0 KiB); downloaded 45.0 KiB.

""",
        )

    def test_no_stats_no_section(self):
        self.assertNotIn("Object storage", report(self.results(None)))

    def test_followers_are_reported_after_object_storage(self):
        followers = "followers: views=0 max-views=2 max-behind=3 max-stale-ms=1250 max-kept=8192\n"
        section = report(self.results(STATS + followers)).split("downloaded 45.0 KiB.", 1)[1]
        self.assertEqual(
            section,
            """

Followers: at most 2 at once, at most 3 commits and 1,250 ms behind their writer, keeping at most 8.0 KiB of its frees.

""",
        )

    def test_no_followers_no_section(self):
        followers = "followers: views=0 max-views=0 max-behind=0 max-stale-ms=0 max-kept=0\n"
        self.assertNotIn("Followers", report(self.results(STATS + followers)))


class Followed(unittest.TestCase):
    """A writer with followers attached (TASK-42.7), against one alone."""

    def results(self, tps):
        directory = Path(tempfile.mkdtemp())
        for backend, value in tps.items():
            case = {
                "case": f"bench-fs-{backend}",
                "backend": backend,
                "host_os": "linux",
                "vmm": "firecracker",
                "iterations": 1,
                "results": [
                    {"workload": "pgbench-tps", "unit": "tps", "higher_is_better": True,
                     "values": [value]}
                ],
            }
            (directory / f"bench-fs-{backend}.json").write_text(json.dumps(case))
        return directory

    def test_followed_compares_with_alone(self):
        directory = self.results({"fxfs-guest-dax": 1600.0, "fxfs-guest-dax-followed": 1200.0})
        self.assertEqual(
            report(directory).split("\n\n", 1)[1].split("\n\n", 1)[0],
            """| Workload | Unit | fxfs-guest-dax | fxfs-guest-dax-followed | fxfs-guest-dax-followed vs fxfs-guest-dax |
|---|---|---|---|---|
| pgbench-tps | tps | 1,600 (1,600–1,600) | 1,200 (1,200–1,200) | 1.33× |""",
        )

    def test_followed_alone_reports(self):
        directory = self.results({"fxfs-guest-dax-followed": 1200.0})
        self.assertEqual(
            report(directory).split("\n\n", 1)[1].split("\n\n", 1)[0],
            """| Workload | Unit | fxfs-guest-dax-followed |
|---|---|---|
| pgbench-tps | tps | 1,200 (1,200–1,200) |""",
        )


if __name__ == "__main__":
    unittest.main()
