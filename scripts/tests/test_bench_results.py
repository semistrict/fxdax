# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests of scripts/bench_results.py, the benchmark's filesystems and what each compares with."""

import re
import unittest

import script

bench = script.load("bench_results")
# The benchmark's cases, which name the filesystems their reports carry (TestFsBench::name).
CASES = script.SCRIPTS.parent / "guest" / "tests" / "test_cases" / "src" / "test_fs_bench.rs"


class Registry(unittest.TestCase):
    def test_the_named_come_in_order_then_the_others_by_name(self):
        self.assertEqual(
            bench.ordered({"zz", "fxfs-guest-dax", "aa", "ext4-virtio-blk"}),
            ["ext4-virtio-blk", "fxfs-guest-dax", "aa", "zz"],
        )

    def test_baselines(self):
        self.assertEqual(bench.baselines("fxfs-guest-dax-followed"), ["fxfs-guest-dax"])
        self.assertEqual(bench.baselines("fxfs-guest-dax"), ["ext4-dax-pmem", "ext4-guest-kernel"])
        self.assertEqual(bench.baselines("fxfs-guest-dax-fork-cold"), [])
        self.assertEqual(bench.baselines("ext4-virtio-blk"), [])
        self.assertEqual(bench.baselines("fxfs-guest-dax-future"), ["ext4-virtio-blk"])

    def test_every_filesystem_the_cases_measure_is_named(self):
        names = re.findall(r'Backend::\w+ => "([a-z0-9-]+)"', CASES.read_text())
        self.assertEqual(
            names,
            [
                "ext4-guest-kernel",
                "ext4-dax-pmem",
                "ext4-pmem",
                "fxfs-guest",
                "fxfs-guest-dax",
                "fxfs-guest-dax-fork-warm",
                "fxfs-guest-dax-fork-cold",
                "fxfs-guest-dax-fork-profiled",
                "fxfs-guest-dax-followed",
                "fxfs-guest-dax-followed-idle",
            ],
        )
        self.assertEqual([name for name in names if name not in bench.ORDER], [])

    def test_the_groups_the_cases_fail_whole_are_their_workloads(self):
        source = CASES.read_text()
        measured = r'\b(?:result|timed)\("([a-z0-9-]+)"'
        # What each function GROUP_workload measures, by GROUP.
        functions = re.findall(r"\n    fn (\w+)_workload\(\) \{\n(.*?)\n    \}\n", source, re.S)
        measures = {group: re.findall(measured, body) for group, body in functions}
        failures = re.findall(r'println!\("FAILED ([a-z0-9-]+)"\)', source)
        self.assertEqual(failures, ["setxattr", "build", "pgbench"])
        groups = {name: measures[name] for name in failures if name in measures}
        self.assertEqual(groups, bench.GROUPS)
        # Any other failure names its workload.
        workloads = set(re.findall(measured, source))
        self.assertEqual(
            [name for name in failures if name not in bench.GROUPS and name not in workloads], []
        )

    def test_a_failure_names_its_workload_or_its_group(self):
        self.assertTrue(bench.failed("setxattr", {"setxattr": 3}))
        self.assertTrue(bench.failed("make", {"build": 1}))
        self.assertTrue(bench.failed("pgbench-tps", {"setxattr": 1, "pgbench": 2}))
        self.assertFalse(bench.failed("pgbench-tps", {"build": 1}))
        self.assertFalse(bench.failed("seq-write", {}))

    def test_every_baseline_is_named(self):
        named = set(bench.BASELINES) | {base for bases in bench.BASELINES.values() for base in bases}
        self.assertEqual(sorted(named - set(bench.ORDER)), [])


class Ratios(unittest.TestCase):
    def test_a_time_over_the_baselines(self):
        self.assertEqual(bench.ratio(3.0, 2.0, higher_is_better=False), 1.5)

    def test_the_baselines_rate_over_a_rate(self):
        self.assertEqual(bench.ratio(400.0, 1000.0, higher_is_better=True), 2.5)

    def test_cells_are_bold_over_the_limit(self):
        self.assertEqual(bench.times(1.234), "1.23×")
        self.assertEqual(bench.times(2.0), "2.00×")
        self.assertEqual(bench.times(2.001), "**2.00×**")
        self.assertEqual(bench.times(0.5), "0.50×")

    def test_measurements(self):
        self.assertEqual(bench.fmt(1376.4), "1,376")
        self.assertEqual(bench.fmt(12.34), "12.3")
        self.assertEqual(bench.fmt(1.234), "1.23")


if __name__ == "__main__":
    unittest.main()
