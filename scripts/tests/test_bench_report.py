# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/bench-report.py: python3 -m unittest discover scripts/tests (scripts/test.sh)."""

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

import script

bench_report = script.load("bench-report")

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

    def test_requests_served_and_commits_by_phase_are_reported(self):
        followers = "followers: views=0 max-views=0 max-behind=0 max-stale-ms=0 max-kept=0\n"
        daemon = (
            "daemon: maps=10 map-ns=2000000 commits=4 commit-ns=1200000 commit-write-ns=200000 "
            "commit-written-bytes=16384 commit-store-ns=200000 commit-seal-ns=600000 "
            "commit-wait-ns=100000 segment-closes=2 segment-close-ns=30000000\n"
        )
        requests = "requests: write=4 write-ns=1400000 map=10 map-ns=2500000\n"
        section = report(self.results(STATS + followers + daemon + requests)).split(
            "downloaded 45.0 KiB.", 1
        )[1]
        self.assertEqual(
            section,
            """

Extent daemon: the VM processes' requests, as it served them, from their dispatch to their
reply.

| Request | Count | Each |
|---|---|---|
| write | 4 | 350.0 µs |
| map | 10 | 250.0 µs |

Commits: 4, 300.0 µs each: writing the open extents' 4.0 KiB 50.0 µs, committing the store 50.0 µs, sealing the windows' mappings 150.0 µs, waiting for object storage 25.0 µs.

Segments closed and framed: 2, 15.0 ms each.

""",
        )

    def test_no_requests_no_section(self):
        self.assertNotIn("Extent daemon", report(self.results(STATS + "requests:\n")))


def pgbench(tps):
    """A run's results directory: pgbench's transactions a second on each filesystem of `tps`."""
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


def measured(cases):
    """A run's results directory: each filesystem of `cases` with its workloads, as (workload,
    unit, higher is better, values), and the iterations of each that failed."""
    directory = Path(tempfile.mkdtemp())
    for backend, (workloads, failures) in cases.items():
        case = {
            "case": f"bench-fs-{backend}",
            "backend": backend,
            "host_os": "linux",
            "vmm": "firecracker",
            "iterations": 1,
            "results": [
                {"workload": workload, "unit": unit, "higher_is_better": higher, "values": values}
                for workload, unit, higher, values in workloads
            ],
            "failures": failures,
        }
        (directory / f"bench-fs-{backend}.json").write_text(json.dumps(case))
    return directory


def table(directory):
    """The report's table of the run in `directory`."""
    return report(directory).split("\n\n", 1)[1].split("\n\n", 1)[0]


class Followed(unittest.TestCase):
    """A writer with followers attached (TASK-42.7), against one alone."""

    def test_followed_compares_with_alone(self):
        directory = pgbench({"fxfs-guest-dax": 1600.0, "fxfs-guest-dax-followed": 1200.0})
        self.assertEqual(
            table(directory),
            """| Workload | Unit | fxfs-guest-dax | fxfs-guest-dax-followed | fxfs-guest-dax-followed vs fxfs-guest-dax |
|---|---|---|---|---|
| pgbench-tps | tps | 1,600 (1,600–1,600) | 1,200 (1,200–1,200) | 1.33× |""",
        )

    def test_followed_alone_reports(self):
        directory = pgbench({"fxfs-guest-dax-followed": 1200.0})
        self.assertEqual(
            table(directory),
            """| Workload | Unit | fxfs-guest-dax-followed |
|---|---|---|
| pgbench-tps | tps | 1,200 (1,200–1,200) |""",
        )


class Workloads(unittest.TestCase):
    """The table's rows: every workload a filesystem measured, though others did not."""

    def test_filesystems_of_other_workloads_share_the_table(self):
        directory = measured(
            {
                "ext4-dax-pmem": ([("pgbench-tps", "tps", True, [1000.0])], {}),
                "fxfs-guest-dax-fork-cold": ([("fork-boot", "s", False, [2.0])], {}),
                "fxfs-guest-dax-fork-warm": ([("fork-boot", "s", False, [1.0])], {}),
            }
        )
        self.assertEqual(
            table(directory),
            """| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax-fork-cold | fxfs-guest-dax-fork-warm \
| fxfs-guest-dax-fork-warm vs fxfs-guest-dax-fork-cold |
|---|---|---|---|---|---|
| pgbench-tps | tps | 1,000 (1,000–1,000) | – | – | – |
| fork-boot | s | – | 2.00 (2.00–2.00) | 1.00 (1.00–1.00) | 0.50× |""",
        )

    def test_what_failed_is_failed_and_what_was_not_run_is_not(self):
        directory = measured(
            {
                # Its every iteration of setxattr failed, which the guest records by its name.
                "fxfs-guest-dax": ([("pgbench-tps", "tps", True, [1600.0])], {"setxattr": 3}),
                "fxfs-guest-dax-followed": (
                    [("pgbench-tps", "tps", True, [1200.0]), ("setxattr", "ops/s", True, [450.0])],
                    {},
                ),
                "fxfs-guest-dax-followed-idle": ([("pgbench-tps", "tps", True, [1500.0])], {}),
            }
        )
        self.assertEqual(
            table(directory),
            """| Workload | Unit | fxfs-guest-dax | fxfs-guest-dax-followed | fxfs-guest-dax-followed-idle \
| fxfs-guest-dax-followed vs fxfs-guest-dax | fxfs-guest-dax-followed-idle vs fxfs-guest-dax |
|---|---|---|---|---|---|---|
| pgbench-tps | tps | 1,600 (1,600–1,600) | 1,200 (1,200–1,200) | 1,500 (1,500–1,500) | 1.33× | 1.07× |
| setxattr | ops/s | failed | 450 (450–450) | – | – | – |""",
        )

    def test_a_group_that_failed_fails_its_workloads(self):
        # The guest records a failed build or pgbench by its group (test_fs_bench.rs): in
        # fxfs-guest-dax's iteration, make failed, after untar, and so did pgbench.
        directory = measured(
            {
                "ext4-dax-pmem": (
                    [
                        ("untar", "s", False, [1.0]),
                        ("make", "s", False, [20.0]),
                        ("build-total", "s", False, [21.0]),
                        ("pgbench-init", "s", False, [5.0]),
                        ("pgbench-tps", "tps", True, [1000.0]),
                    ],
                    {},
                ),
                "fxfs-guest-dax": ([("untar", "s", False, [2.0])], {"build": 1, "pgbench": 1}),
            }
        )
        self.assertEqual(
            table(directory),
            """| Workload | Unit | ext4-dax-pmem | fxfs-guest-dax | fxfs-guest-dax vs ext4-dax-pmem |
|---|---|---|---|---|
| untar | s | 1.00 (1.00–1.00) | 2.00 (2.00–2.00) | 2.00× |
| make | s | 20.0 (20.0–20.0) | failed | – |
| build-total | s | 21.0 (21.0–21.0) | failed | – |
| pgbench-init | s | 5.00 (5.00–5.00) | failed | – |
| pgbench-tps | tps | 1,000 (1,000–1,000) | failed | – |""",
        )


class UnknownBackends(unittest.TestCase):
    """A filesystem scripts/bench_results.py does not name yet, as a newer case measures."""

    def test_an_unknown_filesystem_is_shown_last(self):
        directory = pgbench({"fxfs-guest-dax-future": 1700.0, "fxfs-guest-dax": 1600.0})
        self.assertEqual(
            table(directory),
            """| Workload | Unit | fxfs-guest-dax | fxfs-guest-dax-future |
|---|---|---|---|
| pgbench-tps | tps | 1,600 (1,600–1,600) | 1,700 (1,700–1,700) |""",
        )

    def test_an_unknown_filesystem_compares_with_ext4_on_virtio_blk(self):
        directory = pgbench({"zz-future": 400.0, "ext4-virtio-blk": 1000.0})
        self.assertEqual(
            table(directory),
            """| Workload | Unit | ext4-virtio-blk | zz-future | zz-future vs ext4-virtio-blk |
|---|---|---|---|---|
| pgbench-tps | tps | 1,000 (1,000–1,000) | 400 (400–400) | **2.50×** |""",
        )


if __name__ == "__main__":
    unittest.main()
